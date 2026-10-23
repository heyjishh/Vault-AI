//! `/api/v1/knowledge/*` — RAG query over the indexed corpus.

use axum::extract::State;
use axum::Json;
use legal_rag_auth::{AdminUser, AuthUser};
use legal_rag_rag::{Entity, QueryAnswer, QueryMode, Relation};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct QueryRequest {
    pub query: String,
    pub mode: Option<QueryMode>,
    pub top_k: Option<usize>,
    /// When true, blend in live web results via the stealth sidecar.
    pub web: Option<bool>,
    pub web_results: Option<usize>,
    /// When set, scope retrieval to this matter's documents.
    pub matter_id: Option<String>,
    /// When true, also search the user's confidential vault (must be
    /// unlocked) and blend its top hits as `[private:N]` citations.
    pub private: Option<bool>,
    /// Legal-validity filter: only cite documents in force on this date
    /// (YYYY-MM-DD). A hard boundary — no full-vault fallback.
    pub as_of: Option<String>,
    /// Legal-validity filter: only cite documents of this jurisdiction
    /// (documents with no jurisdiction set stay eligible).
    pub jurisdiction: Option<String>,
}

/// Apply the legal-validity filters (as-of date / jurisdiction) to a base
/// document scope. Returns the effective scope and whether filters are
/// active. When active, the effective scope is always `Some` (possibly
/// empty) and callers must use the strict query paths: out-of-force law
/// is never served via the brief fallback.
fn filtered_scope(
    state: &AppState,
    base: Option<Vec<String>>,
    body: &QueryRequest,
) -> (Option<Vec<String>>, bool) {
    let as_of = body.as_of.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let juris = body.jurisdiction.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if as_of.is_none() && juris.is_none() {
        return (base, false);
    }
    let universe: Vec<String> = match base {
        Some(docs) => docs,
        None => state.rag.list_documents().into_iter().map(|d| d.id).collect(),
    };
    let kept = universe
        .into_iter()
        .filter(|id| {
            state
                .rag
                .document_meta(id)
                .map(|m| m.eligible(as_of, juris))
                .unwrap_or(true)
        })
        .collect();
    (Some(kept), true)
}

/// Resolve the request's confidential sources: embed the question, cosine
/// search the owner's vault, decrypt the top hits for this request only.
/// Asking for private sources with a locked vault is an explicit error —
/// never a silent skip.
async fn private_sources(
    state: &AppState,
    user_id: &str,
    question: &str,
    want: bool,
) -> Result<Vec<legal_rag_rag::ExtraSource>, ApiError> {
    if !want {
        return Ok(Vec::new());
    }
    if !state.private_vault.is_unlocked(user_id) {
        return Err(legal_rag_common::Error::BadRequest(
            "your confidential vault is locked — unlock it to include private documents"
                .to_string(),
        )
        .into());
    }
    let qvec = state
        .private_embeddings
        .embed(&[question.to_string()])
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .next()
        .unwrap_or_default();
    let hits = state.private_vault.search(user_id, &qvec, 3).unwrap_or_default();
    Ok(hits
        .into_iter()
        .map(|(title, text, score)| legal_rag_rag::ExtraSource { title, text, score })
        .collect())
}

/// The admin-configured default mode, used when the request doesn't
/// specify one (the user panel no longer offers a mode picker).
fn settings_mode(state: &AppState) -> QueryMode {
    serde_json::from_value(serde_json::json!(state.settings.get().default_mode))
        .unwrap_or_default()
}

/// Resolve a request's matter to its document scope (404 on unknown id).
fn doc_scope(
    state: &AppState,
    matter_id: &Option<String>,
    user: &legal_rag_auth::User,
) -> Result<Option<Vec<String>>, ApiError> {
    match matter_id {
        Some(mid) if !mid.trim().is_empty() => {
            Ok(Some(crate::routes::matters::scope_docs(state, mid, user)?))
        }
        _ => Ok(None),
    }
}

