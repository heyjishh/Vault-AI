//! legal-rag-server: end-to-end legal RAG API.
//!
//! Boot order: config -> auth (local users + optional OIDC) -> vault (S3)
//! -> RAG pipeline (embeddings, vector store, graph, LLM) -> axum server.

use std::env;
use std::sync::Arc;

use legal_rag_auth::{AuthService, InMemoryUserStore, OidcClient};
use legal_rag_common::Config;
use legal_rag_ingest::IngestService;
use legal_rag_rag::{
    query_service_for, CachedEmbeddings, CachedLlm, Embeddings, GraphStore, InMemoryGraphStore,
    HnswParams, HnswVectorStore, InMemoryVectorStore, LlmClient, MockEmbeddings, MockLlm,
    NoopWebSearch, OpenAiEmbeddings, OpenAiLlm, RagAnything, RagAnythingConfig, S3VectorsConfig,
    S3VectorsStore, SemanticCache, SereneDbVectorStore,
    StealthWebSearch, VectorStore, WebSearch,
};
use legal_rag_server::{durability, routes, AppState};
use tracing::info;

fn use_mocks() -> bool {
    matches!(
        env::var("LEGAL_RAG_USE_MOCK_PROVIDERS").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env first so file-based config works.
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env().expect("invalid configuration");
    info!("starting legal-rag-server");

    // Auth: local user store + JWT; OIDC (Auth0-compatible) when configured.
    // Keep a concrete handle for persistence snapshots.
    let user_store = Arc::new(InMemoryUserStore::new());
    let auth = Arc::new(AuthService::new(user_store.clone(), &config));
    let oidc = match &config.oidc {
        Some(oidc_config) => match OidcClient::discover(
            &oidc_config.issuer,
            &oidc_config.client_id,
            oidc_config.client_secret.as_deref(),
            &oidc_config.redirect_uri,
        )
        .await
        {
            Ok(client) => {
                info!(issuer = %oidc_config.issuer, "OIDC discovery succeeded");
                Some(client)
            }
            Err(e) => {
                tracing::warn!(error = %e, "OIDC discovery failed; continuing without OIDC");
                None
            }
        },
        None => {
            info!("OIDC not configured; local auth only");
            None
        }
    };

    // Vault: S3-backed ingestion.
    let ingest = IngestService::new(&config).await?;
    if ingest.s3_configured() {
        info!("S3 vault configured");
    } else {
        info!("S3 not configured; direct upload only");
    }

    // Admin customization settings load before providers: they can
    // override the embedding model (boot-applied) and the LLM model.
    let settings = std::sync::Arc::new(legal_rag_server::settings::SettingsStore::load(
        legal_rag_rag::persist::data_dir().join("settings.json"),
    ));
    let boot_settings = settings.get();
    let boot_embedding_model = boot_settings
        .embedding_model
        .clone()
        .unwrap_or_else(|| config.embedding.model.clone());
    let boot_llm_model = boot_settings
        .llm_model
        .clone()
        .unwrap_or_else(|| config.llm.model.clone());

    // RAG backends: real providers or deterministic mocks.
    let embeddings: Arc<dyn Embeddings> = if use_mocks() {
        info!("using mock embeddings");
        Arc::new(MockEmbeddings::new(config.embedding.dims))
    } else {
        Arc::new(OpenAiEmbeddings::new(
            &config.embedding.url,
            config.embedding.api_key.as_deref(),
            &boot_embedding_model,
            config.embedding.dims,
        ))
    };
    let (llm, dyn_llm): (Arc<dyn LlmClient>, Option<Arc<legal_rag_rag::DynamicLlm>>) =
        if use_mocks() {
            info!("using mock LLM");
            (
                Arc::new(MockLlm::new(
                    "Mock answer: configure LEGAL_RAG_LLM_URL to enable grounded generation. [doc:0]",
                )),
                None,
            )
        } else {
            let base = OpenAiLlm::new(
                &config.llm.url,
                config.llm.api_key.as_deref(),
                &boot_llm_model,
            )
            // Generous cap: some providers default to tiny limits that
            // truncate JSON graph-extraction output mid-string.
            .with_max_tokens(2000);
            let dyn_llm = Arc::new(legal_rag_rag::DynamicLlm::new(
                Arc::new(base),
                Some(&config.llm.url),
                config.llm.api_key.as_deref(),
                &boot_llm_model,
            ));
            (dyn_llm.clone() as Arc<dyn LlmClient>, Some(dyn_llm))
        };

    // Undecorated handle on the same client, captured before the
    // cache wrap below: confidential generation (Piece 16) must never
    // flow through CachedLlm.
    let llm_uncached = llm.clone();

    // Semantic cache (application-level KV cache): extraction + embedding
    // results keyed by content hash. Disable with LEGAL_RAG_CACHE_ENABLED=0.
    let cache_enabled = !matches!(
        env::var("LEGAL_RAG_CACHE_ENABLED").as_deref(),
        Ok("0") | Ok("false") | Ok("no")
    );
    let (embeddings, llm, llm_cache, emb_cache): (
        Arc<dyn Embeddings>,
        Arc<dyn LlmClient>,
        Option<Arc<SemanticCache>>,
        Option<Arc<SemanticCache>>,
    ) = if cache_enabled {
        let lc = Arc::new(SemanticCache::new(1024));
        let ec = Arc::new(SemanticCache::new(8192));
        info!("semantic cache enabled (llm=1024, embeddings=8192 entries)");
        (
            Arc::new(CachedEmbeddings::new(embeddings, ec.clone())),
            Arc::new(CachedLlm::new(llm, lc.clone())),
            Some(lc),
            Some(ec),
        )
    } else {
        info!("semantic cache disabled");
        (embeddings, llm, None, None)
    };

    // Web search via the invisible_playwright stealth sidecar.
    // LEGAL_RAG_WEB_SIDECAR overrides the sidecar dir (default tools/web).
    let web: Arc<dyn WebSearch> = {
        let sidecar = env::var("LEGAL_RAG_WEB_SIDECAR").unwrap_or_else(|_| "tools/web".to_string());
        let sidecar_path = std::path::PathBuf::from(&sidecar);
        let enabled = !matches!(
            env::var("LEGAL_RAG_WEB_SEARCH").as_deref(),
            Ok("0") | Ok("false") | Ok("no")
        );
        if enabled && StealthWebSearch::available(&sidecar_path) {
            info!(sidecar = %sidecar, "web search enabled (stealth playwright)");
            Arc::new(StealthWebSearch::new(sidecar_path))
        } else {
            if enabled {
                info!(sidecar = %sidecar, "web sidecar not found; web search disabled");
            }
            Arc::new(NoopWebSearch)
        }
    };
    // Vector store selection (ADR-0020): LEGAL_RAG_VECTOR_STORE picks
    // the engine — `hnsw` (native HNSW, Spice's DuckDB-engine design),
    // `s3vectors` (Spice's external engine: AWS S3 Vectors), `serenedb`,
    // or `memory`. Unset keeps the legacy behavior: SereneDB when
    // SERENEDB_URL is set, else in-memory. Non-memory engines persist
    // themselves; the in-memory index needs disk snapshots, so keep its
    // concrete handle when used.
    let choice = env::var("LEGAL_RAG_VECTOR_STORE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| {
            if env::var("SERENEDB_URL").ok().filter(|u| !u.is_empty()).is_some() {
                "serenedb".to_string()
            } else {
                "memory".to_string()
            }
        });
    let mem_store: Option<Arc<InMemoryVectorStore>>;
    let store: Arc<dyn VectorStore> = match choice.as_str() {
        "hnsw" => {
            let path = legal_rag_rag::persist::data_dir().join("hnsw-vectors.json");
            match HnswVectorStore::open(Some(path), HnswParams::from_env()) {
                Ok(s) => {
                    info!("using native HNSW vector store");
                    mem_store = None;
                    Arc::new(s)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "HNSW store failed to open; falling back to in-memory index");
                    let m = Arc::new(InMemoryVectorStore::new());
                    mem_store = Some(m.clone());
                    m
                }
            }
        }
        "s3vectors" => {
            let connected = match S3VectorsConfig::from_env() {
                Some(Ok(cfg)) => S3VectorsStore::connect(cfg, config.embedding.dims).await.map(|s| Arc::new(s) as Arc<dyn VectorStore>),
                Some(Err(e)) => Err(e),
                None => Err(legal_rag_common::Error::Other(anyhow::anyhow!(
                    "LEGAL_RAG_S3V_BUCKET not set"
                ))),
            };
            match connected {
                Ok(s) => {
                    mem_store = None;
                    s
                }
                Err(e) => {
                    tracing::warn!(error = %e, "S3 Vectors unavailable; falling back to in-memory index");
                    let m = Arc::new(InMemoryVectorStore::new());
                    mem_store = Some(m.clone());
                    m
                }
            }
        }
        "serenedb" => {
            let url = env::var("SERENEDB_URL").unwrap_or_default();
            let table = env::var("SERENEDB_TABLE").unwrap_or_else(|_| "legal_chunks".to_string());
            match SereneDbVectorStore::connect(&url, &table, config.embedding.dims).await {
                Ok(s) => {
                    info!(table = %table, "using SereneDB vector store");
                    mem_store = None;
                    Arc::new(s)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "SereneDB unavailable; falling back to in-memory index");
                    let m = Arc::new(InMemoryVectorStore::new());
                    mem_store = Some(m.clone());
                    m
                }
            }
        }
        _ => {
            info!("using in-memory vector index");
            let m = Arc::new(InMemoryVectorStore::new());
            mem_store = Some(m.clone());
            m
        }
    };
    let graph_store = Arc::new(InMemoryGraphStore::new());
    let graph: Arc<dyn GraphStore> = graph_store.clone();

    // RAG-Anything pipeline: parse -> content blocks -> chunk/embed/graph ->
    // modal processors -> LightRAG query modes.
    let rag_config = RagAnythingConfig::from_env();
    let rag = Arc::new(RagAnything::new(
        rag_config,
        embeddings.clone(),
        store.clone(),
        graph.clone(),
        llm.clone(),
        config.chunk_size,
        config.chunk_overlap,
    ));
    let rag_for_query = rag.clone();
    // Reranker (precision pass over candidates). Default decision layer is
    // the local cross-encoder service (LEGAL_RAG_RERANKER=local +
    // LEGAL_RAG_RERANK_URL, see tools/rerank/); the LLM listwise reranker
    // stays available as an optional deep judge (LEGAL_RAG_RERANKER=llm).
    // Disable entirely with LEGAL_RAG_RERANK=0.
    let reranker: Option<Arc<dyn legal_rag_rag::Reranker>> = {
        let enabled = !matches!(
            env::var("LEGAL_RAG_RERANK").as_deref(),
            Ok("0") | Ok("false") | Ok("no")
        );
        if !enabled {
            info!("reranker disabled (LEGAL_RAG_RERANK=0)");
            None
        } else if use_mocks() {
            info!("reranker disabled (mock providers)");
            None
        } else {
            match env::var("LEGAL_RAG_RERANKER").as_deref() {
                Ok("llm") => {
                    info!("LLM reranker enabled (deep judge)");
                    Some(Arc::new(legal_rag_rag::LlmReranker::new(llm.clone())))
                }
                _ => match env::var("LEGAL_RAG_RERANK_URL") {
                    Ok(url) if !url.trim().is_empty() => {
                        info!(%url, "local cross-encoder reranker enabled");
                        Some(Arc::new(legal_rag_rag::HttpReranker::new(url)))
                    }
                    _ => {
                        tracing::warn!(
                            "LEGAL_RAG_RERANKER=local but LEGAL_RAG_RERANK_URL is unset; \
                             falling back to the LLM reranker"
                        );
                        Some(Arc::new(legal_rag_rag::LlmReranker::new(llm.clone())))
                    }
                },
            }
        }
    };
    let query = Arc::new(query_service_for(
        &rag,
        Arc::new(move |id: &str| rag_for_query.doc_title(id)),
        web.clone(),
        reranker,
    ));
    // Legal-validity metadata (jurisdiction / in-force / superseded) for
    // citation staleness flags, read live from the document registry.
    let rag_for_meta = rag.clone();
    query.set_doc_meta_source(Arc::new(move |id: &str| rag_for_meta.document_meta(id)));
    // Citator: the query service reads the same treatment store the
    // ingest pipeline writes (new uploads) and backfill populates.
    query.set_treatment_store(rag.treatments());
    query.set_v2_edge_store(rag.v2_treatments());
    // Let the retrieval cache spawn background revalidation tasks.
    query.bind_self(&query);
    // ColBERT late interaction (optional, Tier-2): LEGAL_RAG_COLBERT_URL
    // points at the MaxSim sidecar (tools/colbert). Unset = stage off;
    // sidecar failures degrade to the reranker's order.
    if !use_mocks() {
        if let Ok(url) = env::var("LEGAL_RAG_COLBERT_URL") {
            if !url.trim().is_empty() {
                info!(%url, "ColBERT late-interaction rescoring enabled");
                query.set_late_interactor(Arc::new(
                    legal_rag_rag::HttpLateInteractor::new(url),
                ));
            }
        }
    }
    let workflows = Arc::new(legal_rag_rag::WorkflowEngine::new(query.clone()));
    let agent = Arc::new(legal_rag_rag::AutonomousAgent::new(query.clone()));
    let orchestrator = Arc::new(legal_rag_rag::SubAgentOrchestrator::new(query.clone()));

    // ---- Persistence: load snapshots, then save periodically + on shutdown.
    let data_dir = legal_rag_rag::persist::data_dir();
    restore_snapshots(&data_dir, mem_store.as_ref(), &store, &graph_store, &rag, &rag.treatments(), &user_store).await;

    let snapshot_handles = SnapshotHandles {
        data_dir: data_dir.clone(),
        mem_store: mem_store.clone(),
        store: store.clone(),
        graph_store: graph_store.clone(),
        rag: rag.clone(),
        treatments: rag.treatments(),
        v2_treatments: rag.v2_treatments(),
        user_store: user_store.clone(),
    };
    // Periodic saver every 5 minutes (also covers crash-durability between saves).
    let saver_handles = snapshot_handles.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
        loop {
            interval.tick().await;
            if let Err(e) = saver_handles.save_all().await {
                tracing::warn!(error = %e, "periodic snapshot failed");
            }
        }
    });

    let billing = std::sync::Arc::new(
        legal_rag_server::billing::BillingStore::load(data_dir.join("billing.json")),
    );
    tracing::info!(provider = %billing.settings().provider, "billing ready");
    let activity = std::sync::Arc::new(
        legal_rag_server::activity::ActivityStore::load(data_dir.join("activity.json")),
    );
    let golden = std::sync::Arc::new(
        legal_rag_server::golden::GoldenStore::load(data_dir.join("golden.json")),
    );
    let feedback = std::sync::Arc::new(
        legal_rag_server::feedback::FeedbackStore::load(data_dir.join("feedback.json")),
    );
    let annotations = std::sync::Arc::new(
        legal_rag_server::annotations::AnnotationStore::load(data_dir.join("annotations.json")),
    );
    let matters = std::sync::Arc::new(legal_rag_server::matters::MatterStore::load(
        data_dir.join("matters.json"),
    ));
    let spaces = std::sync::Arc::new(legal_rag_server::spaces::SpaceStore::load(
        data_dir.join("spaces.json"),
    ));
    // Confidential-vault providers: dedicated (local) endpoints when
    // configured, never wrapped in the shared semantic cache — private
    // text must not land in shared caches.
    let private_embeddings: Arc<dyn Embeddings> = match &config.private_embedding {
        Some(pe) if !use_mocks() => {
            info!(url = %pe.url, model = %pe.model, "confidential vault: dedicated embeddings endpoint");
            Arc::new(OpenAiEmbeddings::new(
                &pe.url,
                pe.api_key.as_deref(),
                &pe.model,
                pe.dims,
            ))
        }
        _ => embeddings.clone(),
    };
    let private_llm: Option<Arc<dyn LlmClient>> = config.private_llm.as_ref().map(|pl| {
        info!(url = %pl.url, model = %pl.model, "confidential vault: dedicated LLM endpoint");
        Arc::new(OpenAiLlm::new(&pl.url, pl.api_key.as_deref(), &pl.model).with_max_tokens(2000))
            as Arc<dyn LlmClient>
    });
    let private_vault = std::sync::Arc::new(legal_rag_server::private_vault::PrivateVault::load(
        data_dir.join("private_vault.json"),
    ));
    // Boot migration: a default matter always exists and adopts every
    // indexed document that is not filed anywhere yet.
    let known_docs: Vec<String> = rag.list_documents().into_iter().map(|d| d.id).collect();
    let default_matter = matters.ensure_default(&known_docs);
    tracing::info!(default_matter = %default_matter, matters = matters.list().len(), "matters ready");

    let state = AppState {
        config: config.clone(),
        auth,
        ingest,
        rag,
        query,
        oidc,
        embeddings,
        store,
        graph,
        llm,
        llm_uncached,
        web,
        llm_cache,
        emb_cache,
        workflows,
        agent,
        orchestrator,
        billing,
        activity,
        golden,
        feedback,
        annotations,
        matters,
        spaces,
        private_vault,
        private_embeddings,
        private_llm,
        settings,
        dyn_llm,
        default_llm_model: config.llm.model.clone(),
        boot_embedding_model: boot_embedding_model.clone(),
    };
    // Push verified annotations into the query service.
    legal_rag_server::routes::annotations::sync(&state);
    // Apply the admin's prompt overrides, if any are saved.
    legal_rag_server::routes::settings::apply_prompts(&state, &boot_settings);
    if boot_settings.prompts.ask.is_some()
        || boot_settings.prompts.research_frame.is_some()
        || boot_settings.prompts.research_adjudicate.is_some()
        || boot_settings.prompts.agent.is_some()
    {
        info!("custom prompts applied from settings");
    }

    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind(&config.server_addr).await?;
    info!(addr = %config.server_addr, data_dir = %data_dir.display(), "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    info!("shutting down; writing final snapshot");
    if let Err(e) = snapshot_handles.save_all().await {
        tracing::warn!(error = %e, "final snapshot failed");
    }
    Ok(())
}

