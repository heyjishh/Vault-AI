//! `/api/v1/vault/*` — document intake (S3 + direct upload) and management.
//!
//! Intake feeds raw bytes into the RAG-Anything pipeline
//! (`process_document_complete` / `process_documents_batch`).

use axum::extract::{Path, Query, State};
use axum::Json;
use legal_rag_auth::AuthUser;
use legal_rag_rag::filter_supported_files;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zeroize::Zeroize;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct UploadRequest {
    pub title: String,
    pub file_name: String,
    /// Pasted text content (for .txt/.md).
    pub text: Option<String>,
    /// Base64-encoded raw file bytes (for PDFs and other binary formats).
    pub content_base64: Option<String>,
    /// File the document under this matter (default matter when omitted).
    pub matter_id: Option<String>,
    /// Password for a password-protected input file (PDFs). The file is
    /// decrypted in memory before extraction; the password is zeroized
    /// once decryption returns, and is never stored or logged. Ignored
    /// for unprotected files; absent for unprotected uploads as before.
    pub file_password: Option<String>,
    /// Legal-validity metadata (jurisdiction / in-force dates / what
    /// superseded this document) — all optional, dates as YYYY-MM-DD.
    pub jurisdiction: Option<String>,
    pub effective_from: Option<String>,
    pub effective_to: Option<String>,
    pub superseded_by: Option<String>,
}

/// A `YYYY-MM-DD` date string (shape check only).
fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && s.chars().all(|c| c.is_ascii_digit() || c == '-')
}

/// Build a `DocMeta` from optional request fields, rejecting malformed
/// dates. Returns `Ok(None)` when no field was supplied.
fn meta_from_fields(
    jurisdiction: &Option<String>,
    effective_from: &Option<String>,
    effective_to: &Option<String>,
    superseded_by: &Option<String>,
) -> Result<Option<legal_rag_rag::DocMeta>, ApiError> {
    let clean = |o: &Option<String>| o.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let (jurisdiction, effective_from, effective_to, superseded_by) = (
        clean(jurisdiction),
        clean(effective_from),
        clean(effective_to),
        clean(superseded_by),
    );
    if jurisdiction.is_none() && effective_from.is_none() && effective_to.is_none() && superseded_by.is_none() {
        return Ok(None);
    }
    for d in [&effective_from, &effective_to].into_iter().flatten() {
        if !valid_date(d) {
            return Err(legal_rag_common::Error::BadRequest(format!(
                "dates must be YYYY-MM-DD, got '{d}'"
            ))
            .into());
        }
    }
    Ok(Some(legal_rag_rag::DocMeta {
        jurisdiction,
        effective_from,
        effective_to,
        superseded_by,
    }))
}

#[derive(Serialize)]
pub struct IngestReport {
    pub documents: usize,
    pub chunks: usize,
    pub entities: usize,
    pub relations: usize,
    pub modal_processed: usize,
    /// The indexed document id (single-document uploads).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_id: Option<String>,
    /// The matter the document was filed under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matter_id: Option<String>,
    /// S3 keys skipped because the object is an encrypted PDF (P2: a
    /// per-file password map is a later phase, so encrypted objects are
    /// skipped with a clear per-file record while the batch continues).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped_encrypted: Vec<String>,
    /// Per-file failures from the batch (including download failures).
    /// Empty for single-document uploads.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<FailedIngest>,
}

/// One file's batch failure, surfaced instead of swallowed.
#[derive(Serialize)]
pub struct FailedIngest {
    pub file: String,
    pub error: String,
}

/// Split downloaded S3 objects into (ingestible, encrypted-PDF skipped).
/// Only `.pdf` objects are probed; anything else passes through.
pub(crate) fn split_encrypted_s3(
    items: Vec<(String, Vec<u8>)>,
) -> (Vec<(String, Vec<u8>)>, Vec<String>) {
    let mut kept = Vec::new();
    let mut skipped = Vec::new();
    for (key, bytes) in items {
        if key.to_lowercase().ends_with(".pdf")
            && legal_rag_ingest::pdf_password::is_encrypted_pdf(&bytes)
        {
            skipped.push(key);
        } else {
            kept.push((key, bytes));
        }
    }
    (kept, skipped)
}

