//! HTTP API: auth, vault (S3 ingestion), knowledge (RAG query).
//!
//! Route map mirrors the reference product's surfaces:
//! - `/api/v1/auth/*`      — login wall (local JWT + Auth0-compatible OIDC)
//! - `/api/v1/vault/*`     — document vault (S3 ingestion)
//! - `/api/v1/knowledge/*` — RAG query over the indexed corpus

pub mod activity;
pub mod annotations;
pub mod matters;
pub mod originals;
pub mod spaces;
pub mod private_vault;
pub mod settings;
pub mod billing;
pub mod durability;
pub mod error;
pub mod frontend;
pub mod feedback;
pub mod golden;
pub mod routes;
pub mod state;

pub use error::ApiError;
pub use state::AppState;
