//! Retrieval-augmented generation over the legal vault.
//!
//! Architecture mirrors [RAG-Anything](https://github.com/hkuds/rag-anything):
//! `RagAnythingConfig` drives the `RagAnything` pipeline — parse documents
//! into typed content blocks → separate text from multimodal content →
//! chunk/embed/index text (vector + knowledge graph) → modal processors
//! describe images/tables/equations with extracted context → query with
//! LightRAG's six modes (naive/local/global/hybrid/mix/bypass).

pub mod agent;
pub mod orchestrator;
pub mod audit;
pub mod batch;
pub mod bm25;
pub mod cache;
pub mod config;
pub mod content;
pub mod context;
pub mod embeddings;
pub mod graph;
pub mod llm;
pub mod ocr;
pub mod parser;
pub mod persist;
pub mod pipeline;
pub mod processors;
pub mod query;
pub mod rerank;
pub mod research;
pub mod serenedb;
pub mod usage;
pub mod vector_store;
pub mod web;

pub use batch::{filter_supported_files, BatchItemResult};
pub use agent::{StepResult, WorkflowDef, WorkflowEngine, WorkflowRun, BUILTIN_WORKFLOWS};
pub use orchestrator::{AgentStepTrace, AutonomousAgent, AutonomousRun};
pub use cache::{CacheStats, CachedEmbeddings, CachedLlm, SemanticCache};
pub use config::RagAnythingConfig;
pub use content::{ContentBlock, ModalItem, PageInterval};
pub use embeddings::{Embeddings, MockEmbeddings, OpenAiEmbeddings};
pub use graph::{Entity, GraphStore, InMemoryGraphStore, Relation};
pub use llm::{DynamicLlm, LlmClient, MockLlm, OpenAiLlm};
pub use ocr::{CommandOcr, OcrEngine};
pub use pipeline::{
    DocMeta, DocStatus, DocStatusRecord, DocumentInfo, ExtractedChunk, ProcessReport, RagAnything,
};
pub use processors::{ModalProcessor, ProcessedModal};
pub use query::{
    query_service_for, Annotation, Citation, ExtraSource, HitStage, HitTestReport, HitView,
    QueryAnswer, QueryEvent, QueryMode, QueryService,
};
pub use rerank::{HttpReranker, LlmReranker, Reranker};
pub use research::{Conflict, EvidenceNote, Hypothesis, ResearchReport};
pub use serenedb::SereneDbVectorStore;
pub use vector_store::{ChunkRecord, InMemoryVectorStore, ScoredChunk, VectorStore};
pub use web::{FetchedPage, NoopWebSearch, StealthWebSearch, WebResult, WebSearch};
