//! Batch ingestion — mirrors `raganything.batch.BatchMixin`.
//!
//! `process_documents_batch` runs `process_document_complete` over many files
//! with concurrency capped at `max_concurrent_files` and an extension filter,
//! like RAG-Anything's folder/batch processing.

use std::sync::Arc;

use legal_rag_common::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tracing::{info, warn};

use crate::pipeline::{ProcessReport, RagAnything};

/// One file's batch outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchItemResult {
    pub file_name: String,
    pub doc_id: Option<String>,
    pub success: bool,
    pub error: Option<String>,
    pub chunks: usize,
    pub modal_processed: usize,
}

/// Filter a file list to supported extensions (case-insensitive).
pub fn filter_supported_files<'a>(
    files: &'a [String],
    supported: &[String],
) -> Vec<&'a String> {
    files
        .iter()
        .filter(|f| {
            let lower = f.to_lowercase();
            supported.iter().any(|e| lower.ends_with(e.as_str()))
        })
        .collect()
}

impl RagAnything {
    /// Process many documents concurrently, capped at `max_concurrent_files`.
    ///
    /// Each item is `(file_name, bytes, optional doc_id)`.
    pub async fn process_documents_batch(
        self: &Arc<Self>,
        items: Vec<(String, Vec<u8>, Option<String>)>,
    ) -> Vec<BatchItemResult> {
        let max_concurrent = self.config().max_concurrent_files.max(1);
        let semaphore = Arc::new(Semaphore::new(max_concurrent));
        let mut handles = Vec::with_capacity(items.len());

        for (file_name, bytes, doc_id) in items {
            let rag = self.clone();
            let permit_slot = semaphore.clone();
            handles.push(tokio::spawn(async move {
                let _permit = permit_slot.acquire_owned().await.expect("semaphore closed");
                let result: Result<ProcessReport> =
                    rag.process_document_complete(&file_name, &bytes, None, doc_id).await;
                match result {
                    Ok(report) => BatchItemResult {
                        file_name,
                        doc_id: Some(report.doc_id),
                        success: true,
                        error: None,
                        chunks: report.chunks,
                        modal_processed: report.modal_processed,
                    },
                    Err(e) => {
                        warn!(file = %file_name, error = %e, "batch item failed");
                        BatchItemResult {
                            file_name,
                            doc_id: None,
                            success: false,
                            error: Some(e.to_string()),
                            chunks: 0,
                            modal_processed: 0,
                        }
                    }
                }
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(item) => results.push(item),
                Err(e) => {
                    warn!(error = %e, "batch task panicked");
                    results.push(BatchItemResult {
                        file_name: "<unknown>".into(),
                        doc_id: None,
                        success: false,
                        error: Some(format!("task failed: {e}")),
                        chunks: 0,
                        modal_processed: 0,
                    });
                }
            }
        }
        let ok = results.iter().filter(|r| r.success).count();
        info!(total = results.len(), succeeded = ok, "batch processing complete");
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RagAnythingConfig;
    use crate::embeddings::MockEmbeddings;
    use crate::graph::InMemoryGraphStore;
    use crate::llm::MockLlm;
    use crate::vector_store::InMemoryVectorStore;

    #[tokio::test]
    async fn batch_processes_supported_files() {
        let rag = Arc::new(RagAnything::new(
            RagAnythingConfig::default(),
            Arc::new(MockEmbeddings::new(8)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(MockLlm::new(
                r#"{"entities":[],"relations":[]}"#,
            )),
            200,
            20,
        ));
        let files = vec!["a.txt".to_string(), "b.md".to_string(), "c.xyz".to_string()];
        let supported = rag.config().supported_file_extensions.clone();
        let kept: Vec<String> =
            filter_supported_files(&files, &supported).into_iter().cloned().collect();
        assert_eq!(kept, vec!["a.txt".to_string(), "b.md".to_string()]);

        let items = kept
            .into_iter()
            .enumerate()
            .map(|(i, f)| {
                (
                    f,
                    format!("Contract text about obligations, file number {i}.").into_bytes(),
                    None,
                )
            })
            .collect();
        let results = rag.process_documents_batch(items).await;
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.success));
        assert!(results.iter().all(|r| r.chunks > 0));
    }

    /// The Interact upload loop's contract at the pipeline level: a
    /// batch mixing a good text file, a corrupt PDF and a good
    /// markdown file yields per-item results — the corrupt file
    /// fails with its own error and does not block the others, and
    /// both good documents land in the corpus, immediately usable.
    #[tokio::test]
    async fn batch_mixed_types_isolate_failures() {
        let rag = Arc::new(RagAnything::new(
            RagAnythingConfig::default(),
            Arc::new(MockEmbeddings::new(8)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(MockLlm::new(r#"{"entities":[],"relations":[]}"#)),
            200,
            20,
        ));
        let items = vec![
            (
                "notes.txt".to_string(),
                b"Plain text about limitation periods in contract claims.".to_vec(),
                None,
            ),
            (
                "broken.pdf".to_string(),
                b"this is not a pdf at all, just garbage bytes".to_vec(),
                None,
            ),
            (
                "memo.md".to_string(),
                b"# Memo\n\nMarkdown about stamp duty on agreements.".to_vec(),
                None,
            ),
        ];
        let results = rag.process_documents_batch(items).await;
        assert_eq!(results.len(), 3);
        let by_name = |n: &str| results.iter().find(|r| r.file_name == n).unwrap();
        assert!(by_name("notes.txt").success, "{:?}", by_name("notes.txt").error);
        assert!(by_name("memo.md").success, "{:?}", by_name("memo.md").error);
        let broken = by_name("broken.pdf");
        assert!(!broken.success);
        assert!(broken.error.is_some(), "the failed file carries its own error");
        assert!(broken.doc_id.is_none());
        // Both good documents are in the corpus with readable text.
        assert_eq!(rag.list_documents().len(), 2);
        let txt_id = by_name("notes.txt").doc_id.clone().unwrap();
        let text = rag.document_text(&txt_id, 10_000).await.unwrap();
        assert!(text.contains("limitation periods"));
    }
}
