//! Corpus durability: monotonic chunk snapshots + hollow-document detection.
//!
//! The loss this guards against (diagnosed 2026-10-30): chunks live in the
//! vector store and are mirrored to `chunks.json` on every snapshot save;
//! the snapshot is upserted back into the store at boot. When the store's
//! database died uncleanly (sandbox replacements kill it hard) it came
//! back missing committed chunks — and the next snapshot save then dumped
//! the *thinned* store over `chunks.json`, propagating the loss into the
//! only backup instead of healing from it. Both copies mirrored the
//! thinner one, and nothing reconciled either against the document
//! registry.
//!
//! Two defences, both keyed on the registry (`documents.json`) as the
//! authority on what should exist:
//!
//! 1. **Monotonic snapshot merge** — writing `chunks.json` unions the
//!    active chunk set with the snapshot already on disk. The active
//!    store's record wins for ids it holds; snapshot-only records survive
//!    only while their document is still registered, so legitimate
//!    deletions (which remove the registry entry and purge the store)
//!    still shrink the snapshot and deleted chunks never resurrect.
//! 2. **Hollow detection** — after boot restore, and again on every
//!    snapshot save, every registered document is checked for its chunks
//!    in the active store. A document whose chunks are (partly) missing
//!    is logged loudly by name; the counts are also exposed on `/health`.

use std::collections::{HashMap, HashSet};

use legal_rag_common::Result;
use legal_rag_rag::{ChunkRecord, DocumentInfo, VectorStore};

/// Union the chunk set about to be snapshotted with the snapshot already
/// on disk. Active records win by chunk id; disk-only records are kept
/// only when their document is still in the registry (`registered`).
/// Output order is deterministic: active records in their given order,
/// then preserved disk-only records sorted by (doc_id, ordinal, id).
pub fn merge_chunk_snapshot(
    active: Vec<ChunkRecord>,
    on_disk: Vec<ChunkRecord>,
    registered: &HashSet<String>,
) -> Vec<ChunkRecord> {
    let mut by_id: HashMap<String, ChunkRecord> = HashMap::with_capacity(active.len());
    for rec in &active {
        by_id.insert(rec.id.clone(), rec.clone());
    }
    let mut preserved: Vec<ChunkRecord> = on_disk
        .into_iter()
        .filter(|rec| !by_id.contains_key(&rec.id) && registered.contains(&rec.doc_id))
        .collect();
    preserved.sort_by(|a, b| {
        a.doc_id
            .cmp(&b.doc_id)
            .then(a.ordinal.cmp(&b.ordinal))
            .then(a.id.cmp(&b.id))
    });
    let mut out = active;
    out.extend(preserved);
    out
}

/// A registered document whose chunks are missing (wholly or partly) from
/// the active store. `expected` is the registry's chunk count for the
/// document; `found` is how many of its chunk ids the store actually holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HollowDoc {
    pub doc_id: String,
    pub title: String,
    pub expected: usize,
    pub found: usize,
}

/// Pure hollow computation: `present` maps doc_id → the chunk ids the
/// store holds for it. Documents are flagged in registry order when any
/// of their registered chunk ids are absent.
pub fn hollow_from_presence(
    documents: &[DocumentInfo],
    present: &HashMap<String, HashSet<String>>,
) -> Vec<HollowDoc> {
    documents
        .iter()
        .filter_map(|doc| {
            let held = present.get(&doc.id);
            let found = doc
                .chunk_ids
                .iter()
                .filter(|id| held.map(|s| s.contains(*id)).unwrap_or(false))
                .count();
            let expected = doc.chunk_ids.len();
            (found < expected).then(|| HollowDoc {
                doc_id: doc.id.clone(),
                title: doc.title.clone(),
                expected,
                found,
            })
        })
        .collect()
}