/// Upload a document directly (pasted text or base64 file bytes).
pub async fn upload_document(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(mut body): Json<UploadRequest>,
) -> Result<Json<IngestReport>, ApiError> {
    use base64::Engine as _;
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let bytes: Vec<u8> = match (body.content_base64, body.text) {
        (Some(b64), _) => base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| {
                legal_rag_common::Error::BadRequest(format!("invalid base64 content: {e}"))
            })?,
        (None, Some(text)) => text.into_bytes(),
        (None, None) => {
            return Err(legal_rag_common::Error::BadRequest(
                "upload needs either text or content_base64".to_string(),
            )
            .into())
        }
    };
    if bytes.is_empty() {
        return Err(legal_rag_common::Error::BadRequest("empty upload".to_string()).into());
    }
    // Resolve the filing matter up front (unknown id → 404). Filing
    // into a space brief requires an editor+ role there.
    let target_matter = match &body.matter_id {
        Some(mid) if !mid.trim().is_empty() => {
            let matter = state
                .matters
                .get(mid)
                .ok_or_else(|| legal_rag_common::Error::NotFound("brief not found".to_string()))?;
            crate::routes::matters::require_edit(&state, &user, &matter)?;
            Some(mid.clone())
        }
        _ => state.matters.default_id(),
    };
    // Validate legal-validity metadata before doing any ingest work.
    let meta = meta_from_fields(
        &body.jurisdiction,
        &body.effective_from,
        &body.effective_to,
        &body.superseded_by,
    )?;
    // Take the file password out of the request body so it cannot leak
    // through any later use of `body`; zeroize it the moment decryption
    // has returned, on success and on error alike.
    let mut file_password = body.file_password.take();
    let scope = legal_rag_rag::usage::new_scope();
    let report_result = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        state
            .rag
            .process_document_complete_with_password(
                &body.file_name,
                &bytes,
                Some(body.title.clone()),
                None,
                file_password.as_deref(),
            )
            .await
    })
    .await;
    if let Some(pw) = file_password.as_mut() {
        pw.zeroize();
    }
    drop(file_password);
    let report = report_result?;
    if let Some(mid) = &target_matter {
        state.matters.add_document(mid, &report.doc_id);
    }
    // Legal-validity metadata supplied with the upload, if any (already
    // validated before ingest, above).
    if let Some(meta) = meta {
        state.rag.set_document_meta(&report.doc_id, meta);
    }
    crate::routes::billing::settle_matter(
        &state,
        &user,
        &scope,
        "upload",
        &body.file_name,
        "direct upload",
        &format!(
            "{} chunks · {} entities · {} relations",
            report.chunks, report.entities, report.relations
        ),
        0,
        target_matter.clone(),
    );
    Ok(Json(IngestReport {
        documents: 1,
        chunks: report.chunks,
        entities: report.entities,
        relations: report.relations,
        modal_processed: report.modal_processed,
        doc_id: Some(report.doc_id),
        matter_id: target_matter,
        skipped_encrypted: Vec::new(),
        failed: Vec::new(),
    }))
}

#[derive(Deserialize)]
pub struct UpdateMetaRequest {
    pub jurisdiction: Option<String>,
    pub effective_from: Option<String>,
    pub effective_to: Option<String>,
    pub superseded_by: Option<String>,
}

/// Replace a document's legal-validity metadata (jurisdiction, in-force
/// dates, superseded-by). An all-empty body clears the metadata.
pub async fn update_document_meta(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<UpdateMetaRequest>,
) -> Result<Json<legal_rag_rag::DocumentInfo>, ApiError> {
    let meta = meta_from_fields(
        &body.jurisdiction,
        &body.effective_from,
        &body.effective_to,
        &body.superseded_by,
    )?
    .unwrap_or_default();
    if !state.rag.set_document_meta(&id, meta) {
        return Err(legal_rag_common::Error::NotFound(format!("document {id}")).into());
    }
    state
        .rag
        .list_documents()
        .into_iter()
        .find(|d| d.id == id)
        .map(Json)
        .ok_or_else(|| legal_rag_common::Error::NotFound(format!("document {id}")).into())
}

#[derive(Deserialize)]
pub struct IngestQuery {
    pub prefix: Option<String>,
}

