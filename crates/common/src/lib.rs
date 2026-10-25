//! Shared configuration, error type, and helpers for the legal-rag workspace.

use std::env;

use serde::{Deserialize, Serialize};

/// Application-wide error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("quota exceeded: {0}")]
    Quota(String),
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Newtype for ids used across crates.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn now_utc() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_opt(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// S3 / Vault configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Config {
    pub bucket: String,
    pub region: String,
    pub prefix: String,
    /// S3-compatible endpoint override (`LEGAL_RAG_S3_ENDPOINT_URL`).
    /// Set for Cloudflare R2 (`https://<account-id>.r2.cloudflarestorage.com`)
    /// or any other S3-compatible store; `None` keeps stock AWS
    /// endpoints. When set, the client uses path-style addressing
    /// (`<endpoint>/<bucket>/<key>`), which is what R2-style stores
    /// expect, instead of virtual-hosted addressing.
    #[serde(default)]
    pub endpoint_url: Option<String>,
}

/// Embedding provider configuration (OpenAI-compatible HTTP API).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub dims: usize,
}

/// LLM configuration (OpenAI-compatible chat completions API).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub url: String,
    pub api_key: Option<String>,
    pub model: String,
}

/// Auth0 / OIDC configuration (optional).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
}

/// JWT configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtConfig {
    pub secret: String,
    pub access_ttl_secs: i64,
    pub refresh_ttl_secs: i64,
}

/// Full application configuration, loaded from the environment.
#[derive(Debug, Clone)]
pub struct Config {
    pub server_addr: String,
    pub jwt: JwtConfig,
    pub s3: Option<S3Config>,
    pub embedding: EmbeddingConfig,
    pub llm: LlmConfig,
    /// Confidential-vault-only providers (`LEGAL_RAG_PRIVATE_*`): when
    /// set, private uploads, private question embeddings and private
    /// answer generation use these (typically local) endpoints instead
    /// of the main providers, so confidential text and the questions
    /// about it never leave the deployment.
    pub private_embedding: Option<EmbeddingConfig>,
    pub private_llm: Option<LlmConfig>,
    pub oidc: Option<OidcConfig>,
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    /// Default retrieval depth when a request does not specify `top_k`
    /// (`LEGAL_RAG_DEFAULT_TOP_K`, 1..=20, default 5). Raising it fights
    /// corpus dilution in larger corpora at some reranker-latency cost.
    pub default_top_k: usize,
    /// Emails promoted to the `admin` role on login/register
    /// (`LEGAL_RAG_ADMIN_EMAILS`, comma-separated).
    pub admin_emails: Vec<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let s3 = match (env_opt("LEGAL_RAG_S3_BUCKET"), env_opt("LEGAL_RAG_S3_REGION")) {
            (Some(bucket), Some(region)) => Some(S3Config {
                bucket,
                region,
                prefix: env_or("LEGAL_RAG_S3_PREFIX", ""),
                endpoint_url: env_opt("LEGAL_RAG_S3_ENDPOINT_URL"),
            }),
            _ => None,
        };
        let oidc = match (
            env_opt("LEGAL_RAG_OIDC_ISSUER"),
            env_opt("LEGAL_RAG_OIDC_CLIENT_ID"),
        ) {
            (Some(issuer), Some(client_id)) => Some(OidcConfig {
                issuer,
                client_id,
                client_secret: env_opt("LEGAL_RAG_OIDC_CLIENT_SECRET"),
                redirect_uri: env_or(
                    "LEGAL_RAG_OIDC_REDIRECT_URI",
                    "http://localhost:3000/api/v1/auth/oidc/callback",
                ),
            }),
            _ => None,
        };
        Ok(Self {
            server_addr: env_or("LEGAL_RAG_ADDR", "127.0.0.1:3000"),
            jwt: JwtConfig {
                secret: env_or("LEGAL_RAG_JWT_SECRET", "dev-only-change-me"),
                access_ttl_secs: env_or("LEGAL_RAG_JWT_ACCESS_TTL", "900").parse().unwrap_or(900),
                refresh_ttl_secs: env_or("LEGAL_RAG_JWT_REFRESH_TTL", "2592000")
                    .parse()
                    .unwrap_or(2592000),
            },
            s3,
            embedding: EmbeddingConfig {
                url: env_or("LEGAL_RAG_EMBEDDING_URL", "http://localhost:11434/v1/embeddings"),
                api_key: env_opt("LEGAL_RAG_EMBEDDING_API_KEY"),
                model: env_or("LEGAL_RAG_EMBEDDING_MODEL", "nomic-embed-text"),
                dims: env_or("LEGAL_RAG_EMBEDDING_DIMS", "768").parse().unwrap_or(768),
            },
            llm: LlmConfig {
                url: env_or("LEGAL_RAG_LLM_URL", "http://localhost:11434/v1/chat/completions"),
                api_key: env_opt("LEGAL_RAG_LLM_API_KEY"),
                model: env_or("LEGAL_RAG_LLM_MODEL", "llama3.1"),
            },
            private_embedding: env_opt("LEGAL_RAG_PRIVATE_EMBEDDING_URL").map(|url| {
                EmbeddingConfig {
                    url,
                    api_key: env_opt("LEGAL_RAG_PRIVATE_EMBEDDING_API_KEY"),
                    model: env_or(
                        "LEGAL_RAG_PRIVATE_EMBEDDING_MODEL",
                        &env_or("LEGAL_RAG_EMBEDDING_MODEL", "nomic-embed-text"),
                    ),
                    dims: env_or(
                        "LEGAL_RAG_PRIVATE_EMBEDDING_DIMS",
                        &env_or("LEGAL_RAG_EMBEDDING_DIMS", "768"),
                    )
                    .parse()
                    .unwrap_or(768),
                }
            }),
            private_llm: env_opt("LEGAL_RAG_PRIVATE_LLM_URL").map(|url| LlmConfig {
                url,
                api_key: env_opt("LEGAL_RAG_PRIVATE_LLM_API_KEY"),
                model: env_or(
                    "LEGAL_RAG_PRIVATE_LLM_MODEL",
                    &env_or("LEGAL_RAG_LLM_MODEL", "llama3.1"),
                ),
            }),
            oidc,
            chunk_size: env_or("LEGAL_RAG_CHUNK_SIZE", "1200").parse().unwrap_or(1200),
            chunk_overlap: env_or("LEGAL_RAG_CHUNK_OVERLAP", "200").parse().unwrap_or(200),
            default_top_k: env_or("LEGAL_RAG_DEFAULT_TOP_K", "5")
                .parse::<usize>()
                .unwrap_or(5)
                .clamp(1, 20),
            admin_emails: env_or("LEGAL_RAG_ADMIN_EMAILS", "")
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
        })
    }
}
