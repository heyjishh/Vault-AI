//! `/api/v1/matters/*` — matter workspaces: CRUD, document filing, the
//! living timeline (activity scoped to the matter) and the matter memory
//! (entities the matter's documents mention, key stats).

use axum::extract::{Path, State};
use axum::Json;
use legal_rag_auth::AuthUser;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::matters::Matter;
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

/// The caller's role in the matter's space, if any applies.
fn matter_role(state: &AppState, user: &legal_rag_auth::User, matter: &Matter) -> Option<String> {
    matter
        .space_id
        .as_ref()
        .and_then(|sid| state.spaces.get(sid))
        .and_then(|space| space.role_of(&user.id).map(|r| r.to_string()))
}

/// Read gate: spaceless briefs are open to every authenticated user;
/// space briefs require membership. Denial is a 404 — a private brief's
/// existence is itself confidential.
pub(crate) fn require_read(state: &AppState, user: &legal_rag_auth::User, matter: &Matter) -> Result<(), ApiError> {
    if matter.space_id.is_none() || is_admin(user) || matter_role(state, user, matter).is_some() {
        Ok(())
    } else {
        Err(not_found("brief not found"))
    }
}

/// Edit gate: space briefs require editor+; denial is a 404 when the
/// caller cannot even read, a 403 when they can read but not edit.
pub(crate) fn require_edit(state: &AppState, user: &legal_rag_auth::User, matter: &Matter) -> Result<(), ApiError> {
    require_read(state, user, matter)?;
    if matter.space_id.is_none() || is_admin(user) {
        return Ok(());
    }
    match matter_role(state, user, matter) {
        Some(role) if crate::spaces::role_can_edit(&role) => Ok(()),
        _ => Err(forbidden("your role in this space is read-only")),
    }
}

/// Editor gate for filing a NEW brief into a space.
fn require_space_edit(state: &AppState, user: &legal_rag_auth::User, space_id: &str) -> Result<(), ApiError> {
    let space = state.spaces.get(space_id).ok_or_else(|| not_found("space not found"))?;
    if is_admin(user) {
        return Ok(());
    }
    match space.role_of(&user.id) {
        Some(role) if crate::spaces::role_can_edit(role) => Ok(()),
        Some(_) => Err(forbidden("your role in this space is read-only")),
        None => Err(not_found("space not found")),
    }
}

/// Document visibility under Spaces: a document filed only inside
/// space briefs is visible solely to members of those spaces (and
/// admins). Unfiled documents, and documents also filed in a spaceless
/// brief, keep the legacy open visibility.
pub(crate) fn doc_visible(state: &AppState, user: &legal_rag_auth::User, doc_id: &str) -> bool {
    let containing = state.matters.matters_of_document(doc_id);
    if containing.is_empty() {
        return true;
    }
    containing.iter().any(|m| require_read(state, user, m).is_ok())
}

/// Resolve a matter id to its document ids for retrieval scoping,
/// enforcing the matter's space boundary for this caller.
pub fn scope_docs(
    state: &AppState,
    matter_id: &str,
    user: &legal_rag_auth::User,
) -> Result<Vec<String>, ApiError> {
    let matter = state
        .matters
        .get(matter_id)
        .ok_or_else(|| not_found("brief not found"))?;
    require_read(state, user, &matter)?;
    Ok(matter.document_ids)
}

#[derive(Serialize)]
pub struct MatterSummary {
    #[serde(flatten)]
    pub matter: Matter,
    pub document_count: usize,
    pub activity_count: usize,
}

fn summarize(state: &AppState, matter: Matter) -> MatterSummary {
    let activity_count = state.activity.for_matter(&matter.id, 10_000).len();
    MatterSummary {
        document_count: matter.document_ids.len(),
        activity_count,
        matter,
    }
}