/// Trigger an S3 ingestion run over the configured bucket/prefix.
///
/// Mirrors RAG-Anything's batch folder processing: extension filter +
/// concurrency-capped batch through the full document pipeline.
pub async fn ingest_s3(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<IngestQuery>,
) -> Result<Json<IngestReport>, ApiError> {
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    if !state.ingest.s3_configured() {
        return Err(legal_rag_common::Error::Config("S3 is not configured".to_string()).into());
    }
    let keys = state.ingest.list_s3_keys(q.prefix.as_deref()).await?;
    let supported = state.rag.config().supported_file_extensions.clone();
    let mut downloaded = Vec::new();
    let mut failed: Vec<FailedIngest> = Vec::new();
    for key in filter_supported_files(&keys, &supported) {
        match state.ingest.download_s3_object(key).await {
            Ok((_uri, bytes)) => downloaded.push((key.clone(), bytes)),
            Err(e) => {
                tracing::warn!(key = %key, error = %e, "skipping object");
                failed.push(FailedIngest {
                    file: key.clone(),
                    error: e.to_string(),
                });
            }
        }
    }
    // P2: encrypted PDFs are skipped with a per-file record while the
    // batch continues — never batch-halting, never silent. (A per-file
    // password map for S3 objects is a later phase, not v1.)
    let (kept, skipped_encrypted) = split_encrypted_s3(downloaded);
    for key in &skipped_encrypted {
        tracing::warn!(key = %key, "skipping encrypted S3 object (no file password in v1)");
    }
    let items: Vec<(String, Vec<u8>, Option<String>)> =
        kept.into_iter().map(|(k, b)| (k, b, None)).collect();
    let scope = legal_rag_rag::usage::new_scope();
    let results = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        state.rag.process_documents_batch(items).await
    })
    .await;
    let mut report = IngestReport {
        documents: 0,
        chunks: 0,
        entities: 0,
        relations: 0,
        modal_processed: 0,
        doc_id: None,
        matter_id: state.matters.default_id(),
        skipped_encrypted,
        failed,
    };
    for r in &results {
        if r.success {
            report.documents += 1;
            report.chunks += r.chunks;
            report.modal_processed += r.modal_processed;
            // Batch-ingested documents are filed under the default matter.
            if let (Some(mid), Some(doc_id)) = (&report.matter_id, &r.doc_id) {
                state.matters.add_document(mid, doc_id);
            }
        } else {
            report.failed.push(FailedIngest {
                file: r.file_name.clone(),
                error: r.error.clone().unwrap_or_else(|| "batch item failed".to_string()),
            });
        }
    }
    // Entities/relations counted at corpus level; sum per-doc would double-count merges.
    report.entities = state.graph.entity_count().await;
    report.relations = state.graph.relation_count().await;
    crate::routes::billing::settle_matter(
        &state,
        &user,
        &scope,
        "upload",
        "S3 batch ingest",
        q.prefix.as_deref().unwrap_or(""),
        &format!("{} documents · {} chunks", report.documents, report.chunks),
        0,
        report.matter_id.clone(),
    );
    Ok(Json(report))
}

#[derive(Deserialize)]
pub struct ListDocsQuery {
    pub matter_id: Option<String>,
}

/// List indexed documents, optionally only those filed under a matter.
/// Under Spaces, documents filed only inside inaccessible space briefs
/// are hidden (see `doc_visible`).
pub async fn list_documents(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<ListDocsQuery>,
) -> Result<Json<Vec<legal_rag_rag::DocumentInfo>>, ApiError> {
    let docs = state.rag.list_documents();
    let visible = |d: &legal_rag_rag::DocumentInfo| {
        crate::routes::matters::doc_visible(&state, &user, &d.id)
    };
    match q.matter_id {
        Some(mid) if !mid.trim().is_empty() => {
            let matter = state
                .matters
                .get(&mid)
                .ok_or_else(|| legal_rag_common::Error::NotFound("brief not found".to_string()))?;
            crate::routes::matters::require_read(&state, &user, &matter)?;
            let filed: std::collections::HashSet<String> =
                matter.document_ids.into_iter().collect();
            Ok(Json(
                docs.into_iter()
                    .filter(|d| filed.contains(&d.id) && visible(d))
                    .collect(),
            ))
        }
        _ => Ok(Json(docs.into_iter().filter(visible).collect())),
    }
}

/// One parent chunk with its index-time augmentation, for the
/// Vault's "Indexing" view.
#[derive(Serialize)]
pub struct ChunkAugView {
    pub id: String,
    pub ordinal: usize,
    pub text_preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aug_keywords: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aug_questions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hyper_id: Option<String>,
}

