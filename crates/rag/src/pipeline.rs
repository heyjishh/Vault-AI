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

/// Prepare PDF bytes for the pipeline: a password-protected PDF is
/// decrypted in memory (see `legal_rag_ingest::pdf_password`); anything
/// else passes through untouched. Non-PDF inputs ignore the password.
/// An encrypted PDF without a password fails here, before extraction.
fn prepared_pdf_bytes(
    file_name: &str,
    bytes: &[u8],
    file_password: Option<&str>,
) -> Result<Vec<u8>> {
    if !file_name.to_lowercase().ends_with(".pdf") {
        return Ok(bytes.to_vec());
    }
    if legal_rag_ingest::pdf_password::is_encrypted_pdf(bytes) {
        let pw = file_password.ok_or_else(|| {
            legal_rag_common::Error::BadRequest(
                legal_rag_ingest::pdf_password::PROTECTED_NO_PASSWORD_MSG.to_string(),
            )
        })?;
        legal_rag_ingest::pdf_password::decrypt_pdf(bytes, pw)
    } else {
        Ok(bytes.to_vec())
    }
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
        self.process_document_complete_with_password(file_name, bytes, title, doc_id, None)
            .await
    }

    /// Like [`Self::process_document_complete`], but a password-protected
    /// PDF is decrypted in memory first (see `legal_rag_ingest::
    /// pdf_password`). The content-based doc id is computed from the
    /// *decrypted* bytes, so the same document uploaded encrypted or plain
    /// deduplicates. The caller zeroizes `file_password` after this returns;
    /// it is never stored or logged. An encrypted PDF without a password
    /// fails here with the protected-file error, before any extraction.
    pub async fn process_document_complete_with_password(
        &self,
        file_name: &str,
        bytes: &[u8],
        title: Option<String>,
        doc_id: Option<String>,
        file_password: Option<&str>,
    ) -> Result<ProcessReport> {
        let bytes = prepared_pdf_bytes(file_name, bytes, file_password)?;
        let bytes = bytes.as_slice();
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
        self.extract_chunks_with_password(file_name, bytes, embeddings, None)
            .await
    }

    /// Like [`Self::extract_chunks_with`], but a password-protected PDF is
    /// decrypted in memory first, so the parser (and its render → Surya
    /// OCR path for scanned pages) always sees plain PDF bytes. The caller
    /// zeroizes `file_password` after this returns.
    pub async fn extract_chunks_with_password(
        &self,
        file_name: &str,
        bytes: &[u8],
        embeddings: Arc<dyn Embeddings>,
        file_password: Option<&str>,
    ) -> Result<Vec<ExtractedChunk>> {
        let bytes = prepared_pdf_bytes(file_name, bytes, file_password)?;
        let parser = get_parser(&self.config.parser, file_name, &self.config)?;
        let file_name_owned = file_name.to_string();
        let bytes_owned = bytes;
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
                // Index-time augmentation (RAGFlow pattern, Tier-2):
                // one batched LLM call per document generates keywords
                // + questions per parent chunk; they are embedded as
                // auxiliary match units (pc=aug) that retrieval maps
                // back to the source chunk — attacking vocabulary
                // mismatch (incl. Hindi phrasing of English text) at
                // zero query-time cost. Kill switch LEGAL_RAG_AUGMENT=0.
                let mut aux_records: Vec<ChunkRecord> = Vec::new();
                if std::env::var("LEGAL_RAG_AUGMENT").map(|v| v != "0").unwrap_or(true) {
                    match augment_chunks(self.llm.as_ref(), title.as_deref(), &records).await {
                        Some(items) => {
                            let mut aux_texts: Vec<String> = Vec::new();
                            let mut aux_meta: Vec<(String, String, String)> = Vec::new();
                            for (i, parent) in records.iter_mut().enumerate() {
                                let Some(item) = items.get(i) else { continue };
                                if !item.keywords.is_empty() {
                                    parent.metadata.insert(
                                        "aug_keywords".to_string(),
                                        item.keywords.join(", "),
                                    );
                                }
                                if !item.questions.is_empty() {
                                    let joined = item.questions.join(" ;; ");
                                    parent.metadata.insert(
                                        "aug_questions".to_string(),
                                        joined.chars().take(1000).collect(),
                                    );
                                }
                                for (n, q) in item.questions.iter().enumerate() {
                                    aux_texts.push(q.clone());
                                    aux_meta.push((
                                        format!("{}#q{n}", parent.id),
                                        "question".to_string(),
                                        parent.id.clone(),
                                    ));
                                }
                                if !item.keywords.is_empty() {
                                    aux_texts.push(item.keywords.join(" "));
                                    aux_meta.push((
                                        format!("{}#kw", parent.id),
                                        "keywords".to_string(),
                                        parent.id.clone(),
                                    ));
                                }
                            }
                            if !aux_texts.is_empty() {
                                match self.embeddings.embed(&aux_texts).await {
                                    Ok(vecs) if vecs.len() == aux_texts.len() => {
                                        for ((id, kind, source), (text, embedding)) in
                                            aux_meta.into_iter().zip(aux_texts.into_iter().zip(vecs))
                                        {
                                            let src = records.iter().find(|r| r.id == source);
                                            let mut metadata = HashMap::new();
                                            metadata.insert("pc".to_string(), "aug".to_string());
                                            metadata.insert("aug_kind".to_string(), kind);
                                            metadata.insert("source_id".to_string(), source);
                                            aux_records.push(ChunkRecord {
                                                id,
                                                doc_id: src.map(|r| r.doc_id.clone()).unwrap_or_default(),
                                                ordinal: src.map(|r| r.ordinal).unwrap_or(0),
                                                text,
                                                embedding,
                                                metadata,
                                            });
                                        }
                                        info!(doc_id = %doc_id, aux = aux_records.len(), "index-time augmentation indexed");
                                    }
                                    _ => warn!(doc_id = %doc_id, "augmentation embedding failed; skipping aux units"),
                                }
                            }
                        }
                        None => warn!(doc_id = %doc_id, "augmentation LLM call failed; indexing without aux units"),
                    }
                }
                // Hyperedges (Hyper-RAG pattern, Tier-2): one retrievable
                // unit per (Act, section) in the document, binding the
                // member chunks — a query naming the Act + section hits
                // the unit, which expands to its members. Kill switch
                // LEGAL_RAG_HYPEREDGES=0.
                if std::env::var("LEGAL_RAG_HYPEREDGES").map(|v| v != "0").unwrap_or(true) {
                    let hyper = build_hyperedges(doc_id, title.as_deref(), &mut records);
                    for (hid, text, members, act, section) in hyper {
                        match self.embeddings.embed(&[text.clone()]).await {
                            Ok(mut v) if v.len() == 1 => {
                                let mut metadata = HashMap::new();
                                metadata.insert("pc".to_string(), "hyper".to_string());
                                metadata.insert("hyper_act".to_string(), act);
                                metadata.insert("hyper_section".to_string(), section);
                                metadata.insert("members".to_string(), members.join(","));
                                aux_records.push(ChunkRecord {
                                    id: hid,
                                    doc_id: doc_id.to_string(),
                                    ordinal: 0,
                                    text,
                                    embedding: v.remove(0),
                                    metadata,
                                });
                            }
                            _ => warn!(doc_id = %doc_id, "hyperedge embedding failed; skipping one unit"),
                        }
                    }
                }
                // Upsert parents + children + aux/hyper units together;
                // graph extraction and the document registry below stay
                // on parents only.
                let mut all_records = records.clone();
                all_records.extend(child_records);
                all_records.extend(aux_records);
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
/// One chunk's index-time augmentation (RAGFlow pattern).
#[derive(Debug, serde::Deserialize)]
pub struct AugmentItem {
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub questions: Vec<String>,
}

/// Generate keywords + questions for every chunk in ONE batched LLM
/// call (per document). Returns None on any failure — augmentation is
/// an enhancement, never a reason to fail an ingest.
pub async fn augment_chunks(
    llm: &dyn LlmClient,
    title: Option<&str>,
    records: &[ChunkRecord],
) -> Option<Vec<AugmentItem>> {
    if records.is_empty() {
        return Some(Vec::new());
    }
    let mut user = format!(
        "Document: {}\n\nChunks:\n",
        title.unwrap_or("(untitled)")
    );
    for (i, r) in records.iter().enumerate() {
        let excerpt: String = r.text.chars().take(600).collect();
        user.push_str(&format!("[{i}] {excerpt}\n\n"));
    }
    let raw = llm
        .complete(
            "You generate search augmentation for a legal document index. \
             For EACH numbered chunk return keywords (5-8 exact legal terms, section numbers, \
             synonyms, and Hindi equivalents of the 2-3 most important terms) and questions \
             (2-3 natural questions the chunk answers, phrased the way a lawyer or litigant \
             would ask, at least one in Hinglish/Hindi if the chunk is Indian law). \
             Reply with ONLY a JSON array, one object per chunk in order: \
             [{\"keywords\": [\"...\"], \"questions\": [\"...\"]}]",
            &user,
        )
        .await
        .ok()?;
    crate::graph::parse_llm_json::<Vec<AugmentItem>>(&raw)
}

/// Section/article references in lowercased text, normalized as
/// "section 86" / "article 21".
pub fn find_section_refs(text_lower: &str) -> Vec<String> {
    find_section_refs_pos(text_lower)
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

/// Section/article references WITH their byte positions — hyperedge
/// construction pairs each reference with its nearest Act mention,
/// so multi-statute chunks don't cross-pair every Act with every
/// section.
pub fn find_section_refs_pos(text_lower: &str) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for kind in ["section", "article"] {
        let mut offset = 0usize;
        let mut rest = text_lower;
        while let Some(pos) = rest.find(kind) {
            let after = &rest[pos + kind.len()..];
            let trimmed = after.trim_start();
            if trimmed.len() != after.len() || after.starts_with(|c: char| c.is_whitespace()) {
                let num: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !num.is_empty() {
                    let key = format!("{kind} {num}");
                    if !out.iter().any(|(k, _)| *k == key) {
                        out.push((key, offset + pos));
                    }
                }
            }
            offset += pos + kind.len();
            rest = &rest[pos + kind.len()..];
        }
    }
    out
}

/// Byte positions of every mention (canonical name or alias) of each
/// statute in the table, in lowercased text. Matching is by token
/// sequence (the same semantics as `statute_mentions`), so short
/// aliases like "ipc" never match inside other words.
pub fn statute_mention_positions(text_lower: &str) -> Vec<(&'static str, usize)> {
    // Tokenize with byte offsets.
    let mut tokens: Vec<(&str, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text_lower.char_indices() {
        if c.is_alphanumeric() {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            tokens.push((&text_lower[s..i], s));
        }
    }
    if let Some(s) = start {
        tokens.push((&text_lower[s..], s));
    }
    let toks: Vec<&str> = tokens.iter().map(|(t, _)| *t).collect();
    let mut out = Vec::new();
    for (canonical, aliases) in crate::query::STATUTES {
        let mut forms: Vec<Vec<&str>> = vec![canonical.split_whitespace().collect()];
        for a in aliases.iter() {
            forms.push(a.split_whitespace().collect());
        }
        for form in forms {
            if form.is_empty() || form.len() > toks.len() {
                continue;
            }
            for w in 0..=(toks.len() - form.len()) {
                if toks[w..w + form.len()] == form[..] {
                    out.push((*canonical, tokens[w].1));
                }
            }
        }
    }
    out
}

/// Short slug for a hyperedge id: the Act's distinguishing word
/// ("indian penal code" → "penal", "income-tax act" → "income-tax").
fn act_slug(act: &str) -> String {
    const STOP: &[&str] = &["indian", "act", "of", "the", "code"];
    act.split_whitespace()
        .find(|w| !STOP.contains(w))
        .unwrap_or("act")
        .to_string()
}

/// Build hyperedge units for a document: group parent chunks by the
/// (Act, section) they belong to. Stamps `hyper_id` on member parents.
/// Returns (unit_text, member_ids, act, section) tuples to embed.
/// The Act comes from a statute mentioned in the chunk itself, or the
/// document title as a fallback (a doc titled "Section 86 IT Act"
/// scopes all its sections to that Act).
pub fn build_hyperedges(
    doc_id: &str,
    title: Option<&str>,
    records: &mut [ChunkRecord],
) -> Vec<(String, String, Vec<String>, String, String)> {
    let title_lower = title.unwrap_or_default().to_lowercase();
    let title_acts = crate::query::statute_mentions(&title_lower);
    let mut groups: Vec<(String, String, Vec<usize>)> = Vec::new();
    for (i, rec) in records.iter().enumerate() {
        let lower = rec.text.to_lowercase();
        // Pair each section reference with its NEAREST Act mention in
        // the chunk (by byte distance, preceding mentions preferred on
        // ties) — never the cross product, which would manufacture
        // wrong (Act, section) pairs in multi-statute chunks. The
        // title's Acts are the fallback only when the chunk names none.
        let act_positions = statute_mention_positions(&lower);
        for (section, sec_pos) in find_section_refs_pos(&lower) {
            let act: Option<String> = if act_positions.is_empty() {
                title_acts.first().map(|a| a.to_string())
            } else {
                act_positions
                    .iter()
                    .min_by_key(|(_, apos)| {
                        let d = (*apos as i64 - sec_pos as i64).abs();
                        // Prefer a mention at or before the section.
                        (d, if *apos <= sec_pos { 0 } else { 1 })
                    })
                    .map(|(a, _)| a.to_string())
            };
            let Some(act) = act else { continue };
            match groups
                .iter_mut()
                .find(|(a, s, _)| a == &act && s == &section)
            {
                Some((_, _, members)) => {
                    if !members.contains(&i) {
                        members.push(i);
                    }
                }
                None => groups.push((act, section, vec![i])),
            }
        }
    }
    let mut out = Vec::new();
    for (act, section, members) in groups {
        let member_ids: Vec<String> =
            members.iter().map(|&i| records[i].id.clone()).collect();
        let hid = format!(
            "{doc_id}#he-{}-{}",
            act_slug(&act),
            section.replace(' ', "")
        );
        let mut text = format!("{} — {}: ", title_case_act(&act), title_case_word(&section));
        for &i in members.iter().take(3) {
            let lead: String = records[i].text.chars().take(380).collect();
            text.push_str(&lead);
            text.push_str(" … ");
        }
        for &i in &members {
            // First edge wins the display stamp: a chunk spanning two
            // statutes belongs to several edges, but the chip shows
            // its primary one.
            records[i]
                .metadata
                .entry("hyper_id".to_string())
                .or_insert_with(|| hid.clone());
        }
        out.push((hid, text, member_ids, act, section));
    }
    out
}

fn title_case_act(act: &str) -> String {
    act.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn title_case_word(s: &str) -> String {
    let mut parts = s.splitn(2, ' ');
    let kind = parts.next().unwrap_or_default();
    let num = parts.next().unwrap_or_default();
    format!("{} {}", title_case_act(kind), num)
}

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

    #[test]
    fn section_refs_found_and_normalized() {
        let refs = find_section_refs("under section 86 of the act, and article 21 read with section 86");
        assert_eq!(refs, vec!["section 86".to_string(), "article 21".to_string()]);
        assert!(find_section_refs("sectional title only").is_empty());
    }

    #[test]
    fn hyperedges_group_by_act_and_section() {
        let mk = |id: &str, text: &str| ChunkRecord {
            id: id.to_string(),
            doc_id: "d1".to_string(),
            ordinal: 0,
            text: text.to_string(),
            embedding: vec![0.0; 4],
            metadata: HashMap::new(),
        };
        let mut records = vec![
            mk("d1#0", "Section 86 of the Information Technology Act provides a savings clause for other laws."),
            mk("d1#1", "Section 378 of the Indian Penal Code defines theft as dishonestly moving property."),
            mk("d1#2", "Nothing in section 86 shall affect any other law for the time being in force."),
        ];
        let edges = build_hyperedges("d1", Some("IT Act notes"), &mut records);
        // Chunk 0 names the IT Act itself; chunk 2 mentions only
        // "section 86" but inherits the Act from the document title
        // ("IT Act" matches the alias table) — so both group together.
        let it86 = edges.iter().find(|(_, _, _, a, s)| a == "information technology act" && s == "section 86");
        assert!(it86.is_some(), "edges: {edges:?}");
        let (_, _, members, _, _) = it86.unwrap();
        assert_eq!(members, &vec!["d1#0".to_string(), "d1#2".to_string()]);
        let ipc = edges.iter().find(|(_, _, _, a, _)| a == "indian penal code");
        assert!(ipc.is_some());
        assert!(records[0].metadata.contains_key("hyper_id"));
    }

    #[test]
    fn hyperedges_never_cross_pair_acts_and_sections() {
        // Regression (found live 2026-10-24): a chunk citing two
        // statutes must pair each section with its NEAREST Act —
        // (Contract Act, s.378) style phantom edges are exactly the
        // collisions hyperedges exist to prevent.
        let mut records = vec![ChunkRecord {
            id: "d9#0".to_string(),
            doc_id: "d9".to_string(),
            ordinal: 0,
            text: "Section 10 of the Indian Contract Act, 1872 provides that agreements made by free consent are contracts. Section 378 of the Indian Penal Code defines theft as dishonestly moving movable property. The punishment is in Section 379 of the Indian Penal Code.".to_string(),
            embedding: vec![0.0; 4],
            metadata: HashMap::new(),
        }];
        let edges = build_hyperedges("d9", None, &mut records);
        let pairs: Vec<(&str, &str)> = edges
            .iter()
            .map(|(_, _, _, a, s)| (a.as_str(), s.as_str()))
            .collect();
        assert!(pairs.contains(&("indian contract act", "section 10")), "{pairs:?}");
        assert!(pairs.contains(&("indian penal code", "section 378")), "{pairs:?}");
        assert!(pairs.contains(&("indian penal code", "section 379")), "{pairs:?}");
        assert!(!pairs.contains(&("indian contract act", "section 378")), "{pairs:?}");
        assert!(!pairs.contains(&("indian penal code", "section 10")), "{pairs:?}");
        assert_eq!(edges.len(), 3);
    }

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

    fn fixture(name: &str) -> Vec<u8> {
        // Shared with the ingest crate's test data (same workspace).
        let path = format!(
            "{}/../ingest/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"))
    }

    #[tokio::test]
    async fn encrypted_pdf_round_trips_through_pipeline() {
        // The private-vault path (extract_chunks_with_password) and the
        // corpus path share prepared_pdf_bytes; exercise both here.
        let rag = rag();
        let enc = fixture("enc-aes256.pdf");
        let chunks = rag
            .extract_chunks_with_password(
                "enc-aes256.pdf",
                &enc,
                rag.embeddings.clone(),
                Some("test-fixture-pw"),
            )
            .await
            .unwrap();
        let joined = chunks
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("Section 86 of the Information Technology Act"),
            "decrypted chunks must carry the fixture text, got: {joined}"
        );
        let plain = fixture("plain.pdf");
        let plain_chunks = rag
            .extract_chunks("plain.pdf", &plain)
            .await
            .unwrap();
        let plain_joined = plain_chunks
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(joined, plain_joined, "encrypted and plain must chunk identically");
    }

    #[tokio::test]
    async fn encrypted_pdf_errors_are_distinct_in_pipeline() {
        let rag = rag();
        let enc = fixture("enc-rc4.pdf");
        let err = rag
            .process_document_complete_with_password("enc-rc4.pdf", &enc, None, None, None)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("password-protected"),
            "no-password case must name protection, got {err}"
        );
        let err = rag
            .process_document_complete_with_password(
                "enc-rc4.pdf",
                &enc,
                None,
                None,
                Some("wrong-password"),
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("wrong file password"),
            "wrong-password case must be distinct, got {err}"
        );
        // Unprotected + password: ignored, normal path.
        let plain = fixture("plain.pdf");
        let report = rag
            .process_document_complete_with_password(
                "plain.pdf",
                &plain,
                None,
                None,
                Some("test-fixture-pw"),
            )
            .await
            .unwrap();
        assert!(report.chunks > 0);
    }
}
