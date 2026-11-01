//! `/api/v1/prompts` (Piece 63): the prompt library — the curated
//! Juris Vault collection plus the signed-in user's own prompts and
//! prompts shared with them. See `crate::prompts`.

use axum::Json;
use axum::extract::{Path, State};
use legal_rag_auth::AuthUser;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::prompts::{Prompt, PromptView};
use crate::state::AppState;

#[derive(Serialize)]
pub struct PromptListView {
    pub prompts: Vec<PromptView>,
    /// Category names in display order: Suggested first, then the
    /// curated categories, then any user-created categories.
    pub categories: Vec<String>,
}

pub async fn list_prompts(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Json<PromptListView> {
    let prompts = state.prompts.list_for(&user.email);
    let mut categories: Vec<String> = vec![
        "Suggested Prompts".to_string(),
        "Contracts".to_string(),
        "Drafting".to_string(),
        "Dispute Resolution".to_string(),
        "Corporate".to_string(),
        "Banking & Finance".to_string(),
        "Research".to_string(),
        "Inhouse Teams".to_string(),
    ];
    for p in &prompts {
        if !categories.contains(&p.prompt.category) {
            categories.push(p.prompt.category.clone());
        }
    }
    Json(PromptListView { prompts, categories })
}

#[derive(Deserialize)]
pub struct CreatePromptRequest {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub category: String,
}

pub async fn create_prompt(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<CreatePromptRequest>,
) -> Result<Json<Prompt>, ApiError> {
    if body.title.trim().is_empty() {
        return Err(
            legal_rag_common::Error::BadRequest("title is empty".into()).into(),
        );
    }
    if body.body.trim().is_empty() {
        return Err(
            legal_rag_common::Error::BadRequest("prompt text is empty".into()).into(),
        );
    }
    Ok(Json(state.prompts.create(
        &user.email,
        body.title.trim(),
        body.body.trim(),
        &body.category,
    )))
}

#[derive(Deserialize)]
pub struct UpdatePromptRequest {
    pub title: Option<String>,
    pub body: Option<String>,
    pub category: Option<String>,
}

pub async fn update_prompt(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdatePromptRequest>,
) -> Result<Json<Prompt>, ApiError> {
    state
        .prompts
        .update(
            &user.email,
            &id,
            body.title.as_deref().map(str::trim),
            body.body.as_deref().map(str::trim),
            body.category.as_deref().map(str::trim),
        )
        .map(Json)
        .ok_or_else(|| {
            legal_rag_common::Error::BadRequest(
                "prompt not found or not yours".into(),
            )
            .into()
        })
}

pub async fn delete_prompt(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.prompts.delete(&user.email, &id) {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(legal_rag_common::Error::BadRequest(
            "prompt not found or not yours".into(),
        )
        .into())
    }
}

#[derive(Serialize)]
pub struct ToggleView {
    pub active: bool,
}

pub async fn star_prompt(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Json<ToggleView> {
    Json(ToggleView {
        active: state.prompts.toggle_star(&user.email, &id),
    })
}

pub async fn share_prompt(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<ToggleView>, ApiError> {
    state
        .prompts
        .toggle_share(&user.email, &id)
        .map(|active| Json(ToggleView { active }))
        .ok_or_else(|| {
            legal_rag_common::Error::BadRequest(
                "prompt not found or not yours".into(),
            )
            .into()
        })
}
