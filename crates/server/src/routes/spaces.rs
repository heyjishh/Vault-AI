//! `/api/v1/spaces/*` — multi-tenant Spaces: CRUD + membership.
//!
//! A Space is a container above Briefs with an owner and members
//! (owner / editor / viewer). Briefs filed in a Space are visible only
//! to its members; enforcement lives in the matters/knowledge/vault
//! routes — this module manages the spaces themselves.

use axum::extract::{Path, State};
use axum::Json;
use legal_rag_auth::AuthUser;
use serde::Deserialize;

use crate::error::ApiError;
use crate::spaces::{Space, ROLE_EDITOR, ROLE_OWNER, ROLE_VIEWER};
use crate::state::AppState;

fn not_found(what: &str) -> ApiError {
    legal_rag_common::Error::NotFound(what.to_string()).into()
}

fn forbidden(what: &str) -> ApiError {
    legal_rag_common::Error::Forbidden(what.to_string()).into()
}

fn is_admin(user: &legal_rag_auth::User) -> bool {
    user.role == "admin"
}

/// Fetch a space the caller may at least see (member or admin); denial
/// is a 404 so private spaces do not leak their existence.
fn visible_space(state: &AppState, user: &legal_rag_auth::User, id: &str) -> Result<Space, ApiError> {
    let space = state.spaces.get(id).ok_or_else(|| not_found("space not found"))?;
    if is_admin(user) || space.role_of(&user.id).is_some() {
        Ok(space)
    } else {
        Err(not_found("space not found"))
    }
}

/// Only the space owner (or a platform admin) manages members/settings.
fn ownable_space(state: &AppState, user: &legal_rag_auth::User, id: &str) -> Result<Space, ApiError> {
    let space = visible_space(state, user, id)?;
    if is_admin(user) || space.owner_id == user.id {
        Ok(space)
    } else {
        Err(forbidden("only the space owner can manage this space"))
    }
}

/// List the spaces the caller belongs to (admins see all).
pub async fn list_spaces(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Json<Vec<Space>> {
    if is_admin(&user) {
        Json(state.spaces.list_all())
    } else {
        Json(state.spaces.for_user(&user.id))
    }
}

#[derive(Deserialize)]
pub struct CreateSpaceRequest {
    pub name: String,
}

/// Create a space; the creator becomes its owner.
pub async fn create_space(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<CreateSpaceRequest>,
) -> Result<Json<Space>, ApiError> {
    if body.name.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("space name is empty".to_string()).into());
    }
    Ok(Json(state.spaces.create(body.name.trim(), &user.id, &user.email)))
}

/// One space with its members.
pub async fn get_space(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Space>, ApiError> {
    Ok(Json(visible_space(&state, &user, &id)?))
}

#[derive(Deserialize)]
pub struct UpdateSpaceRequest {
    pub name: String,
}

/// Rename a space (owner/admin).
pub async fn update_space(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateSpaceRequest>,
) -> Result<Json<Space>, ApiError> {
    ownable_space(&state, &user, &id)?;
    if body.name.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("space name is empty".to_string()).into());
    }
    state
        .spaces
        .rename(&id, body.name.trim())
        .map(Json)
        .ok_or_else(|| not_found("space not found"))
}

/// Delete a space (owner/admin). Its briefs are NOT deleted — they move
/// out of the space and become legacy (spaceless) briefs.
pub async fn delete_space(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ownable_space(&state, &user, &id)?;
    state.matters.clear_space(&id);
    state.spaces.delete(&id);
    Ok(Json(serde_json::json!({ "deleted": true })))
}

#[derive(Deserialize)]
pub struct AddMemberRequest {
    pub email: String,
    /// owner | editor | viewer (the space owner cannot be demoted).
    pub role: String,
}

/// Add a member by email, or change an existing member's role
/// (owner/admin only). The account must already exist.
pub async fn add_member(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Json(body): Json<AddMemberRequest>,
) -> Result<Json<Space>, ApiError> {
    ownable_space(&state, &user, &id)?;
    let role = body.role.trim().to_lowercase();
    if !crate::spaces::valid_role(&role) || role == ROLE_OWNER {
        return Err(legal_rag_common::Error::BadRequest(format!(
            "role must be {ROLE_EDITOR} or {ROLE_VIEWER}"
        ))
        .into());
    }
    let email = body.email.trim().to_lowercase();
    let target = state
        .auth
        .list_users()?
        .into_iter()
        .find(|u| u.email.to_lowercase() == email)
        .ok_or_else(|| not_found("no account with that email"))?;
    state
        .spaces
        .upsert_member(&id, &target.id, &target.email, &role)
        .map(Json)
        .ok_or_else(|| not_found("space not found"))
}

/// Remove a member (owner/admin only; the owner cannot be removed).
pub async fn remove_member(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, user_id)): Path<(String, String)>,
) -> Result<Json<Space>, ApiError> {
    let space = ownable_space(&state, &user, &id)?;
    if space.owner_id == user_id {
        return Err(legal_rag_common::Error::BadRequest(
            "the space owner cannot be removed".to_string(),
        )
        .into());
    }
    state
        .spaces
        .remove_member(&id, &user_id)
        .map(Json)
        .ok_or_else(|| not_found("space not found"))
}
