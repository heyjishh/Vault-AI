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
    /// Brief-vault mode (Piece 12): when true, the corpus leg is
    /// HARD-scoped to the active brief's vault — no fallback to the
    /// full vault — and the answer's scope note names the brief. The
    /// active brief is `matter_id`, or the default brief when no
    /// matter is named (after Piece 11 a brief is always active).
    pub brief_vault: Option<bool>,
    /// When true, also search the user's confidential vault (must be
    /// unlocked) and blend its top hits as `[private:N]` citations.
    pub private: Option<bool>,
    /// Legal-validity filter: only cite documents in force on this date
    /// (YYYY-MM-DD). A hard boundary — no full-vault fallback.
    pub as_of: Option<String>,
    /// Legal-validity filter: only cite documents of this jurisdiction
    /// (documents with no jurisdiction set stay eligible).
    pub jurisdiction: Option<String>,
    /// Citator V2 context: the asker's state code (e.g. "MH", full
    /// name, or dotted form — unknown codes behave as absent) so that
    /// state's High Court can govern within it.
    pub state: Option<String>,
    /// Citator V2 context: the applicant name for the AAR/AAAR rule
    /// (an AAR/AAAR ruling governs only for its own applicant).
    pub applicant: Option<String>,
    /// Refinement terms (Piece 13): retrieval terms the client's
    /// Refine step produced for this prompt. Sanitized server-side
    /// and folded into the retrieval expansion pool.
    pub refine_terms: Option<Vec<String>>,
}

/// Sanitize client-supplied refinement terms the same way the rag
/// layer sanitizes model-produced ones (trim, dedupe, caps) — client
/// terms steer retrieval only and are never trusted raw.
pub(crate) fn body_terms(terms: &Option<Vec<String>>) -> Vec<String> {
    terms
        .as_ref()
        .map(|t| legal_rag_rag::clean_refine_terms(t))
        .unwrap_or_default()
}

