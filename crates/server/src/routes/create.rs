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
    /// Refinement terms (Piece 13) for the chat question. Interact
    /// has no retrieval leg — the selected documents are the
    /// evidence — so the handler folds them into the instruction as
    /// an explicit focus instead of an expansion query.
    pub refine_terms: Option<Vec<String>>,
    /// Source selector (Piece 16): "vault" (default — the active
    /// brief's documents), "private" (ONLY the user's Confidential
    /// Vault), or "both" (brief documents + private retrieval).
    /// Server-enforced; Interact never uses web or system data.
    pub source: Option<String>,
}

/// Piece 16 — where an Interact run may read from. Exactly three
/// modes; anything else is a clean 400, never a silent default.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum InteractSource {
    Vault,
    Private,
    Both,
}

impl InteractSource {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            InteractSource::Vault => "vault",
            InteractSource::Private => "private",
            InteractSource::Both => "both",
        }
    }
    /// Does this mode read the Confidential Vault at all?
    fn uses_private(&self) -> bool {
        matches!(self, InteractSource::Private | InteractSource::Both)
    }
}

fn parse_source(raw: Option<&str>) -> Result<InteractSource, ApiError> {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("vault") => Ok(InteractSource::Vault),
        Some("private") => Ok(InteractSource::Private),
        Some("both") => Ok(InteractSource::Both),
        Some(other) => Err(legal_rag_common::Error::BadRequest(format!(
            "unknown interact source: {other} (expected vault, private or both)"
        ))
        .into()),
    }
}

/// Document actions (summary, chronology, …) operate on selected
/// vault documents. In Private mode there are none by definition —
/// refuse plainly instead of faking a private action. In Both mode
/// actions apply to the vault-document side only.
fn ensure_action_allowed(source: InteractSource, action: &str) -> Result<(), ApiError> {
    if source == InteractSource::Private && action != "chat" {
        return Err(legal_rag_common::Error::BadRequest(
            "document actions need Vault documents — switch source to Vault or Both".to_string(),
        )
        .into());
    }
    Ok(())
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

/// The confidential block appended to a chat prompt in Private/Both
/// modes: the owner's decrypted top hits, labelled exactly as Ask's
/// query layer labels them, so `[private:N]` means the same thing on
/// both surfaces.
fn private_prompt_block(hits: &[legal_rag_rag::ExtraSource]) -> String {
    let body = hits
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "[private:{}] {}\n{}",
                i,
                s.title,
                s.text.chars().take(1500).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "Confidential sources (the user's own private documents, visible to them alone):\n{body}"
    )
}

/// Chat prompt for Private mode: the confidential block is the ONLY
/// evidence — no brief document is loaded or referenced.
fn build_private_prompt(hits: &[legal_rag_rag::ExtraSource], instruction: &str) -> String {
    format!(
        "{}\n\n---\nTask: {instruction}\nCite the confidential sources as [private:N] using the labels above.",
        private_prompt_block(hits)
    )
}

/// Chat prompt for Both mode: the legacy document prompt plus the
/// confidential block, with the two citation conventions spelled out
/// so the answer's markers stay attributable by source.
fn build_both_prompt(
    docs: &[(String, String)],
    hits: &[legal_rag_rag::ExtraSource],
    instruction: &str,
) -> String {
    format!(
        "{}\n\n{}\nCite the documents above as [doc:N] by their order (first document is [doc:0]) \
         and the confidential sources as [private:N].",
        build_interact_prompt(docs, instruction),
        private_prompt_block(hits)
    )
}

/// Citations for the selected vault documents in a chat run, in
/// selection order with Ask's positional `[doc:N]` convention. The
/// real document id rides in `chunk_id` so the UI can resolve it.
fn doc_citations(docs: &[(String, String, String)], chat: bool) -> Vec<Citation> {
    if !chat {
        return Vec::new();
    }
    docs.iter()
        .enumerate()
        .map(|(i, (id, title, text))| Citation {
            doc_title: Some(title.clone()),
            stale: None,
            sentence: None,
            snippet: text.chars().take(300).collect(),
            doc_id: format!("doc:{i}"),
            chunk_id: id.clone(),
            ordinal: i,
            score: 1.0,
            source_url: None,
            source_date: None,
            treatment: None,
        })
        .collect()
}

