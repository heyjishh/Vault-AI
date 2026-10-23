//! `RagAnything` — the document pipeline, mirroring `raganything.RAGAnything`.
//!
//! `process_document_complete` follows RAG-Anything's staged workflow:
//! 1. parse → typed content blocks (text / image / table / equation)
//! 2. separate text from multimodal content (with page provenance)
//! 3. insert text: chunk → embed → vector upsert → LLM graph extraction
//! 4. modal processors describe each multimodal block (with extracted
//!    context) and index the descriptions as entities + chunks
//! 5. document status tracking (pending → processing → ready / failed)

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use legal_rag_common::Result;
use legal_rag_ingest::Chunker;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::config::RagAnythingConfig;
use crate::content::separate_content;
use crate::context::ContextExtractor;
use crate::embeddings::Embeddings;
use crate::graph::{extract_graph_elements, Entity, GraphStore};
use crate::llm::LlmClient;
use crate::parser::get_parser;
use crate::processors::{
    EquationModalProcessor, GenericModalProcessor, ImageModalProcessor, ModalProcessor,
    TableModalProcessor,
};
use crate::vector_store::{ChunkRecord, VectorStore};

/// One chunk of a document extracted without indexing (confidential vault).
#[derive(Debug, Clone)]
pub struct ExtractedChunk {
    pub text: String,
    pub vector: Vec<f32>,
}

/// Processing state of one document, like LightRAG's `DocStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocStatus {
    Pending,
    Processing,
    Ready,
    Failed,
}

/// Status record kept per document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocStatusRecord {
    pub doc_id: String,
    pub file_name: String,
    pub status: DocStatus,
    pub error: Option<String>,
    pub chunks: usize,
    pub modal_items: usize,
    pub updated_at: DateTime<Utc>,
}

/// Legal-validity metadata for a document: where it applies and when it
/// is in force. All fields optional — an unset field means "unknown",
/// and unknown never excludes a document from retrieval (only explicit
/// metadata filters). Dates are ISO `YYYY-MM-DD` strings, compared
/// lexicographically. `effective_to` is the first day the document is
/// NO longer in force.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DocMeta {
    /// Jurisdiction code, e.g. "IN", "IN-DL", "US-CA".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction: Option<String>,
    /// First day in force (inclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_from: Option<String>,
    /// First day no longer in force (exclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_to: Option<String>,
    /// What replaced this document (title / citation note), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

impl DocMeta {
    /// Is this document eligible under an as-of date and/or jurisdiction
    /// filter? A document is excluded only when its own metadata
    /// contradicts the filter: not yet in force, no longer in force, or
    /// a different (explicit) jurisdiction.
    pub fn eligible(&self, as_of: Option<&str>, jurisdiction: Option<&str>) -> bool {
        if let Some(day) = as_of {
            if let Some(from) = &self.effective_from {
                if day < from.as_str() {
                    return false;
                }
            }
            if let Some(to) = &self.effective_to {
                if day >= to.as_str() {
                    return false;
                }
            }
        }
        if let Some(want) = jurisdiction {
            if let Some(have) = &self.jurisdiction {
                if !have.eq_ignore_ascii_case(want) {
                    return false;
                }
            }
        }
        true
    }

    /// Human-readable staleness note for citations, when the document is
    /// known to have been replaced.
    pub fn stale_note(&self) -> Option<String> {
        self.superseded_by
            .as_ref()
            .map(|s| format!("Superseded by {s}"))
    }
}

/// Stored metadata for an indexed document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentInfo {
    pub id: String,
    pub title: String,
    pub source_uri: String,
    pub chars: usize,
    pub chunks: usize,
    pub chunk_ids: Vec<String>,
    pub ingested_at: DateTime<Utc>,
    /// Jurisdiction / in-force metadata (empty for legacy documents).
    #[serde(default)]
    pub meta: DocMeta,
}

/// Report for one `process_document_complete` run.
#[derive(Debug, Clone)]
pub struct ProcessReport {
    pub doc_id: String,
    pub chunks: usize,
    pub modal_processed: usize,
    pub entities: usize,
    pub relations: usize,
}

