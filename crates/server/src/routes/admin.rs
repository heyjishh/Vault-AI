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

fn find_user(state: &AppState, id: &str) -> Result<User, ApiError> {
    state
        .auth
        .list_users()?
        .into_iter()
        .find(|u| u.id == id)
        .ok_or_else(|| {
            legal_rag_common::Error::NotFound("user not found".into()).into()
        })
}

/// Counts of what a user-data purge WOULD remove for this user —
/// shown in the Command Center before the admin confirms. Only the
/// two in-scope classes are counted: documents the user uploaded
/// (recorded owner) and their history entries.
pub async fn user_data_summary(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = find_user(&state, &id)?;
    Ok(Json(serde_json::json!({
        "user_id": user.id,
        "email": user.email,
        "documents": state.rag.documents_of_user(&user.id).len(),
        "history_entries": state.activity.count_for_user(&user.id),
    })))
}

/// PERMANENTLY delete a user's uploaded documents and history —
/// nothing else. The account, feedback, Private Vault, payment and
/// usage records, system data, briefs (as records) and every other
/// user's data are preserved by construction: no code path here
/// touches those stores. Documents are removed through the full
/// retrieval stack (document record, chunks, vectors, graph,
/// treatments) and unfiled from every brief holding them; history is
/// the user's activity entries (system purge records stay). The
/// purge itself is recorded as a system activity event, and the
/// operation is idempotent — a second run removes nothing.
pub async fn delete_user_data(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = find_user(&state, &id)?;
    let docs = state.rag.documents_of_user(&user.id);
    let mut chunks_removed = 0usize;
    let mut filings_removed = 0usize;
    for doc in &docs {
        chunks_removed += state.rag.remove_document(&doc.id).await?;
        filings_removed += state.matters.unfile_document_everywhere(&doc.id);
    }
    let history_removed = state.activity.remove_for_user(&user.id);
    let receipt = serde_json::json!({
        "user_id": user.id,
        "email": user.email,
        "documents_removed": docs.len(),
        "chunks_removed": chunks_removed,
        "brief_filings_removed": filings_removed,
        "history_entries_removed": history_removed,
    });
    state.activity.record(crate::activity::ActivityEvent::new(
        &admin.id,
        &admin.email,
        crate::activity::ActivityStore::PURGE_KIND,
        &format!("Purged uploaded data of {}", user.email),
        "admin user-data purge",
        &format!(
            "{} documents · {} chunks · {} brief filings · {} history entries removed",
            docs.len(),
            chunks_removed,
            filings_removed,
            history_removed
        ),
        0,
        0,
    ));
    Ok(Json(receipt))
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