/// Ask a question; returns a grounded answer with citations.
pub async fn query(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<QueryRequest>,
) -> Result<Json<QueryAnswer>, ApiError> {
    if body.query.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("query is empty".to_string()).into());
    }
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let mode = body.mode.unwrap_or_else(|| settings_mode(&state));
    let top_k = body.top_k.unwrap_or(state.config.default_top_k);
    let docs = doc_scope(&state, &body.matter_id, &user)?;
    let (docs, strict) = filtered_scope(&state, docs, &body);
    let want_private = body.private.unwrap_or(false);
    let extra = private_sources(&state, &user.id, &body.query, want_private).await?;
    let scope = legal_rag_rag::usage::new_scope();
    let answer = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        if !extra.is_empty() {
            // Confidential sources take precedence over web blending.
            state
                .query
                .query_full(&body.query, mode, top_k, docs.clone(), extra, strict, state.private_llm.clone())
                .await
        } else if body.web.unwrap_or(false) {
            state
                .query
                .query_with_web(
                    &body.query,
                    mode,
                    top_k,
                    body.web_results.unwrap_or(3).clamp(1, 5),
                    docs.clone(),
                    strict,
                )
                .await
        } else if let Some(docs) = &docs {
            if strict {
                state.query.query_scoped_strict(&body.query, mode, top_k, docs.clone()).await
            } else {
                state.query.query_scoped(&body.query, mode, top_k, docs.clone()).await
            }
        } else {
            state.query.query(&body.query, mode, top_k).await
        }
    })
    .await?;
    // A private answer may quote confidential text verbatim — never write
    // it into the (plaintext) activity log.
    let activity_result = if want_private { String::new() } else { answer.answer.clone() };
    let payload = if want_private {
        None
    } else {
        Some(serde_json::json!({
            "question": body.query,
            "answer": answer.answer,
            "citations": answer.citations,
            "mode": format!("{:?}", mode).to_lowercase(),
            "scope_note": answer.scope_note,
        }))
    };
    crate::routes::billing::settle_matter_payload(
        &state,
        &user,
        &scope,
        "query",
        &body.query,
        &format!("{:?}", mode).to_lowercase(),
        &activity_result,
        answer.citations.len(),
        body.matter_id.clone(),
        payload,
    );
    Ok(Json(answer))
}

#[derive(Deserialize)]
pub struct SourceDocumentRequest {
    /// Web citation: fetch this URL's full text via the web pipeline.
    pub source_url: Option<String>,
    /// Corpus citation: a chunk id (record UUID) from the citation —
    /// resolved to its owning document server-side.
    pub chunk_id: Option<String>,
    /// Corpus document id directly, when the caller already knows it.
    pub doc_id: Option<String>,
}

#[derive(Serialize)]
pub struct SourceDocumentResponse {
    pub title: Option<String>,
    pub url: Option<String>,
    pub date: Option<String>,
    pub text: String,
    /// The resolved corpus document id (corpus sources only).
    pub document_id: Option<String>,
}

/// The full text behind one citation, for the in-app source viewer
/// (the modal highlights the cited passage inside it). Web sources are
/// fetched on demand and cached in the query service; corpus documents
/// come from the index. Confidential-vault citations are not served
/// here — their text never leaves the vault routes.
pub async fn source_document(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<SourceDocumentRequest>,
) -> Result<Json<SourceDocumentResponse>, ApiError> {
    if let Some(url) = body.source_url.filter(|u| !u.trim().is_empty()) {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(legal_rag_common::Error::BadRequest(
                "source_url must be an http(s) URL".to_string(),
            )
            .into());
        }
        let page = state.query.fetch_web_source(&url).await?;
        return Ok(Json(SourceDocumentResponse {
            title: Some(page.title),
            url: Some(page.url),
            date: page.date,
            text: page.text,
            document_id: None,
        }));
    }
    if let Some(chunk_id) = body.chunk_id.filter(|d| !d.trim().is_empty()) {
        let (document_id, text) =
            state.rag.document_text_of_chunk(&chunk_id, 200_000).await?;
        if !crate::routes::matters::doc_visible(&state, &user, &document_id) {
            return Err(legal_rag_common::Error::NotFound("document not found".to_string()).into());
        }
        return Ok(Json(SourceDocumentResponse {
            title: None,
            url: None,
            date: None,
            text,
            document_id: Some(document_id),
        }));
    }
    if let Some(doc_id) = body.doc_id.filter(|d| !d.trim().is_empty()) {
        if !crate::routes::matters::doc_visible(&state, &user, &doc_id) {
            return Err(legal_rag_common::Error::NotFound("document not found".to_string()).into());
        }
        let text = state.rag.document_text(&doc_id, 200_000).await?;
        return Ok(Json(SourceDocumentResponse {
            title: None,
            url: None,
            date: None,
            text,
            document_id: Some(doc_id),
        }));
    }
    Err(legal_rag_common::Error::BadRequest(
        "source_url, chunk_id or doc_id is required".to_string(),
    )
    .into())
}

