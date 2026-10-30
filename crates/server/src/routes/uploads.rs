//! Chunked upload sessions (Piece 25).
//!
//! A single-shot JSON upload cannot carry a 100 MB file to the public
//! site: the request would be ~133 MB of base64 JSON, the tunnel's
//! WebSocket framing drops messages above ~32 MB, and the edge caps
//! any single request body at 100 MB (all measured live, Piece 25).
//! So large files travel as a sequence of small part requests — each
//! a few MB, comfortably inside every ceiling — and are assembled
//! server-side, then handed to the EXACT same completions as a
//! single-shot upload (`vault::run_upload` / `private_vault::seal_upload`).
//! Small files keep using the single-shot routes unchanged.
//!
//! Sessions live in memory with their parts as temp files; they are
//! owner-scoped, expire after two hours (pruned opportunistically on
//! init), and an unfinished session never touches the corpus.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::Json;
use base64::Engine as _;
use legal_rag_auth::AuthUser;
use legal_rag_common::Error as DomainError;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;

/// The per-file size cap the whole product promises: 100 MB.
pub const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;
/// Largest single chunk part the server accepts (decoded bytes). The
/// client sends 4 MiB parts; 8 MiB leaves retry/headroom while staying
/// far below the tunnel's per-message ceiling.
pub const MAX_PART_BYTES: usize = 8 * 1024 * 1024;
const SESSION_TTL: Duration = Duration::from_secs(2 * 3600);

fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError(DomainError::BadRequest(msg.into()))
}
fn not_found() -> ApiError {
    ApiError(DomainError::NotFound("upload session not found".to_string()))
}