/// Live hollow check against the store: fetch each registered document's
/// chunks and compare ids with the registry. A store read error is
/// treated as "holds nothing" (and logged) so the document is flagged
/// loudly rather than silently passed over.
pub async fn hollow_documents(
    store: &dyn VectorStore,
    documents: &[DocumentInfo],
) -> Vec<HollowDoc> {
    let mut present: HashMap<String, HashSet<String>> = HashMap::new();
    for doc in documents {
        match store.chunks_of_doc(&doc.id).await {
            Ok(chunks) => {
                present.insert(doc.id.clone(), chunks.into_iter().map(|c| c.id).collect());
            }
            Err(e) => {
                tracing::warn!(
                    error = %e, doc_id = %doc.id,
                    "hollow check: chunk read failed; treating document as chunk-less"
                );
                present.insert(doc.id.clone(), HashSet::new());
            }
        }
    }
    hollow_from_presence(documents, &present)
}

/// Log every hollow document as a loud WARN naming it, with expected vs
/// found chunk counts. Called after boot restore and on every snapshot
/// save while any hollow document exists.
pub fn report_hollow(hollow: &[HollowDoc], when: &str) {
    if hollow.is_empty() {
        return;
    }
    for doc in hollow {
        tracing::warn!(
            doc_id = %doc.doc_id,
            title = %doc.title,
            expected_chunks = doc.expected,
            found_chunks = doc.found,
            "HOLLOW DOCUMENT after {when}: registered document is missing chunks in the \
             vector store — Interact will fail and Ask cannot cite it until its chunks \
             are recovered"
        );
    }
    tracing::warn!(
        hollow = hollow.len(),
        "corpus durability: {} hollow document(s) after {when}",
        hollow.len()
    );
}

