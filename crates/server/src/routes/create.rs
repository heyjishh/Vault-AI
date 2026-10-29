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

/// Total analysis-budget ceiling for multi-document Interact runs
/// (5 × the legacy single-document budget). Selections up to the cap
/// share this ceiling, so a document in a small set keeps its full
/// legacy share and only large sets divide the ceiling.
const MULTI_DOC_TOTAL_CEILING: usize = 120_000;

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

/// Maximum documents one Interact run may cover. The total analysis
/// budget grows with the selection up to MULTI_DOC_TOTAL_CEILING and
/// is split across it, so the cap keeps every document's share
/// usable; the UI states the same cap.
pub(crate) const MAX_INTERACT_DOCS: usize = 20;

#[derive(Deserialize)]
pub struct InteractRequest {
    /// Legacy single-document field (still accepted).
    pub document_id: Option<String>,
    /// Multi-document selection (Piece 5). Takes precedence over
    /// `document_id` when non-empty.
    pub document_ids: Option<Vec<String>>,
    pub action: String,
    pub question: Option<String>,
}

/// Resolve the effective selection: `document_ids` (deduped, order
/// preserved) wins; otherwise the single `document_id`. An empty
/// selection and an over-cap selection are both clean 400s.
fn resolve_selection(body: &InteractRequest) -> Result<Vec<String>, ApiError> {
    let mut ids: Vec<String> = Vec::new();
    if let Some(list) = &body.document_ids {
        for raw in list {
            let id = raw.trim();
            if !id.is_empty() && !ids.iter().any(|x| x == id) {
                ids.push(id.to_string());
            }
        }
    }
    if ids.is_empty() {
        if let Some(single) = body
            .document_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            ids.push(single.to_string());
        }
    }
    if ids.is_empty() {
        return Err(legal_rag_common::Error::BadRequest(
            "no documents selected".into(),
        )
        .into());
    }
    if ids.len() > MAX_INTERACT_DOCS {
        return Err(legal_rag_common::Error::BadRequest(format!(
            "interact supports at most {MAX_INTERACT_DOCS} documents at once"
        ))
        .into());
    }
    Ok(ids)
}

/// Per-document share of the analysis budget. A single document gets
/// the full historical budget, so single-doc behaviour is unchanged.
/// Larger selections keep the full per-document share until the
/// total would pass MULTI_DOC_TOTAL_CEILING; beyond that point the
/// ceiling is split evenly (20 documents → 6,000 chars each), so the
/// prompt never exceeds the ceiling however wide the selection is.
fn doc_budget(selection_len: usize) -> usize {
    let n = selection_len.max(1);
    if n == 1 {
        return DOC_CHAR_BUDGET;
    }
    DOC_CHAR_BUDGET.min(MULTI_DOC_TOTAL_CEILING / n)
}

/// Assemble the analysis prompt. One document keeps the exact legacy
/// format; several documents are presented as labelled sections with
/// an attribution instruction, so cross-document answers cite the
/// document each point comes from.
fn build_interact_prompt(docs: &[(String, String)], instruction: &str) -> String {
    if docs.len() == 1 {
        let (title, text) = &docs[0];
        return format!("Document: {title}\n\n{text}\n\n---\nTask: {instruction}");
    }
    let mut prompt = format!(
        "Analyse the following {} documents together. Attribute every point to the \
         document it comes from, by title.\n",
        docs.len()
    );
    for (i, (title, text)) in docs.iter().enumerate() {
        prompt.push_str(&format!(
            "\n===== Document {} of {}: {title} =====\n{text}\n",
            i + 1,
            docs.len()
        ));
    }
    prompt.push_str(&format!("\n---\nTask: {instruction}"));
    prompt
}

#[derive(Serialize)]
pub struct InteractDocumentResult {
    pub document_id: String,
    pub document_title: String,
    /// True when this document was longer than its budget share.
    pub truncated: bool,
    /// Citator V2 treatment of THIS document (None when the switch is
    /// off) — resolved per document, never merged across the set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub treatment: Option<legal_rag_rag::CitationTreatment>,
}

