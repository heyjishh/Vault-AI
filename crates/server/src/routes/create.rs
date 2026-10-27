//! `/api/v1/knowledge/interact` and `/api/v1/knowledge/draft` — the
//! Lexlegis-style trio next to Ask: **Interact** works a single document
//! (summary, chronology, key clauses, risks, or chat over its text);
//! **Draft** generates structured legal drafts, optionally grounded in
//! the corpus. Both are metered and captured like every AI operation.

use axum::extract::State;
use axum::Json;
use legal_rag_auth::AuthUser;
use legal_rag_rag::usage::{new_scope, scope_usage};
use legal_rag_rag::{Citation, QueryMode};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

const DOC_CHAR_BUDGET: usize = 24_000;

pub(crate) const ANALYST_SYSTEM: &str = "You are a precise legal document analyst. Work ONLY from the \
document text provided. If the text does not contain something, say so explicitly — never \
invent names, dates, or clauses. Respond in clean markdown.";

fn interact_instruction(action: &str, question: Option<&str>) -> Result<String, ApiError> {
    let instruction = match action {
        "summary" => "Summarize this document for a practising lawyer. Use headings: \
            Parties & Subject, Key Terms / Obligations, Important Dates, Outcomes or Demands, \
            Anything Unusual. Be specific; quote section or clause numbers where present."
            .to_string(),
        "chronology" => "Extract a chronology of events from this document as a markdown \
            table with columns Date | Event | Source detail. Include only dates actually \
            stated in the text; order chronologically."
            .to_string(),
        "clauses" => "Extract the key clauses or provisions of this document. For each: \
            the clause heading/number, a one-line summary of its effect, and who it favours \
            or burdens. If the document is a statute or notice rather than a contract, \
            list its operative provisions instead."
            .to_string(),
        "risks" => "Review this document for risks: onerous or one-sided terms, ambiguities, \
            missing protections, deadlines or limitation periods, and anything a lawyer \
            should verify. Rank by severity with a short reason for each."
            .to_string(),
        "chat" => {
            let q = question.unwrap_or("").trim();
            if q.is_empty() {
                return Err(legal_rag_common::Error::BadRequest(
                    "chat action needs a question".into(),
                )
                .into());
            }
            format!("Answer this question about the document: {q}")
        }
        other => {
            return Err(legal_rag_common::Error::BadRequest(format!(
                "unknown interact action: {other}"
            ))
            .into())
        }
    };
    Ok(instruction)
}

#[derive(Deserialize)]
pub struct InteractRequest {
    pub document_id: String,
    pub action: String,
    pub question: Option<String>,
}

#[derive(Serialize)]
pub struct InteractResponse {
    pub document_id: String,
    pub document_title: String,
    pub action: String,
    pub output: String,
    /// True when the document was longer than the analysis budget.
    pub truncated: bool,
}

pub async fn interact(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<InteractRequest>,
) -> Result<Json<InteractResponse>, ApiError> {
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let title = state
        .rag
        .doc_title(&body.document_id)
        .ok_or_else(|| legal_rag_common::Error::NotFound("document not found".into()))?;
    let instruction = interact_instruction(&body.action, body.question.as_deref())?;
    let full_len = state.rag.document_text(&body.document_id, usize::MAX).await?.len();
    let text = state.rag.document_text(&body.document_id, DOC_CHAR_BUDGET).await?;
    if text.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest(
            "document has no extractable text".into(),
        )
        .into());
    }
    let prompt = format!("Document: {title}\n\n{text}\n\n---\nTask: {instruction}");
    let analyst_system = state
        .settings
        .get()
        .prompts
        .interact
        .unwrap_or_else(|| ANALYST_SYSTEM.to_string());
    let scope = new_scope();
    let output = scope_usage(scope.clone(), async {
        state.llm.complete(&analyst_system, &prompt).await
    })
    .await?;
    crate::routes::billing::settle_matter_payload(
        &state,
        &user,
        &scope,
        "interact",
        &title,
        &body.action,
        &output,
        0,
        // Interact work belongs to the matter holding the document.
        state.matters.matter_of_document(&body.document_id),
        Some(serde_json::json!({
            "document_id": body.document_id,
            "document_title": title,
            "action": body.action,
            "output": output,
            "truncated": full_len > DOC_CHAR_BUDGET,
        })),
    );
    Ok(Json(InteractResponse {
        document_id: body.document_id,
        document_title: title,
        action: body.action,
        output,
        truncated: full_len > DOC_CHAR_BUDGET,
    }))
}

// ------------------------------------------------------------ draft

