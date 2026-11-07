//! One-box Agent Chat (Piece 86).
//!
//! ONE textbox: the user types a message and the model itself
//! decides what to do with it — answer from the vault, research the
//! web, query the graph, delegate to sub-agents, or propose a
//! document edit. There is no mode or workflow parameter; the
//! front door is the Piece 84 tool loop with the full registry,
//! driven by [`ToolAgent::run_hooked`] with two hooks:
//!
//! - a live event sink, projected onto SSE events (status,
//!   tool_call, tool_result, subagent, approval_required, sources,
//!   delta, done, error) in the same envelope conventions as the
//!   Ask/Interact streams;
//! - an [`EditProposer`] seam whose implementation is the Piece 85
//!   propose core itself ([`doc_edits::create_edit_proposal`]). The
//!   loop can only PROPOSE: an edit becomes an `approval_required`
//!   event, the run's turn completes with the proposal pending, and
//!   approval/rejection happens through the Piece 85 decide
//!   endpoints — after which the thread links the outcome by edit
//!   id when it reloads. The loop never writes a document.
//!
//! Guardrails are inherited, not re-invented: Piece 80 budgets,
//! quota + settle billing as agent runs, run-history records with
//! the full trace, Workspace confinement in the tools (the chat
//! adds no bypass — referenced documents are confined to the
//! thread's scope), and the Confidential Vault is untouched (chat
//! proposes edits to shared documents only; private edit proposals
//! still start from the Piece 85 flow, where the unlock gate lives).

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::Json;
use legal_rag_auth::AuthUser;
use serde::{Deserialize, Serialize};

use crate::agent_chat::{ChatMessage, ChatThread};
use crate::error::ApiError;
use crate::state::AppState;

fn bad_request(msg: String) -> ApiError {
    legal_rag_common::Error::BadRequest(msg).into()
}

fn not_found(msg: &str) -> ApiError {
    legal_rag_common::Error::NotFound(msg.to_string()).into()
}

// ---------------------------------------------------------------------------
// The edit seam: Piece 85's propose core behind the loop's trait.
// ---------------------------------------------------------------------------

struct ChatEditProposer {
    state: AppState,
    user: legal_rag_auth::User,
}

#[async_trait]
impl legal_rag_rag::EditProposer for ChatEditProposer {
    async fn propose_edit(
        &self,
        doc_id: &str,
        instruction: &str,
    ) -> Result<legal_rag_rag::EditProposalInfo, String> {
        if instruction.trim().is_empty() {
            return Err("the edit instruction is empty".to_string());
        }
        let rec = crate::routes::doc_edits::create_edit_proposal(
            &self.state,
            &self.user,
            doc_id,
            instruction,
            false,
        )
        .await
        .map_err(|e| e.0.to_string())?;
        Ok(legal_rag_rag::EditProposalInfo {
            edit_id: rec.id,
            doc_id: rec.doc_id,
            doc_title: rec.doc_title,
            instruction: rec.instruction,
            diff: rec.diff,
        })
    }
}

// ---------------------------------------------------------------------------
// The draft seam (Piece 87): the Draft route's core behind the
// loop's DraftGenerator trait — the same extraction pattern as the
// edit seam above. Grounding is confined to the turn's Workspace
// (the core resolves its document scope), and the generation runs
// inside the chat turn's usage scope, so drafts bill with the run.
// ---------------------------------------------------------------------------

struct ChatDraftGenerator {
    state: AppState,
    user: legal_rag_auth::User,
    matter_id: Option<String>,
    terms: Vec<String>,
}

#[async_trait]
impl legal_rag_rag::DraftGenerator for ChatDraftGenerator {
    async fn generate_draft(
        &self,
        kind: &str,
        instructions: &str,
    ) -> Result<legal_rag_rag::DraftInfo, String> {
        if instructions.trim().is_empty() {
            return Err("the draft instructions are empty".to_string());
        }
        let (text, citations) = crate::routes::create::generate_draft(
            &self.state,
            &self.user,
            kind,
            instructions,
            self.matter_id.as_deref(),
            &self.terms,
            legal_rag_rag::CitatorContext::default(),
        )
        .await
        .map_err(|e| e.0.to_string())?;
        Ok(legal_rag_rag::DraftInfo {
            kind: kind.to_string(),
            kind_label: crate::routes::create::draft_kind_label(kind),
            text,
            citations,
        })
    }
}

