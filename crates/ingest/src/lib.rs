//! Document vault: S3 ingestion, text extraction, and chunking.
//!
//! Mirrors the reference product's "Vault" surface: documents land in
//! object storage and are normalized to plain text before indexing.

pub mod chunk;
pub mod document;
pub mod extract;
pub mod s3;
pub mod service;

pub use chunk::{Chunk, Chunker};
pub use document::Document;
pub use service::IngestService;