/// Waits for Ctrl-C or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("ctrl_c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("sigterm handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

use std::path::PathBuf;

/// Everything the snapshot saver needs; all behind Arcs.
#[derive(Clone)]
struct SnapshotHandles {
    data_dir: PathBuf,
    mem_store: Option<Arc<InMemoryVectorStore>>,
    store: Arc<dyn VectorStore>,
    graph_store: Arc<InMemoryGraphStore>,
    rag: Arc<RagAnything>,
    treatments: Arc<legal_rag_rag::TreatmentStore>,
    v2_treatments: Arc<legal_rag_rag::citator_v2::V2EdgeStore>,
    user_store: Arc<InMemoryUserStore>,
}

impl SnapshotHandles {
    /// Full snapshot: with an external vector store (SereneDB) the chunk
    /// snapshot is dumped from the active store, so a later boot on the
    /// in-memory fallback still finds every chunk on disk.
    ///
    /// Durability (Piece 20): the chunk snapshot is MONOTONIC — it is
    /// merged with the snapshot already on disk (registry-filtered)
    /// instead of replacing it, so a store that came back from an
    /// unclean shutdown missing committed chunks cannot propagate that
    /// loss into the backup. After each save, hollow documents
    /// (registered, but chunk-less in the store) are recomputed and
    /// re-logged while any exist.
    async fn save_all(&self) -> legal_rag_common::Result<()> {
        if self.mem_store.is_none() {
            match self.store.all_chunks().await {
                Ok(chunks) => {
                    let merged = self.merge_with_disk(chunks);
                    legal_rag_rag::persist::save_json(&self.data_dir, "chunks.json", &merged)?;
                }
                Err(e) => tracing::warn!(error = %e, "chunk dump from vector store failed"),
            }
        }
        let result = self.save();
        self.report_hollow("snapshot save").await;
        result
    }