/// The RAG-Anything pipeline orchestrator.
pub struct RagAnything {
    config: RagAnythingConfig,
    embeddings: Arc<dyn Embeddings>,
    store: Arc<dyn VectorStore>,
    graph: Arc<dyn GraphStore>,
    llm: Arc<dyn LlmClient>,
    processors: HashMap<String, Arc<dyn ModalProcessor>>,
    chunker: Chunker,
    doc_status: RwLock<HashMap<String, DocStatusRecord>>,
    documents: RwLock<HashMap<String, DocumentInfo>>,
}

impl RagAnything {
    pub fn new(
        config: RagAnythingConfig,
        embeddings: Arc<dyn Embeddings>,
        store: Arc<dyn VectorStore>,
        graph: Arc<dyn GraphStore>,
        llm: Arc<dyn LlmClient>,
        chunk_size: usize,
        chunk_overlap: usize,
    ) -> Self {
        let context_extractor = ContextExtractor::new(&config);
        let mut processors: HashMap<String, Arc<dyn ModalProcessor>> = HashMap::new();
        if config.enable_image_processing {
            processors.insert(
                "image".into(),
                Arc::new(ImageModalProcessor::new(
                    llm.clone(),
                    ContextExtractor::new(&config),
                )),
            );
        }
        if config.enable_table_processing {
            processors.insert(
                "table".into(),
                Arc::new(TableModalProcessor::new(
                    llm.clone(),
                    ContextExtractor::new(&config),
                )),
            );
        }
        if config.enable_equation_processing {
            processors.insert(
                "equation".into(),
                Arc::new(EquationModalProcessor::new(
                    llm.clone(),
                    ContextExtractor::new(&config),
                )),
            );
        }
        // Generic fallback is always registered, like RAG-Anything.
        processors.insert(
            "generic".into(),
            Arc::new(GenericModalProcessor::new(llm.clone(), context_extractor)),
        );
        Self {
            config,
            embeddings,
            store,
            graph,
            llm,
            processors,
            chunker: Chunker::new(chunk_size, chunk_overlap),
            doc_status: RwLock::new(HashMap::new()),
            documents: RwLock::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &RagAnythingConfig {
        &self.config
    }

    pub fn is_indexed(&self, doc_id: &str) -> bool {
        matches!(
            self.doc_status.read().expect("lock poisoned").get(doc_id),
            Some(r) if r.status == DocStatus::Ready
        )
    }

    pub fn is_document_fully_processed(&self, doc_id: &str) -> bool {
        self.is_indexed(doc_id)
    }

    pub fn get_document_processing_status(&self, doc_id: &str) -> Option<DocStatusRecord> {
        self.doc_status.read().expect("lock poisoned").get(doc_id).cloned()
    }

    pub fn doc_info(&self, doc_id: &str) -> Option<DocumentInfo> {
        self.documents.read().expect("lock poisoned").get(doc_id).cloned()
    }

    pub fn doc_title(&self, doc_id: &str) -> Option<String> {
        self.doc_info(doc_id).map(|d| d.title)
    }

    /// Snapshot the document registry (documents + statuses) for disk persistence.
    pub fn snapshot_documents(
        &self,
    ) -> (
        std::collections::HashMap<String, DocumentInfo>,
        std::collections::HashMap<String, DocStatusRecord>,
    ) {
        (
            self.documents.read().expect("lock poisoned").clone(),
            self.doc_status.read().expect("lock poisoned").clone(),
        )
    }

    /// Restore the document registry from a disk snapshot.
    pub fn restore_documents(
        &self,
        documents: std::collections::HashMap<String, DocumentInfo>,
        status: std::collections::HashMap<String, DocStatusRecord>,
    ) {
        *self.documents.write().expect("lock poisoned") = documents;
        *self.doc_status.write().expect("lock poisoned") = status;
    }

    pub fn list_documents(&self) -> Vec<DocumentInfo> {
        let mut docs: Vec<DocumentInfo> =
            self.documents.read().expect("lock poisoned").values().cloned().collect();
        docs.sort_by(|a, b| b.ingested_at.cmp(&a.ingested_at));
        docs
    }

    fn set_status(
        &self,
        doc_id: &str,
        file_name: &str,
        status: DocStatus,
        error: Option<String>,
        chunks: usize,
        modal_items: usize,
    ) {
        self.doc_status.write().expect("lock poisoned").insert(
            doc_id.to_string(),
            DocStatusRecord {
                doc_id: doc_id.to_string(),
                file_name: file_name.to_string(),
                status,
                error,
                chunks,
                modal_items,
                updated_at: Utc::now(),
            },
        );
    }

    fn file_reference(&self, file_name: &str) -> String {
        if self.config.use_full_path {
            file_name.to_string()
        } else {
            file_name.rsplit(['/', '\\']).next().unwrap_or(file_name).to_string()
        }
    }

    /// Content-based doc id, like `_generate_content_based_doc_id`.
    fn content_doc_id(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("doc-{:x}", hasher.finalize())[..16].to_string()
    }

    /// Complete document workflow: parse → text insert → multimodal processing.
    ///
    /// `title` overrides the display title (defaults to the file name).
    pub async fn process_document_complete(
        &self,
        file_name: &str,
        bytes: &[u8],
        title: Option<String>,
        doc_id: Option<String>,
    ) -> Result<ProcessReport> {
        let doc_id = doc_id.unwrap_or_else(|| Self::content_doc_id(bytes));
        let file_ref = self.file_reference(file_name);

        if self.is_indexed(&doc_id) {
            info!(doc_id = %doc_id, "document already processed, skipping");
            return Ok(ProcessReport {
                doc_id,
                chunks: 0,
                modal_processed: 0,
                entities: 0,
                relations: 0,
            });
        }
        self.set_status(&doc_id, &file_ref, DocStatus::Processing, None, 0, 0);

        let result = self.process_inner(&doc_id, &file_ref, title, file_name, bytes).await;
        match result {
            Ok(report) => {
                self.set_status(
                    &doc_id,
                    &file_ref,
                    DocStatus::Ready,
                    None,
                    report.chunks,
                    report.modal_processed,
                );
                info!(doc_id = %doc_id, chunks = report.chunks, modal = report.modal_processed,
                      entities = report.entities, relations = report.relations,
                      "document processing complete");
                Ok(report)
            }
            Err(e) => {
                self.set_status(
                    &doc_id,
                    &file_ref,
                    DocStatus::Failed,
                    Some(e.to_string()),
                    0,
                    0,
                );
                Err(e)
            }
        }
    }

    /// Parse + chunk + embed WITHOUT indexing anything: no vector upsert, no
    /// graph extraction, no document-registry entry. The confidential vault
    /// uses this and stores the results encrypted under the owner's key —
    /// private content never touches the shared corpus.
    pub async fn extract_chunks(
        &self,
        file_name: &str,
        bytes: &[u8],
    ) -> Result<Vec<ExtractedChunk>> {
        self.extract_chunks_with(file_name, bytes, self.embeddings.clone()).await
    }

    /// `extract_chunks` with a caller-chosen embeddings provider — the
    /// confidential vault passes its dedicated (local) provider here so
    /// private text is never embedded by the main/cloud provider.
    pub async fn extract_chunks_with(
        &self,
        file_name: &str,
        bytes: &[u8],
        embeddings: Arc<dyn Embeddings>,
    ) -> Result<Vec<ExtractedChunk>> {
        let parser = get_parser(&self.config.parser, file_name, &self.config)?;
        let file_name_owned = file_name.to_string();
        let bytes_owned = bytes.to_vec();
        let blocks = tokio::task::spawn_blocking(move || parser.parse(&file_name_owned, &bytes_owned))
            .await
            .map_err(|e| {
                legal_rag_common::Error::BadRequest(format!("parser task failed: {e}"))
            })??;
        let (text_content, _modal, _intervals) = separate_content(&blocks);
        if text_content.trim().is_empty() {
            return Err(legal_rag_common::Error::BadRequest(
                "no extractable text in this document".to_string(),
            ));
        }
        let chunks = self.chunker.chunk("private-doc", &text_content);
        let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
        let vectors = embeddings.embed(&texts).await?;
        if vectors.len() != texts.len() {
            return Err(legal_rag_common::Error::Upstream(
                "embedding count mismatch for private document".to_string(),
            ));
        }
        Ok(texts
            .into_iter()
            .zip(vectors)
            .map(|(text, vector)| ExtractedChunk { text, vector })
            .collect())
    }

    async fn process_inner(
        &self,
        doc_id: &str,
        file_ref: &str,
        title: Option<String>,
        file_name: &str,
        bytes: &[u8],
    ) -> Result<ProcessReport> {
        // Step 1: parse → content blocks (blocking: subprocess OCR; run off the async executor).
        let parser = get_parser(&self.config.parser, file_name, &self.config)?;
        let file_name_owned = file_name.to_string();
        let bytes_owned = bytes.to_vec();
        let blocks = tokio::task::spawn_blocking(move || parser.parse(&file_name_owned, &bytes_owned))
            .await
            .map_err(|e| {
                legal_rag_common::Error::BadRequest(format!("parser task failed: {e}"))
            })??;
        if self.config.display_content_stats {
            info!(doc_id = %doc_id, stats = %crate::content::content_stats(&blocks), "parsed document");
        }

        // Step 2: separate text and multimodal content.
        let (text_content, modal_items, _page_intervals) = separate_content(&blocks);

        let mut entities_total = 0usize;
        let mut relations_total = 0usize;
        let mut chunk_ids: Vec<String> = Vec::new();

        // Step 3: insert pure text (chunk → embed → graph).
        let mut chunk_count = 0usize;
        if !text_content.trim().is_empty() {
            let chunks = self.chunker.chunk(doc_id, &text_content);
            let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
            let vectors = self.embeddings.embed(&texts).await?;
            if vectors.len() != chunks.len() {
                warn!(doc_id = %doc_id, "embedding count mismatch, skipping text insert");
            } else {
                let mut records: Vec<ChunkRecord> = chunks
                    .iter()
                    .zip(vectors)
                    .map(|(c, embedding)| ChunkRecord {
                        id: c.id.clone(),
                        doc_id: c.doc_id.clone(),
                        ordinal: c.ordinal,
                        text: c.text.clone(),
                        embedding,
                        metadata: HashMap::new(),
                    })
                    .collect();
                // Parent-child chunking: split each parent into small
                // child chunks — the precise match units retrieval
                // searches — while the parent stays the context unit.
                // Small parents stay searchable as-is. Disable with
                // LEGAL_RAG_PARENT_CHILD=0.
                let mut child_records: Vec<ChunkRecord> = Vec::new();
                if std::env::var("LEGAL_RAG_PARENT_CHILD").map(|v| v != "0").unwrap_or(true) {
                    let target = std::env::var("LEGAL_RAG_CHILD_CHARS")
                        .ok()
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(220)
                        .clamp(80, 800);
                    let mut splits: Vec<(usize, Vec<String>)> = Vec::new();
                    let mut child_texts: Vec<String> = Vec::new();
                    for (i, parent) in records.iter().enumerate() {
                        let pieces = split_children(&parent.text, target);
                        if pieces.len() >= 2 {
                            child_texts.extend(pieces.iter().cloned());
                            splits.push((i, pieces));
                        }
                    }
                    if !child_texts.is_empty() {
                        match self.embeddings.embed(&child_texts).await {
                            Ok(child_vecs) if child_vecs.len() == child_texts.len() => {
                                let mut vec_iter = child_vecs.into_iter();
                                let mut children: Vec<ChunkRecord> = Vec::new();
                                for (pi, pieces) in splits {
                                    let (pid, ordinal, did) = {
                                        let p = &records[pi];
                                        (p.id.clone(), p.ordinal, p.doc_id.clone())
                                    };
                                    records[pi]
                                        .metadata
                                        .insert("pc".to_string(), "parent".to_string());
                                    for (n, piece) in pieces.into_iter().enumerate() {
                                        let mut metadata = HashMap::new();
                                        metadata.insert("pc".to_string(), "child".to_string());
                                        metadata.insert("parent_id".to_string(), pid.clone());
                                        children.push(ChunkRecord {
                                            id: format!("{pid}#c{n}"),
                                            doc_id: did.clone(),
                                            ordinal,
                                            text: piece,
                                            embedding: vec_iter.next().expect("child vector"),
                                            metadata,
                                        });
                                    }
                                }
                                info!(doc_id = %doc_id, children = children.len(), "parent-child chunks indexed");
                                child_records = children;
                            }
                            _ => warn!(doc_id = %doc_id, "child embedding failed; parents stay searchable"),
                        }
                    }
                }
                // Upsert parents + children together; graph extraction and
                // the document registry below stay on parents only.
                let mut all_records = records.clone();
                all_records.extend(child_records);
                self.store.upsert(all_records).await?;
                for record in &records {
                    let (entities, relations) =
                        extract_graph_elements(self.llm.as_ref(), record).await;
                    entities_total += entities.len();
                    relations_total += relations.len();
                    if !entities.is_empty() {
                        self.graph.upsert_entities(entities).await?;
                    }
                    if !relations.is_empty() {
                        self.graph.upsert_relations(relations).await?;
                    }
                }
                chunk_count = records.len();
                chunk_ids.extend(records.iter().map(|r| r.id.clone()));
            }
        }

        // Step 4: multimodal content via specialized processors.
        let mut modal_processed = 0usize;
        for item in &modal_items {
            let kind = item.block.kind();
            let processor = self
                .processors
                .get(kind)
                .or_else(|| self.processors.get("generic"));
            let Some(processor) = processor else { continue };
            match processor.process(&item.block, item.index, &blocks, doc_id).await {
                Ok(processed) => {
                    // Index the description as its own chunk + graph entity.
                    let chunk_id = format!("{doc_id}#modal-{}", item.index);
                    let text =
                        format!("[{}] {}\n\n{}", processed.kind, processed.entity_name, processed.description);
                    let embedding = self.embeddings.embed(&[text.clone()]).await?
                        .into_iter().next().unwrap_or_default();
                    self.store
                        .upsert(vec![ChunkRecord {
                            id: chunk_id.clone(),
                            doc_id: doc_id.to_string(),
                            ordinal: 10_000 + item.index,
                            text,
                            embedding,
                            metadata: HashMap::from([
                                ("modal_kind".to_string(), processed.kind.to_string()),
                                ("entity".to_string(), processed.entity_name.clone()),
                            ]),
                        }])
                        .await?;
                    self.graph
                        .upsert_entities(vec![Entity {
                            name: processed.entity_name.clone(),
                            kind: processed.kind.to_string(),
                            description: processed.description.clone(),
                            chunk_ids: vec![chunk_id.clone()],
                        }])
                        .await?;
                    entities_total += 1;
                    chunk_count += 1;
                    chunk_ids.push(chunk_id);
                    modal_processed += 1;
                }
                Err(e) => {
                    warn!(doc_id = %doc_id, kind = kind, error = %e, "modal processor failed, continuing");
                }
            }
        }

        let chars = text_content.len();
        let display_title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| file_ref.to_string());
        self.documents.write().expect("lock poisoned").insert(
            doc_id.to_string(),
            DocumentInfo {
                id: doc_id.to_string(),
                title: display_title,
                source_uri: file_name.to_string(),
                chars,
                chunks: chunk_count,
                chunk_ids: chunk_ids.clone(),
                ingested_at: Utc::now(),
                meta: DocMeta::default(),
            },
        );

        Ok(ProcessReport {
            doc_id: doc_id.to_string(),
            chunks: chunk_count,
            modal_processed,
            entities: entities_total,
            relations: relations_total,
        })
    }

    /// Replace a document's legal-validity metadata. Returns false when
    /// the document is unknown. The registry snapshot persists it.
    pub fn set_document_meta(&self, doc_id: &str, meta: DocMeta) -> bool {
        let mut docs = self.documents.write().expect("lock poisoned");
        match docs.get_mut(doc_id) {
            Some(info) => {
                info.meta = meta;
                true
            }
            None => false,
        }
    }

    /// One document's metadata, when the document exists.
    pub fn document_meta(&self, doc_id: &str) -> Option<DocMeta> {
        self.documents
            .read()
            .expect("lock poisoned")
            .get(doc_id)
            .map(|d| d.meta.clone())
    }

    pub async fn remove_document(&self, doc_id: &str) -> Result<usize> {
        let chunk_ids = self
            .documents
            .read()
            .expect("lock poisoned")
            .get(doc_id)
            .map(|d| d.chunk_ids.clone())
            .unwrap_or_default();
        let removed = self.store.remove_doc(doc_id).await?;
        if !chunk_ids.is_empty() {
            self.graph.remove_chunks(&chunk_ids).await?;
        }
        self.doc_status.write().expect("lock poisoned").remove(doc_id);
        self.documents.write().expect("lock poisoned").remove(doc_id);
        Ok(removed)
    }

    pub async fn corpus_size(&self) -> usize {
        self.store.len().await
    }

    /// The document's full text (chunks joined in reading order), capped at
    /// `max_chars`. Powers the Interact surface (summary/chronology/chat).
    pub async fn document_text(&self, doc_id: &str, max_chars: usize) -> Result<String> {
        let chunks = self.store.chunks_of_doc(doc_id).await?;
        let mut out = String::new();
        for chunk in chunks {
            // Child chunks duplicate their parent's text — reading order
            // is the parents (plus un-split legacy chunks) only.
            if crate::vector_store::parent_id(&chunk).is_some() {
                continue;
            }
            if out.len() >= max_chars {
                break;
            }
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&chunk.text);
        }
        if out.chars().count() > max_chars {
            out = out.chars().take(max_chars).collect();
        }
        Ok(out)
    }

