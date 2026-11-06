//! Document edit flow (Piece 85): propose → approve / reject →
//! write-back as a new version.
//!
//! - **Propose**: an LLM edit pass (`legal_rag_rag::apply_edit`) over
//!   the document's text — the retained original's own text for
//!   text-native files, the pipeline's extracted text otherwise —
//!   plus a paragraph diff. NOTHING is written back at this stage;
//!   the proposal waits in the edit store, the Piece 80 approval
//!   pattern applied to documents.
//! - **Approve (shared)**: the edited text is re-ingested through the
//!   very same pipeline entry the upload route uses
//!   (`process_document_complete`), retained as the new version's
//!   original, filed into the source's Workspaces IN PLACE of the
//!   previous version (which stays in the vault, downloadable, in the
//!   version list), with the source's legal-validity metadata carried
//!   over. Format: txt/md are stored natively, RTF is regenerated
//!   from the edited text, and formats with no server-side writer
//!   (PDF, DOCX) fall back to a stored text version — the receipt
//!   says which happened, never silently.
//! - **Approve (private)**: the edited text is chunked/embedded with
//!   the confidential path (`extract_chunks_with` + the private
//!   embeddings provider) and sealed as a NEW encrypted document via
//!   `PrivateVault::add_doc`. Plaintext never enters the shared
//!   corpus, the snapshots, or the originals directory. A locked
//!   vault refuses at propose time and again at write-back (add_doc
//!   itself requires the session key).
//! - **Reject**: the proposal is discarded (held text dropped) and
//!   the outcome recorded.
//!
//! Every step lands in the run history (kind `document_edit`) and on
//! the edit record itself: instruction, editor run id, approver,
//! timestamps, diff summary, and the write-back receipt.

use axum::extract::{Path, Query, State};
use axum::Json;
use legal_rag_auth::AuthUser;
use serde::{Deserialize, Serialize};

use crate::edits::EditRecord;
use crate::error::ApiError;
use crate::state::AppState;

/// An edit pass sends the whole document to the model; beyond this
/// the honest answer is "too large for one pass", not a silent
/// truncation the approver cannot see.
const EDIT_TEXT_LIMIT: usize = 120_000;

fn bad_request(msg: String) -> ApiError {
    legal_rag_common::Error::BadRequest(msg).into()
}

fn not_found(msg: &str) -> ApiError {
    legal_rag_common::Error::NotFound(msg.to_string()).into()
}

#[derive(Deserialize)]
pub struct ProposeEditRequest {
    pub doc_id: String,
    pub instruction: String,
    /// True when `doc_id` names a Confidential Vault document.
    #[serde(default)]
    pub private: bool,
}

/// The editable text of a shared document: the retained original's
/// own bytes when the file is text-native, else the pipeline's
/// extracted text (the only faithful text a PDF/DOCX has here).
async fn shared_source_text(state: &AppState, doc_id: &str) -> Result<String, ApiError> {
    if let Some((path, name, _)) = crate::originals::resolve_original(doc_id) {
        let ext = crate::originals::sanitize_extension(&name);
        if ext == "txt" || ext == "md" {
            if let Ok(bytes) = std::fs::read(&path) {
                if let Ok(text) = String::from_utf8(bytes) {
                    if !text.trim().is_empty() {
                        return Ok(text);
                    }
                }
            }
        }
    }
    Ok(state.rag.document_text(doc_id, EDIT_TEXT_LIMIT + 1).await?)
}

/// Can this user edit this shared document? The uploader can; so can
/// an editor of any Workspace the document is filed in.
fn can_edit_shared(state: &AppState, user: &legal_rag_auth::User, doc_id: &str) -> bool {
    if let Some(info) = state.rag.doc_info(doc_id) {
        if info.uploaded_by.as_deref() == Some(user.id.as_str()) {
            return true;
        }
    }
    state
        .matters
        .matters_of_document(doc_id)
        .into_iter()
        .any(|m| crate::routes::matters::require_edit(state, user, &m).is_ok())
}

/// POST /api/v1/knowledge/agents/edits/propose — run the edit pass
/// and park the proposal for approval. Writes nothing back.
pub async fn propose_edit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<ProposeEditRequest>,
) -> Result<Json<EditRecord>, ApiError> {
    let instruction = body.instruction.trim().to_string();
    if instruction.is_empty() {
        return Err(bad_request("instruction is empty".to_string()));
    }
    if body.doc_id.trim().is_empty() {
        return Err(bad_request("doc_id is empty".to_string()));
    }
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let record = create_edit_proposal(&state, &user, &body.doc_id, &instruction, body.private).await?;
    Ok(Json(record))
}

