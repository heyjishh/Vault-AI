//! Shared application state.

use std::sync::Arc;

use axum::extract::FromRef;
use legal_rag_auth::{AuthService, OidcClient};
use legal_rag_common::Config;
use legal_rag_ingest::IngestService;
use legal_rag_rag::{Embeddings, GraphStore, LlmClient, QueryService, RagAnything, SemanticCache, VectorStore, WebSearch, WorkflowEngine};

/// Everything handlers need, built once at startup.
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub auth: Arc<AuthService>,
    pub ingest: IngestService,
    pub rag: Arc<RagAnything>,
    pub query: Arc<QueryService>,
    pub oidc: Option<OidcClient>,
    pub embeddings: Arc<dyn Embeddings>,
    pub store: Arc<dyn VectorStore>,
    pub graph: Arc<dyn GraphStore>,
    pub llm: Arc<dyn LlmClient>,
    pub web: Arc<dyn WebSearch>,
    pub llm_cache: Option<Arc<SemanticCache>>,
    pub emb_cache: Option<Arc<SemanticCache>>,
    pub workflows: Arc<WorkflowEngine>,
    pub agent: Arc<legal_rag_rag::AutonomousAgent>,
    pub billing: Arc<crate::billing::BillingStore>,
    pub activity: Arc<crate::activity::ActivityStore>,
    pub golden: Arc<crate::golden::GoldenStore>,
    pub annotations: Arc<crate::annotations::AnnotationStore>,
    pub matters: Arc<crate::matters::MatterStore>,
    pub spaces: Arc<crate::spaces::SpaceStore>,
    pub private_vault: Arc<crate::private_vault::PrivateVault>,
    /// Embeddings provider for confidential-vault traffic only — the
    /// dedicated local endpoint when `LEGAL_RAG_PRIVATE_EMBEDDING_URL`
    /// is set, otherwise the main provider.
    pub private_embeddings: Arc<dyn Embeddings>,
    /// LLM override for private answer generation, when
    /// `LEGAL_RAG_PRIVATE_LLM_URL` is set.
    pub private_llm: Option<Arc<dyn LlmClient>>,
    pub settings: Arc<crate::settings::SettingsStore>,
    /// Hot-swappable LLM handle (admin model changes); None in mock mode.
    pub dyn_llm: Option<Arc<legal_rag_rag::DynamicLlm>>,
    /// The deployment's configured LLM model (revert target).
    pub default_llm_model: String,
    /// The embedding model this boot actually applied.
    pub boot_embedding_model: String,
}

impl FromRef<AppState> for Arc<AuthService> {
    fn from_ref(state: &AppState) -> Arc<AuthService> {
        state.auth.clone()
    }
}