/// Build the per-request citator context: state normalized through
/// the shared vocabulary (unknown → None, never a crash), applicant
/// trimmed with empty → None.
fn citator_ctx(body: &QueryRequest) -> legal_rag_rag::CitatorContext {
    crate::routes::citator_context(body.state.as_deref(), body.applicant.as_deref())
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
pub(crate) async fn private_sources(
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

/// Piece 12 — brief-vault resolution for one Ask request. Flag off:
/// the request's matter passes through and there is no label (today's
/// behavior, soft scoping with the disclosed zero-hit fallback). Flag
/// on: the effective matter is the request's matter or, when none is
/// named, the default brief; its documents are resolved with the
/// caller's read access enforced, and the brief's title rides along —
/// the query layer hard-scopes to those documents and stamps the
/// label into the answer's scope note.
struct BriefVaultScope {
    matter_id: Option<String>,
    docs: Option<Vec<String>>,
    label: Option<String>,
}

fn brief_vault_scope(
    state: &AppState,
    body: &QueryRequest,
    user: &legal_rag_auth::User,
) -> Result<BriefVaultScope, ApiError> {
    if !body.brief_vault.unwrap_or(false) {
        return Ok(BriefVaultScope {
            matter_id: body.matter_id.clone(),
            docs: None,
            label: None,
        });
    }
    let mid = body
        .matter_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| state.matters.default_id())
        .ok_or_else(|| {
            ApiError::from(legal_rag_common::Error::BadRequest(
                "Brief vault mode needs an active brief".to_string(),
            ))
        })?;
    let docs = crate::routes::matters::scope_docs(state, &mid, user)?;
    let label = state
        .matters
        .get(&mid)
        .map(|m| m.title)
        .unwrap_or_else(|| "this brief".to_string());
    Ok(BriefVaultScope {
        matter_id: Some(mid),
        docs: Some(docs),
        label: Some(label),
    })
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
    let ctx = citator_ctx(&body);
    let bv = brief_vault_scope(&state, &body, &user)?;
    let docs = match bv.docs.clone() {
        Some(d) => Some(d),
        None => doc_scope(&state, &body.matter_id, &user)?,
    };
    let (docs, strict) = filtered_scope(&state, docs, &body);
    // Brief-vault mode is a hard boundary regardless of the
    // legal-validity filters (which can only narrow it further).
    let strict = strict || bv.label.is_some();
    let opts = legal_rag_rag::AskOptions {
        brief_vault: bv.label.clone(),
        refine_terms: body_terms(&body.refine_terms),
    };
    let want_private = body.private.unwrap_or(false);
    let extra = private_sources(&state, &user.id, &body.query, want_private).await?;
    let scope = legal_rag_rag::usage::new_scope();
    let answer = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        if !extra.is_empty() {
            // Confidential sources take precedence over web blending.
            state
                .query
                .query_full(&body.query, mode, top_k, docs.clone(), extra, strict, state.private_llm.clone(), ctx.clone(), &opts)
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
                    ctx.clone(),
                    &opts,
                )
                .await
        } else {
            state
                .query
                .query_opts(&body.query, mode, top_k, docs.clone(), strict, &opts, ctx.clone())
                .await
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
        bv.matter_id.clone(),
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
    let ctx = citator_ctx(&body);
    let web = body.web.unwrap_or(false);
    let web_results = body.web_results.unwrap_or(3).clamp(1, 5);
    let bv = brief_vault_scope(&state, &body, &user)?;
    let docs = match bv.docs.clone() {
        Some(d) => Some(d),
        None => doc_scope(&state, &body.matter_id, &user)?,
    };
    let (docs, strict) = filtered_scope(&state, docs, &body);
    // Brief-vault mode is a hard boundary on the stream too.
    let strict = strict || bv.label.is_some();
    let opts = legal_rag_rag::AskOptions {
        brief_vault: bv.label.clone(),
        refine_terms: body_terms(&body.refine_terms),
    };
    let matter_id = bv.matter_id.clone();
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
        let opts2 = opts.clone();
        let run = tokio::spawn(async move {
            legal_rag_rag::usage::scope_usage(scope_in, async move {
                svc2.query_stream_scoped(
                    &question2, mode, top_k, web, web_results, docs2, &extra2, strict, private_llm, ctx, &opts2, inner_tx,
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
    /// Citator V2 context, as on Ask: asker state (unknown codes
    /// behave as absent) and AAR/AAAR applicant.
    pub state: Option<String>,
    pub applicant: Option<String>,
    /// Refinement terms (Piece 13), folded into every hypothesis's
    /// retrieval.
    pub refine_terms: Option<Vec<String>>,
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
    let ctx = crate::routes::citator_context(body.state.as_deref(), body.applicant.as_deref());
    let report = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        state.query.research(&body.query, body.web.unwrap_or(false), &body_terms(&body.refine_terms), ctx).await
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

/// Maximum prompt length the refine endpoint accepts, in characters.
/// Prompts are chat inputs; anything longer is a document, not a
/// prompt, and gets a clean refusal rather than a giant LLM call.
pub const REFINE_MAX_CHARS: usize = 4000;

#[derive(Deserialize)]
pub struct RefineRequest {
    pub text: String,
    /// Which chat surface the prompt belongs to ("ask", "interact",
    /// "research", "draft", "agents") — context for the refinement,
    /// not a behavior switch.
    pub surface: Option<String>,
}

#[derive(Serialize)]
pub struct RefineResponse {
    pub refined: String,
    pub terms: Vec<String>,
}

/// Validate + normalize a refine request's text (pure; unit-tested).
/// Empty and over-cap inputs are clean client errors — the client
/// keeps the user's original text untouched in both cases.
fn validate_refine_text(raw: &str) -> Result<String, ApiError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(
            legal_rag_common::Error::BadRequest("nothing to refine — the prompt is empty".into())
                .into(),
        );
    }
    if text.chars().count() > REFINE_MAX_CHARS {
        return Err(legal_rag_common::Error::BadRequest(format!(
            "prompt is too long to refine (max {REFINE_MAX_CHARS} characters)"
        ))
        .into());
    }
    Ok(text.to_string())
}

/// Refine a chat prompt (Piece 13): a user-clicked, visible rewrite
/// plus retrieval terms. Never called implicitly — the rewrite is
/// shown to the user, who edits or undoes it before sending.
/// Identical inputs are served by the shared LLM response cache the
/// QueryService's client is wrapped in (same system+prompt → cached
/// completion), so repeat clicks on unchanged text cost no second
/// provider call; no second cache is kept. Metered through the
/// standard usage scope like every LLM-backed route. Deliberately
/// NOT settled as an activity record: a refine click is editing
/// assistance on text the user is still composing, not a work
/// product — the send that follows is what lands in history.
pub async fn refine(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<RefineRequest>,
) -> Result<Json<RefineResponse>, ApiError> {
    let text = validate_refine_text(&body.text)?;
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let surface = body
        .surface
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("ask");
    let scope = legal_rag_rag::usage::new_scope();
    let out = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        state.query.refine_prompt(&text, surface).await
    })
    .await?;
    let totals = scope.lock().map(|t| *t).unwrap_or_default();
    state.billing.add_usage(&user.id, &totals);
    Ok(Json(RefineResponse {
        refined: out.refined,
        terms: out.terms,
    }))
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

/// Admin: live retrieval-stack status for the Command Center —
/// which engines and Tier-1/Tier-2 stages are active in this process.
/// Env-gated features report their startup configuration; runtime
/// state (late interaction bound, MCP) is read live.
pub async fn retrieval_status(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Json<serde_json::Value> {
    let env_on = |k: &str| std::env::var(k).map(|v| v != "0").unwrap_or(true);
    let vector_store = match std::env::var("LEGAL_RAG_VECTOR_STORE").as_deref() {
        Ok("hnsw") => "hnsw (embedded)",
        Ok("s3vectors") => "s3 vectors",
        Ok("memory") => "in-memory",
        Ok("serenedb") => "serenedb",
        _ if std::env::var("SERENEDB_URL").is_ok() => "serenedb",
        _ => "in-memory",
    };
    Json(serde_json::json!({
        "vector_store": vector_store,
        "fusion": std::env::var("LEGAL_RAG_FUSION").unwrap_or_else(|_| "rrf".to_string()),
        "statute_boost": std::env::var("LEGAL_RAG_STATUTE_BOOST").unwrap_or_else(|_| "1.35".to_string()),
        "bm25_engine": std::env::var("LEGAL_RAG_BM25_ENGINE").unwrap_or_else(|_| "native".to_string()),
        "bm25_enabled": env_on("LEGAL_RAG_BM25"),
        "parent_child": env_on("LEGAL_RAG_PARENT_CHILD"),
        "augmentation": env_on("LEGAL_RAG_AUGMENT"),
        "hyperedges": env_on("LEGAL_RAG_HYPEREDGES"),
        "hyde": env_on("LEGAL_RAG_HYDE"),
        "late_interaction": state.query.late_interaction_enabled(),
        "retrieval_cache": std::env::var("LEGAL_RAG_RETRIEVAL_CACHE").map(|v| v != "0").unwrap_or(true),
        "semantic_cache": env_on("LEGAL_RAG_SEMANTIC_CACHE"),
        "mcp_enabled": crate::routes::mcp::mcp_enabled(),
    }))
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
    /// Refinement terms (Piece 13), folded into every step's retrieval.
    pub refine_terms: Option<Vec<String>>,
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
        state.workflows.run(&body.workflow_id, &body.input, &body_terms(&body.refine_terms)).await
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
    /// Planner step budget, 1..=10 (default 6). Legacy mode only;
    /// ignored by orchestrated runs.
    pub max_steps: Option<usize>,
    /// Orchestrated sub-agent run: lead → parallel researchers →
    /// drafter → checker → reconciler (default true). `false` runs
    /// the legacy single-agent planner-executor loop instead.
    pub orchestrated: Option<bool>,
    /// When true (orchestrated only), stream SSE instead of JSON:
    /// a `status` event per sub-agent, then a `done` event carrying
    /// the final report plus every sub-agent's receipt.
    pub stream: Option<bool>,
    /// Citator V2 context for researcher legs + the governing line,
    /// as on Ask: asker state (unknown codes behave as absent) and
    /// AAR/AAAR applicant.
    pub state: Option<String>,
    pub applicant: Option<String>,
    /// Refinement terms (Piece 13), folded into the researcher legs'
    /// retrieval (orchestrated) and the planner's steps (legacy).
    pub refine_terms: Option<Vec<String>>,
}

/// Orchestrator config for this server: commissioned defaults, with
/// the per-run token cap overridable by the admin settings knob.
fn orchestration_config(state: &AppState) -> legal_rag_rag::OrchestratorConfig {
    let mut cfg = legal_rag_rag::OrchestratorConfig::default();
    if let Some(cap) = state.settings.get().orchestration_run_cap_tokens {
        cfg.per_run_token_cap = cap.clamp(10_000, 1_000_000);
    }
    cfg
}

/// Autonomous agent run. Default: the orchestrated sub-agent pipeline
/// (P1 sequential specialists + P2 parallel research) with per-sub-agent
/// receipts; `orchestrated: false` keeps the legacy planner-executor.
/// Billed like a workflow run in every mode.
pub async fn autonomous_run(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<AutonomousRunRequest>,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    if body.goal.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("goal is empty".to_string()).into());
    }
    let orchestrated = body.orchestrated.unwrap_or(true);
    let stream = body.stream.unwrap_or(false);
    if stream && !orchestrated {
        return Err(legal_rag_common::Error::BadRequest(
            "stream requires an orchestrated run".to_string(),
        )
        .into());
    }
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let docs = doc_scope(&state, &body.matter_id, &user)?;
    let ctx = crate::routes::citator_context(body.state.as_deref(), body.applicant.as_deref());

    if stream {
        return Ok(autonomous_stream(state, user, body, docs, ctx).await.into_response());
    }

    if orchestrated {
        let cfg = orchestration_config(&state);
        let scope = legal_rag_rag::usage::new_scope();
        let run = legal_rag_rag::usage::scope_usage(scope.clone(), async {
            state
                .orchestrator
                .run(body.goal.trim(), docs, cfg, ctx, &body_terms(&body.refine_terms), None)
                .await
        })
        .await?;
        crate::routes::billing::settle_with_payload(
            &state,
            &user,
            &scope,
            "autonomous",
            &body.goal,
            "Autonomous research (orchestrated)",
            &run.final_report,
            run.citations.len(),
            Some(serde_json::json!({ "goal": body.goal, "run": run })),
        );
        return Ok(Json(run).into_response());
    }

    let scope = legal_rag_rag::usage::new_scope();
    let run = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        state
            .agent
            .run(body.goal.trim(), docs, body.max_steps.unwrap_or(6), &body_terms(&body.refine_terms))
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
    Ok(Json(run).into_response())
}

/// SSE variant of the orchestrated run: status per sub-agent, then an
/// extended Done event carrying the final report, the citation audit
/// and every sub-agent's receipt. Billing/activity settle when the run
/// completes, mirroring the streamed Ask path.
async fn autonomous_stream(
    state: AppState,
    user: legal_rag_auth::User,
    body: AutonomousRunRequest,
    docs: Option<Vec<String>>,
    ctx: legal_rag_rag::CitatorContext,
) -> axum::response::Sse<
    impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use legal_rag_rag::QueryEvent;

    let cfg = orchestration_config(&state);
    let goal = body.goal.trim().to_string();
    let terms = body_terms(&body.refine_terms);
    let matter_id = body.matter_id.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<QueryEvent>();
    let orchestrator = state.orchestrator.clone();
    let billing = state.billing.clone();
    let activity = state.activity.clone();
    let user_id = user.id.clone();
    let user_email = user.email.clone();
    tokio::spawn(async move {
        let progress_tx = tx.clone();
        let scope = legal_rag_rag::usage::new_scope();
        let scope_in = scope.clone();
        let run = legal_rag_rag::usage::scope_usage(scope_in, async {
            orchestrator
                .run(&goal, docs, cfg, ctx, &terms, Some(progress_tx))
                .await
        })
        .await;
        match run {
            Ok(run) => {
                let totals = scope.lock().map(|t| *t).unwrap_or_default();
                billing.add_usage(&user_id, &totals);
                activity.record(
                    crate::activity::ActivityEvent::new(
                        &user_id,
                        &user_email,
                        "autonomous",
                        &goal,
                        "orchestrated",
                        &run.final_report,
                        run.citations.len(),
                        totals.total(),
                    )
                    .with_matter(matter_id)
                    .with_payload(Some(serde_json::json!({
                        "goal": goal,
                        "run": run,
                    }))),
                );
                let _ = tx.send(QueryEvent::Done {
                    answer: run.final_report,
                    audit: run.citation_audit,
                    expansions: Vec::new(),
                    sub_agents: run.sub_agents,
                });
            }
            Err(e) => {
                let _ = tx.send(QueryEvent::Status {
                    message: format!("error: {e}"),
                });
            }
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
    Sse::new(stream).keep_alive(KeepAlive::default())
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

#[cfg(test)]
mod refine_validation_tests {
    //! Piece 13: the refine endpoint's input contract — empty and
    //! over-cap prompts are clean client errors (the client keeps
    //! the original text); valid text passes trimmed.
    use super::{validate_refine_text, REFINE_MAX_CHARS};

    #[test]
    fn empty_and_whitespace_prompts_are_rejected() {
        assert!(validate_refine_text("").is_err());
        assert!(validate_refine_text("   \n\t ").is_err());
    }

    #[test]
    fn over_cap_prompts_are_rejected_at_the_boundary() {
        let at_cap = "x".repeat(REFINE_MAX_CHARS);
        assert!(validate_refine_text(&at_cap).is_ok());
        let over = "x".repeat(REFINE_MAX_CHARS + 1);
        let err = validate_refine_text(&over).unwrap_err();
        assert!(err.0.to_string().contains("too long"), "clean cap error: {}", err.0);
    }

    #[test]
    fn valid_prompts_pass_trimmed() {
        assert_eq!(
            validate_refine_text("  batao kiraya kitna hai  ")
                .ok()
                .as_deref(),
            Some("batao kiraya kitna hai")
        );
    }
}