#[derive(Serialize)]
pub struct InteractResponse {
    /// First document of the selection (legacy field).
    pub document_id: String,
    /// First document's title (legacy field).
    pub document_title: String,
    pub action: String,
    pub output: String,
    /// True when any selected document was longer than its budget share.
    pub truncated: bool,
    /// Per-document results for the whole selection, in selection
    /// order — titles, truncation and treatments stay per document.
    pub documents: Vec<InteractDocumentResult>,
    /// Citator V2 treatment of the (first) document itself (None when
    /// the switch is off). Interact never calls query(), so there is
    /// no query call to thread state/applicant into — the
    /// citator-meaningful surface here is flagging an
    /// overruled/reversed document under analysis. Note a single-doc
    /// view cannot determine "governing" (that needs a chain), only
    /// the document's own treatment; an overruled document is flagged
    /// even when the treating judgment is not in the selection,
    /// because stored edges are scope-free here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub treatment: Option<legal_rag_rag::CitationTreatment>,
}

/// Citator V2 treatment of one document under analysis (switch on
/// only), resolved over a synthesized single citation for that
/// document: stored V2 edges (full-text ingest) + V1 edges + stored
/// metadata, with the real→positional map so stored JudgmentId edges
/// match. Ingest never creates self-edges and the adapter's self-only
/// guard drops a same-caption affirmance when no lower judgment
/// shares the caption, so a document cannot self-affirm here.
fn doc_treatment(
    state: &AppState,
    document_id: &str,
    title: &str,
    text: &str,
) -> Option<legal_rag_rag::CitationTreatment> {
    if !legal_rag_rag::citator_v2::citator_v2_enabled() {
        return None;
    }
    let citation = legal_rag_rag::Citation {
        doc_title: Some(title.to_string()),
        stale: None,
        sentence: None,
        snippet: text.chars().take(1500).collect(),
        doc_id: "doc:0".to_string(),
        chunk_id: String::new(),
        ordinal: 0,
        score: 1.0,
        source_url: None,
        source_date: None,
        treatment: None,
    };
    let mut meta_map = std::collections::HashMap::new();
    if let Some(m) = state.rag.document_meta(document_id) {
        meta_map.insert("doc:0".to_string(), m.clone());
        meta_map.insert(document_id.to_string(), m);
    }
    let mut real_to_positional = std::collections::HashMap::new();
    real_to_positional.insert(document_id.to_string(), "doc:0".to_string());
    let res = legal_rag_rag::citator_v2::resolve_v2(
        std::slice::from_ref(&citation),
        &state.rag.treatments().all(),
        &state.rag.v2_treatments().all(),
        None,
        &meta_map,
        &legal_rag_rag::CitatorContext::default(),
        &real_to_positional,
    );
    res.treatments
        .into_iter()
        .find(|(id, _)| id == "doc:0")
        .map(|(_, t)| t)
}

