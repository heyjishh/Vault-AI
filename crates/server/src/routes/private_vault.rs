//! Confidential vault endpoints: per-user, password-encrypted documents.
//!
//! Nothing here ever returns another user's data: every operation is
//! scoped by the authenticated user's id inside the store itself. Titles
//! and chunk texts exist only as AES-256-GCM ciphertext at rest; they are
//! decrypted in memory for the owner's own requests while unlocked.

use axum::extract::{Path, State};
use axum::Json;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use legal_rag_auth::AuthUser;
use legal_rag_common::Error as DomainError;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::private_vault::sha256_hex;
use crate::routes::billing::settle;
use crate::state::AppState;

fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError(DomainError::BadRequest(msg.into()))
}
fn locked() -> ApiError {
    bad_request("your confidential vault is locked — unlock it first")
}

#[derive(Deserialize)]
pub struct PasswordBody {
    pub password: String,
}

#[derive(Deserialize)]
pub struct ChangePasswordBody {
    pub current_password: String,
    pub new_password: String,
}

fn check_password_strength(pw: &str) -> Result<(), ApiError> {
    if pw.chars().count() < 8 {
        return Err(bad_request("vault password must be at least 8 characters"));
    }
    Ok(())
}

pub async fn status(State(state): State<AppState>, AuthUser(user): AuthUser) -> Json<Value> {
    Json(json!({
        "has_password": state.private_vault.has_password(&user.id),
        "unlocked": state.private_vault.is_unlocked(&user.id),
        "documents": state.private_vault.doc_count(&user.id),
        // Where confidential traffic goes: "local" = the dedicated
        // LEGAL_RAG_PRIVATE_* endpoint, "provider" = the main provider.
        "routing": {
            "embeddings": if state.config.private_embedding.is_some() { "local" } else { "provider" },
            "llm": if state.config.private_llm.is_some() { "local" } else { "provider" },
        },
    }))
}

pub async fn set_password(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<PasswordBody>,
) -> Result<Json<Value>, ApiError> {
    check_password_strength(&body.password)?;
    state
        .private_vault
        .set_password(&user.id, &body.password)
        .map_err(|e| ApiError(DomainError::Conflict(e)))?;
    Ok(Json(json!({ "ok": true, "unlocked": true })))
}

pub async fn unlock(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<PasswordBody>,
) -> Result<Json<Value>, ApiError> {
    match state.private_vault.unlock(&user.id, &body.password) {
        Ok(true) => Ok(Json(json!({ "ok": true, "unlocked": true }))),
        Ok(false) => Err(ApiError(DomainError::Auth("wrong vault password".to_string()))),
        Err(e) => Err(bad_request(e)),
    }
}

pub async fn lock(State(state): State<AppState>, AuthUser(user): AuthUser) -> Json<Value> {
    state.private_vault.lock(&user.id);
    Json(json!({ "ok": true, "unlocked": false }))
}

pub async fn change_password(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<ChangePasswordBody>,
) -> Result<Json<Value>, ApiError> {
    check_password_strength(&body.new_password)?;
    match state
        .private_vault
        .change_password(&user.id, &body.current_password, &body.new_password)
    {
        Ok(true) => Ok(Json(json!({ "ok": true }))),
        Ok(false) => Err(ApiError(DomainError::Auth("wrong current password".to_string()))),
        Err(e) => Err(bad_request(e)),
    }
}

pub async fn list_documents(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !state.private_vault.is_unlocked(&user.id) {
        return Ok(Json(json!({ "locked": true, "documents": [] })));
    }
    let docs = state.private_vault.list_docs(&user.id).map_err(bad_request)?;
    Ok(Json(json!({ "locked": false, "documents": docs })))
}

#[derive(Deserialize)]
pub struct PrivateUpload {
    pub title: Option<String>,
    pub file_name: String,
    pub text: Option<String>,
    pub content_base64: Option<String>,
}

pub async fn upload_document(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<PrivateUpload>,
) -> Result<Json<Value>, ApiError> {
    if !state.private_vault.is_unlocked(&user.id) {
        return Err(locked());
    }
    if body.file_name.trim().is_empty() {
        return Err(bad_request("file_name is required"));
    }
    let bytes: Vec<u8> = if let Some(text) = &body.text {
        text.clone().into_bytes()
    } else if let Some(b64) = &body.content_base64 {
        B64.decode(b64).map_err(|_| bad_request("content_base64 is not valid base64"))?
    } else {
        return Err(bad_request("provide text or content_base64"));
    };
    if bytes.is_empty() {
        return Err(bad_request("empty document"));
    }
    let title = body
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| body.file_name.clone());

    // Parse + chunk + embed WITHOUT indexing: nothing enters the shared
    // corpus, the snapshots, or the knowledge graph.
    let usage_scope = legal_rag_rag::usage::new_scope();
    let chunks = legal_rag_rag::usage::scope_usage(
        usage_scope.clone(),
        state
            .rag
            .extract_chunks_with(&body.file_name, &bytes, state.private_embeddings.clone()),
    )
    .await?;
    let pairs: Vec<(String, Vec<f32>)> =
        chunks.into_iter().map(|c| (c.text, c.vector)).collect();
    let chunk_count = pairs.len();
    let meta = state
        .private_vault
        .add_doc(&user.id, &title, sha256_hex(&bytes), pairs)
        .map_err(bad_request)?;
    // Bill the embedding usage; log no filename, no excerpt — the activity
    // trail must not leak what the vault holds.
    settle(
        &state,
        &user,
        &usage_scope,
        "private_upload",
        "Confidential document",
        "",
        "",
        chunk_count,
    );
    Ok(Json(json!({
        "id": meta.id,
        "title": meta.title,
        "sha256": meta.sha256,
        "chunks": meta.chunks,
        "created_at": meta.created_at,
    })))
}

pub async fn delete_document(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if state.private_vault.delete_doc(&user.id, &id) {
        Ok(Json(json!({ "ok": true })))
    } else {
        Err(ApiError(DomainError::NotFound("private document not found".to_string())))
    }
}
