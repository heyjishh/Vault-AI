//! Vector storage: trait + in-memory brute-force cosine index.

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use legal_rag_common::Result;
use serde::{Deserialize, Serialize};

/// An embedded chunk ready for retrieval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRecord {
    pub id: String,
    pub doc_id: String,
    pub ordinal: usize,
    pub text: String,
    pub embedding: Vec<f32>,
    pub metadata: HashMap<String, String>,
}

/// A chunk with its retrieval score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredChunk {
    pub record: ChunkRecord,
    pub score: f32,
}

/// Parent-child chunking: parents that were split into child chunks are
/// context units only (`metadata.pc == "parent"`) — retrieval matches
/// their children (`metadata.pc == "child"`, `metadata.parent_id` set)
/// and expands hits back to the parent for context. Records without the
/// marker (small parents, and every pre-feature record) are searchable
/// as-is.
pub fn is_searchable(record: &ChunkRecord) -> bool {
    record.metadata.get("pc").map(|v| v != "parent").unwrap_or(true)
}

/// The parent chunk id if this record is a child chunk.
pub fn parent_id(record: &ChunkRecord) -> Option<&str> {
    record.metadata.get("parent_id").map(|s| s.as_str())
}

/// Vector index backend. Implementations: in-memory (here), Qdrant/pgvector (future).
#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn upsert(&self, records: Vec<ChunkRecord>) -> Result<()>;
    async fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<ScoredChunk>>;
    /// Fetch records by id (graph-driven retrieval).
    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<ChunkRecord>>;
    async fn len(&self) -> usize;
    async fn remove_doc(&self, doc_id: &str) -> Result<usize>;
    /// All chunks of one document, in reading order (Interact feature).
    async fn chunks_of_doc(&self, doc_id: &str) -> Result<Vec<ChunkRecord>>;
    /// Every chunk id in the store (cheap corpus fingerprint input).
    async fn chunk_ids(&self) -> Result<Vec<String>>;
    /// Every chunk in the store. Backends that keep embeddings in a
    /// separate column may return records with an empty `embedding` —
    /// consumers (BM25 index, fingerprints) must not rely on it.
    async fn all_chunks(&self) -> Result<Vec<ChunkRecord>>;
}

pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Brute-force in-memory index. Fine for small corpora; swap for ANN at scale.
#[derive(Debug, Default)]
pub struct InMemoryVectorStore {
    records: RwLock<Vec<ChunkRecord>>,
}

impl InMemoryVectorStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot all chunk records for disk persistence.
    pub fn snapshot(&self) -> Vec<ChunkRecord> {
        self.records.read().expect("lock poisoned").clone()
    }

    /// Restore records from a disk snapshot.
    pub fn restore(&self, records: Vec<ChunkRecord>) {
        *self.records.write().expect("lock poisoned") = records;
    }
}

#[async_trait]
impl VectorStore for InMemoryVectorStore {
    async fn upsert(&self, records: Vec<ChunkRecord>) -> Result<()> {
        let mut guard = self.records.write().expect("lock poisoned");
        for record in records {
            if let Some(existing) = guard.iter_mut().find(|r| r.id == record.id) {
                *existing = record;
            } else {
                guard.push(record);
            }
        }
        Ok(())
    }

    async fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<ScoredChunk>> {
        let guard = self.records.read().expect("lock poisoned");
        let mut scored: Vec<ScoredChunk> = guard
            .iter()
            .map(|r| ScoredChunk { record: r.clone(), score: cosine(query, &r.embedding) })
            .collect();
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored)
    }

    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<ChunkRecord>> {
        let wanted: std::collections::HashSet<&str> =
            ids.iter().map(|s| s.as_str()).collect();
        let guard = self.records.read().expect("lock poisoned");
        Ok(guard
            .iter()
            .filter(|r| wanted.contains(r.id.as_str()))
            .cloned()
            .collect())
    }

    async fn len(&self) -> usize {
        self.records.read().expect("lock poisoned").len()
    }

    async fn remove_doc(&self, doc_id: &str) -> Result<usize> {
        let mut guard = self.records.write().expect("lock poisoned");
        let before = guard.len();
        guard.retain(|r| r.doc_id != doc_id);
        Ok(before - guard.len())
    }

    async fn chunks_of_doc(&self, doc_id: &str) -> Result<Vec<ChunkRecord>> {
        let guard = self.records.read().expect("lock poisoned");
        let mut chunks: Vec<ChunkRecord> =
            guard.iter().filter(|r| r.doc_id == doc_id).cloned().collect();
        chunks.sort_by_key(|r| r.ordinal);
        Ok(chunks)
    }

    async fn chunk_ids(&self) -> Result<Vec<String>> {
        let guard = self.records.read().expect("lock poisoned");
        Ok(guard.iter().map(|r| r.id.clone()).collect())
    }

    async fn all_chunks(&self) -> Result<Vec<ChunkRecord>> {
        Ok(self.records.read().expect("lock poisoned").clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, embedding: Vec<f32>) -> ChunkRecord {
        ChunkRecord {
            id: id.into(),
            doc_id: "d1".into(),
            ordinal: 0,
            text: id.into(),
            embedding,
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn upsert_search_remove() {
        let store = InMemoryVectorStore::new();
        store
            .upsert(vec![
                rec("a", vec![1.0, 0.0]),
                rec("b", vec![0.0, 1.0]),
                rec("c", vec![0.7, 0.7]),
            ])
            .await
            .unwrap();
        assert_eq!(store.len().await, 3);
        let hits = store.search(&[1.0, 0.0], 2).await.unwrap();
        assert_eq!(hits[0].record.id, "a");
        assert_eq!(hits.len(), 2);
        assert_eq!(store.remove_doc("d1").await.unwrap(), 3);
        assert_eq!(store.len().await, 0);
    }
}