/// The propose core, shared by the propose route above and the
/// Agent Chat's edit seam (Piece 86): visibility and edit-access
/// checks, the edit pass on the uncached LLM (the confidential path
/// when `private`), the parked proposal, its run-history record,
/// and the billing settle for the pass. Callers own quota checks
/// and request validation. Writes nothing back — a proposal is all
/// this ever creates.
pub(crate) async fn create_edit_proposal(
    state: &AppState,
    user: &legal_rag_auth::User,
    doc_id: &str,
    instruction: &str,
    private: bool,
) -> Result<EditRecord, ApiError> {
    let (source, title, text) = if private {
        // Locked vaults refuse here — doc_text requires the key.
        let (title, text) = state
            .private_vault
            .doc_text(&user.id, doc_id)
            .map_err(bad_request)?;
        ("private", title, text)
    } else {
        if !crate::routes::matters::doc_visible(&state, &user, doc_id) {
            return Err(not_found("document not found"));
        }
        if !can_edit_shared(&state, &user, doc_id) {
            return Err(bad_request(
                "you do not have edit access to this document".to_string(),
            ));
        }
        let info = state
            .rag
            .doc_info(doc_id)
            .ok_or_else(|| not_found("document not found"))?;
        let text = shared_source_text(&state, doc_id).await?;
        ("shared", info.title, text)
    };
    if text.trim().is_empty() {
        return Err(bad_request("document has no editable text".to_string()));
    }
    if text.chars().count() > EDIT_TEXT_LIMIT {
        return Err(bad_request(format!(
            "document is too large for a single edit pass ({} chars, limit {EDIT_TEXT_LIMIT})",
            text.chars().count()
        )));
    }

    // Edit passes never go through the cached LLM (a document's text
    // must not land in the shared completion cache); confidential
    // documents use the dedicated private LLM when configured — the
    // Interact Private path's handles, unchanged.
    let llm = if source == "private" {
        state
            .private_llm
            .clone()
            .unwrap_or_else(|| state.llm_uncached.clone())
    } else {
        state.llm_uncached.clone()
    };
    let scope = legal_rag_rag::usage::new_scope();
    let edited = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        legal_rag_rag::apply_edit(llm.as_ref(), &title, &text, &instruction).await
    })
    .await?;
    let diff = legal_rag_rag::compute_diff(&text, &edited);
    let root = state.edits.root_of(doc_id);
    let record = state.edits.propose(
        &user.email,
        &user.id,
        source,
        doc_id,
        &root,
        &title,
        &instruction,
        &edited,
        diff.clone(),
    );
    let tokens = scope.lock().map(|t| t.total()).unwrap_or(0);
    let run = state.runs.record(
        &user.email,
        "document_edit",
        "Document edit (proposed)",
        &format!("{title} — {instruction}"),
        "pending",
        1,
        tokens,
        &format!(
            "Edit proposed for “{title}”: {}. Awaiting approval — nothing has been written back.",
            diff.summary_line()
        ),
        None,
    );
    state.edits.set_run_id(&record.id, &run.id);
    crate::routes::billing::settle_with_payload(
        &state,
        &user,
        &scope,
        "autonomous",
        &title,
        "Document edit proposal",
        &diff.summary_line(),
        0,
        Some(serde_json::json!({ "edit_id": record.id, "doc_id": doc_id, "private": source == "private" })),
    );
    Ok(state.edits.get(&user.email, &record.id).unwrap_or(record))
}

#[derive(Deserialize)]
pub struct ListEditsQuery {
    pub status: Option<String>,
}

/// GET /api/v1/knowledge/agents/edits — the caller's edit proposals.
pub async fn list_edits(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<ListEditsQuery>,
) -> Result<Json<Vec<EditRecord>>, ApiError> {
    Ok(Json(state.edits.list_for(&user.email, q.status.as_deref())))
}

/// GET /api/v1/knowledge/agents/edits/{id} — one proposal, with the
/// pending edited text and its diff (the approval view).
pub async fn get_edit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<EditRecord>, ApiError> {
    state
        .edits
        .get(&user.email, &id)
        .map(Json)
        .ok_or_else(|| not_found("edit not found"))
}

