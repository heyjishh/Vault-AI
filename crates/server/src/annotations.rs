//! Verified annotations: human-curated answers to real questions.
//!
//! A curator takes a captured query (or writes a question by hand),
//! corrects the answer, and the corrected answer is embedded and
//! SERVED verbatim to semantically matching future questions — the
//! verified tier that sits above the automatic semantic answer cache.
//! Each annotation binds to the corpus fingerprint at creation time:
//! when the corpus changes, the annotation stops serving (and shows
//! as stale in the admin) until a curator re-verifies it. Persisted to
//! `annotations.json` in the data directory.

use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use legal_rag_rag::Citation;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationRecord {
    pub id: String,
    pub question: String,
    /// The verified answer text (markdown).
    pub answer: String,
    pub citations: Vec<Citation>,
    /// Question embedding (bge-m3), persisted so serving needs no
    /// provider call at boot.
    pub qvec: Vec<f32>,
    /// None = serves unscoped queries; Some = only that brief's
    /// document set.
    pub scope_doc_ids: Option<Vec<String>>,
    /// Corpus fingerprint at creation; mismatch = stale, not served.
    pub corpus_fp: String,
    /// manual | captured
    pub source: String,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub enabled: bool,
}

impl AnnotationRecord {
    pub fn to_rag(&self) -> legal_rag_rag::Annotation {
        legal_rag_rag::Annotation {
            id: self.id.clone(),
            question: self.question.clone(),
            qvec: self.qvec.clone(),
            answer: self.answer.clone(),
            citations: self.citations.clone(),
            scope_docs: self.scope_doc_ids.clone(),
            corpus_fp: self.corpus_fp.clone(),
            enabled: self.enabled,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AnnotationData {
    records: Vec<AnnotationRecord>,
}

pub struct AnnotationStore {
    data: RwLock<AnnotationData>,
    path: PathBuf,
}

impl AnnotationStore {
    pub fn load(path: PathBuf) -> Self {
        let data: AnnotationData = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { data: RwLock::new(data), path }
    }

    fn save(&self) {
        if let Ok(data) = self.data.read() {
            if let Ok(bytes) = serde_json::to_vec_pretty(&*data) {
                let tmp = self.path.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(tmp, &self.path);
                }
            }
        }
    }

    pub fn list(&self) -> Vec<AnnotationRecord> {
        self.data.read().unwrap().records.clone()
    }

    /// The full set, converted for the query service.
    pub fn rag_set(&self) -> Vec<legal_rag_rag::Annotation> {
        self.list().iter().map(|r| r.to_rag()).collect()
    }

    pub fn add(&self, record: AnnotationRecord) -> AnnotationRecord {
        {
            let mut data = self.data.write().unwrap();
            data.records.push(record.clone());
        }
        self.save();
        record
    }

    pub fn delete(&self, id: &str) -> bool {
        let removed = {
            let mut data = self.data.write().unwrap();
            let before = data.records.len();
            data.records.retain(|r| r.id != id);
            data.records.len() != before
        };
        if removed {
            self.save();
        }
        removed
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> bool {
        let found = {
            let mut data = self.data.write().unwrap();
            if let Some(r) = data.records.iter_mut().find(|r| r.id == id) {
                r.enabled = enabled;
                true
            } else {
                false
            }
        };
        if found {
            self.save();
        }
        found
    }
}

/// The corpus fingerprint the query service uses: hash of the sorted
/// chunk-id set. Kept byte-identical with `QueryService`'s computation.
pub fn corpus_fingerprint(chunk_ids: &[String]) -> String {
    let mut ids: Vec<&str> = chunk_ids.iter().map(|s| s.as_str()).collect();
    ids.sort_unstable();
    legal_rag_rag::cache::hash_key(&[&ids.join(",")])
}