/// Streaming query over SSE: status → sources → delta… → done.
/// The answer text arrives token-by-token as the LLM generates it.
pub async fn query_stream(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<QueryRequest>,
) -> Result<axum::response::Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>, ApiError> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use legal_rag_rag::QueryEvent;

    if body.query.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("query is empty".to_string()).into());
    }
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let mode = body.mode.unwrap_or_else(|| settings_mode(&state));
    let top_k = body.top_k.unwrap_or(state.config.default_top_k);
    let web = body.web.unwrap_or(false);
    let web_results = body.web_results.unwrap_or(3).clamp(1, 5);
    let docs = doc_scope(&state, &body.matter_id, &user)?;
    let (docs, strict) = filtered_scope(&state, docs, &body);
    let matter_id = body.matter_id.clone();
    let want_private = body.private.unwrap_or(false);
    let extra = private_sources(&state, &user.id, &body.query, want_private).await?;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<QueryEvent>();
    let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel::<QueryEvent>();
    let svc = state.query.clone();
    let private_llm = if extra.is_empty() {
        None
    } else {
        state.private_llm.clone()
    };
    let question = body.query.clone();
    let billing = state.billing.clone();
    let activity = state.activity.clone();
    let user_id = user.id.clone();
    let user_email = user.email.clone();
    let mode_label = format!("{:?}", mode).to_lowercase();
    tokio::spawn(async move {
        let scope = legal_rag_rag::usage::new_scope();
        let scope_in = scope.clone();
        let svc2 = svc.clone();
        let question2 = question.clone();
        let docs2 = docs.clone();
        let extra2 = extra.clone();
        let run = tokio::spawn(async move {
            legal_rag_rag::usage::scope_usage(scope_in, async move {
                svc2.query_stream_scoped(
                    &question2, mode, top_k, web, web_results, docs2, &extra2, strict, private_llm, inner_tx,
                )
                .await
            })
            .await
        });
        // Forward events to the client, capturing the final answer so the
        // streamed question lands in the activity log like any other.
        let mut answer_text = String::new();
        let mut citation_count = 0usize;
        let mut final_citations: Vec<legal_rag_rag::Citation> = Vec::new();
        while let Some(ev) = inner_rx.recv().await {
            match &ev {
                QueryEvent::Done { answer, .. } => answer_text = answer.clone(),
                QueryEvent::Sources { citations, .. } => {
                    citation_count = citations.len();
                    final_citations = citations.clone();
                }
                _ => {}
            }
            let _ = tx.send(ev);
        }
        let result = match run.await {
            Ok(r) => r,
            Err(e) => Err(legal_rag_common::Error::Other(anyhow::anyhow!("stream task failed: {e}"))),
        };
        let totals = scope.lock().map(|t| *t).unwrap_or_default();
        billing.add_usage(&user_id, &totals);
        // Private answers may quote confidential text — keep them out of
        // the plaintext activity log.
        let activity_result = if want_private { String::new() } else { answer_text.clone() };
        let payload = if want_private {
            None
        } else {
            Some(serde_json::json!({
                "question": question,
                "answer": answer_text,
                "citations": final_citations,
                "mode": mode_label,
            }))
        };
        activity.record(
            crate::activity::ActivityEvent::new(
                &user_id,
                &user_email,
                "query",
                &question,
                &mode_label,
                &activity_result,
                citation_count,
                totals.total(),
            )
            .with_matter(matter_id)
            .with_payload(payload),
        );
        if let Err(e) = result {
            let message = if e.to_string().contains("429") {
                "error: The AI provider is rate-limiting requests right now — wait a few seconds and press Retry.".to_string()
            } else {
                format!("error: {e}")
            };
            let _ = tx.send(QueryEvent::Status { message });
        }
    });

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Some(ev) => {
                let (name, data) = match &ev {
                    QueryEvent::Status { .. } => ("status", serde_json::to_string(&ev).unwrap_or_default()),
                    QueryEvent::Sources { .. } => ("sources", serde_json::to_string(&ev).unwrap_or_default()),
                    QueryEvent::Delta { .. } => ("delta", serde_json::to_string(&ev).unwrap_or_default()),
                    QueryEvent::Done { .. } => ("done", serde_json::to_string(&ev).unwrap_or_default()),
                };
                Some((Ok(Event::default().event(name).data(data)), rx))
            }
            None => None,
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(Deserialize)]
pub struct ResearchRequest {
    pub query: String,
    /// When true, each hypothesis also gathers live web evidence.
    pub web: Option<bool>,
}