/// Reject a declared/received size above the 100 MB per-file cap, with
/// the message the client also shows before sending.
pub fn check_file_size(file_name: &str, len: u64) -> Result<(), ApiError> {
    if len > MAX_FILE_BYTES {
        return Err(bad_request(format!(
            "{file_name} is {:.1} MB — the limit is 100 MB per file",
            len as f64 / 1_048_576.0
        )));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Vault,
    Private,
}

struct Session {
    user_id: String,
    target: Target,
    file_name: String,
    title: Option<String>,
    total_size: u64,
    matter_id: Option<String>,
    parts: HashMap<u32, u64>,
    dir: PathBuf,
    created: Instant,
}

impl Session {
    fn received(&self) -> u64 {
        self.parts.values().sum()
    }
}

fn store() -> &'static Mutex<HashMap<String, Session>> {
    static STORE: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sessions_root() -> PathBuf {
    std::env::temp_dir().join("legal-rag-chunked-uploads")
}

fn create_session(
    target: Target,
    user_id: &str,
    file_name: &str,
    title: Option<String>,
    total_size: u64,
    matter_id: Option<String>,
) -> Result<String, ApiError> {
    check_file_size(file_name, total_size)?;
    if total_size == 0 {
        return Err(bad_request("empty upload"));
    }
    if file_name.trim().is_empty() {
        return Err(bad_request("file_name is required"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let dir = sessions_root().join(&id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| bad_request(format!("could not stage upload: {e}")))?;
    let mut map = store().lock().unwrap();
    // Opportunistic expiry: stale sessions (and their staged parts)
    // are dropped whenever a new session starts.
    let stale: Vec<String> = map
        .iter()
        .filter(|(_, s)| s.created.elapsed() > SESSION_TTL)
        .map(|(k, _)| k.clone())
        .collect();
    for k in stale {
        if let Some(s) = map.remove(&k) {
            let _ = std::fs::remove_dir_all(&s.dir);
        }
    }
    map.insert(
        id.clone(),
        Session {
            user_id: user_id.to_string(),
            target,
            file_name: file_name.to_string(),
            title,
            total_size,
            matter_id,
            parts: HashMap::new(),
            dir,
            created: Instant::now(),
        },
    );
    Ok(id)
}

fn write_part(
    id: &str,
    user_id: &str,
    target: Target,
    index: u32,
    bytes: &[u8],
) -> Result<u64, ApiError> {
    if bytes.len() > MAX_PART_BYTES {
        return Err(bad_request(format!(
            "upload part is {:.1} MB — parts may be at most {} MB",
            bytes.len() as f64 / 1_048_576.0,
            MAX_PART_BYTES / 1_048_576
        )));
    }
    let mut map = store().lock().unwrap();
    let session = map.get_mut(id).ok_or_else(not_found)?;
    if session.user_id != user_id || session.target != target {
        return Err(not_found());
    }
    let path = session.dir.join(format!("{index}.part"));
    std::fs::write(&path, bytes)
        .map_err(|e| bad_request(format!("could not stage upload part: {e}")))?;
    session.parts.insert(index, bytes.len() as u64);
    if session.received() > session.total_size {
        return Err(bad_request("uploaded parts exceed the declared file size"));
    }
    Ok(session.received())
}

/// Remove a session and assemble its parts. Parts must be contiguous
/// from 0 and sum exactly to the declared size.
fn assemble(id: &str, user_id: &str, target: Target) -> Result<(Session, Vec<u8>), ApiError> {
    let session = {
        let mut map = store().lock().unwrap();
        let s = map.get(id).ok_or_else(not_found)?;
        if s.user_id != user_id || s.target != target {
            return Err(not_found());
        }
        map.remove(id).unwrap()
    };
    let finish = |session: &Session| {
        let _ = std::fs::remove_dir_all(&session.dir);
    };
    let mut indices: Vec<u32> = session.parts.keys().copied().collect();
    indices.sort_unstable();
    if indices.iter().copied().collect::<Vec<_>>()
        != (0..indices.len() as u32).collect::<Vec<_>>()
    {
        finish(&session);
        return Err(bad_request("upload is missing parts"));
    }
    if session.received() != session.total_size {
        let got = session.received();
        let want = session.total_size;
        finish(&session);
        return Err(bad_request(format!(
            "upload incomplete: received {got} of {want} bytes"
        )));
    }
    let mut bytes = Vec::with_capacity(session.total_size as usize);
    for idx in indices {
        let part = std::fs::read(session.dir.join(format!("{idx}.part"))).map_err(|e| {
            finish(&session);
            bad_request(format!("could not read staged upload part: {e}"))
        })?;
        bytes.extend_from_slice(&part);
    }
    finish(&session);
    Ok((session, bytes))
}

fn abort(id: &str, user_id: &str, target: Target) -> Result<(), ApiError> {
    let mut map = store().lock().unwrap();
    let s = map.get(id).ok_or_else(not_found)?;
    if s.user_id != user_id || s.target != target {
        return Err(not_found());
    }
    let s = map.remove(id).unwrap();
    let _ = std::fs::remove_dir_all(&s.dir);
    Ok(())
}

#[derive(Deserialize)]
pub struct InitRequest {
    pub file_name: String,
    pub title: Option<String>,
    pub total_size: u64,
    pub matter_id: Option<String>,
}

#[derive(Deserialize)]
pub struct PartRequest {
    pub content_base64: String,
}

fn decode_part(body: &PartRequest) -> Result<Vec<u8>, ApiError> {
    base64::engine::general_purpose::STANDARD
        .decode(body.content_base64.trim())
        .map_err(|e| bad_request(format!("invalid base64 content: {e}")))
}

// ---------- Vault (shared corpus) chunked uploads ----------

pub async fn vault_init(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<InitRequest>,
) -> Result<Json<Value>, ApiError> {
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let id = create_session(
        Target::Vault,
        &user.id,
        &body.file_name,
        body.title,
        body.total_size,
        body.matter_id,
    )?;
    Ok(Json(json!({ "upload_id": id, "max_part_bytes": MAX_PART_BYTES })))
}

pub async fn vault_part(
    AuthUser(user): AuthUser,
    Path((id, index)): Path<(String, u32)>,
    Json(body): Json<PartRequest>,
) -> Result<Json<Value>, ApiError> {
    let bytes = decode_part(&body)?;
    let received = write_part(&id, &user.id, Target::Vault, index, &bytes)?;
    Ok(Json(json!({ "received": received })))
}

pub async fn vault_complete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<crate::routes::vault::IngestReport>, ApiError> {
    let (session, bytes) = assemble(&id, &user.id, Target::Vault)?;
    let title = session
        .title
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| session.file_name.clone());
    let req = crate::routes::vault::UploadRequest {
        title,
        file_name: session.file_name,
        text: None,
        content_base64: None,
        matter_id: session.matter_id,
        file_password: None,
        jurisdiction: None,
        effective_from: None,
        effective_to: None,
        superseded_by: None,
        forum: None,
        state: None,
        decided_on: None,
        applicant: None,
    };
    crate::routes::vault::run_upload(&state, &user, req, bytes, false)
        .await
        .map(Json)
}

pub async fn vault_abort(
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    abort(&id, &user.id, Target::Vault)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------- Confidential vault chunked uploads ----------

fn pv_locked(state: &AppState, user: &legal_rag_auth::User) -> Result<(), ApiError> {
    if !state.private_vault.is_unlocked(&user.id) {
        return Err(bad_request(
            "your confidential vault is locked — unlock it first",
        ));
    }
    Ok(())
}

pub async fn pv_init(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<InitRequest>,
) -> Result<Json<Value>, ApiError> {
    pv_locked(&state, &user)?;
    let id = create_session(
        Target::Private,
        &user.id,
        &body.file_name,
        body.title,
        body.total_size,
        None,
    )?;
    Ok(Json(json!({ "upload_id": id, "max_part_bytes": MAX_PART_BYTES })))
}

pub async fn pv_part(
    AuthUser(user): AuthUser,
    Path((id, index)): Path<(String, u32)>,
    Json(body): Json<PartRequest>,
) -> Result<Json<Value>, ApiError> {
    let bytes = decode_part(&body)?;
    let received = write_part(&id, &user.id, Target::Private, index, &bytes)?;
    Ok(Json(json!({ "received": received })))
}

pub async fn pv_complete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    pv_locked(&state, &user)?;
    let (session, bytes) = assemble(&id, &user.id, Target::Private)?;
    let req = crate::routes::private_vault::PrivateUpload {
        title: session.title,
        file_name: session.file_name,
        text: None,
        content_base64: None,
        file_password: None,
    };
    crate::routes::private_vault::seal_upload(&state, &user, req, bytes)
        .await
        .map(Json)
}

pub async fn pv_abort(
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    abort(&id, &user.id, Target::Private)?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ApiError has no Debug, so unwrap via these helpers.
    fn ok<T>(r: Result<T, ApiError>) -> T {
        match r {
            Ok(v) => v,
            Err(e) => panic!("unexpected error: {}", e.0),
        }
    }
    fn err<T>(r: Result<T, ApiError>) -> String {
        match r {
            Ok(_) => panic!("expected an error"),
            Err(e) => e.0.to_string(),
        }
    }

    #[test]
    fn size_cap_message_names_the_limit() {
        let msg = err(check_file_size("big.pdf", MAX_FILE_BYTES + 1));
        assert!(msg.contains("100 MB"), "{msg}");
        assert!(check_file_size("ok.pdf", MAX_FILE_BYTES).is_ok());
    }

    #[test]
    fn session_round_trip_assembles_in_order() {
        let id = ok(create_session(Target::Vault, "u1", "a.txt", None, 11, None));
        ok(write_part(&id, "u1", Target::Vault, 1, b"world"));
        ok(write_part(&id, "u1", Target::Vault, 0, b"hello "));
        // Another user (or the wrong surface) cannot touch the session.
        assert!(write_part(&id, "u2", Target::Vault, 2, b"!").is_err());
        assert!(assemble(&id, "u1", Target::Private).is_err());
        let (session, bytes) = ok(assemble(&id, "u1", Target::Vault));
        assert_eq!(session.file_name, "a.txt");
        assert_eq!(bytes, b"hello world");
    }

    #[test]
    fn assemble_rejects_gaps_and_short_uploads() {
        let id = ok(create_session(Target::Private, "u1", "b.txt", None, 10, None));
        ok(write_part(&id, "u1", Target::Private, 0, b"12345"));
        ok(write_part(&id, "u1", Target::Private, 2, b"12345"));
        assert!(assemble(&id, "u1", Target::Private).is_err());
        let id = ok(create_session(Target::Private, "u1", "b.txt", None, 10, None));
        ok(write_part(&id, "u1", Target::Private, 0, b"12345"));
        let msg = err(assemble(&id, "u1", Target::Private));
        assert!(msg.contains("incomplete"), "{msg}");
    }

    #[test]
    fn oversize_sessions_are_refused_at_init() {
        assert!(create_session(Target::Vault, "u1", "big.pdf", None, MAX_FILE_BYTES + 1, None).is_err());
        assert!(create_session(Target::Vault, "u1", "empty.pdf", None, 0, None).is_err());
    }
}
