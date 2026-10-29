//! Retrieval-augmented generation over the legal vault.
//!
//! Architecture mirrors [RAG-Anything](https://github.com/hkuds/rag-anything):
//! `RagAnythingConfig` drives the `RagAnything` pipeline — parse documents
//! into typed content blocks → separate text from multimodal content →
//! chunk/embed/index text (vector + knowledge graph) → modal processors
//! describe images/tables/equations with extracted context → query with
//! LightRAG's six modes (naive/local/global/hybrid/mix/bypass).

pub mod agent;
pub mod citator;
pub mod citator_v2;
pub mod orchestrator;
pub mod subagents;
pub mod audit;
pub mod batch;
pub mod bm25;
pub mod cache;
pub mod config;
pub mod content;
pub mod context;
pub mod embeddings;
pub mod graph;
pub mod hnsw;
pub mod llm;
pub mod ocr;
pub mod parser;
pub mod persist;
pub mod pipeline;
pub mod processors;
pub mod query;
pub mod rerank;
pub mod retrieval_cache;
pub mod research;
pub mod s3vectors;
pub mod serenedb;
pub mod tantivy_bm25;
pub mod usage;
pub mod vector_store;
pub mod web;

pub use batch::{filter_supported_files, BatchItemResult};
pub use agent::{StepResult, WorkflowDef, WorkflowEngine, WorkflowRun, BUILTIN_WORKFLOWS};
pub use orchestrator::{AgentStepTrace, AutonomousAgent, AutonomousRun};
pub use subagents::{
    OrchestratedRun, OrchestratorConfig, SubAgentOrchestrator, SubAgentReceipt,
    DEFAULT_MAX_PARALLEL_RESEARCHERS, DEFAULT_PER_RUN_TOKEN_CAP,
    DEFAULT_PER_SUBAGENT_TIMEOUT_SECS, DEFAULT_PER_SUBAGENT_TOKEN_BUDGET,
};
pub use cache::{CacheStats, CachedEmbeddings, CachedLlm, SemanticCache};
pub use citator::{
    CitationTreatment, TreatmentEdge, TreatmentKind, TreatmentStatus, TreatmentStore,
};
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
    clean_refine_terms, query_service_for, Annotation, AskOptions, Citation, CitatorContext,
    ExtraSource, HitStage, HitTestReport, HitView, QueryAnswer, QueryEvent, QueryMode,
    QueryService, RefinedPrompt,
};
pub use rerank::{HttpLateInteractor, HttpReranker, LateInteractor, LlmReranker, Reranker};
pub use research::{Conflict, EvidenceNote, Hypothesis, ResearchReport};
pub use hnsw::{HnswParams, HnswVectorStore};
pub use s3vectors::{S3VectorsConfig, S3VectorsStore};
pub use serenedb::SereneDbVectorStore;
pub use tantivy_bm25::TantivyBm25Index;
pub use vector_store::{ChunkRecord, InMemoryVectorStore, ScoredChunk, VectorStore};
pub use web::{FetchedPage, NoopWebSearch, StealthWebSearch, WebResult, WebSearch};