/// Deep research: epistemic loop (frame → evidence → adjudicate → report).
pub async fn research(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<ResearchRequest>,
) -> Result<Json<legal_rag_rag::ResearchReport>, ApiError> {
    if body.query.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("query is empty".to_string()).into());
    }
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let scope = legal_rag_rag::usage::new_scope();
    let report = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        state.query.research(&body.query, body.web.unwrap_or(false)).await
    })
    .await?;
    crate::routes::billing::settle_with_payload(
        &state,
        &user,
        &scope,
        "research",
        &body.query,
        if body.web.unwrap_or(false) { "web" } else { "corpus" },
        &report.conclusion,
        report.citations.len(),
        Some(serde_json::json!({ "question": body.query, "report": report })),
    );
    Ok(Json(report))
}

/// Semantic-cache (KV cache) hit/miss stats.
pub async fn cache_stats(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({
        "llm": state.llm_cache.as_ref().map(|c| c.stats()),
        "embeddings": state.emb_cache.as_ref().map(|c| c.stats()),
        "answers": state.query.answer_cache_stats(),
    })))
}

#[derive(Deserialize)]
pub struct HitTestRequest {
    pub question: String,
    pub top_k: Option<usize>,
}

/// Admin retrieval hit-testing: one question through every retrieval
/// stage (vector / BM25 / fused / reranked), no answer generated.
pub async fn hit_test(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Json(body): Json<HitTestRequest>,
) -> Result<Json<legal_rag_rag::HitTestReport>, ApiError> {
    if body.question.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("question is empty".into()).into());
    }
    let report = state.query.hit_test(body.question.trim(), body.top_k.unwrap_or(8)).await?;
    Ok(Json(report))
}

/// List available agentic workflows.
pub async fn workflow_list(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
) -> Result<Json<Vec<legal_rag_rag::WorkflowDef>>, ApiError> {
    Ok(Json(state.workflows.list()))
}

#[derive(Deserialize)]
pub struct WorkflowRunRequest {
    pub workflow_id: String,
    pub input: String,
}

/// Run an agentic workflow end to end.
pub async fn workflow_run(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<WorkflowRunRequest>,
) -> Result<Json<legal_rag_rag::WorkflowRun>, ApiError> {
    if body.input.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("input is empty".to_string()).into());
    }
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let scope = legal_rag_rag::usage::new_scope();
    let run = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        state.workflows.run(&body.workflow_id, &body.input).await
    })
    .await?;
    crate::routes::billing::settle_with_payload(
        &state,
        &user,
        &scope,
        "workflow",
        &body.input,
        &run.workflow_name,
        &run.final_report,
        run.citations.len(),
        Some(serde_json::json!({ "workflow_id": body.workflow_id, "input": body.input, "run": run })),
    );
    Ok(Json(run))
}

#[derive(Deserialize)]
pub struct AutonomousRunRequest {
    pub goal: String,
    /// Confine vault searches to this brief (space-checked).
    pub matter_id: Option<String>,
    /// Planner step budget, 1..=10 (default 6).
    pub max_steps: Option<usize>,
}

/// Autonomous agent run: a planner loop over vault/web search tools,
/// ending in a synthesized, cited report. Billed like a workflow run.
pub async fn autonomous_run(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<AutonomousRunRequest>,
) -> Result<Json<legal_rag_rag::AutonomousRun>, ApiError> {
    if body.goal.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("goal is empty".to_string()).into());
    }
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let docs = doc_scope(&state, &body.matter_id, &user)?;
    let scope = legal_rag_rag::usage::new_scope();
    let run = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        state
            .agent
            .run(body.goal.trim(), docs, body.max_steps.unwrap_or(6))
            .await
    })
    .await?;
    crate::routes::billing::settle_with_payload(
        &state,
        &user,
        &scope,
        "autonomous",
        &body.goal,
        "Autonomous research",
        &run.final_report,
        run.citations.len(),
        Some(serde_json::json!({ "goal": body.goal, "run": run })),
    );
    Ok(Json(run))
}

#[derive(Serialize)]
pub struct CorpusStats {
    pub documents: usize,
    pub chunks: usize,
    pub entities: usize,
    pub relations: usize,
}

/// Corpus statistics for dashboards.
pub async fn stats(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
) -> Result<Json<CorpusStats>, ApiError> {
    Ok(Json(CorpusStats {
        documents: state.rag.list_documents().len(),
        chunks: state.rag.corpus_size().await,
        entities: state.graph.entity_count().await,
        relations: state.graph.relation_count().await,
    }))
}

#[derive(Serialize)]
pub struct GraphView {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
}

/// Full knowledge graph for visualization.
pub async fn graph_view(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
) -> Result<Json<GraphView>, ApiError> {
    Ok(Json(GraphView {
        entities: state.graph.all_entities().await?,
        relations: state.graph.all_relations().await?,
    }))
}