/// List the matters this caller may see, most recently active first.
/// Space briefs the caller does not belong to are filtered out.
pub async fn list_matters(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Json<Vec<MatterSummary>> {
    let roles = state.spaces.roles_for(&user.id);
    Json(
        state
            .matters
            .list()
            .into_iter()
            .filter(|m| {
                crate::spaces::can_read(m.space_id.as_deref(), &roles, is_admin(&user))
            })
            .map(|m| summarize(&state, m))
            .collect(),
    )
}

#[derive(Deserialize)]
pub struct CreateMatterRequest {
    pub title: String,
    pub client: Option<String>,
    pub kind: Option<String>,
    pub notes: Option<String>,
    /// File the new brief into this Space (requires editor+ there).
    pub space_id: Option<String>,
}

/// Create a matter workspace.
pub async fn create_matter(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<CreateMatterRequest>,
) -> Result<Json<Matter>, ApiError> {
    if body.title.trim().is_empty() {
        return Err(
            legal_rag_common::Error::BadRequest("matter title is empty".to_string()).into(),
        );
    }
    let space_id = body
        .space_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    if let Some(sid) = &space_id {
        require_space_edit(&state, &user, sid)?;
    }
    let matter = state.matters.create(
        body.title.trim(),
        body.client.as_deref().unwrap_or("").trim(),
        body.kind.as_deref().unwrap_or("General").trim(),
        body.notes.as_deref().unwrap_or(""),
        &user.email,
        space_id,
    );
    Ok(Json(matter))
}

#[derive(Serialize)]
pub struct MatterDetail {
    #[serde(flatten)]
    pub matter: Matter,
    pub documents: Vec<legal_rag_rag::DocumentInfo>,
}

/// One matter with its filed documents.
pub async fn get_matter(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<MatterDetail>, ApiError> {
    let matter = state.matters.get(&id).ok_or_else(|| not_found("brief not found"))?;
    require_read(&state, &user, &matter)?;
    let documents = matter
        .document_ids
        .iter()
        .filter_map(|doc_id| state.rag.doc_info(doc_id))
        .collect();
    Ok(Json(MatterDetail { matter, documents }))
}

#[derive(Deserialize)]
pub struct UpdateMatterRequest {
    pub title: Option<String>,
    pub client: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub notes: Option<String>,
    /// Move the brief: a space id files it there (editor+ required),
    /// an empty string moves it out of any space, absent = unchanged.
    pub space_id: Option<String>,
}

/// Update matter fields (title, client, kind, status, notes, space).
pub async fn update_matter(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateMatterRequest>,
) -> Result<Json<Matter>, ApiError> {
    if let Some(status) = &body.status {
        if !["Active", "On hold", "Closed"].contains(&status.as_str()) {
            return Err(legal_rag_common::Error::BadRequest(format!(
                "unknown matter status: {status}"
            ))
            .into());
        }
    }
    let matter = state.matters.get(&id).ok_or_else(|| not_found("brief not found"))?;
    require_edit(&state, &user, &matter)?;
    let space: Option<Option<String>> = match &body.space_id {
        None => None,
        Some(s) if s.trim().is_empty() => Some(None),
        Some(s) => {
            let sid = s.trim().to_string();
            require_space_edit(&state, &user, &sid)?;
            Some(Some(sid))
        }
    };
    state
        .matters
        .update(&id, body.title, body.client, body.kind, body.status, body.notes, space)
        .map(Json)
        .ok_or_else(|| not_found("brief not found"))
}

/// Delete a matter workspace. Its documents stay in the vault: any
/// document that was filed ONLY here is re-filed into the default
/// brief (the brief-only vault has no unfiled state — a document
/// must always be reachable inside some brief). Its HISTORY goes
/// with it (Piece 27): every activity entry scoped to this brief —
/// Ask, Interact, research, draft, upload — is permanently removed,
/// so a deleted brief leaves nothing restorable in the Dashboard
/// history. Payment and token-usage records are NOT history: they
/// live in the billing/usage stores, which this never touches.
pub async fn delete_matter(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let matter = state.matters.get(&id).ok_or_else(|| not_found("brief not found"))?;
    require_edit(&state, &user, &matter)?;
    if matter.is_default {
        return Err(legal_rag_common::Error::BadRequest(
            "the default brief cannot be deleted".to_string(),
        )
        .into());
    }
    let filed = matter.document_ids.clone();
    state.matters.delete(&id);
    let adopted = state.matters.adopt_unfiled(&filed);
    let history_removed = state.activity.remove_for_matter(&id);
    tracing::info!(
        history_removed,
        documents_rehomed = adopted,
        brief = %id,
        "deleted brief: history removed, orphaned documents re-filed into the default brief"
    );
    Ok(Json(serde_json::json!({
        "deleted": true,
        "history_removed": history_removed,
        "documents_rehomed": adopted,
    })))
}

#[derive(Deserialize)]
pub struct AttachRequest {
    pub doc_id: String,
}

/// File an existing vault document under this matter.
pub async fn attach_document(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Json(body): Json<AttachRequest>,
) -> Result<Json<Matter>, ApiError> {
    let matter = state.matters.get(&id).ok_or_else(|| not_found("brief not found"))?;
    require_edit(&state, &user, &matter)?;
    if state.rag.doc_info(&body.doc_id).is_none() {
        return Err(not_found("document not found"));
    }
    state
        .matters
        .add_document(&id, &body.doc_id)
        .map(Json)
        .ok_or_else(|| not_found("brief not found"))
}

/// Remove a document from this matter (the document itself is
/// kept). If this was the document's only filing, it is re-filed
/// into the default brief — the brief-only vault has no unfiled
/// state, so detaching can never make a document unreachable.
/// (Detaching from the default brief itself is therefore a no-op
/// for the filing: the document stays in the default brief.)
pub async fn detach_document(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, doc_id)): Path<(String, String)>,
) -> Result<Json<Matter>, ApiError> {
    let matter = state.matters.get(&id).ok_or_else(|| not_found("brief not found"))?;
    require_edit(&state, &user, &matter)?;
    state
        .matters
        .remove_document(&id, &doc_id)
        .ok_or_else(|| not_found("brief not found"))?;
    state.matters.adopt_unfiled(std::slice::from_ref(&doc_id));
    // Return the post-state brief (re-filing may have restored the
    // document to this very brief, when it is the default).
    state
        .matters
        .get(&id)
        .map(Json)
        .ok_or_else(|| not_found("brief not found"))
}

