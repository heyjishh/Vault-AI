//! `/api/v1/admin/annotations/*` — verified annotations: human-curated
//! answers served verbatim to matching future questions (the verified
//! tier above the semantic answer cache).

use axum::extract::{Path, State};
use axum::Json;
use chrono::Utc;
use legal_rag_auth::AdminUser;
use legal_rag_rag::Citation;
use serde::{Deserialize, Serialize};

use crate::annotations::{corpus_fingerprint, AnnotationRecord};
use crate::error::ApiError;
use crate::state::AppState;

async fn current_fp(state: &AppState) -> String {
    let ids = state.store.chunk_ids().await.unwrap_or_default();
    corpus_fingerprint(&ids)
}

/// Push the store into the query service after any mutation (and at boot).
pub fn sync(state: &AppState) {
    state.query.set_annotations(state.annotations.rag_set());
}

#[derive(Serialize)]
pub struct ListView {
    pub annotations: Vec<AnnotationRecord>,
    pub current_corpus_fp: String,
}

pub async fn list_annotations(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Json<ListView> {
    Json(ListView {
        annotations: state.annotations.list(),
        current_corpus_fp: current_fp(&state).await,
    })
}

async fn build_record(
    state: &AppState,
    question: String,
    answer: String,
    citations: Vec<Citation>,
    scope_doc_ids: Option<Vec<String>>,
    source: &str,
    created_by: &str,
) -> Result<AnnotationRecord, ApiError> {
    let qvec = state
        .embeddings
        .embed(&[question.clone()])
        .await
        .map(|mut v| v.remove(0))
        .map_err(ApiError::from)?;
    Ok(AnnotationRecord {
        id: legal_rag_common::new_id(),
        question,
        answer,
        citations,
        qvec,
        scope_doc_ids,
        corpus_fp: current_fp(state).await,
        source: source.to_string(),
        created_by: created_by.to_string(),
        created_at: Utc::now(),
        enabled: true,
    })
}

#[derive(Deserialize)]
pub struct CreateRequest {
    pub question: String,
    pub answer: String,
    pub citations: Option<Vec<Citation>>,
    pub scope_doc_ids: Option<Vec<String>>,
}

pub async fn create_annotation(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(body): Json<CreateRequest>,
) -> Result<Json<AnnotationRecord>, ApiError> {
    if body.question.trim().is_empty() || body.answer.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("question and answer are required".into()).into());
    }
    let record = build_record(
        &state,
        body.question.trim().to_string(),
        body.answer.trim().to_string(),
        body.citations.unwrap_or_default(),
        body.scope_doc_ids,
        "manual",
        &admin.email,
    )
    .await?;
    let record = state.annotations.add(record);
    sync(&state);
    Ok(Json(record))
}

#[derive(Deserialize)]
pub struct FromActivityRequest {
    pub activity_id: String,
    /// The curator's corrected answer; defaults to the captured answer.
    pub answer: Option<String>,
}

/// Promote a captured query into a verified annotation — the curator
/// corrects the answer, and that corrected answer is what gets served.
pub async fn from_activity(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(body): Json<FromActivityRequest>,
) -> Result<Json<AnnotationRecord>, ApiError> {
    let event = state
        .activity
        .by_id(&body.activity_id)
        .ok_or_else(|| legal_rag_common::Error::NotFound("activity event not found".into()))?;
    if event.kind != "query" {
        return Err(legal_rag_common::Error::BadRequest("only query events can be annotated".into()).into());
    }
    let payload = event.payload.clone().unwrap_or(serde_json::Value::Null);
    let question = payload["question"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| event.title.clone());
    let answer = body
        .answer
        .filter(|a| !a.trim().is_empty())
        .or_else(|| payload["answer"].as_str().map(|s| s.to_string()))
        .ok_or_else(|| legal_rag_common::Error::BadRequest("captured event has no answer to annotate".into()))?;
    let citations: Vec<Citation> = payload
        .get("citations")
        .cloned()
        .and_then(|c| serde_json::from_value(c).ok())
        .unwrap_or_default();
    // Preserve the scope the question originally ran under: a brief's
    // annotation serves only that brief's document set.
    let scope_doc_ids = event.matter_id.as_ref().and_then(|mid| {
        state.matters.get(mid).map(|m| m.document_ids.clone())
    });
    let record = build_record(&state, question, answer, citations, scope_doc_ids, "captured", &admin.email).await?;
    let record = state.annotations.add(record);
    sync(&state);
    Ok(Json(record))
}

pub async fn delete_annotation(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.annotations.delete(&id) {
        sync(&state);
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(legal_rag_common::Error::NotFound("annotation not found".into()).into())
    }
}

#[derive(Deserialize)]
pub struct ToggleRequest {
    pub enabled: bool,
}

pub async fn toggle_annotation(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
    Json(body): Json<ToggleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.annotations.set_enabled(&id, body.enabled) {
        sync(&state);
        Ok(Json(serde_json::json!({ "enabled": body.enabled })))
    } else {
        Err(legal_rag_common::Error::NotFound("annotation not found".into()).into())
    }
}