/// Build the write-back file for a shared document: (file name,
/// bytes, format note for the receipt).
fn version_file(state: &AppState, rec: &EditRecord, edited: &str, version: usize) -> (String, Vec<u8>, String) {
    let (orig_name, ext) = match crate::originals::resolve_original(&rec.doc_id) {
        Some((_, name, _)) => {
            let ext = crate::originals::sanitize_extension(&name);
            (name, ext)
        }
        None => state
            .rag
            .doc_info(&rec.doc_id)
            .map(|i| {
                let ext = crate::originals::sanitize_extension(&i.source_uri);
                (i.source_uri.clone(), ext)
            })
            .unwrap_or_default(),
    };
    let base = orig_name
        .rsplit_once('.')
        .map(|(b, _)| b.to_string())
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| rec.doc_title.clone());
    match ext.as_str() {
        "txt" | "md" | "" => (
            format!("{base} (v{version}).txt"),
            edited.as_bytes().to_vec(),
            "Stored in the document's own text format.".to_string(),
        ),
        "rtf" => (
            format!("{base} (v{version}).rtf"),
            legal_rag_rag::text_to_rtf(edited).into_bytes(),
            "Regenerated as RTF from the edited text (content faithful; the original's styling is not preserved).".to_string(),
        ),
        other => (
            format!("{base} (v{version}).txt"),
            edited.as_bytes().to_vec(),
            format!(
                "The original format ({other}) cannot be regenerated server-side, so the edited version is stored as a text document; the original {other} file remains downloadable as version 1."
            ),
        ),
    }
}

/// POST /api/v1/knowledge/agents/edits/{id}/approve — write the
/// edited text back as a NEW VERSION and record the receipt. The
/// original version is never modified.
pub async fn approve_edit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<EditRecord>, ApiError> {
    let rec = state
        .edits
        .get(&user.email, &id)
        .ok_or_else(|| not_found("edit not found"))?;
    if rec.status != "pending" {
        return Err(bad_request(format!("edit is no longer pending (status: {})", rec.status)));
    }
    let edited = rec.edited_text.clone();
    if edited.trim().is_empty() {
        return Err(bad_request("the proposal holds no edited text".to_string()));
    }
    let version = state.edits.next_version(&rec.root_doc_id);

    // No textual change: approve on the record, create nothing.
    if rec.diff.is_empty() {
        let done = state
            .edits
            .decide(&user.email, &id, &user.email, true, None)
            .ok_or_else(|| not_found("edit not found"))?;
        return Ok(Json(done));
    }

    let outcome: (String, usize, String) = if rec.source == "private" {
        // Confidential write-back: chunk + embed off-corpus with the
        // private provider, then seal under the vault key. add_doc
        // refuses while locked, so a vault locked since the proposal
        // cannot be written to.
        let new_title = format!("{} (edited v{version})", rec.doc_title);
        let chunks = state
            .rag
            .extract_chunks_with(
                "edited-document.txt",
                edited.as_bytes(),
                state.private_embeddings.clone(),
            )
            .await?;
        let pairs: Vec<(String, Vec<f32>)> =
            chunks.into_iter().map(|c| (c.text, c.vector)).collect();
        let meta = state
            .private_vault
            .add_doc(
                &user.id,
                &new_title,
                crate::private_vault::sha256_hex(edited.as_bytes()),
                pairs,
            )
            .map_err(bad_request)?;
        let receipt = format!(
            "Version {version} sealed into the Confidential Vault as a new encrypted document (AES-256-GCM, {} chunks); the original private document is unchanged, and no plaintext entered the shared corpus. Private documents are stored as text, so no file format applies.",
            meta.chunks
        );
        (meta.id, version, receipt)
    } else {
        let new_title = format!("{} (v{version})", rec.doc_title);
        let (file_name, bytes, format_note) = version_file(&state, &rec, &edited, version);
        let scope = legal_rag_rag::usage::new_scope();
        let report = legal_rag_rag::usage::scope_usage(scope.clone(), async {
            state
                .rag
                .process_document_complete(&file_name, &bytes, Some(new_title.clone()), None)
                .await
        })
        .await?;
        if report.doc_id == rec.doc_id {
            // Identical bytes deduplicated onto the source document:
            // the edit changed nothing the pipeline can see.
            let done = state
                .edits
                .decide(&user.email, &id, &user.email, true, None)
                .ok_or_else(|| not_found("edit not found"))?;
            return Ok(Json(done));
        }
        // Retain the new version's file as its original (Piece 29).
        if let Err(e) = crate::originals::save_original(&report.doc_id, &file_name, None, &bytes) {
            tracing::warn!(
                doc_id = %report.doc_id,
                error = %e,
                "could not retain the edited version's file; the version is indexed without one"
            );
        }
        // File the new version wherever the source was filed, and
        // unfile the source there: Workspace retrieval now serves the
        // new version, while the source stays in the vault (and in
        // the version list) untouched.
        let matters = state.matters.matters_of_document(&rec.doc_id);
        for matter in &matters {
            state.matters.remove_document(&matter.id, &rec.doc_id);
            state.matters.add_document(&matter.id, &report.doc_id);
        }
        // Carry the source's legal-validity metadata over — it is the
        // same document, a new version of it.
        if let Some(meta) = state.rag.document_meta(&rec.doc_id) {
            state.rag.merge_document_meta(&report.doc_id, meta);
        }
        state.rag.set_document_owner(&report.doc_id, &user.id);
        crate::routes::billing::settle_with_payload(
            &state,
            &user,
            &scope,
            "upload",
            &new_title,
            "Document edit write-back",
            &format!("{} chunks", report.chunks),
            0,
            Some(serde_json::json!({ "edit_id": rec.id, "doc_id": report.doc_id })),
        );
        let receipt = format!(
            "Version {version} ingested as a new document ({} chunks) and filed in place of the previous version in {} workspace(s); the original version remains in the vault, downloadable and listed. {format_note}",
            report.chunks,
            matters.len()
        );
        (report.doc_id, version, receipt)
    };

    let done = state
        .edits
        .decide(
            &user.email,
            &id,
            &user.email,
            true,
            Some((outcome.0.clone(), outcome.1, outcome.2.clone())),
        )
        .ok_or_else(|| not_found("edit not found"))?;
    state.runs.record(
        &user.email,
        "document_edit",
        "Document edit (approved)",
        &format!("{} — {}", rec.doc_title, rec.instruction),
        "completed",
        1,
        0,
        &outcome.2,
        None,
    );
    Ok(Json(done))
}

