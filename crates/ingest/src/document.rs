//! Normalized document model.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use legal_rag_common::{new_id, now_utc};
use serde::{Deserialize, Serialize};

/// A document normalized to plain text, ready for chunking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub source_uri: String,
    pub title: String,
    pub text: String,
    pub byte_len: usize,
    pub metadata: HashMap<String, String>,
    pub ingested_at: DateTime<Utc>,
}

impl Document {
    pub fn new(source_uri: impl Into<String>, title: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            id: new_id(),
            source_uri: source_uri.into(),
            title: title.into(),
            byte_len: text.len(),
            text,
            metadata: HashMap::new(),
            ingested_at: now_utc(),
        }
    }
}
