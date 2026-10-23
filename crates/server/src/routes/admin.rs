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