/// Citations for confidential hits, mirroring Ask's query layer
/// (query.rs): positional `private:N` ids, the decrypted excerpt as
/// the snippet — returned only to the owning user's own session.
fn private_citations(hits: &[legal_rag_rag::ExtraSource]) -> Vec<Citation> {
    hits.iter()
        .enumerate()
        .map(|(i, s)| Citation {
            doc_title: Some(s.title.clone()),
            stale: None,
            sentence: None,
            snippet: s.text.chars().take(1500).collect(),
            doc_id: format!("private:{i}"),
            chunk_id: format!("private:{i}"),
            ordinal: i,
            score: s.score,
            source_url: None,
            source_date: None,
            treatment: None,
        })
        .collect()
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
    /// The Piece 16 source mode this run actually used.
    pub source: String,
    /// Chat-run citations with Ask's conventions: `[doc:N]` for the
    /// selected vault documents, `[private:N]` for confidential hits.
    /// Empty for document actions (the documents list covers those).
    pub citations: Vec<Citation>,
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
    let source = parse_source(body.source.as_deref())?;
    ensure_action_allowed(source, &body.action)?;
    let mut instruction = interact_instruction(&body.action, body.question.as_deref())?;
    // Refinement terms fold (Piece 13): no retrieval leg exists here
    // to expand, so for the chat action the terms become an explicit
    // focus line in the instruction — the same steering intent,
    // applied at the only seam Interact has.
    if body.action == "chat" {
        let terms = crate::routes::knowledge::body_terms(&body.refine_terms);
        if !terms.is_empty() {
            instruction = format!(
                "{instruction}\n\nFocus your answer on these aspects of the question: {}.",
                terms.join(", ")
            );
        }
    }
    // Confidential retrieval (Piece 16): chat runs only — document
    // actions never blend private material (in Both mode they apply
    // to the vault-document side only). Uses Ask's private path
    // verbatim: owner-scoped, decrypted in memory for this request,
    // and a locked vault is its explicit 400 — never a silent
    // fallback to Vault mode.
    let private_hits: Vec<legal_rag_rag::ExtraSource> =
        if source.uses_private() && body.action == "chat" {
            crate::routes::knowledge::private_sources(
                &state,
                &user.id,
                body.question.as_deref().unwrap_or(""),
                true,
            )
            .await?
        } else {
            Vec::new()
        };
    // Private mode reads no brief documents at all; every other mode
    // resolves the selection exactly as before.
    let ids = if source == InteractSource::Private {
        Vec::new()
    } else {
        resolve_selection(&body)?
    };
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
    let is_chat = body.action == "chat";
    // Chat citations (Piece 16): vault documents in selection order,
    // then confidential hits — the two conventions side by side so
    // the UI can label every marker by its source.
    let mut citations: Vec<Citation> = Vec::new();
    if is_chat {
        let triples: Vec<(String, String, String)> = docs
            .iter()
            .map(|(id, title, text, _)| (id.clone(), title.clone(), text.clone()))
            .collect();
        citations.extend(doc_citations(&triples, true));
        citations.extend(private_citations(&private_hits));
    }
    let prompt = match source {
        InteractSource::Private => build_private_prompt(&private_hits, &instruction),
        InteractSource::Both if !private_hits.is_empty() => {
            build_both_prompt(&prompt_docs, &private_hits, &instruction)
        }
        _ => {
            let base = build_interact_prompt(&prompt_docs, &instruction);
            if is_chat {
                format!(
                    "{base}\nCite the documents as [doc:N] by their order (first document is [doc:0])."
                )
            } else {
                base
            }
        }
    };
    let first_id = docs.first().map(|d| d.0.clone()).unwrap_or_default();
    let first_title = if source == InteractSource::Private {
        "Confidential Vault".to_string()
    } else {
        docs.first().map(|d| d.1.clone()).unwrap_or_default()
    };
    let any_truncated = documents.iter().any(|d| d.truncated);
    let first_treatment = documents.first().and_then(|d| d.treatment.clone());
    let activity_title = if source == InteractSource::Private {
        // No private title (or question) enters the activity record.
        "Confidential Vault".to_string()
    } else if docs.len() == 1 {
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
    // A chat run in a private-reading mode may quote confidential
    // text: its generation must never flow through the shared
    // CachedLlm — use the dedicated private LLM when configured,
    // else the undecorated client. Vault-only runs (and Both-mode
    // actions, which retrieve nothing private) keep the cached one.
    let touches_private = is_chat && source.uses_private();
    let output = if source == InteractSource::Private && private_hits.is_empty() {
        // Grounded refusal with no generation call: the owner's
        // vault holds nothing at/above the retrieval threshold for
        // this question, and nothing outside it may be searched.
        "Your Confidential Vault has no material matching this question — \
         nothing outside it was searched."
            .to_string()
    } else {
        let gen_llm: std::sync::Arc<dyn legal_rag_rag::LlmClient> = if touches_private {
            match &state.private_llm {
                Some(p) => p.clone(),
                None => state.llm_uncached.clone(),
            }
        } else {
            state.llm.clone()
        };
        scope_usage(scope.clone(), async {
            gen_llm.complete(&analyst_system, &prompt).await
        })
        .await?
    };
    // Activity redaction mirrors Ask's private path exactly: when a
    // run read the Confidential Vault, neither its output nor a
    // restorable payload is written to the plaintext activity log.
    let (activity_result, payload) = if touches_private {
        (String::new(), None)
    } else {
        (
            output.clone(),
            Some(serde_json::json!({
                "document_id": first_id,
                "document_title": first_title,
                "document_ids": ids,
                "action": body.action,
                "output": output,
                "truncated": any_truncated,
            })),
        )
    };
    crate::routes::billing::settle_matter_payload(
        &state,
        &user,
        &scope,
        "interact",
        &activity_title,
        &body.action,
        &activity_result,
        citations.len(),
        // Interact work belongs to the matter holding the (first) document.
        if source == InteractSource::Private {
            None
        } else {
            state.matters.matter_of_document(&first_id)
        },
        payload,
    );
    Ok(Json(InteractResponse {
        document_id: first_id,
        document_title: first_title,
        action: body.action,
        output,
        truncated: any_truncated,
        documents,
        source: source.as_str().to_string(),
        citations,
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
    /// Refinement terms (Piece 13), folded into the grounding
    /// query's expansion pool.
    pub refine_terms: Option<Vec<String>>,
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
            let ground_opts = legal_rag_rag::AskOptions {
                brief_vault: None,
                refine_terms: crate::routes::knowledge::body_terms(&body.refine_terms),
            };
            let answer = state
                .query
                .query_opts(
                    &body.instructions,
                    QueryMode::Mix,
                    5,
                    scoped.clone(),
                    false,
                    &ground_opts,
                    ctx,
                )
                .await;
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
            refine_terms: None,
            source: None,
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
    fn source_parsing_defaults_to_vault_and_rejects_unknown() {
        assert_eq!(parse_source(None).ok(), Some(InteractSource::Vault));
        assert_eq!(parse_source(Some("")).ok(), Some(InteractSource::Vault));
        assert_eq!(parse_source(Some(" Vault ")).ok(), Some(InteractSource::Vault));
        assert_eq!(parse_source(Some("private")).ok(), Some(InteractSource::Private));
        assert_eq!(parse_source(Some("BOTH")).ok(), Some(InteractSource::Both));
        assert!(parse_source(Some("web")).is_err(), "Interact has no web source");
        assert!(parse_source(Some("system")).is_err());
    }

    #[test]
    fn private_mode_refuses_document_actions_but_allows_chat() {
        for action in ["summary", "chronology", "clauses", "risks"] {
            match ensure_action_allowed(InteractSource::Private, action) {
                Ok(_) => panic!("private {action} must be refused"),
                Err(e) => assert!(
                    e.0.to_string().contains("switch source to Vault or Both"),
                    "{}",
                    e.0
                ),
            }
        }
        assert!(ensure_action_allowed(InteractSource::Private, "chat").is_ok());
        // Both mode: actions live on the vault side — allowed.
        assert!(ensure_action_allowed(InteractSource::Both, "summary").is_ok());
        assert!(ensure_action_allowed(InteractSource::Vault, "summary").is_ok());
    }

    fn hit(title: &str, text: &str, score: f32) -> legal_rag_rag::ExtraSource {
        legal_rag_rag::ExtraSource {
            title: title.to_string(),
            text: text.to_string(),
            score,
        }
    }

    #[test]
    fn private_prompt_contains_only_confidential_material() {
        let hits = vec![hit("Meridian Escrow Note", "The escrow amount is Rs 42,00,000.", 0.9)];
        let p = build_private_prompt(&hits, "Answer this question about the document: amount?");
        assert!(p.contains("[private:0] Meridian Escrow Note"), "{p}");
        assert!(p.contains("Rs 42,00,000"), "{p}");
        assert!(p.contains("Confidential sources"), "{p}");
        assert!(p.ends_with("labels above."), "{p}");
    }

    #[test]
    fn both_prompt_blends_documents_and_private_with_both_marker_kinds() {
        let docs = vec![("Lease Deed".to_string(), "Rent is Rs 10,000.".to_string())];
        let hits = vec![hit("Side Letter", "Rent was later revised to Rs 12,000.", 0.8)];
        let p = build_both_prompt(&docs, &hits, "Answer this question about the document: rent?");
        assert!(p.contains("Document: Lease Deed"), "{p}");
        assert!(p.contains("[private:0] Side Letter"), "{p}");
        assert!(p.contains("[doc:N]") && p.contains("[private:N]"), "{p}");
    }

    #[test]
    fn citations_label_every_source_by_kind() {
        let docs = vec![
            ("id-a".to_string(), "Alpha".to_string(), "alpha text".to_string()),
            ("id-b".to_string(), "Beta".to_string(), "beta text".to_string()),
        ];
        let dc = doc_citations(&docs, true);
        assert_eq!(dc.len(), 2);
        assert_eq!(dc[0].doc_id, "doc:0");
        assert_eq!(dc[0].chunk_id, "id-a", "real id rides along for resolution");
        assert_eq!(dc[1].doc_id, "doc:1");
        // Actions carry no citations — the documents list covers them.
        assert!(doc_citations(&docs, false).is_empty());
        let pc = private_citations(&[hit("Note", "secret text", 0.77)]);
        assert_eq!(pc.len(), 1);
        assert_eq!(pc[0].doc_id, "private:0");
        assert_eq!(pc[0].chunk_id, "private:0");
        assert_eq!(pc[0].snippet, "secret text");
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