    /// Resolve a chunk id to its owning document: (document id, full
    /// text). Citations carry chunk ids (record UUIDs), while the
    /// source viewer and Interact need the document behind the chunk.
    pub async fn document_text_of_chunk(
        &self,
        chunk_id: &str,
        max_chars: usize,
    ) -> Result<(String, String)> {
        let record = self
            .store
            .get_by_ids(&[chunk_id.to_string()])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                legal_rag_common::Error::NotFound(format!("chunk {chunk_id}"))
            })?;
        let text = self.document_text(&record.doc_id, max_chars).await?;
        Ok((record.doc_id, text))
    }

    pub fn indexed_doc_ids(&self) -> Vec<String> {
        self.doc_status
            .read()
            .expect("lock poisoned")
            .iter()
            .filter(|(_, r)| r.status == DocStatus::Ready)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Expose internals for the query service and batch runner.
    pub fn backends(
        &self,
    ) -> (
        Arc<dyn Embeddings>,
        Arc<dyn VectorStore>,
        Arc<dyn GraphStore>,
        Arc<dyn LlmClient>,
    ) {
        (
            self.embeddings.clone(),
            self.store.clone(),
            self.graph.clone(),
            self.llm.clone(),
        )
    }
}

/// Segment text into sentences, keeping the terminator with its sentence.
/// Boundaries: `.`, `!`, `?`, and the Devanagari danda `।` (each only when
/// followed by a space or end-of-text — a '.' between digits like "3.5"
/// still closes a usable piece, matching the splitter's historic
/// behaviour), plus newlines. Shared by child-splitting at ingest and by
/// pinpoint (sentence-level) citation extraction at query time.
pub(crate) fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        cur.push(c);
        if matches!(c, '.' | '!' | '?' | '।') {
            if chars.peek() == Some(&' ') || chars.peek().is_none() {
                sentences.push(std::mem::take(&mut cur));
            }
        } else if c == '\n' {
            sentences.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        sentences.push(cur);
    }
    sentences
}