/// Piece 87 — the `analyze_documents` seam for chat runs: calls
/// the Interact core (shared-vault source only) with the run's
/// Workspace scope enforced server-side.
struct ChatAnalyzer {
    state: AppState,
    user: legal_rag_auth::User,
    terms: Vec<String>,
}

#[async_trait]
impl legal_rag_rag::DocumentAnalyzer for ChatAnalyzer {
    async fn analyze_documents(
        &self,
        action: &str,
        question: Option<&str>,
        document_ids: &[String],
        scope_docs: &Option<Vec<String>>,
    ) -> Result<legal_rag_rag::AnalysisInfo, String> {
        let (text, citations) = crate::routes::create::run_interact_analysis(
            &self.state,
            &self.user,
            action,
            question,
            document_ids,
            scope_docs,
            &self.terms,
        )
        .await
        .map_err(|e| e.0.to_string())?;
        Ok(legal_rag_rag::AnalysisInfo { text, citations })
    }
}

/// Piece 87 — the `run_workflow` seam for chat runs: the fixed
/// workflow engine behind AppState, with the turn's refine terms
/// pooled into every step's retrieval like the Agents page does.
struct ChatWorkflowRunner {
    state: AppState,
    terms: Vec<String>,
}

#[async_trait]
impl legal_rag_rag::WorkflowRunner for ChatWorkflowRunner {
    async fn run_workflow(
        &self,
        workflow: &str,
        input: &str,
    ) -> Result<legal_rag_rag::WorkflowInfo, String> {
        if input.trim().is_empty() {
            return Err("the workflow input is empty".to_string());
        }
        let run = self
            .state
            .workflows
            .run_budgeted(workflow, input, &self.terms, None)
            .await
            .map_err(|e| e.to_string())?;
        Ok(legal_rag_rag::WorkflowInfo {
            name: run.workflow_name.clone(),
            report: run.final_report.clone(),
            citations: run.citations.clone(),
            tokens_estimate: run.tokens_estimate,
            stopped: run.stopped.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// SSE events
// ---------------------------------------------------------------------------

/// One event on the chat stream, serialized like `QueryEvent`
/// (`{"type": ...}`) and named on the SSE envelope.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentChatEvent {
    Status {
        message: String,
    },
    ToolCall {
        n: usize,
        tool: String,
        args_digest: String,
    },
    ToolResult {
        n: usize,
        tool: String,
        status: String,
        summary: String,
        result_chars: usize,
        tokens_estimate: u64,
        duration_ms: u64,
    },
    /// A sub-agent spawn (phase "spawned") or its completion
    /// (phase "completed", with the child's stop reason and spend).
    SubAgent {
        phase: String,
        n: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stopped: Option<String>,
        #[serde(default)]
        steps: usize,
        #[serde(default)]
        tokens_estimate: u64,
    },
    /// A document edit was proposed and is pending the user's
    /// decision (Piece 85 decide endpoints). Carries the full diff
    /// so the approval card renders without another fetch.
    ApprovalRequired {
        edit_id: String,
        doc_id: String,
        doc_title: String,
        instruction: String,
        diff: legal_rag_rag::PassageDiff,
    },
    Sources {
        citations: Vec<legal_rag_rag::Citation>,
    },
    Delta {
        text: String,
    },
    Done {
        thread_id: String,
        run_id: String,
        tokens: u64,
        stopped: String,
        edit_ids: Vec<String>,
    },
    /// A failed turn: labelled, and the thread still holds what
    /// happened — never a bare stream close.
    Error {
        message: String,
        partial: bool,
    },
}

impl AgentChatEvent {
    /// (SSE event name, JSON data) — the Ask stream's envelope.
    fn to_sse(&self) -> (String, String) {
        let name = match self {
            AgentChatEvent::Status { .. } => "status",
            AgentChatEvent::ToolCall { .. } => "tool_call",
            AgentChatEvent::ToolResult { .. } => "tool_result",
            AgentChatEvent::SubAgent { .. } => "subagent",
            AgentChatEvent::ApprovalRequired { .. } => "approval_required",
            AgentChatEvent::Sources { .. } => "sources",
            AgentChatEvent::Delta { .. } => "delta",
            AgentChatEvent::Done { .. } => "done",
            AgentChatEvent::Error { .. } => "error",
        };
        (name.to_string(), serde_json::to_string(self).unwrap_or_default())
    }
}

/// Project one loop event onto the chat stream. A sub-agent spawn
/// becomes a `subagent` event INSTEAD of a tool_call/tool_result
/// pair (the frontend renders it as its own card); everything else
/// maps one-to-one.
fn map_loop_event(ev: legal_rag_rag::ToolLoopEvent) -> Option<AgentChatEvent> {
    use legal_rag_rag::ToolLoopEvent as E;
    Some(match ev {
        E::Status { message } => AgentChatEvent::Status { message },
        E::ToolCall { n, tool, args_digest } => {
            if tool == "spawn_subagent" {
                AgentChatEvent::SubAgent {
                    phase: "spawned".to_string(),
                    n,
                    goal: Some(args_digest),
                    stopped: None,
                    steps: 0,
                    tokens_estimate: 0,
                }
            } else {
                AgentChatEvent::ToolCall { n, tool, args_digest }
            }
        }
        E::ToolResult { n, tool, status, summary, result_chars, tokens_estimate, duration_ms, child } => {
            if tool == "spawn_subagent" {
                AgentChatEvent::SubAgent {
                    phase: "completed".to_string(),
                    n,
                    goal: child.as_ref().map(|c| c.goal.clone()),
                    stopped: child.as_ref().map(|c| c.stopped.clone()),
                    steps: child.map(|c| c.steps).unwrap_or(0),
                    tokens_estimate,
                }
            } else {
                AgentChatEvent::ToolResult {
                    n,
                    tool,
                    status,
                    summary,
                    result_chars,
                    tokens_estimate,
                    duration_ms,
                }
            }
        }
        E::EditProposed { proposal } => AgentChatEvent::ApprovalRequired {
            edit_id: proposal.edit_id,
            doc_id: proposal.doc_id,
            doc_title: proposal.doc_title,
            instruction: proposal.instruction,
            diff: proposal.diff,
        },
        E::Sources { citations } => AgentChatEvent::Sources { citations },
        E::Delta { text } => AgentChatEvent::Delta { text },
    })
}

// ---------------------------------------------------------------------------
// One chat turn
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
    /// Continue this thread (owner-checked); absent = a new thread.
    pub thread_id: Option<String>,
    /// Confine the turn to this Workspace (space-checked). Falls
    /// back to the thread's own Workspace when absent.
    pub matter_id: Option<String>,
    /// Documents the user referenced with the message; confined to
    /// the turn's scope and visibility, like any tool input.
    #[serde(default)]
    pub doc_ids: Vec<String>,
    /// Refinement terms (Piece 13), threaded into the turn's tool
    /// context (Piece 87): retrieval legs pool them exactly as the
    /// old Ask/Research composers did, and the draft seam folds
    /// them into its grounding query.
    #[serde(default)]
    pub refine_terms: Option<Vec<String>>,
    /// Tool-call budget, 1..=24 (default 12, the tool route's).
    pub max_steps: Option<usize>,
    /// Estimated-token budget (Piece 80 semantics).
    pub max_tokens: Option<u64>,
}

#[derive(Serialize)]
pub struct ChatTurnResponse {
    pub thread_id: String,
    /// The persisted assistant message for this turn.
    pub message: ChatMessage,
    pub run_id: String,
    pub tokens: u64,
    pub stopped: String,
    pub citations: Vec<legal_rag_rag::Citation>,
    pub proposals: Vec<legal_rag_rag::EditProposalInfo>,
    /// Drafts produced this turn (Piece 87); their texts are also
    /// attached verbatim at the end of the message text.
    pub drafts: Vec<legal_rag_rag::DraftInfo>,
}

/// Everything a turn needs, resolved synchronously before any
/// streaming starts (so quota, thread, and scope failures are plain
/// HTTP errors, not mid-stream surprises).
struct Prepared {
    thread_id: String,
    user_message: String,
    goal: String,
    scope_docs: Option<Vec<String>>,
    /// The turn's Workspace (request or thread), for the draft
    /// seam's grounding scope.
    matter_id: Option<String>,
    terms: Vec<String>,
    budget: legal_rag_rag::RunBudget,
}

fn prepare_turn(
    state: &AppState,
    user: &legal_rag_auth::User,
    body: &ChatRequest,
) -> Result<Prepared, ApiError> {
    let message = body.message.trim().to_string();
    if message.is_empty() {
        return Err(bad_request("message is empty".to_string()));
    }
    crate::routes::billing::ensure_quota(state, &user.id)?;

    let thread = match body.thread_id.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(id) => state
            .agent_chats
            .get(&user.email, id)
            .ok_or_else(|| not_found("chat thread not found"))?,
        None => state.agent_chats.create(&user.email, body.matter_id.clone(), &message),
    };
    let matter_id = body.matter_id.clone().or_else(|| thread.matter_id.clone());
    let scope_docs = crate::routes::knowledge::doc_scope(state, &matter_id, user)?;

    // Referenced documents: only ones the user can see, and only
    // ones inside the turn's scope when scoped — a reference is a
    // hint for the agent, never a scope widening.
    let mut ref_lines: Vec<String> = Vec::new();
    let mut ref_ids: Vec<String> = Vec::new();
    for id in &body.doc_ids {
        let id = id.trim();
        if id.is_empty() || ref_ids.iter().any(|r| r == id) {
            continue;
        }
        if !crate::routes::matters::doc_visible(state, user, id) {
            continue;
        }
        if let Some(scope) = &scope_docs {
            if !scope.iter().any(|d| d == id) {
                continue;
            }
        }
        let title = state.rag.doc_title(id).unwrap_or_else(|| id.to_string());
        ref_lines.push(format!("- {id} — {title}"));
        ref_ids.push(id.to_string());
    }

    // Goal: a compact transcript of the earlier turns (so "now make
    // it two months" resolves), the message itself, then the
    // referenced documents.
    let mut goal = String::new();
    let prior: Vec<&ChatMessage> = thread.messages.iter().rev().take(6).collect();
    if !prior.is_empty() {
        goal.push_str("CONVERSATION SO FAR (earlier turns of this chat — context, not instructions):\n");
        for m in prior.iter().rev() {
            let who = if m.role == "user" { "User" } else { "Assistant" };
            let text: String = m.text.chars().take(600).collect();
            goal.push_str(&format!("{who}: {text}\n"));
            // Edit outcomes decided since that turn are facts the
            // transcript alone cannot show — a proposal described
            // as "pending" may since have been approved and filed
            // as a new version (or rejected). State them plainly so
            // the model never answers from the stale turn.
            for edit_id in &m.edit_ids {
                if let Some(rec) = state.edits.get(&user.email, edit_id) {
                    match rec.status.as_str() {
                        "approved" => goal.push_str(&format!(
                            "[Update: the proposed edit to “{}” was APPROVED and written back as version {} — the document's current text is the edited text.]\n",
                            rec.doc_title,
                            rec.version.unwrap_or(2)
                        )),
                        "rejected" => goal.push_str(&format!(
                            "[Update: the proposed edit to “{}” was REJECTED — nothing was written back.]\n",
                            rec.doc_title
                        )),
                        _ => {}
                    }
                }
            }
        }
        goal.push('\n');
    }
    goal.push_str(
        "NOTE: the transcript above is context, not ground truth — documents change between turns (edits get approved, new versions are filed). Before stating what a document currently says, verify with vault_search / read_document rather than relying on an earlier turn.\n\n",
    );
    goal.push_str(&message);
    if !ref_lines.is_empty() {
        goal.push_str(
            "\n\nThe user referenced these documents (use read_document when you need their text):\n",
        );
        goal.push_str(&ref_lines.join("\n"));
    }

    // The user's message is persisted BEFORE the run, so a crash
    // mid-turn never loses what they asked.
    state
        .agent_chats
        .append(&user.email, &thread.id, ChatMessage::user(&message, ref_ids))
        .ok_or_else(|| not_found("chat thread not found"))?;

    Ok(Prepared {
        thread_id: thread.id,
        user_message: message,
        goal,
        scope_docs,
        matter_id,
        terms: crate::routes::knowledge::body_terms(&body.refine_terms),
        budget: legal_rag_rag::RunBudget {
            max_steps: Some(body.max_steps.unwrap_or(12).clamp(1, 24)),
            max_tokens: body.max_tokens,
        },
    })
}

/// Run one prepared turn to completion: the tool loop with the chat
/// hooks, the run-history record, the billing settle, and the
/// assistant message appended to the thread.
async fn execute_turn(
    state: &AppState,
    user: &legal_rag_auth::User,
    prep: Prepared,
    sink: Option<legal_rag_rag::ToolEventSink>,
) -> Result<ChatTurnResponse, ApiError> {
    let hooks = legal_rag_rag::ToolRunHooks {
        sink,
        edit_proposer: Some(Arc::new(ChatEditProposer {
            state: state.clone(),
            user: user.clone(),
        })),
        allow_edit: true,
        draft_generator: Some(Arc::new(ChatDraftGenerator {
            state: state.clone(),
            user: user.clone(),
            matter_id: prep.matter_id.clone(),
            terms: prep.terms.clone(),
        })),
        allow_draft: true,
        analyzer: Some(Arc::new(ChatAnalyzer {
            state: state.clone(),
            user: user.clone(),
            terms: prep.terms.clone(),
        })),
        allow_analyze: true,
        workflow_runner: Some(Arc::new(ChatWorkflowRunner {
            state: state.clone(),
            terms: prep.terms.clone(),
        })),
        allow_workflow: true,
    };
    let ctx = legal_rag_rag::ToolRunContext {
        scope_docs: prep.scope_docs.clone(),
        terms: prep.terms.clone(),
        citator: legal_rag_rag::CitatorContext::default(),
    };
    let scope = legal_rag_rag::usage::new_scope();
    let goal = prep.goal.clone();
    let result = legal_rag_rag::usage::scope_usage(scope.clone(), async {
        state.tool_agent.run_hooked(&goal, &[], ctx, prep.budget, hooks).await
    })
    .await;
    let tokens = scope.lock().map(|t| t.total()).unwrap_or(0);
    match result {
        Ok(run) => {
            let calls = run
                .trace
                .iter()
                .filter(|t| t.tool != "finish" && t.tool != "model")
                .count();
            let record = state.runs.record_with_trace(
                &user.email,
                "autonomous_tools",
                "Agent chat",
                &prep.user_message,
                &crate::runs::status_from_stopped(&run.stopped),
                calls,
                tokens,
                &run.final_report,
                None,
                run.trace.clone(),
            );
            let edit_ids: Vec<String> = run.proposals.iter().map(|p| p.edit_id.clone()).collect();
            let message = ChatMessage::assistant(
                &run.final_report,
                Some(record.id.clone()),
                tokens,
                &run.stopped,
                run.trace.clone(),
                run.citations.clone(),
                edit_ids.clone(),
            );
            let response = ChatTurnResponse {
                thread_id: prep.thread_id.clone(),
                message: message.clone(),
                run_id: record.id.clone(),
                tokens,
                stopped: run.stopped.clone(),
                citations: run.citations.clone(),
                proposals: run.proposals.clone(),
                drafts: run.drafts.clone(),
            };
            crate::routes::billing::settle_with_payload(
                state,
                user,
                &scope,
                "autonomous",
                &prep.user_message,
                "Agent chat",
                &run.final_report,
                run.citations.len(),
                Some(serde_json::json!({ "thread_id": prep.thread_id, "run": run })),
            );
            state.agent_chats.append(&user.email, &prep.thread_id, message);
            Ok(response)
        }
        Err(e) => {
            state.runs.record(
                &user.email,
                "autonomous_tools",
                "Agent chat",
                &prep.user_message,
                "failed",
                0,
                tokens,
                "",
                Some(e.to_string()),
            );
            // The failure is labelled in the thread, not a bare gap.
            state.agent_chats.append(
                &user.email,
                &prep.thread_id,
                ChatMessage::assistant(
                    &format!("The agent run failed: {e}"),
                    None,
                    tokens,
                    "failed",
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                ),
            );
            Err(e.into())
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/v1/agent/chat — the blocking fallback: one full turn,
/// no streaming (the same turn the stream runs).
pub async fn chat_blocking(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<ChatRequest>,
) -> Result<Json<ChatTurnResponse>, ApiError> {
    let prep = prepare_turn(&state, &user, &body)?;
    let response = execute_turn(&state, &user, prep, None).await?;
    Ok(Json(response))
}

/// POST /api/v1/agent/chat/stream — one turn over SSE: status →
/// tool_call / tool_result / subagent / approval_required → sources
/// → delta… → done (or a labelled error).
pub async fn chat_stream(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<ChatRequest>,
) -> Result<
    axum::response::Sse<
        impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
    >,
    ApiError,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};

    let prep = prepare_turn(&state, &user, &body)?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<AgentChatEvent>();
    let sink_tx = tx.clone();
    let sink: legal_rag_rag::ToolEventSink = Arc::new(move |ev| {
        if let Some(mapped) = map_loop_event(ev) {
            let _ = sink_tx.send(mapped);
        }
    });
    tokio::spawn(async move {
        let _ = tx.send(AgentChatEvent::Status {
            message: "Agent is working on your message…".to_string(),
        });
        match execute_turn(&state, &user, prep, Some(sink)).await {
            Ok(response) => {
                let _ = tx.send(AgentChatEvent::Done {
                    thread_id: response.thread_id,
                    run_id: response.run_id,
                    tokens: response.tokens,
                    stopped: response.stopped,
                    edit_ids: response.message.edit_ids.clone(),
                });
            }
            Err(e) => {
                let _ = tx.send(AgentChatEvent::Error {
                    message: e.0.to_string(),
                    partial: true,
                });
            }
        }
    });

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Some(ev) => {
                let (name, data) = ev.to_sse();
                Some((Ok(Event::default().event(name).data(data)), rx))
            }
            None => None,
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// GET /api/v1/agent/chat/threads — the caller's chat threads,
/// most recently updated first.
pub async fn list_threads(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Json<Vec<crate::agent_chat::ChatThreadSummary>>, ApiError> {
    Ok(Json(state.agent_chats.list_for(&user.email, 100)))
}

/// GET /api/v1/agent/chat/threads/{id} — one thread in full,
/// owner-checked. Edit outcomes resolve against the live edit
/// records (Piece 85 list/get endpoints), keyed by the messages'
/// edit ids.
pub async fn get_thread(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<ChatThread>, ApiError> {
    state
        .agent_chats
        .get(&user.email, &id)
        .map(Json)
        .ok_or_else(|| not_found("chat thread not found"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff() -> legal_rag_rag::PassageDiff {
        legal_rag_rag::compute_diff("within fifteen days", "within three months")
    }

    #[test]
    fn loop_events_map_to_chat_events() {
        // Plain tool activity stays tool_call / tool_result.
        let ev = map_loop_event(legal_rag_rag::ToolLoopEvent::ToolCall {
            n: 1,
            tool: "vault_search".to_string(),
            args_digest: "tenancy deposit".to_string(),
        })
        .unwrap();
        assert!(matches!(ev, AgentChatEvent::ToolCall { ref tool, .. } if tool == "vault_search"));
        let (name, data) = ev.to_sse();
        assert_eq!(name, "tool_call");
        assert!(data.contains("\"type\":\"tool_call\""), "{data}");
        assert!(data.contains("\"args_digest\":\"tenancy deposit\""), "{data}");

        // A spawn becomes a subagent event, spawn then completion.
        let ev = map_loop_event(legal_rag_rag::ToolLoopEvent::ToolCall {
            n: 2,
            tool: "spawn_subagent".to_string(),
            args_digest: "research the deposit rules".to_string(),
        })
        .unwrap();
        let (name, data) = ev.to_sse();
        assert_eq!(name, "subagent");
        assert!(data.contains("\"phase\":\"spawned\""), "{data}");
        let ev = map_loop_event(legal_rag_rag::ToolLoopEvent::ToolResult {
            n: 2,
            tool: "spawn_subagent".to_string(),
            status: "ok".to_string(),
            summary: "done".to_string(),
            result_chars: 10,
            tokens_estimate: 42,
            duration_ms: 3,
            child: Some(legal_rag_rag::ToolChildSummary {
                goal: "research the deposit rules".to_string(),
                stopped: "finished".to_string(),
                steps: 2,
                tokens_estimate: 42,
            }),
        })
        .unwrap();
        let (name, data) = ev.to_sse();
        assert_eq!(name, "subagent");
        assert!(data.contains("\"phase\":\"completed\""), "{data}");
        assert!(data.contains("\"stopped\":\"finished\""), "{data}");
    }

    #[test]
    fn edit_proposals_become_approval_required_with_the_full_diff() {
        let ev = map_loop_event(legal_rag_rag::ToolLoopEvent::EditProposed {
            proposal: legal_rag_rag::EditProposalInfo {
                edit_id: "edit-9".to_string(),
                doc_id: "doc-1".to_string(),
                doc_title: "Tenancy note".to_string(),
                instruction: "make it three months".to_string(),
                diff: diff(),
            },
        })
        .unwrap();
        let (name, data) = ev.to_sse();
        assert_eq!(name, "approval_required");
        let v: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(v["type"], "approval_required");
        assert_eq!(v["edit_id"], "edit-9");
        assert_eq!(v["diff"]["added"][0], "within three months");
        assert_eq!(v["diff"]["removed"][0], "within fifteen days");
    }

    #[test]
    fn chat_request_accepts_refine_terms_and_defaults_without_them() {
        // Piece 87: the unified composer threads Piece 13 refine
        // terms into the turn; older clients send none.
        let body: ChatRequest =
            serde_json::from_str(r#"{"message":"hi","refine_terms":["rent","deposit"]}"#).unwrap();
        assert_eq!(
            body.refine_terms,
            Some(vec!["rent".to_string(), "deposit".to_string()])
        );
        let body: ChatRequest = serde_json::from_str(r#"{"message":"hi"}"#).unwrap();
        assert!(body.refine_terms.is_none());
        assert!(body.doc_ids.is_empty());
    }

    #[test]
    fn done_and_error_payloads_are_labelled() {
        let (name, data) = AgentChatEvent::Done {
            thread_id: "t-1".to_string(),
            run_id: "r-1".to_string(),
            tokens: 99,
            stopped: "finished".to_string(),
            edit_ids: vec!["edit-9".to_string()],
        }
        .to_sse();
        assert_eq!(name, "done");
        assert!(data.contains("\"run_id\":\"r-1\""), "{data}");
        assert!(data.contains("\"edit_ids\":[\"edit-9\"]"), "{data}");
        let (name, data) = AgentChatEvent::Error { message: "provider down".to_string(), partial: true }.to_sse();
        assert_eq!(name, "error");
        assert!(data.contains("\"partial\":true"), "{data}");
    }
}