/// POST /api/v1/knowledge/agents/edits/{id}/reject — discard the
/// proposal; the outcome is recorded, nothing is written.
pub async fn reject_edit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<EditRecord>, ApiError> {
    let rec = state
        .edits
        .get(&user.email, &id)
        .ok_or_else(|| not_found("edit not found"))?;
    if rec.status != "pending" {
        return Err(bad_request(format!("edit is no longer pending (status: {})", rec.status)));
    }
    let done = state
        .edits
        .decide(&user.email, &id, &user.email, false, None)
        .ok_or_else(|| not_found("edit not found"))?;
    state.runs.record(
        &user.email,
        "document_edit",
        "Document edit (rejected)",
        &format!("{} — {}", rec.doc_title, rec.instruction),
        "rejected",
        1,
        0,
        "Proposal rejected by the approver; the edited text was discarded and nothing was written back.",
        None,
    );
    Ok(Json(done))
}

/// One version in a document's lineage (the versions view).
#[derive(Serialize)]
pub struct VersionView {
    pub version: usize,
    pub doc_id: String,
    pub title: String,
    pub current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// GET /api/v1/vault/documents/{id}/versions — the version lineage
/// of a document (v1 = the original upload, then every approved
/// edit), with each version's provenance receipt.
pub async fn document_versions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(doc_id): Path<String>,
) -> Result<Json<Vec<VersionView>>, ApiError> {
    let private = state
        .private_vault
        .list_docs(&user.id)
        .map(|docs| docs.iter().any(|d| d.id == doc_id))
        .unwrap_or(false);
    if !private && !crate::routes::matters::doc_visible(&state, &user, &doc_id) {
        return Err(not_found("document not found"));
    }
    let root = state.edits.root_of(&doc_id);
    let lineage = state.edits.lineage(&root);
    let latest = lineage
        .last()
        .and_then(|e| e.result_doc_id.clone())
        .unwrap_or_else(|| root.clone());
    let title_of = |id: &str, fallback: &str| -> String {
        if private {
            state
                .private_vault
                .list_docs(&user.id)
                .ok()
                .and_then(|docs| docs.into_iter().find(|d| d.id == id).map(|d| d.title))
                .unwrap_or_else(|| fallback.to_string())
        } else {
            state.rag.doc_title(id).unwrap_or_else(|| fallback.to_string())
        }
    };
    let root_title = lineage
        .first()
        .map(|e| e.doc_title.clone())
        .unwrap_or_else(|| title_of(&root, "Document"));
    let mut views = vec![VersionView {
        version: 1,
        doc_id: root.clone(),
        title: title_of(&root, &root_title),
        current: latest == root,
        instruction: None,
        approver: None,
        receipt: Some("The original document as uploaded.".to_string()),
        decided_at: None,
    }];
    for e in &lineage {
        let Some(result) = &e.result_doc_id else { continue };
        views.push(VersionView {
            version: e.version.unwrap_or(0),
            doc_id: result.clone(),
            title: title_of(result, &e.doc_title),
            current: *result == latest,
            instruction: Some(e.instruction.clone()),
            approver: e.approver.clone(),
            receipt: e.receipt.clone(),
            decided_at: e.decided_at,
        });
    }
    Ok(Json(views))
}