/// Sentence segmentation for pinpoint extraction: collapse line breaks
/// (extracted statute text is hard-wrapped mid-sentence) into spaces
/// first, then split on terminal punctuation only — so a "sentence" is
/// a complete sentence, never a wrapped line fragment.
pub(crate) fn split_sentences_flowing(text: &str) -> Vec<String> {
    let mut flowing = String::with_capacity(text.len());
    let mut last_space = true; // also trims the head
    for c in text.chars() {
        if c.is_whitespace() {
            if !last_space {
                flowing.push(' ');
                last_space = true;
            }
        } else {
            flowing.push(c);
            last_space = false;
        }
    }
    split_sentences(flowing.trim())
}

/// Split a parent chunk into child match-units of roughly `target` chars,
/// cutting at sentence boundaries (`.`, `!`, `?`, and the Devanagari
/// danda `।`). A single sentence longer than 2× target is hard-split at
/// char boundaries. Returns fewer than 2 pieces when the text is already
/// child-sized — callers then leave the parent searchable as-is.
pub(crate) fn split_children(text: &str, target: usize) -> Vec<String> {
    let text = text.trim();
    if text.chars().count() <= target {
        return vec![text.to_string()];
    }
    let sentences = split_sentences(text);
    let mut pieces: Vec<String> = Vec::new();
    let mut buf = String::new();
    for s in sentences {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        if s.chars().count() > target * 2 {
            if !buf.is_empty() {
                pieces.push(std::mem::take(&mut buf));
            }
            let sc: Vec<char> = s.chars().collect();
            for w in sc.chunks(target) {
                pieces.push(w.iter().collect::<String>().trim().to_string());
            }
            continue;
        }
        if !buf.is_empty() && buf.chars().count() + s.chars().count() > target {
            pieces.push(std::mem::take(&mut buf));
        }
        if !buf.is_empty() {
            buf.push(' ');
        }
        buf.push_str(s);
        if buf.chars().count() >= target {
            pieces.push(std::mem::take(&mut buf));
        }
    }
    if !buf.trim().is_empty() {
        pieces.push(buf.trim().to_string());
    }
    pieces.into_iter().filter(|p| !p.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbeddings;
    use crate::graph::InMemoryGraphStore;
    use crate::llm::MockLlm;
    use crate::vector_store::InMemoryVectorStore;

    fn rag() -> RagAnything {
        RagAnything::new(
            RagAnythingConfig::default(),
            Arc::new(MockEmbeddings::new(16)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(MockLlm::new(
                r#"{"entities":[{"name":"Acme","kind":"party","description":"buyer"}],"relations":[]}"#,
            )),
            500,
            50,
        )
    }

    #[test]
    fn doc_meta_eligibility_rules() {
        let mut m = DocMeta::default();
        // No metadata: never excluded.
        assert!(m.eligible(Some("2015-01-01"), Some("IN")));
        m.effective_from = Some("1961-04-01".to_string());
        m.effective_to = Some("2023-04-01".to_string());
        m.jurisdiction = Some("IN".to_string());
        assert!(m.eligible(Some("2000-06-01"), None), "in force mid-range");
        assert!(!m.eligible(Some("1950-01-01"), None), "before effective_from");
        assert!(!m.eligible(Some("2023-04-01"), None), "effective_to is exclusive");
        assert!(m.eligible(Some("2023-03-31"), None), "last day in force");
        assert!(m.eligible(None, Some("in")), "jurisdiction is case-insensitive");
        assert!(!m.eligible(None, Some("US")), "wrong jurisdiction excluded");
        assert!(m.eligible(None, None), "no filters, no exclusion");
        m.superseded_by = Some("the 2023 Amendment Act".to_string());
        assert_eq!(m.stale_note().as_deref(), Some("Superseded by the 2023 Amendment Act"));
    }

    #[test]
    fn split_children_respects_sentences_and_size() {
        let text = "Section 86 deals with the share of a member of an association of persons. \
                    Where tax has already been charged on the association, the member's share shall not be taxed again. \
                    This provision applies notwithstanding anything contained in any other provision of this Act.";
        let pieces = split_children(text, 100);
        assert!(pieces.len() >= 2, "long parent must split: {pieces:?}");
        assert!(pieces.iter().all(|p| p.chars().count() <= 320), "pieces stay near target: {pieces:?}");
        // Every sentence survives intact somewhere in the pieces.
        let joined = pieces.join(" ");
        assert!(joined.contains("shall not be taxed again"));
        // Child-sized text is returned whole (caller keeps it searchable).
        assert_eq!(split_children("Short text.", 220).len(), 1);
        // Hindi danda boundaries work too.
        let hindi = "धारा 86 आयकर अधिनियम से संबंधित है। यह सदस्य के हिस्से पर कर का प्रावधान करती है। इसका उद्देश्य दोहरे कराधान को रोकना है।";
        assert!(split_children(hindi, 60).len() >= 2);
    }

    #[tokio::test]
    async fn ingest_creates_child_chunks_for_long_parents() {
        let rag = rag();
        let para = "The Commissioner shall, before making any assessment under this section, \
                    give the assessee a reasonable opportunity of being heard in the matter. \
                    Any order passed without such opportunity shall be void and of no effect in law. \
                    The assessee may prefer an appeal against the order within thirty days of its communication.";
        let text = format!("# Assessment\n\n{}", para.repeat(3));
        rag.process_document_complete("assessment.md", text.as_bytes(), None, None).await.unwrap();
        let (_e, store, _g, _l) = rag.backends();
        let all = store.all_chunks().await.unwrap();
        let children: Vec<_> = all.iter().filter(|c| crate::vector_store::parent_id(c).is_some()).collect();
        let parents: Vec<_> = all.iter().filter(|c| c.metadata.get("pc").map(|v| v == "parent").unwrap_or(false)).collect();
        assert!(!parents.is_empty(), "long parents must be tagged as context-only");
        assert!(children.len() >= 2, "children must be indexed, got {}", children.len());
        assert!(children.iter().all(|c| parents.iter().any(|p| Some(p.id.as_str()) == crate::vector_store::parent_id(c))));
        // Reading text (Interact) shows parents only — no duplicated spans.
        let doc_id = parents[0].doc_id.clone();
        let full = rag.document_text(&doc_id, 100_000).await.unwrap();
        assert_eq!(full.matches("reasonable opportunity").count(), 3, "each repetition appears once");
    }

    #[tokio::test]
    async fn process_complete_indexes_text_and_table() {
        let rag = rag();
        let bytes = b"# Services Agreement\n\nAcme Corp hires Beta LLC.\n\n| item | price |\n|---|---|\n| work | 100 |\n";
        let report = rag.process_document_complete("agreement.md", bytes, None, None).await.unwrap();
        assert!(report.chunks >= 2); // text chunk + table modal chunk
        assert_eq!(report.modal_processed, 1);
        assert!(rag.is_document_fully_processed(&report.doc_id));
        let status = rag.get_document_processing_status(&report.doc_id).unwrap();
        assert_eq!(status.status, DocStatus::Ready);
    }

    #[tokio::test]
    async fn process_is_idempotent() {
        let rag = rag();
        let bytes = b"Hello world, this is a test document.";
        let r1 = rag.process_document_complete("a.txt", bytes, None, None).await.unwrap();
        let r2 = rag.process_document_complete("a.txt", bytes, None, None).await.unwrap();
        assert!(r1.chunks > 0);
        assert_eq!(r2.chunks, 0);
    }

    #[tokio::test]
    async fn unsupported_type_fails_with_status() {
        let rag = rag();
        let err = rag.process_document_complete("a.xyz", b"data", None, None).await.unwrap_err();
        assert!(err.to_string().contains("unsupported file type"));
    }
}
