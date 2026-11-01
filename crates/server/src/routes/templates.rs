//! `/api/v1/templates` (Piece 63): the template library — the curated
//! Juris Vault collection plus the signed-in user's own templates and
//! templates shared with them. See `crate::templates`.

use axum::Json;
use axum::extract::{Path, State};
use legal_rag_auth::AuthUser;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::templates::{Template, TemplateView};
use crate::state::AppState;

#[derive(Serialize)]
pub struct TemplateListView {
    pub templates: Vec<TemplateView>,
    /// Category names in display order: Suggested first, then the
    /// curated categories, then any user-created categories.
    pub categories: Vec<String>,
}

pub async fn list_templates(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Json<TemplateListView> {
    let templates = state.templates.list_for(&user.email);
    let mut categories: Vec<String> = vec![
        "Suggested Templates".to_string(),
        "Notices".to_string(),
        "Agreements".to_string(),
        "Pleadings".to_string(),
        "Affidavits".to_string(),
        "Corporate".to_string(),
        "Employment".to_string(),
    ];
    for p in &templates {
        if !categories.contains(&p.template.category) {
            categories.push(p.template.category.clone());
        }
    }
    Json(TemplateListView { templates, categories })
}

#[derive(Deserialize)]
pub struct CreateTemplateRequest {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub category: String,
}

pub async fn create_template(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<CreateTemplateRequest>,
) -> Result<Json<Template>, ApiError> {
    if body.title.trim().is_empty() {
        return Err(
            legal_rag_common::Error::BadRequest("title is empty".into()).into(),
        );
    }
    if body.body.trim().is_empty() {
        return Err(
            legal_rag_common::Error::BadRequest("template text is empty".into()).into(),
        );
    }
    Ok(Json(state.templates.create(
        &user.email,
        body.title.trim(),
        body.body.trim(),
        &body.category,
    )))
}

#[derive(Deserialize)]
pub struct UpdateTemplateRequest {
    pub title: Option<String>,
    pub body: Option<String>,
    pub category: Option<String>,
}

pub async fn update_template(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateTemplateRequest>,
) -> Result<Json<Template>, ApiError> {
    state
        .templates
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
                "template not found or not yours".into(),
            )
            .into()
        })
}

pub async fn delete_template(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.templates.delete(&user.email, &id) {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(legal_rag_common::Error::BadRequest(
            "template not found or not yours".into(),
        )
        .into())
    }
}

#[derive(Serialize)]
pub struct ToggleView {
    pub active: bool,
}

pub async fn star_template(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Json<ToggleView> {
    Json(ToggleView {
        active: state.templates.toggle_star(&user.email, &id),
    })
}

pub async fn share_template(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<ToggleView>, ApiError> {
    state
        .templates
        .toggle_share(&user.email, &id)
        .map(|active| Json(ToggleView { active }))
        .ok_or_else(|| {
            legal_rag_common::Error::BadRequest(
                "template not found or not yours".into(),
            )
            .into()
        })
}