    /// Union an active chunk set with the `chunks.json` already on disk,
    /// keeping disk-only records whose document is still registered.
    /// The registry is read from the live pipeline — the same registry
    /// `save` writes to `documents.json` in this pass.
    fn merge_with_disk(
        &self,
        active: Vec<legal_rag_rag::ChunkRecord>,
    ) -> Vec<legal_rag_rag::ChunkRecord> {
        use legal_rag_rag::persist;
        let on_disk: Vec<legal_rag_rag::ChunkRecord> =
            match persist::load_json(&self.data_dir, "chunks.json") {
                Ok(Some(chunks)) => chunks,
                Ok(None) => Vec::new(),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "existing chunk snapshot unreadable; writing the active set only"
                    );
                    Vec::new()
                }
            };
        let (documents, _status) = self.rag.snapshot_documents();
        let registered: std::collections::HashSet<String> =
            documents.keys().cloned().collect();
        durability::merge_chunk_snapshot(active, on_disk, &registered)
    }

    /// Recompute hollow documents against the active store and log them.
    async fn report_hollow(&self, when: &str) {
        let documents = self.rag.list_documents();
        if documents.is_empty() {
            return;
        }
        let hollow = durability::hollow_documents(self.store.as_ref(), &documents).await;
        durability::report_hollow(&hollow, when);
    }

    fn save(&self) -> legal_rag_common::Result<()> {
        use legal_rag_rag::persist;
        if let Some(store) = &self.mem_store {
            let merged = self.merge_with_disk(store.snapshot());
            persist::save_json(&self.data_dir, "chunks.json", &merged)?;
        }
        let (entities, relations) = self.graph_store.snapshot();
        persist::save_json(
            &self.data_dir,
            "graph.json",
            &serde_json::json!({ "entities": entities, "relations": relations }),
        )?;
        let (documents, status) = self.rag.snapshot_documents();
        persist::save_json(
            &self.data_dir,
            "documents.json",
            &serde_json::json!({ "documents": documents, "status": status }),
        )?;
        persist::save_json(&self.data_dir, "citator.json", &self.treatments.snapshot())?;
        persist::save_json(
            &self.data_dir,
            "citator_v2.json",
            &self.v2_treatments.snapshot(),
        )?;
        persist::save_json(&self.data_dir, "users.json", &self.user_store.snapshot())?;
        tracing::info!(dir = %self.data_dir.display(), "snapshot saved");
        Ok(())
    }
}

