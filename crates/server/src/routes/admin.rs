//! `/api/v1/admin/*` — Command Center administration. Admin role required.

use axum::extract::{Path, State};
use axum::Json;
use legal_rag_auth::{AdminUser, User};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Serialize)]
pub struct AdminUserView {
    pub id: String,
    pub email: String,
    pub role: String,
    pub created_at: String,
}

impl From<User> for AdminUserView {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            email: u.email,
            role: u.role,
            created_at: u.created_at.to_rfc3339(),
        }
    }
}

/// List all users in the workspace.
pub async fn list_users(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Result<Json<Vec<AdminUserView>>, ApiError> {
    let users = state.auth.list_users()?;
    Ok(Json(users.into_iter().map(AdminUserView::from).collect()))
}

#[derive(Deserialize)]
pub struct RoleRequest {
    pub role: String,
}

/// Change a user's role (`user` | `admin`).
pub async fn set_role(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
    Json(body): Json<RoleRequest>,
) -> Result<Json<AdminUserView>, ApiError> {
    let user = state
        .auth
        .set_user_role(&id, &body.role)?
        .ok_or_else(|| legal_rag_common::Error::NotFound("user not found".into()))?;
    Ok(Json(user.into()))
}

/// Backfill citator treatment edges for the existing corpus: a pure
/// text scan over stored parent chunks — no re-embedding, no LLM
/// calls. Idempotent (re-running adds nothing already present).
/// Returns new edges added and the store total.
pub async fn backfill_citator(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let added = state.rag.backfill_treatments().await?;
    Ok(Json(serde_json::json!({
        "added": added,
        "total": state.rag.treatment_count(),
    })))
}

/// Backfill citator V2 document metadata for the existing corpus: a
/// pure deterministic scan over stored parent chunks — no re-embedding,
/// no LLM calls. Only fills fields still empty (explicit overrides are
/// never overwritten), so re-running is a no-op. Returns documents
/// changed.
pub async fn backfill_citator_v2(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let meta_updated = state.rag.backfill_doc_meta().await?;
    let v2_edges_added = state.rag.backfill_v2_edges().await?;
    Ok(Json(serde_json::json!({
        "meta_updated": meta_updated,
        "v2_edges_added": v2_edges_added,
        "v2_total": state.rag.v2_treatment_count(),
    })))
}

/// Delete a user. Admins cannot delete themselves (prevents lockout).
pub async fn delete_user(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if id == admin.id {
        return Err(legal_rag_common::Error::BadRequest(
            "you cannot delete your own admin account".into(),
        )
        .into());
    }
    let deleted = state.auth.delete_user(&id)?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}