/// Boot-restore step for an external store: load the on-disk chunk
/// snapshot into the store. This is what heals a store that came back
/// from an unclean shutdown missing committed chunks — the snapshot
/// (kept monotonic by [`merge_chunk_snapshot`]) still holds them.
pub async fn restore_chunk_snapshot(
    store: &dyn VectorStore,
    chunks: Vec<ChunkRecord>,
) -> Result<()> {
    store.upsert(chunks).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use legal_rag_rag::{DocMeta, InMemoryVectorStore};

    fn rec(id: &str, doc_id: &str, ordinal: usize, text: &str) -> ChunkRecord {
        ChunkRecord {
            id: id.to_string(),
            doc_id: doc_id.to_string(),
            ordinal,
            text: text.to_string(),
            embedding: vec![0.1, 0.2],
            metadata: HashMap::new(),
        }
    }

    fn doc(id: &str, title: &str, chunk_ids: &[&str]) -> DocumentInfo {
        DocumentInfo {
            id: id.to_string(),
            title: title.to_string(),
            source_uri: format!("{id}.pdf"),
            chars: 100,
            chunks: chunk_ids.len(),
            chunk_ids: chunk_ids.iter().map(|s| s.to_string()).collect(),
            ingested_at: Utc::now(),
            meta: DocMeta::default(),
            uploaded_by: None,
        }
    }

    fn registry(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    /// (a) The merge unions store + disk: active records win on shared
    /// ids, and snapshot-only chunks of still-registered documents are
    /// preserved (the heal-the-backup direction).
    #[test]
    fn merge_unions_and_preserves_registered_snapshot_only_chunks() {
        let active = vec![
            rec("d1#0", "doc-1", 0, "fresh text wins"),
            rec("d2#0", "doc-2", 0, "only in store"),
        ];
        let on_disk = vec![
            rec("d1#0", "doc-1", 0, "stale snapshot copy"),
            rec("d1#1", "doc-1", 1, "lost from store, still registered"),
            rec("d3#0", "doc-3", 0, "snapshot-only doc, registered"),
        ];
        let merged = merge_chunk_snapshot(active, on_disk, &registry(&["doc-1", "doc-2", "doc-3"]));
        let ids: Vec<&str> = merged.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["d1#0", "d2#0", "d1#1", "d3#0"]);
        // Active store's record wins for the shared id.
        assert_eq!(merged[0].text, "fresh text wins");
    }

    /// (b) Chunks of a document removed from the registry do NOT survive
    /// the merge — legitimate deletions still shrink the snapshot and
    /// deleted chunks never resurrect.
    #[test]
    fn merge_drops_chunks_of_unregistered_documents() {
        // Post-delete state: the store was purged via remove_doc (so the
        // active set has none of the deleted doc's chunks), the registry
        // no longer lists it, but the old snapshot on disk still does.
        let active = vec![rec("keep#0", "doc-keep", 0, "kept")];
        let on_disk = vec![
            rec("keep#0", "doc-keep", 0, "kept"),
            rec("gone#0", "doc-gone", 0, "deleted doc chunk"),
            rec("gone#1", "doc-gone", 1, "deleted doc chunk"),
        ];
        let merged = merge_chunk_snapshot(active, on_disk, &registry(&["doc-keep"]));
        let ids: Vec<&str> = merged.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["keep#0"], "deleted doc chunks must not resurrect: {ids:?}");
    }

    /// (c) Boot restore from a full snapshot heals a store that is
    /// missing a document's chunks: afterwards `chunks_of_doc` (the call
    /// `document_text` is built on) returns the document's text.
    #[tokio::test]
    async fn boot_restore_heals_store_missing_a_docs_chunks() {
        let store = InMemoryVectorStore::new();
        // Post-crash store: only the most recent document survived.
        store
            .upsert(vec![rec("new#0", "doc-new", 0, "the surviving document")])
            .await
            .unwrap();
        assert!(store.chunks_of_doc("doc-lost").await.unwrap().is_empty());

        // The on-disk snapshot (kept monotonic by the merge) is whole.
        let snapshot = vec![
            rec("lost#0", "doc-lost", 0, "Section 86 IT Act text, part one."),
            rec("lost#1", "doc-lost", 1, "Section 86 IT Act text, part two."),
            rec("new#0", "doc-new", 0, "the surviving document"),
        ];
        restore_chunk_snapshot(&store, snapshot).await.unwrap();

        let chunks = store.chunks_of_doc("doc-lost").await.unwrap();
        assert_eq!(chunks.len(), 2);
        let text = chunks
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Section 86 IT Act"), "healed text: {text}");
        // And the document is no longer hollow.
        let docs = vec![
            doc("doc-lost", "Section 86 IT Act", &["lost#0", "lost#1"]),
            doc("doc-new", "Survivor", &["new#0"]),
        ];
        assert!(hollow_documents(&store, &docs).await.is_empty());
    }

    /// (d) Hollow detection flags exactly the registered documents whose
    /// chunks the store does not hold — no more, no fewer.
    #[tokio::test]
    async fn hollow_detection_flags_exactly_the_chunkless_documents() {
        let store = InMemoryVectorStore::new();
        store
            .upsert(vec![
                rec("a#0", "doc-a", 0, "a0"),
                rec("a#1", "doc-a", 1, "a1"),
                rec("b#0", "doc-b", 0, "b0"), // doc-b's second chunk is gone
            ])
            .await
            .unwrap();
        let docs = vec![
            doc("doc-a", "Whole Doc", &["a#0", "a#1"]),
            doc("doc-b", "Partial Doc", &["b#0", "b#1"]),
            doc("doc-c", "Lost Doc", &["c#0"]),
        ];
        let hollow = hollow_documents(&store, &docs).await;
        assert_eq!(
            hollow,
            vec![
                HollowDoc {
                    doc_id: "doc-b".into(),
                    title: "Partial Doc".into(),
                    expected: 2,
                    found: 1,
                },
                HollowDoc {
                    doc_id: "doc-c".into(),
                    title: "Lost Doc".into(),
                    expected: 1,
                    found: 0,
                },
            ]
        );
    }
}