/// GET /api/v1/vault/documents/{id}/chunks — the document's parent
/// chunks with the augmentation metadata generated at ingest
/// (keywords, generated questions, hyperedge membership).
pub async fn document_chunks(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<ChunkAugView>>, ApiError> {
    let doc = state
        .rag
        .list_documents()
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| legal_rag_common::Error::NotFound(format!("document {id}")))?;
    crate::routes::matters::doc_visible(&state, &user, &doc.id)
        .then_some(())
        .ok_or_else(|| legal_rag_common::Error::NotFound(format!("document {id}")))?;
    let all = state.store.all_chunks().await?;
    let mut chunks: Vec<ChunkAugView> = all
        .into_iter()
        .filter(|c| c.doc_id == id)
        .filter(|c| {
            !matches!(
                c.metadata.get("pc").map(|s| s.as_str()),
                Some("child") | Some("aug") | Some("hyper")
            )
        })
        .map(|c| ChunkAugView {
            id: c.id.clone(),
            ordinal: c.ordinal,
            text_preview: c.text.chars().take(220).collect(),
            aug_keywords: c.metadata.get("aug_keywords").cloned(),
            aug_questions: c.metadata.get("aug_questions").cloned(),
            hyper_id: c.metadata.get("hyper_id").cloned(),
        })
        .collect();
    chunks.sort_by_key(|c| c.ordinal);
    Ok(Json(chunks))
}

#[derive(Deserialize)]
pub struct SearchDocsQuery {
    pub q: String,
}

#[derive(Serialize)]
pub struct DocSearchHit {
    pub doc_id: String,
    pub title: String,
    pub matches: usize,
    pub title_match: bool,
    pub snippets: Vec<String>,
}

/// Keyword search across the corpus — titles and full document text —
/// returning context snippets around each match. (Semantic questions
/// belong to Ask; this is "find the document that mentions X".)
pub async fn search_documents(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
    Query(q): Query<SearchDocsQuery>,
) -> Json<Vec<DocSearchHit>> {
    let needle = q.q.trim().to_lowercase();
    if needle.chars().count() < 2 {
        return Json(Vec::new());
    }
    let snap = |text: &str, mut i: usize, back: bool| -> usize {
        // Snap a byte index to a char boundary.
        if back {
            while i > 0 && !text.is_char_boundary(i) {
                i -= 1;
            }
        } else {
            while i < text.len() && !text.is_char_boundary(i) {
                i += 1;
            }
        }
        i
    };
    let mut hits: Vec<DocSearchHit> = Vec::new();
    for doc in state.rag.list_documents() {
        let title_match = doc.title.to_lowercase().contains(&needle);
        let text = state
            .rag
            .document_text(&doc.id, 200_000)
            .await
            .unwrap_or_default();
        let lower = text.to_lowercase();
        let mut snippets: Vec<String> = Vec::new();
        let mut count = 0usize;
        for (idx, _) in lower.match_indices(&needle) {
            count += 1;
            if snippets.len() < 3 {
                let start = snap(&text, idx.saturating_sub(90), true);
                let end = snap(&text, (idx + needle.len() + 110).min(text.len()), false);
                let mut snippet = text[start..end].replace('\n', " ");
                if start > 0 {
                    snippet = format!("…{snippet}");
                }
                if end < text.len() {
                    snippet.push('…');
                }
                snippets.push(snippet);
            }
        }
        if count > 0 || title_match {
            hits.push(DocSearchHit {
                doc_id: doc.id,
                title: doc.title,
                matches: count,
                title_match,
                snippets,
            });
        }
    }
    hits.sort_by(|a, b| {
        (b.title_match, b.matches)
            .cmp(&(a.title_match, a.matches))
            .then_with(|| a.title.cmp(&b.title))
    });
    hits.truncate(20);
    Json(hits)
}

/// Remove a document from the index.
pub async fn delete_document(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<HashMap<String, usize>>, ApiError> {
    let removed = state.rag.remove_document(&id).await?;
    // Unfile it from whichever matter held it.
    if let Some(mid) = state.matters.matter_of_document(&id) {
        state.matters.remove_document(&mid, &id);
    }
    Ok(Json(HashMap::from([("removed_chunks".to_string(), removed)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/../ingest/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"))
    }

    #[test]
    fn s3_split_skips_encrypted_pdfs_and_keeps_the_rest() {
        let items = vec![
            ("plain.pdf".to_string(), fixture("plain.pdf")),
            ("enc-aes256.pdf".to_string(), fixture("enc-aes256.pdf")),
            ("notes.txt".to_string(), b"plain text, never probed".to_vec()),
        ];
        let (kept, skipped) = split_encrypted_s3(items);
        assert_eq!(
            kept.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            vec!["plain.pdf", "notes.txt"]
        );
        assert_eq!(skipped, vec!["enc-aes256.pdf".to_string()]);
    }
}