pub async fn interact(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<InteractRequest>,
) -> Result<Json<InteractResponse>, ApiError> {
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let ids = resolve_selection(&body)?;
    let instruction = interact_instruction(&body.action, body.question.as_deref())?;
    // Load every selected document with its share of the budget. Any
    // missing or text-less document fails the run with a clear error
    // (the single-document behaviour, applied per document) — a
    // silent hole in a cross-document analysis would be worse.
    let per_doc = doc_budget(ids.len());
    let mut docs: Vec<(String, String, String, bool)> = Vec::new();
    for id in &ids {
        let title = state
            .rag
            .doc_title(id)
            .ok_or_else(|| legal_rag_common::Error::NotFound("document not found".into()))?;
        let full_len = state.rag.document_text(id, usize::MAX).await?.len();
        let text = state.rag.document_text(id, per_doc).await?;
        if text.trim().is_empty() {
            return Err(legal_rag_common::Error::BadRequest(if ids.len() == 1 {
                "document has no extractable text".to_string()
            } else {
                format!("document has no extractable text: {title}")
            })
            .into());
        }
        docs.push((id.clone(), title, text, full_len > per_doc));
    }
    // Treatments resolve per document — never merged across the set.
    let documents: Vec<InteractDocumentResult> = docs
        .iter()
        .map(|(id, title, text, truncated)| InteractDocumentResult {
            document_id: id.clone(),
            document_title: title.clone(),
            truncated: *truncated,
            treatment: doc_treatment(&state, id, title, text),
        })
        .collect();
    let prompt_docs: Vec<(String, String)> = docs
        .iter()
        .map(|(_, title, text, _)| (title.clone(), text.clone()))
        .collect();
    let prompt = build_interact_prompt(&prompt_docs, &instruction);
    let first_id = docs[0].0.clone();
    let first_title = docs[0].1.clone();
    let any_truncated = documents.iter().any(|d| d.truncated);
    let first_treatment = documents.first().and_then(|d| d.treatment.clone());
    let activity_title = if docs.len() == 1 {
        first_title.clone()
    } else {
        format!("{first_title} +{} more", docs.len() - 1)
    };
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
        &activity_title,
        &body.action,
        &output,
        0,
        // Interact work belongs to the matter holding the (first) document.
        state.matters.matter_of_document(&first_id),
        Some(serde_json::json!({
            "document_id": first_id,
            "document_title": first_title,
            "document_ids": ids,
            "action": body.action,
            "output": output,
            "truncated": any_truncated,
        })),
    );
    Ok(Json(InteractResponse {
        document_id: first_id,
        document_title: first_title,
        action: body.action,
        output,
        truncated: any_truncated,
        documents,
        treatment: first_treatment,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn req(document_id: Option<&str>, document_ids: Option<Vec<&str>>) -> InteractRequest {
        InteractRequest {
            document_id: document_id.map(str::to_string),
            document_ids: document_ids
                .map(|v| v.into_iter().map(str::to_string).collect()),
            action: "summary".to_string(),
            question: None,
        }
    }

    #[test]
    fn selection_prefers_document_ids_and_dedupes() {
        let ids = resolve_selection(&req(
            Some("legacy"),
            Some(vec!["a", "b", "a", " ", "c"]),
        ))
        .ok()
        .expect("selection resolves");
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn selection_falls_back_to_single_document_id() {
        let ids = resolve_selection(&req(Some("only"), None))
            .ok()
            .expect("selection resolves");
        assert_eq!(ids, vec!["only"]);
        // An empty document_ids list also falls back.
        let ids = resolve_selection(&req(Some("only"), Some(vec![])))
            .ok()
            .expect("selection resolves");
        assert_eq!(ids, vec!["only"]);
    }

    #[test]
    fn empty_selection_rejected_cleanly() {
        assert!(resolve_selection(&req(None, None)).is_err());
        assert!(resolve_selection(&req(Some("  "), Some(vec![""]))).is_err());
    }

    #[test]
    fn over_cap_selection_rejected() {
        let over: Vec<String> = (1..=21).map(|i| i.to_string()).collect();
        let over_refs: Vec<&str> = over.iter().map(String::as_str).collect();
        match resolve_selection(&req(None, Some(over_refs))) {
            Ok(_) => panic!("21 documents must be rejected"),
            Err(e) => assert!(e.0.to_string().contains("at most"), "{}", e.0),
        }
        let at_cap: Vec<String> = (1..=20).map(|i| i.to_string()).collect();
        let at_cap_refs: Vec<&str> = at_cap.iter().map(String::as_str).collect();
        assert_eq!(
            resolve_selection(&req(None, Some(at_cap_refs)))
                .ok()
                .expect("20 documents allowed")
                .len(),
            20
        );
    }

    #[test]
    fn budget_scales_to_the_ceiling_and_single_doc_keeps_full_budget() {
        // Single document: the exact legacy budget.
        assert_eq!(doc_budget(1), DOC_CHAR_BUDGET);
        // Small selections keep the full per-document share...
        assert_eq!(doc_budget(2), DOC_CHAR_BUDGET);
        assert_eq!(doc_budget(5), DOC_CHAR_BUDGET);
        // ...until the ceiling binds, then it splits evenly.
        assert_eq!(doc_budget(10), MULTI_DOC_TOTAL_CEILING / 10);
        assert_eq!(doc_budget(20), MULTI_DOC_TOTAL_CEILING / 20);
        // The total prompt never exceeds the ceiling at any width.
        for n in 1..=MAX_INTERACT_DOCS {
            assert!(
                doc_budget(n) * n <= MULTI_DOC_TOTAL_CEILING.max(DOC_CHAR_BUDGET),
                "n={n}"
            );
        }
    }

    #[test]
    fn single_doc_prompt_is_the_legacy_format() {
        let p = build_interact_prompt(
            &[("Title A".to_string(), "body a".to_string())],
            "Do the thing.",
        );
        assert_eq!(p, "Document: Title A\n\nbody a\n\n---\nTask: Do the thing.");
    }

    #[test]
    fn multi_doc_prompt_covers_every_document() {
        let p = build_interact_prompt(
            &[
                ("Alpha v Beta".to_string(), "alpha body".to_string()),
                ("Gamma Order".to_string(), "gamma body".to_string()),
            ],
            "Compare them.",
        );
        assert!(p.contains("Attribute every point"), "{p}");
        assert!(p.contains("Document 1 of 2: Alpha v Beta"));
        assert!(p.contains("Document 2 of 2: Gamma Order"));
        assert!(p.contains("alpha body") && p.contains("gamma body"));
        assert!(p.ends_with("Task: Compare them."));
    }
}