/// Load any existing snapshots into the fresh in-memory stores.
async fn restore_snapshots(
    data_dir: &std::path::Path,
    mem_store: Option<&Arc<InMemoryVectorStore>>,
    store: &Arc<dyn VectorStore>,
    graph_store: &Arc<InMemoryGraphStore>,
    rag: &Arc<RagAnything>,
    treatments: &Arc<legal_rag_rag::TreatmentStore>,
    user_store: &Arc<InMemoryUserStore>,
) {
    use legal_rag_rag::persist;
    match persist::load_json::<Vec<legal_rag_rag::ChunkRecord>>(data_dir, "chunks.json") {
        Ok(Some(chunks)) => {
            info!(chunks = chunks.len(), "restored chunk snapshot");
            if let Some(mem) = mem_store {
                mem.restore(chunks);
            } else {
                // External store (SereneDB): load the snapshot corpus into
                // it — without this the running corpus is invisible to the
                // backed store on first boot. This upsert is also the
                // heal path for a store that lost committed chunks to an
                // unclean shutdown (Piece 20): the monotonic snapshot
                // still holds them.
                match durability::restore_chunk_snapshot(store.as_ref(), chunks).await {
                    Ok(()) => info!("chunk snapshot loaded into the vector store"),
                    Err(e) => tracing::warn!(error = %e, "chunk snapshot upsert failed"),
                }
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "chunk snapshot load failed"),
    }
    match persist::load_json::<serde_json::Value>(data_dir, "graph.json") {
        Ok(Some(v)) => {
            let entities: Vec<legal_rag_rag::graph::Entity> =
                serde_json::from_value(v.get("entities").cloned().unwrap_or_default())
                    .unwrap_or_default();
            let relations: Vec<legal_rag_rag::graph::Relation> =
                serde_json::from_value(v.get("relations").cloned().unwrap_or_default())
                    .unwrap_or_default();
            info!(entities = entities.len(), relations = relations.len(), "restored graph snapshot");
            graph_store.restore(entities, relations);
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "graph snapshot load failed"),
    }
    match persist::load_json::<serde_json::Value>(data_dir, "documents.json") {
        Ok(Some(v)) => {
            let documents = serde_json::from_value(
                v.get("documents").cloned().unwrap_or_default(),
            )
            .unwrap_or_default();
            let status = serde_json::from_value(v.get("status").cloned().unwrap_or_default())
                .unwrap_or_default();
            rag.restore_documents(documents, status);
            info!("restored document registry snapshot");
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "document snapshot load failed"),
    }
    match persist::load_json::<Vec<legal_rag_rag::TreatmentEdge>>(data_dir, "citator.json") {
        Ok(Some(edges)) => {
            info!(edges = edges.len(), "restored citator snapshot");
            treatments.restore(edges);
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "citator snapshot load failed"),
    }
    match persist::load_json::<Vec<legal_rag_rag::citator_v2::StoredV2Edge>>(
        data_dir,
        "citator_v2.json",
    ) {
        Ok(Some(edges)) => {
            info!(edges = edges.len(), "restored citator V2 snapshot");
            rag.v2_treatments().restore(edges);
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "citator V2 snapshot load failed"),
    }
    match persist::load_json::<Vec<legal_rag_auth::UserSnapshot>>(data_dir, "users.json") {
        Ok(Some(users)) => {
            info!(users = users.len(), "restored user snapshot");
            user_store.restore(users);
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "user snapshot load failed"),
    }
    // Corpus durability (Piece 20): with registry and store both
    // restored, name every registered document the store holds no
    // chunks for — loudly, at boot, instead of letting Interact/Ask
    // discover it per-request.
    let documents = rag.list_documents();
    if !documents.is_empty() {
        let hollow = durability::hollow_documents(store.as_ref(), &documents).await;
        durability::report_hollow(&hollow, "boot restore");
    }
}