pub(crate) const DRAFTER_SYSTEM: &str = "You are an experienced Indian legal drafting assistant. Draft \
carefully and completely. Use [BRACKETED PLACEHOLDERS] for names, dates, amounts, and facts \
not provided — never invent case citations or statutory text. Write in clean markdown with \
clear headings. If source material is provided, stay consistent with it.";

fn draft_template(kind: &str) -> Result<&'static str, ApiError> {
    match kind {
        "legal_notice" => Ok("Draft a formal legal notice in Indian practice: sender and \
            recipient blocks, subject line, numbered facts, the demand, consequences of \
            non-compliance, and a reply period."),
        "agreement" => Ok("Draft a structured agreement: parties, recitals, definitions, \
            operative obligations, consideration/payment, term, confidentiality, \
            termination, liability, governing law (India) and dispute resolution."),
        "memo" => Ok("Draft a legal memorandum in IRAC form: Questions Presented, Brief \
            Answer, Facts, Analysis (Issue, Rule, Application, Counter-arguments), \
            Conclusion. Ground the Rule section in the provided source material."),
        "petition" => Ok("Draft a petition / written statement outline: cause title \
            placeholder, parties, jurisdiction, numbered facts, grounds, interim relief \
            sought, and the prayer."),
        "letter" => Ok("Draft a professional advocate's letter: firm placeholder header, \
            reference, subject, body making the client's position and request clearly, \
            and a formal closing."),
        other => Err(legal_rag_common::Error::BadRequest(format!(
            "unknown draft kind: {other}"
        ))
        .into()),
    }
}

#[derive(Deserialize)]
pub struct DraftRequest {
    pub kind: String,
    pub instructions: String,
    pub use_corpus: Option<bool>,
    /// When set, ground the draft in this matter's documents first.
    pub matter_id: Option<String>,
    /// Citator V2 context for the grounding query, as on Ask: asker
    /// state (unknown codes behave as absent) and AAR/AAAR applicant.
    pub state: Option<String>,
    pub applicant: Option<String>,
}

#[derive(Serialize)]
pub struct DraftResponse {
    pub kind: String,
    pub draft: String,
    pub citations: Vec<Citation>,
}

pub async fn draft(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<DraftRequest>,
) -> Result<Json<DraftResponse>, ApiError> {
    if body.instructions.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("instructions are empty".into()).into());
    }
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let template = draft_template(&body.kind)?;
    let scope = new_scope();
    let (draft_text, citations) = scope_usage(scope.clone(), async {
        let mut grounding = String::new();
        let mut citations: Vec<Citation> = Vec::new();
        if body.use_corpus.unwrap_or(true) {
            let scoped = match body.matter_id.as_deref() {
                Some(mid) if !mid.trim().is_empty() => Some(
                    crate::routes::matters::scope_docs(&state, mid, &user)
                        .map_err(|e| e.0)?,
                ),
                _ => None,
            };
            let ctx = crate::routes::citator_context(body.state.as_deref(), body.applicant.as_deref());
            let answer = match &scoped {
                Some(docs) => {
                    state
                        .query
                        .query_scoped(&body.instructions, QueryMode::Mix, 5, docs.clone(), ctx)
                        .await
                }
                None => state.query.query(&body.instructions, QueryMode::Mix, 5, ctx).await,
            };
            if let Ok(answer) = answer {
                for (i, c) in answer.citations.iter().enumerate() {
                    grounding.push_str(&format!(
                        "[doc:{}] {} — {}\n",
                        i + 1,
                        c.doc_title.clone().unwrap_or_else(|| c.doc_id.clone()),
                        c.snippet
                    ));
                }
                citations = answer.citations;
            }
        }
        let prompt = format!(
            "{template}\n\nClient instructions:\n{}\n\n{}",
            body.instructions.trim(),
            if grounding.is_empty() {
                String::new()
            } else {
                format!("Source material from the vault (cite as [doc:N] where relied on):\n{grounding}")
            }
        );
        let drafter_system = state
            .settings
            .get()
            .prompts
            .draft
            .unwrap_or_else(|| DRAFTER_SYSTEM.to_string());
        let text = state.llm.complete(&drafter_system, &prompt).await?;
        Ok::<_, legal_rag_common::Error>((text, citations))
    })
    .await?;
    crate::routes::billing::settle_matter_payload(
        &state,
        &user,
        &scope,
        "draft",
        &body.instructions,
        &body.kind,
        &draft_text,
        citations.len(),
        body.matter_id.clone(),
        Some(serde_json::json!({
            "kind": body.kind,
            "instructions": body.instructions,
            "draft": draft_text,
            "citations": citations,
        })),
    );
    Ok(Json(DraftResponse { kind: body.kind, draft: draft_text, citations }))
}
