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

#[derive(Deserialize)]
pub struct FromDocumentRequest {
    pub file_name: String,
    pub content_base64: String,
}

#[derive(Serialize)]
pub struct FromDocumentView {
    pub title: String,
    /// The template body: the document converted to format-only
    /// skeleton when `skeletonized`, else the raw extracted text
    /// (the UI warns the user to strip specifics before saving).
    pub body: String,
    pub skeletonized: bool,
}

/// POST /api/v1/templates/from-document (Piece 73): create-a-
/// template by uploading a document. The document is extracted
/// with the SAME parsers as vault ingest, then converted by the
/// LLM into a format-only skeleton — names, dates, amounts and
/// facts replaced with [bracketed placeholders] — because a
/// template must carry format and layout, never content. The
/// document itself is NOT ingested or stored.
pub async fn from_document(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<FromDocumentRequest>,
) -> Result<Json<FromDocumentView>, ApiError> {
    use base64::Engine as _;
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    if body.content_base64.len() > 14 * 1024 * 1024 {
        return Err(legal_rag_common::Error::BadRequest(
            "document too large to convert into a template (keep it under ~10 MB)".into(),
        )
        .into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body.content_base64.trim())
        .map_err(|e| {
            legal_rag_common::Error::BadRequest(format!("invalid base64 content: {e}"))
        })?;
    // Pipeline-grade extraction (PDF/DOCX/RTF/Office/text), on a
    // blocking thread like the ingest path.
    let config = state.rag.config().clone();
    let file_name = body.file_name.clone();
    let blocks = tokio::task::spawn_blocking(move || {
        let parser = legal_rag_rag::parser::get_parser("auto", &file_name, &config)?;
        parser.parse(&file_name, &bytes)
    })
    .await
    .map_err(|e| {
        legal_rag_common::Error::BadRequest(format!("parser task failed: {e}"))
    })??;
    let (text, _modal, _pages) = legal_rag_rag::content::separate_content(&blocks);
    if text.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest(
            "no extractable text in this document".into(),
        )
        .into());
    }
    // Bound the conversion call; the skeleton keeps the layout of
    // the opening and clauses, which live in the first pages.
    let truncated: String = text.chars().take(12_000).collect();
    let scope = legal_rag_rag::usage::new_scope();
    let converted = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        state.query.skeletonize_template(&truncated).await
    })
    .await;
    let totals = scope.lock().map(|t| *t).unwrap_or_default();
    state.billing.add_usage(&user.id, &totals);
    let (tpl_body, skeletonized) = match converted {
        Ok(s) => (s, true),
        Err(e) => {
            tracing::warn!(error = %e, "template skeletonization failed; returning raw text");
            (truncated, false)
        }
    };
    let title = body
        .file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem.to_string())
        .unwrap_or_else(|| body.file_name.clone());
    Ok(Json(FromDocumentView { title, body: tpl_body, skeletonized }))
}
