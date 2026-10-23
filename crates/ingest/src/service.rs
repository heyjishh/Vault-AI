//! Ingestion orchestration: bytes/S3 -> Document -> chunks.

use legal_rag_common::{Config, Error, Result};
use tracing::info;

use crate::chunk::{Chunk, Chunker};
use crate::document::Document;
use crate::extract::extract_text;
use crate::s3::S3Source;

/// Coordinates document intake for the vault.
#[derive(Clone)]
pub struct IngestService {
    chunker: Chunker,
    s3: Option<S3Source>,
    s3_prefix: String,
}

impl IngestService {
    pub async fn new(config: &Config) -> Result<Self> {
        let (s3, s3_prefix) = match &config.s3 {
            Some(s3c) => {
                let source = S3Source::connect(&s3c.bucket, &s3c.region).await?;
                (Some(source), s3c.prefix.clone())
            }
            None => (None, String::new()),
        };
        Ok(Self {
            chunker: Chunker::new(config.chunk_size, config.chunk_overlap),
            s3,
            s3_prefix,
        })
    }

    /// For tests: build without touching S3.
    pub fn for_tests(chunk_size: usize, chunk_overlap: usize) -> Self {
        Self {
            chunker: Chunker::new(chunk_size, chunk_overlap),
            s3: None,
            s3_prefix: String::new(),
        }
    }

    pub fn s3_configured(&self) -> bool {
        self.s3.is_some()
    }

    /// List object keys under a prefix (for batch ingestion).
    pub async fn list_s3_keys(&self, prefix_override: Option<&str>) -> Result<Vec<String>> {
        let s3 = self.s3.as_ref().ok_or_else(|| Error::Config("S3 is not configured".into()))?;
        let prefix = prefix_override.unwrap_or(&self.s3_prefix);
        s3.list_keys(prefix).await
    }

    /// Download one S3 object as raw bytes with its source URI.
    pub async fn download_s3_object(&self, key: &str) -> Result<(String, Vec<u8>)> {
        let s3 = self.s3.as_ref().ok_or_else(|| Error::Config("S3 is not configured".into()))?;
        let bytes = s3.download(key).await?;
        Ok((format!("s3://{}/{}", s3.bucket(), key), bytes))
    }

    /// Ingest raw bytes as one document.
    pub fn ingest_bytes(
        &self,
        source_uri: &str,
        title: &str,
        file_name: &str,
        bytes: &[u8],
    ) -> Result<Document> {
        let text = extract_text(file_name, bytes)?;
        if text.trim().is_empty() {
            return Err(Error::BadRequest("document has no extractable text".into()));
        }
        let mut doc = Document::new(source_uri, title, text);
        doc.metadata.insert("file_name".into(), file_name.into());
        info!(doc_id = %doc.id, source = %source_uri, chars = doc.text.len(), "ingested document");
        Ok(doc)
    }

    /// Ingest every object under the configured S3 prefix (or an override).
    pub async fn ingest_s3(&self, prefix_override: Option<&str>) -> Result<Vec<Document>> {
        let s3 = self.s3.as_ref().ok_or_else(|| Error::Config("S3 is not configured".into()))?;
        let prefix = prefix_override.unwrap_or(&self.s3_prefix);
        let keys = s3.list_keys(prefix).await?;
        let mut docs = Vec::new();
        for key in keys {
            let bytes = s3.download(&key).await?;
            let source_uri = format!("s3://{}/{}", s3.bucket(), key);
            match self.ingest_bytes(&source_uri, &key, &key, &bytes) {
                Ok(doc) => docs.push(doc),
                Err(e) => tracing::warn!(key = %key, error = %e, "skipping object"),
            }
        }
        Ok(docs)
    }

    /// Split a document into chunks.
    pub fn chunk_document(&self, doc: &Document) -> Vec<Chunk> {
        self.chunker.chunk(&doc.id, &doc.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_and_chunk_text() {
        let svc = IngestService::for_tests(80, 10);
        let doc = svc
            .ingest_bytes("mem://c1", "Contract", "contract.txt", b"Clause 1. Payment due.\n\nClause 2. Termination rights.")
            .unwrap();
        let chunks = svc.chunk_document(&doc);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|c| c.doc_id == doc.id));
    }
}
