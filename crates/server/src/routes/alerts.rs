//! `/api/v1/matters/{id}/alerts` + `/api/v1/alerts/*` (Piece 79):
//! Workspace case/query alerts. A watch re-runs its question against
//! the vault and the web; citations it has never seen before become
//! hits. The first check only sets the baseline (see `crate::alerts`).
//!
//! Checks are deliberately NOT billed: a watch refresh is monitoring,
//! not an answer the user commissioned — the provider cost is the
//! operator's, like the health watch. Manual "check now" is the same
//! code path the background loop runs every 6 hours per watch.

use axum::Json;
use axum::extract::{Path, State};
use legal_rag_auth::AuthUser;
use legal_rag_rag::query::{AskOptions, CitatorContext, QueryMode};
use serde::{Deserialize, Serialize};

use crate::alerts::{AlertHit, AlertWatch};
use crate::error::ApiError;
use crate::state::AppState;

/// One watch with its hits and unread count, as the Workspace tab shows it.
#[derive(Serialize)]
pub struct WatchView {
    #[serde(flatten)]
    pub watch: AlertWatch,
    pub hits: Vec<AlertHit>,
    pub unread: usize,
}

#[derive(Serialize)]
pub struct AlertListView {
    pub watches: Vec<WatchView>,
}

fn view(state: &AppState, watch: AlertWatch) -> WatchView {
    let hits = state.alerts.hits_for(&watch.id);
    let unread = hits.iter().filter(|h| !h.read).count();
    WatchView {
        watch,
        hits,
        unread,
    }
}

pub async fn list_alerts(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(matter_id): Path<String>,
) -> Result<Json<AlertListView>, ApiError> {
    if state.matters.get(&matter_id).is_none() {
        return Err(legal_rag_common::Error::BadRequest("workspace not found".into()).into());
    }
    let watches = state
        .alerts
        .list_for(&user.email, Some(&matter_id))
        .into_iter()
        .map(|w| view(&state, w))
        .collect();
    Ok(Json(AlertListView { watches }))
}

#[derive(Deserialize)]
pub struct CreateAlertRequest {
    /// `case` | `query`
    pub kind: String,
    pub text: String,
}

pub async fn create_alert(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(matter_id): Path<String>,
    Json(body): Json<CreateAlertRequest>,
) -> Result<Json<WatchView>, ApiError> {
    if state.matters.get(&matter_id).is_none() {
        return Err(legal_rag_common::Error::BadRequest("workspace not found".into()).into());
    }
    if body.text.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("watch text is empty".into()).into());
    }
    if body.kind != "case" && body.kind != "query" {
        return Err(legal_rag_common::Error::BadRequest("kind must be case or query".into()).into());
    }
    let watch = state
        .alerts
        .create(&user.email, &matter_id, &body.kind, &body.text);
    Ok(Json(view(&state, watch)))
}

pub async fn delete_alert(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.alerts.delete(&user.email, &id) {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(legal_rag_common::Error::BadRequest("alert not found".into()).into())
    }
}

#[derive(Serialize)]
pub struct CheckView {
    /// True when this check only established the baseline (no hits
    /// are reported for material that already existed).
    pub baseline: bool,
    pub new_hits: usize,
    /// Legs that failed this run (`corpus` / `web`); empty = both ran.
    pub legs_failed: Vec<String>,
    pub watch: WatchView,
}

/// Run one watch check: corpus leg + web leg, diff citations against
/// the watch's seen set, store the new ones as hits. Shared by the
/// manual endpoint and the background loop in `main.rs`.
pub async fn run_watch_check(state: &AppState, watch: &AlertWatch) -> CheckView {
    let question = if watch.kind == "case" {
        format!(
            "Developments, orders, judgments or filings concerning the case: {}",
            watch.text
        )
    } else {
        watch.text.clone()
    };
    let opts = AskOptions {
        brief_vault: None,
        refine_terms: Vec::new(),
    };
    let mut legs_failed: Vec<String> = Vec::new();
    let mut citations: Vec<legal_rag_rag::Citation> = Vec::new();

    match state
        .query
        .query_opts(
            &question,
            QueryMode::Mix,
            5,
            None,
            false,
            &opts,
            CitatorContext::default(),
        )
        .await
    {
        Ok(ans) => citations.extend(ans.citations),
        Err(_) => legs_failed.push("corpus".to_string()),
    }
    match state
        .query
        .query_with_web(
            &question,
            QueryMode::Naive,
            3,
            2,
            None,
            false,
            CitatorContext::default(),
            &opts,
        )
        .await
    {
        Ok(ans) => citations.extend(ans.citations),
        Err(_) => legs_failed.push("web".to_string()),
    }

    let baseline = watch.last_checked_at.is_none();
    let mut observed: Vec<String> = Vec::new();
    let mut new_hits: Vec<AlertHit> = Vec::new();
    let now = chrono::Utc::now();
    for c in citations {
        if observed.contains(&c.chunk_id) {
            continue;
        }
        observed.push(c.chunk_id.clone());
        if baseline || watch.seen_keys.contains(&c.chunk_id) {
            continue;
        }
        let is_web = c.source_url.is_some();
        new_hits.push(AlertHit {
            id: legal_rag_common::new_id(),
            watch_id: watch.id.clone(),
            key: c.chunk_id.clone(),
            title: c
                .doc_title
                .clone()
                .filter(|t| !t.trim().is_empty())
                .or_else(|| c.source_url.clone())
                .unwrap_or_else(|| "Untitled source".to_string()),
            snippet: c.snippet.chars().take(400).collect(),
            url: c.source_url.clone(),
            source: if is_web { "web" } else { "corpus" }.to_string(),
            found_at: now,
            read: false,
        });
    }

    // Only stamp the check when at least one leg actually ran —
    // otherwise a provider outage would silently move the baseline.
    let new_count = if legs_failed.len() < 2 {
        state
            .alerts
            .record_check(&watch.id, observed, new_hits)
    } else {
        0
    };
    let fresh = state.alerts.get(&watch.id).unwrap_or_else(|| watch.clone());
    CheckView {
        baseline,
        new_hits: new_count,
        legs_failed,
        watch: view(state, fresh),
    }
}

pub async fn check_alert(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<CheckView>, ApiError> {
    let Some(watch) = state.alerts.get(&id) else {
        return Err(legal_rag_common::Error::BadRequest("alert not found".into()).into());
    };
    if watch.owner_email != user.email {
        return Err(legal_rag_common::Error::BadRequest("alert not found".into()).into());
    }
    Ok(Json(run_watch_check(&state, &watch).await))
}

pub async fn read_alert(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.alerts.mark_watch_read(&user.email, &id) {
        Ok(Json(serde_json::json!({ "read": true })))
    } else {
        // Nothing unread (or not the owner's watch) — still a success
        // state for the UI, but say so honestly.
        Ok(Json(serde_json::json!({ "read": false })))
    }
}