#[derive(Serialize)]
pub struct TimelineItem {
    pub at: chrono::DateTime<chrono::Utc>,
    /// activity kind (query/research/…) or system kind (created/document).
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub excerpt: String,
    pub citations: usize,
    pub tokens: u64,
    pub user_email: String,
}

/// The matter's living history: everything done inside it, plus document
/// filings and its own creation, newest first.
pub async fn matter_timeline(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<TimelineItem>>, ApiError> {
    let matter = state.matters.get(&id).ok_or_else(|| not_found("brief not found"))?;
    require_read(&state, &user, &matter)?;
    let mut items: Vec<TimelineItem> = state
        .activity
        .for_matter(&id, 200)
        .into_iter()
        .map(|e| TimelineItem {
            at: e.created_at,
            kind: e.kind,
            title: e.title,
            detail: e.detail,
            excerpt: e.result_excerpt,
            citations: e.citations,
            tokens: e.tokens,
            user_email: e.user_email,
        })
        .collect();
    for doc_id in &matter.document_ids {
        if let Some(doc) = state.rag.doc_info(doc_id) {
            items.push(TimelineItem {
                at: doc.ingested_at,
                kind: "document".to_string(),
                title: doc.title.clone(),
                detail: "filed in this brief".to_string(),
                excerpt: format!("{} chunks · {} chars", doc.chunks, doc.chars),
                citations: 0,
                tokens: 0,
                user_email: String::new(),
            });
        }
    }
    items.push(TimelineItem {
        at: matter.created_at,
        kind: "created".to_string(),
        title: format!("Brief opened: {}", matter.title),
        detail: matter.kind.clone(),
        excerpt: String::new(),
        citations: 0,
        tokens: 0,
        user_email: matter.created_by.clone(),
    });
    items.sort_by(|a, b| b.at.cmp(&a.at));
    items.truncate(200);
    Ok(Json(items))
}

#[derive(Serialize)]
pub struct MatterEntity {
    pub name: String,
    pub kind: String,
    pub mentions: usize,
}

#[derive(Serialize)]
pub struct MatterMemory {
    pub document_count: usize,
    pub chunk_count: usize,
    pub entities: Vec<MatterEntity>,
    pub activity_count: usize,
    pub last_activity_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The matter's living memory: what its documents are about (entities by
/// mention count), corpus weight, and how much work has happened in it.
pub async fn matter_memory(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<MatterMemory>, ApiError> {
    let matter = state.matters.get(&id).ok_or_else(|| not_found("brief not found"))?;
    require_read(&state, &user, &matter)?;
    let mut chunk_ids: std::collections::HashSet<String> = Default::default();
    let mut chunk_count = 0usize;
    for doc_id in &matter.document_ids {
        if let Some(doc) = state.rag.doc_info(doc_id) {
            chunk_count += doc.chunks;
            chunk_ids.extend(doc.chunk_ids.iter().cloned());
        }
    }
    let mut entities: Vec<MatterEntity> = Vec::new();
    if !chunk_ids.is_empty() {
        for entity in state.graph.all_entities().await? {
            let mentions = entity
                .chunk_ids
                .iter()
                .filter(|c| chunk_ids.contains(*c))
                .count();
            if mentions > 0 {
                entities.push(MatterEntity {
                    name: entity.name,
                    kind: entity.kind,
                    mentions,
                });
            }
        }
        entities.sort_by(|a, b| b.mentions.cmp(&a.mentions));
        entities.truncate(30);
    }
    let activity = state.activity.for_matter(&id, 10_000);
    Ok(Json(MatterMemory {
        document_count: matter.document_ids.len(),
        chunk_count,
        entities,
        activity_count: activity.len(),
        last_activity_at: activity.first().map(|e| e.created_at),
    }))
}
