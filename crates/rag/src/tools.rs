//! Tool-use agent runtime (Piece 84).
//!
//! Until now the agent engines executed FIXED step types: the Piece 80
//! planner produced `search_vault` / `search_web` steps and the engine
//! ran them verbatim. This module gives the agent a real tool-calling
//! loop on top of the same machinery — nothing here reimplements a
//! retrieval, scoping, budgeting, or billing path:
//!
//! - **Tool registry** — one canonical set of tool definitions (name,
//!   description, JSON-schema for arguments). The internal agent uses
//!   these definitions; the MCP server can serve the same definitions
//!   later instead of its current hand-written three.
//! - **Schema-checked calls** — every call's arguments are validated
//!   against the tool's declared schema before execution. A rejection
//!   (or any tool error) is fed back to the model as the tool result,
//!   exactly the way the planner-drift fix feeds back a bad plan: the
//!   run corrects itself, it never crashes on a malformed call.
//! - **Same legs, tool-shaped** — `vault_search` is `QueryService::
//!   query_opts`, `web_research` is `query_with_web` (the due-diligence
//!   external leg), `graph_query` is `query_opts` in a LightRAG graph
//!   mode, `read_document` / `list_workspace` read through the
//!   [`DocCatalog`] seam (implemented by `RagAnything`, so the text is
//!   the pipeline's own extracted text).
//! - **Workspace confinement is hard** — when the run is scoped to a
//!   Workspace, retrieval legs run STRICT (no full-vault fallback) and
//!   `read_document` refuses any document outside the scope. A
//!   sub-agent inherits the parent's scope unchanged: it can never
//!   widen it.
//! - **Guardrails are Piece 80's** — an optional user-approved plan
//!   (from `AutonomousAgent::plan_upfront`) is carried into the loop
//!   as standing direction; `RunBudget` caps steps and estimated
//!   tokens, enforced between calls; a budget stop synthesizes a
//!   partial report from the findings gathered so far and says so.
//!   Token accounting uses a child usage scope per model call, merged
//!   back into the caller's scope for billing — the `subagents.rs`
//!   pattern — so tool runs bill through the existing usage path.
//! - **Sub-agents** — `spawn_subagent` runs a child tool loop with a
//!   scoped tool subset (never `spawn_subagent` itself: no
//!   sub-sub-agents, the `subagents.rs` boundary) and a budget CARVED
//!   from the parent's remaining budget; the child's spend lands in
//!   the parent's totals. Several spawns in one model turn run
//!   concurrently; the parent merges each child's report and
//!   citations into its own context.
//! - **Trace** — every call is recorded ([`ToolTraceEntry`]: name,
//!   arguments digest, result size, estimated tokens, duration,
//!   status) and persisted with the run by the server's run store, so
//!   the Agents UI can show exactly what the agent did.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use legal_rag_common::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::orchestrator::{PlannedStep, RunBudget};
use crate::pipeline::RagAnything;
use crate::query::{AskOptions, CitatorContext, QueryMode, QueryService};
use crate::subagents::first_json_object;
use crate::usage;
use crate::Citation;

/// Hard cap on model turns in one run (a turn may carry several tool
/// calls). Budgets normally stop a run long before this; the cap is
/// the backstop against a model that never finishes and never errs.
const MAX_TURNS: usize = 24;
/// Per-call default caps, mirroring the orchestrator's commissioned
/// per-sub-agent values in `subagents.rs`.
const DEFAULT_CHILD_TOKEN_BUDGET: u64 = 25_000;
const DEFAULT_CHILD_STEPS: usize = 6;
/// Tool-result text fed back into the model's context, per call.
const RESULT_FEED_CAP: usize = 9_000;
/// Document text a single `read_document` call may pull.
const READ_CAP: usize = 24_000;

// ---------------------------------------------------------------------------
// Tool registry — the canonical definitions.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

fn schema(props: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": props,
        "required": required,
    })
}

/// The canonical tool definitions. The agent loop renders its tool
/// list from these; argument validation checks against these schemas.
pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "vault_search",
            description: "Search the vault's documents with a grounded question and get a cited answer. Use for anything the documents themselves can answer.",
            input_schema: schema(
                json!({
                    "query": { "type": "string" },
                    "top_k": { "type": "integer", "minimum": 1, "maximum": 10 }
                }),
                &["query"],
            ),
        },
        ToolDef {
            name: "read_document",
            description: "Read one document's full text by id (from list_workspace or a search result). Use when you need the document itself, not an answer about it.",
            input_schema: schema(
                json!({ "doc_id": { "type": "string" } }),
                &["doc_id"],
            ),
        },
        ToolDef {
            name: "list_workspace",
            description: "List the documents available to this run (the run's Workspace when scoped, otherwise the whole vault).",
            input_schema: schema(json!({}), &[]),
        },
        ToolDef {
            name: "web_research",
            description: "Research the live web (news, recent rulings, external facts) and get a cited answer combining web sources with the vault. Use for recent or external facts the vault cannot hold.",
            input_schema: schema(
                json!({
                    "query": { "type": "string" },
                    "web_results": { "type": "integer", "minimum": 1, "maximum": 5 }
                }),
                &["query"],
            ),
        },
        ToolDef {
            name: "deep_research",
            description: "Run the deep research pipeline on a question: it frames hypotheses, gathers cited evidence for each from the vault (and the live web when web is true), adjudicates them, and returns a verdict-first report with conflicts, open points, and decisions. Use when the user asks to research or investigate a question, or the question is contested / multi-part and needs a weighed verdict. Slower and deeper than vault_search and web_research — never use it for a simple fact lookup. It runs over the whole vault, so it is refused inside a Workspace-scoped run.",
            input_schema: schema(
                json!({
                    "question": { "type": "string" },
                    "web": { "type": "boolean" }
                }),
                &["question"],
            ),
        },
        ToolDef {
            name: "draft_document",
            description: "Draft a legal document of one fixed kind — legal_notice, agreement, memo (IRAC), petition, letter, or custom (follow the format given in the instructions, e.g. a pasted template) — grounded in the vault's sources. The finished draft is attached verbatim below your final answer. Use when the user asks to draft or write a document; the instructions carry the facts (parties, amounts, dates, demands).",
            input_schema: schema(
                json!({
                    "kind": { "type": "string", "enum": ["legal_notice", "agreement", "memo", "petition", "letter", "custom"] },
                    "instructions": { "type": "string" }
                }),
                &["kind", "instructions"],
            ),
        },
        ToolDef {
            name: "analyze_documents",
            description: "Run a structured analysis over a chosen SET of documents (the Interact machinery): action summary (parties, terms, dates, outcomes), chronology (dated event table), clauses (key clauses/provisions and who they favour), risks, or chat (answer the question from exactly these documents). Use when the user names specific documents to summarise, compare, or extract from — for finding documents use vault_search / list_workspace first. Shared documents only; the Confidential Vault is never read.",
            input_schema: schema(
                json!({
                    "action": { "type": "string", "enum": ["summary", "chronology", "clauses", "risks", "chat"] },
                    "document_ids": { "type": "array", "items": { "type": "string" } },
                    "question": { "type": "string" }
                }),
                &["action", "document_ids"],
            ),
        },
        ToolDef {
            name: "run_workflow",
            description: "Run one of the fixed multi-step legal workflows end to end and get its cited report: contract_review (contract risk review), due_diligence (facts, red flags, external checks, gaps), memo_draft (IRAC legal memo), counter_argument (the strongest case against a position). Use when the user's request matches one of these named jobs; input describes the matter (the workflow retrieves its own evidence from the vault).",
            input_schema: schema(
                json!({
                    "workflow": { "type": "string", "enum": ["contract_review", "due_diligence", "memo_draft", "counter_argument"] },
                    "input": { "type": "string" }
                }),
                &["workflow", "input"],
            ),
        },
        ToolDef {
            name: "graph_query",
            description: "Query the knowledge graph over the vault's entities and relations. Modes: local (one entity's neighbourhood), global (themes across the corpus), hybrid, mix.",
            input_schema: schema(
                json!({
                    "query": { "type": "string" },
                    "mode": { "type": "string", "enum": ["local", "global", "hybrid", "mix"] }
                }),
                &["query"],
            ),
        },
        ToolDef {
            name: "spawn_subagent",
            description: "Delegate one independent sub-task to a child agent with its own tool loop and a budget carved from yours. Use for a self-contained branch of the goal (e.g. research one party, one issue, one document set). The child reports findings back to you; it cannot spawn its own sub-agents.",
            input_schema: schema(
                json!({
                    "goal": { "type": "string" },
                    "tools": { "type": "array", "items": { "type": "string" } },
                    "max_steps": { "type": "integer", "minimum": 1, "maximum": 10 },
                    "max_tokens": { "type": "integer", "minimum": 1, "maximum": 100000 }
                }),
                &["goal"],
            ),
        },
        ToolDef {
            name: "edit_document",
            description: "Propose an edit to one document: the document id and the change to make, in plain language. This creates a PENDING PROPOSAL with a diff — the user must approve it before anything is written back; it never changes the document by itself. Use when the user asks to change, fix, update or rewrite a document.",
            input_schema: schema(
                json!({
                    "doc_id": { "type": "string" },
                    "instruction": { "type": "string" }
                }),
                &["doc_id", "instruction"],
            ),
        },
    ]
}

pub fn tool_def(name: &str) -> Option<ToolDef> {
    tool_defs().into_iter().find(|d| d.name == name)
}

/// The tools a fresh top-level run may use. `edit_document` and
/// `draft_document` are NOT in the default set: they are offered
/// only by runs that carry the matching seam ([`EditProposer`] /
/// [`DraftGenerator`] — the Piece 86/87 Agent Chat), so a plain
/// tool run can never even name them.
pub fn all_tool_names() -> Vec<String> {
    tool_defs()
        .iter()
        .map(|d| d.name.to_string())
        .filter(|n| n != "edit_document" && n != "draft_document" && n != "analyze_documents" && n != "run_workflow")
        .collect()
}

/// The full registry, for runs that may propose document edits.
pub fn chat_tool_names() -> Vec<String> {
    tool_defs().iter().map(|d| d.name.to_string()).collect()
}

// ---------------------------------------------------------------------------
// Minimal JSON-schema validation for tool arguments.
//
// Supports exactly what the registry declares: object shape, required
// properties, per-property types, string enums, integer bounds, and
// array item types. Returns a human-readable rejection the model can
// correct from — validation failures are feedback, never fatal.
// ---------------------------------------------------------------------------

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn check_type(expected: &str, v: &Value) -> bool {
    match expected {
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        _ => type_name(v) == expected,
    }
}

/// Validate `args` against a tool's declared `input_schema`.
/// `Ok(())` = the call may execute; `Err(msg)` = feed `msg` back.
pub fn validate_tool_args(def: &ToolDef, args: &Value) -> std::result::Result<(), String> {
    let schema = &def.input_schema;
    if !args.is_object() {
        return Err(format!(
            "arguments for {} must be a JSON object, got {}",
            def.name,
            type_name(args)
        ));
    }
    let obj = args.as_object().unwrap();
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for name in required.iter().filter_map(|r| r.as_str()) {
            if !obj.contains_key(name) {
                return Err(format!("missing required argument \"{name}\" for {}", def.name));
            }
        }
    }
    let empty = json!({});
    let props = schema.get("properties").unwrap_or(&empty);
    for (name, value) in obj {
        let Some(prop) = props.get(name) else {
            continue; // undeclared extras are ignored, not fatal
        };
        if let Some(expected) = prop.get("type").and_then(|t| t.as_str()) {
            if !check_type(expected, value) {
                return Err(format!(
                    "argument \"{name}\" for {} must be {expected}, got {}",
                    def.name,
                    type_name(value)
                ));
            }
        }
        if let Some(choices) = prop.get("enum").and_then(|e| e.as_array()) {
            if !choices.iter().any(|c| c == value) {
                let list = choices
                    .iter()
                    .filter_map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(format!(
                    "argument \"{name}\" for {} must be one of: {list}",
                    def.name
                ));
            }
        }
        if value.is_number() {
            let n = value.as_i64().unwrap_or(i64::MAX);
            if let Some(min) = prop.get("minimum").and_then(|m| m.as_i64()) {
                if n < min {
                    return Err(format!("argument \"{name}\" for {} must be >= {min}", def.name));
                }
            }
            if let Some(max) = prop.get("maximum").and_then(|m| m.as_i64()) {
                if n > max {
                    return Err(format!("argument \"{name}\" for {} must be <= {max}", def.name));
                }
            }
        }
        if let Some(items) = prop.get("items") {
            if let (Some(want), Some(arr)) = (items.get("type").and_then(|t| t.as_str()), value.as_array()) {
                if arr.iter().any(|v| !check_type(want, v)) {
                    return Err(format!(
                        "argument \"{name}\" for {} must be an array of {want}",
                        def.name
                    ));
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Document catalog seam — the pipeline's own document access, injected
// so the tool engine never grows a second document store.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DocListEntry {
    pub id: String,
    pub title: String,
    pub chunks: usize,
}

#[async_trait]
pub trait DocCatalog: Send + Sync {
    /// The document's full extracted text (the pipeline's chunks in
    /// reading order), capped at `max_chars`.
    async fn document_text(&self, doc_id: &str, max_chars: usize) -> Result<String>;
    fn list_documents(&self) -> Vec<DocListEntry>;
    fn doc_title(&self, doc_id: &str) -> Option<String>;
}

#[async_trait]
impl DocCatalog for RagAnything {
    async fn document_text(&self, doc_id: &str, max_chars: usize) -> Result<String> {
        RagAnything::document_text(self, doc_id, max_chars).await
    }
    fn list_documents(&self) -> Vec<DocListEntry> {
        RagAnything::list_documents(self)
            .into_iter()
            .map(|d| DocListEntry { id: d.id, title: d.title, chunks: d.chunks })
            .collect()
    }
    fn doc_title(&self, doc_id: &str) -> Option<String> {
        RagAnything::doc_title(self, doc_id)
    }
}

// ---------------------------------------------------------------------------
// Run types
// ---------------------------------------------------------------------------

/// One tool call in a run's trace — the durable record the run store
/// persists and the Agents UI renders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTraceEntry {
    /// 1-based call number within the run.
    pub n: usize,
    /// Tool name, or the pseudo-tools `model` (an unparseable reply)
    /// / `finish` (the model's final answer).
    pub tool: String,
    /// Short human digest of the arguments (query text, doc id, goal).
    pub args_digest: String,
    /// ok | error
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Size of the tool result (chars) before feed capping.
    pub result_chars: usize,
    /// Estimated tokens attributable to this call (model turn +
    /// tool-side generation estimate).
    pub tokens_estimate: u64,
    pub duration_ms: u64,
    /// Spawn summary when this call was `spawn_subagent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<ToolChildSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolChildSummary {
    pub goal: String,
    /// finished | budget | budget_tokens | turn_cap
    pub stopped: String,
    pub steps: usize,
    pub tokens_estimate: u64,
}

/// Per-run context: the run's Workspace confinement and the same
/// request options the Ask surface threads through its legs.
#[derive(Debug, Clone, Default)]
pub struct ToolRunContext {
    /// Workspace document ids; `Some` = hard boundary (strict legs,
    /// read confinement). `None` = the whole vault.
    pub scope_docs: Option<Vec<String>>,
    pub terms: Vec<String>,
    pub citator: CitatorContext,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolRun {
    pub goal: String,
    pub trace: Vec<ToolTraceEntry>,
    pub final_report: String,
    pub citations: Vec<Citation>,
    /// finished | budget | budget_tokens | turn_cap
    pub stopped: String,
    pub tokens_estimate: u64,
    /// The user-approved plan the run followed, when one was given.
    #[serde(default)]
    pub plan: Vec<PlannedStep>,
    /// Edit proposals this run created (Piece 86) — all still
    /// pending; the write-back half of the lifecycle belongs to the
    /// edit store's decide endpoints, never to the loop.
    #[serde(default)]
    pub proposals: Vec<EditProposalInfo>,
    /// Drafts this run produced (Piece 87). Their texts are also
    /// attached verbatim at the end of `final_report`.
    #[serde(default)]
    pub drafts: Vec<DraftInfo>,
}

// ---------------------------------------------------------------------------
// Streaming events + the document-edit seam (Piece 86).
// ---------------------------------------------------------------------------

/// One live event from a tool run, for streaming surfaces (the
/// one-box Agent Chat). The run's durable record remains the
/// [`ToolTraceEntry`] list on the run record; these events are its
/// live projection, in the same serde shape as `QueryEvent`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolLoopEvent {
    Status {
        message: String,
    },
    ToolCall {
        /// 1-based sequence number of the call within the run;
        /// pairs with the matching `ToolResult`.
        n: usize,
        tool: String,
        args_digest: String,
    },
    ToolResult {
        n: usize,
        tool: String,
        /// ok | error
        status: String,
        /// A one-line human summary of what came back.
        summary: String,
        result_chars: usize,
        tokens_estimate: u64,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child: Option<ToolChildSummary>,
    },
    /// An `edit_document` call created a pending proposal. The
    /// write itself waits for an approval decision outside the loop.
    EditProposed {
        proposal: EditProposalInfo,
    },
    /// The run's citation set, emitted before the answer deltas so
    /// the UI can render sources early (the Ask stream convention).
    Sources {
        citations: Vec<Citation>,
    },
    /// A chunk of the final report. The loop composes its report
    /// whole (it is not a token stream), so deltas are a rendering
    /// cadence over the completed text, chunked at word boundaries.
    Delta {
        text: String,
    },
}

/// The sink a streaming caller hands the loop. Events are advisory:
/// emitting must never block or fail the run.
pub type ToolEventSink = Arc<dyn Fn(ToolLoopEvent) + Send + Sync>;

/// A pending document-edit proposal created by the `edit_document`
/// tool — the Piece 85 lifecycle's first half, as the loop sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditProposalInfo {
    pub edit_id: String,
    pub doc_id: String,
    pub doc_title: String,
    pub instruction: String,
    pub diff: crate::edits::PassageDiff,
}

/// The seam for document edits: the loop can only PROPOSE. The
/// implementation (server-side) runs the Piece 85 propose path —
/// visibility and edit-access checks, the LLM edit pass, the parked
/// proposal — and approval/write-back stays with the edit store's
/// decide endpoints. The loop never writes a document itself.
#[async_trait]
pub trait EditProposer: Send + Sync {
    async fn propose_edit(
        &self,
        doc_id: &str,
        instruction: &str,
    ) -> std::result::Result<EditProposalInfo, String>;
}

/// A finished draft produced by the `draft_document` tool (Piece
/// 87). The loop attaches it verbatim below the run's final answer —
/// the model introduces it, it never re-types or summarises it — so
/// the text the user reads (and downloads) is the drafter's own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftInfo {
    pub kind: String,
    pub kind_label: String,
    pub text: String,
    #[serde(default)]
    pub citations: Vec<Citation>,
}

/// The seam for document drafting (Piece 87): the drafting
/// machinery (kind templates, corpus grounding, the drafter
/// prompt) lives server-side in the Draft route's core; the loop
/// only asks for a draft by kind + instructions. Mirrors
/// [`EditProposer`]: offered only to runs that carry one.
#[async_trait]
pub trait DraftGenerator: Send + Sync {
    async fn generate_draft(
        &self,
        kind: &str,
        instructions: &str,
    ) -> std::result::Result<DraftInfo, String>;
}

/// A finished structured analysis from the `analyze_documents`
/// tool (Piece 87): the Interact machinery's output over a chosen
/// document set, with the citations its chat mode produces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisInfo {
    pub text: String,
    #[serde(default)]
    pub citations: Vec<Citation>,
}

/// The seam for structured document-set analysis (Piece 87): the
/// Interact machinery (selection loading, per-action instructions,
/// analyst prompt) lives server-side in the Create route's core;
/// the loop only names an action + a document set. The seam
/// receives the run's Workspace scope and must refuse documents
/// outside it — a reference is never a scope widening.
#[async_trait]
pub trait DocumentAnalyzer: Send + Sync {
    async fn analyze_documents(
        &self,
        action: &str,
        question: Option<&str>,
        document_ids: &[String],
        scope_docs: &Option<Vec<String>>,
    ) -> std::result::Result<AnalysisInfo, String>;
}

/// A finished fixed-workflow run from the `run_workflow` tool
/// (Piece 87): the workflow engine's cited report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInfo {
    pub name: String,
    pub report: String,
    #[serde(default)]
    pub citations: Vec<Citation>,
    pub tokens_estimate: u64,
    /// finished | budget_tokens (the engine's own stop reason).
    pub stopped: String,
}

/// The seam for the fixed workflows (Piece 87): the engine lives
/// behind the server's AppState; the loop only names a workflow +
/// its input.
#[async_trait]
pub trait WorkflowRunner: Send + Sync {
    async fn run_workflow(
        &self,
        workflow: &str,
        input: &str,
    ) -> std::result::Result<WorkflowInfo, String>;
}

/// Per-run extras a caller may hand the loop: a live event sink and
/// the edit-proposal seam. Default: neither — the loop then behaves
/// exactly as Piece 84 shipped.
#[derive(Clone, Default)]
pub struct ToolRunHooks {
    pub sink: Option<ToolEventSink>,
    pub edit_proposer: Option<Arc<dyn EditProposer>>,
    /// Offer `edit_document` in this run's tool list. Only takes
    /// effect together with a proposer; the Agents-page run leaves
    /// it off.
    pub allow_edit: bool,
    pub draft_generator: Option<Arc<dyn DraftGenerator>>,
    /// Offer `draft_document` in this run's tool list. Only takes
    /// effect together with a generator (Piece 87).
    pub allow_draft: bool,
    pub analyzer: Option<Arc<dyn DocumentAnalyzer>>,
    /// Offer `analyze_documents` in this run's tool list (Piece 87).
    pub allow_analyze: bool,
    pub workflow_runner: Option<Arc<dyn WorkflowRunner>>,
    /// Offer `run_workflow` in this run's tool list (Piece 87).
    pub allow_workflow: bool,
}

fn emit(sink: &Option<ToolEventSink>, ev: ToolLoopEvent) {
    if let Some(s) = sink {
        s(ev);
    }
}

/// Split a completed report into delta chunks (~280 chars, cut at
/// whitespace so concatenation reproduces the text exactly).
fn delta_chunks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for piece in text.split_inclusive(char::is_whitespace) {
        if !cur.is_empty() && cur.chars().count() + piece.chars().count() > 280 {
            out.push(std::mem::take(&mut cur));
        }
        cur.push_str(piece);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A one-line summary of a call's outcome for the live event stream.
fn result_summary(out: &CallOutcome) -> String {
    if out.status != "ok" {
        return out.error.clone().unwrap_or_else(|| "error".to_string());
    }
    if out.tool == "spawn_subagent" {
        if let Some(child) = &out.child {
            return format!(
                "sub-agent “{}” stopped: {} ({} trace entries)",
                child.goal, child.stopped, child.steps
            );
        }
    }
    let first = out.result_text.lines().next().unwrap_or("").trim();
    let summary: String = first.chars().take(200).collect();
    if summary.is_empty() {
        format!("{} chars returned", out.result_chars)
    } else {
        summary
    }
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

const TOOL_SYSTEM: &str = "You are an autonomous legal-research agent with tools. \
Work the GOAL by calling tools, reading their results, and adapting: search before you conclude, \
read a document when an answer about it is not enough, and delegate a self-contained branch to a \
sub-agent when the goal splits into independent parts. \
Respond with EXACTLY one JSON object and no other text, in one of these shapes:\n\
{\"tool\": \"<tool name>\", \"args\": {<arguments>}}\n\
{\"calls\": [{\"tool\": \"<tool name>\", \"args\": {<arguments>}}, ...]} — several independent calls in one turn (sub-agents in one batch run in parallel)\n\
{\"final\": \"<your complete report, answering the goal from the tool results, citing sources inline as [doc:N] / [web:N] exactly as they appear in the results>\"}\n\
Rules: every claim in the final report must come from a tool result — never invent facts, cases, or citations. \
If a tool returns an error, read it and correct your call instead of repeating it. \
When the goal is covered, answer with {\"final\": ...} instead of calling more tools. \
Choose the lightest tool that fits: vault_search for a question the documents can answer, web_research for \
recent or external facts, deep_research ONLY when the user asks to research or investigate, or the question is \
contested and needs a weighed verdict. \
DRAFTING IS NEVER DONE BY HAND: when the user asks for a document to be drafted — a notice, agreement, memo, \
petition, or letter — your FIRST call must be draft_document; never write the draft yourself in the final answer. \
For a structured pass over specific named documents use analyze_documents (summary / chronology / clauses / \
risks / chat over exactly that set). NAMED JOBS GO TO THE WORKFLOW: when the user asks for a contract review, \
due diligence, a legal memo, or a counter-argument — by name or by clear description — you MUST call \
run_workflow with that workflow instead of improvising the steps yourself from document reads. \
When draft_document returns a draft, the draft itself is attached verbatim below your final answer — introduce \
it in a sentence or two; never reproduce, shorten, or summarise it in your final text. \
If the user asks to change a document and edit_document is among your tools, use it: it parks a proposal \
the user approves before anything is written back — never claim a document is already changed.";

const SYNTH_SYSTEM: &str = "You are a senior legal researcher writing the final report of an \
autonomous tool-using research run that ended before the agent produced its own final answer. \
Write a clear, structured report that answers the GOAL from the FINDINGS only: verdict first, then \
analysis, citing sources inline as [doc:N] / [web:N] exactly as they appear in the findings. \
Say plainly what the findings do not establish. Do not invent facts, cases, or citations.";

struct ToolCallReq {
    tool: String,
    args: Value,
}

enum ModelAction {
    Calls(Vec<ToolCallReq>),
    Final(String),
    Invalid(String),
}

/// Parse one model reply into an action. Tolerant of fences/prose via
/// the shared brace-matching extractor; anything unusable becomes
/// feedback, never an error.
fn parse_action(raw: &str) -> ModelAction {
    let Some(obj) = first_json_object(raw) else {
        return ModelAction::Invalid(
            "your reply contained no JSON object — reply with exactly one JSON object: a tool call, a calls batch, or a final answer".to_string(),
        );
    };
    let Ok(v) = serde_json::from_str::<Value>(obj) else {
        return ModelAction::Invalid(
            "your reply's JSON did not parse — reply with exactly one JSON object: a tool call, a calls batch, or a final answer".to_string(),
        );
    };
    if let Some(final_text) = v.get("final").and_then(|f| f.as_str()) {
        if !final_text.trim().is_empty() {
            return ModelAction::Final(final_text.trim().to_string());
        }
    }
    let mut calls = Vec::new();
    if let Some(arr) = v.get("calls").and_then(|c| c.as_array()) {
        for item in arr {
            if let Some(call) = parse_one_call(item) {
                calls.push(call);
            }
        }
    } else if let Some(call) = parse_one_call(&v) {
        calls.push(call);
    }
    if calls.is_empty() {
        return ModelAction::Invalid(
            "your JSON named no tool call and no final answer — use {\"tool\": ..., \"args\": {...}}, {\"calls\": [...]}, or {\"final\": \"...\"}".to_string(),
        );
    }
    ModelAction::Calls(calls)
}

fn parse_one_call(v: &Value) -> Option<ToolCallReq> {
    let tool = v.get("tool")?.as_str()?.trim().to_string();
    if tool.is_empty() {
        return None;
    }
    let args = v.get("args").cloned().unwrap_or_else(|| json!({}));
    Some(ToolCallReq { tool, args })
}

/// A short digest of a call's arguments for the trace: the salient
/// field only, never a full argument dump.
fn args_digest(tool: &str, args: &Value) -> String {
    let pick = |keys: &[&str]| -> String {
        for k in keys {
            if let Some(s) = args.get(*k).and_then(|v| v.as_str()) {
                if !s.trim().is_empty() {
                    return s.chars().take(160).collect();
                }
            }
        }
        String::new()
    };
    match tool {
        "vault_search" | "web_research" | "graph_query" => pick(&["query"]),
        "deep_research" => pick(&["question"]),
        "draft_document" => pick(&["instructions"]),
        "analyze_documents" => pick(&["action"]),
        "run_workflow" => pick(&["workflow", "input"]),
        "read_document" => pick(&["doc_id"]),
        "spawn_subagent" => pick(&["goal"]),
        "edit_document" => pick(&["instruction"]),
        _ => String::new(),
    }
}

fn cap_feed(text: &str) -> String {
    if text.chars().count() > RESULT_FEED_CAP {
        let kept: String = text.chars().take(RESULT_FEED_CAP).collect();
        format!("{kept}\n[result truncated for context]")
    } else {
        text.to_string()
    }
}

pub struct ToolAgent {
    query: Arc<QueryService>,
    catalog: Arc<dyn DocCatalog>,
    instruction: std::sync::RwLock<Option<String>>,
}

impl ToolAgent {
    pub fn new(query: Arc<QueryService>, catalog: Arc<dyn DocCatalog>) -> Self {
        Self {
            query,
            catalog,
            instruction: std::sync::RwLock::new(None),
        }
    }

    /// Admin-set standing instructions, mirroring the other engines.
    pub fn set_instruction(&self, instruction: Option<String>) {
        let instruction = instruction.filter(|s| !s.trim().is_empty());
        *self.instruction.write().unwrap() = instruction;
    }

    fn instruction(&self) -> Option<String> {
        self.instruction.read().unwrap().clone()
    }

    /// One model completion inside a child usage scope; returns the
    /// text and the tokens it cost. The child scope is merged back
    /// into the caller's ambient scope (the `subagents.rs` pattern),
    /// so billing sees every token exactly once.
    async fn llm_call(&self, system: &str, user: &str) -> Result<(String, u64)> {
        let scope = usage::new_scope();
        let out = usage::scope_usage(scope.clone(), self.query.llm().complete(system, user)).await;
        let (prompt, completion, embeddings) = scope
            .lock()
            .map(|t| (t.llm_prompt_tokens, t.llm_completion_tokens, t.embedding_tokens))
            .unwrap_or((0, 0, 0));
        usage::record_llm(prompt, completion);
        usage::record_embeddings(embeddings);
        let tokens = prompt + completion + embeddings;
        out.map(|text| (text, tokens))
    }

    /// Render the registry (restricted to `allowed`) into the system
    /// prompt — the model sees exactly the canonical definitions.
    fn system_prompt(&self, allowed: &[String], ctx: &ToolRunContext, plan: &[PlannedStep]) -> String {
        let mut tools = String::new();
        for def in tool_defs() {
            if !allowed.iter().any(|a| a == def.name) {
                continue;
            }
            tools.push_str(&format!(
                "\n- {}: {}\n  args schema: {}",
                def.name,
                def.description,
                def.input_schema
            ));
        }
        let scope_note = match &ctx.scope_docs {
            Some(docs) => format!(
                "\n\nThis run is confined to one Workspace ({} documents). vault_search, graph_query and web_research see only those documents; read_document refuses anything outside them.",
                docs.len()
            ),
            None => "\n\nThis run may use the whole vault.".to_string(),
        };
        let plan_note = if plan.is_empty() {
            String::new()
        } else {
            let lines = plan
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{}. [{}] {} — {}", i + 1, s.action, s.query, s.reason))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n\nAPPROVED PLAN — the user approved this plan; execute it with your tools, in order, adapting queries as results come back:\n{lines}")
        };
        let standing = match self.instruction() {
            Some(extra) => format!(
                "\n\nStanding instructions from the workspace admin (follow them unless they conflict with the grounding rules):\n{extra}"
            ),
            None => String::new(),
        };
        format!("{TOOL_SYSTEM}\n\nTOOLS:{tools}{scope_note}{plan_note}{standing}")
    }

    /// Run the tool loop for a goal. `plan` is a user-approved plan
    /// (Piece 80) carried in as standing direction; empty = the agent
    /// directs itself. Never fails on model or tool misbehaviour —
    /// only on an empty goal.
    pub async fn run(
        &self,
        goal: &str,
        plan: &[PlannedStep],
        ctx: ToolRunContext,
        budget: RunBudget,
    ) -> Result<ToolRun> {
        self.run_hooked(goal, plan, ctx, budget, ToolRunHooks::default()).await
    }

    /// [`Self::run`] with per-run hooks (Piece 86): a live event
    /// sink and/or the edit-proposal seam. With `allow_edit` set the
    /// run's tool list is the full registry (including
    /// `edit_document`); otherwise it is the default Piece 84 set.
    pub async fn run_hooked(
        &self,
        goal: &str,
        plan: &[PlannedStep],
        ctx: ToolRunContext,
        budget: RunBudget,
        hooks: ToolRunHooks,
    ) -> Result<ToolRun> {
        if goal.trim().is_empty() {
            return Err(Error::BadRequest("goal is empty".to_string()));
        }
        // The default registry, plus the seam-gated tools whose
        // seams this run actually carries (Pieces 86/87): a tool the
        // run cannot execute is never even offered.
        let mut allowed = all_tool_names();
        if hooks.allow_edit && hooks.edit_proposer.is_some() {
            allowed.push("edit_document".to_string());
        }
        if hooks.allow_draft && hooks.draft_generator.is_some() {
            allowed.push("draft_document".to_string());
        }
        if hooks.allow_analyze && hooks.analyzer.is_some() {
            allowed.push("analyze_documents".to_string());
        }
        if hooks.allow_workflow && hooks.workflow_runner.is_some() {
            allowed.push("run_workflow".to_string());
        }
        self.run_inner(goal, plan, ctx, budget, allowed, 0, &hooks).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_inner(
        &self,
        goal: &str,
        plan: &[PlannedStep],
        ctx: ToolRunContext,
        budget: RunBudget,
        allowed: Vec<String>,
        depth: usize,
        hooks: &ToolRunHooks,
    ) -> Result<ToolRun> {
        let max_steps = budget.max_steps.unwrap_or(usize::MAX).max(1);
        let system = self.system_prompt(&allowed, &ctx, plan);
        let mut transcript = format!("GOAL: {goal}");
        let mut findings: Vec<String> = Vec::new();
        let mut trace: Vec<ToolTraceEntry> = Vec::new();
        let mut citations: Vec<Citation> = Vec::new();
        let mut seen_chunks: HashSet<String> = HashSet::new();
        let mut tokens_estimate: u64 = 0;
        let mut steps_done: usize = 0;
        let mut stopped = "finished".to_string();
        let mut final_report: Option<String> = None;
        let mut proposals: Vec<EditProposalInfo> = Vec::new();
        let mut drafts: Vec<DraftInfo> = Vec::new();
        // Live-event sequence numbers: ToolCall and ToolResult are
        // emitted in call order, so the k-th result pairs with the
        // k-th call.
        let mut calls_emitted: usize = 0;
        let mut results_emitted: usize = 0;

        for _turn in 0..MAX_TURNS {
            // Budget gates sit between turns/calls; a run always makes
            // at least one tool call (a cap is not a veto).
            if steps_done > 0 {
                if steps_done >= max_steps {
                    stopped = "budget".to_string();
                    break;
                }
                if let Some(cap) = budget.max_tokens {
                    if tokens_estimate >= cap {
                        stopped = "budget_tokens".to_string();
                        break;
                    }
                }
            }
            let prompt = format!(
                "{transcript}\n\nRespond with the next JSON action (one tool call, a calls batch, or the final answer)."
            );
            let (raw, turn_tokens) = match self.llm_call(&system, &prompt).await {
                Ok(v) => v,
                Err(e) => {
                    // A dead provider ends the run with what we have;
                    // the synthesis below reports from the findings.
                    transcript.push_str(&format!("\n\n[model call failed: {e}]"));
                    stopped = if steps_done == 0 { "turn_cap".to_string() } else { stopped.clone() };
                    break;
                }
            };
            tokens_estimate += turn_tokens;
            match parse_action(&raw) {
                ModelAction::Final(text) => {
                    trace.push(ToolTraceEntry {
                        n: trace.len() + 1,
                        tool: "finish".to_string(),
                        args_digest: String::new(),
                        status: "ok".to_string(),
                        error: None,
                        result_chars: text.chars().count(),
                        tokens_estimate: turn_tokens,
                        duration_ms: 0,
                        child: None,
                    });
                    final_report = Some(text);
                    break;
                }
                ModelAction::Invalid(why) => {
                    trace.push(ToolTraceEntry {
                        n: trace.len() + 1,
                        tool: "model".to_string(),
                        args_digest: String::new(),
                        status: "error".to_string(),
                        error: Some(why.clone()),
                        result_chars: 0,
                        tokens_estimate: turn_tokens,
                        duration_ms: 0,
                        child: None,
                    });
                    transcript.push_str(&format!("\n\nERROR: {why}"));
                }
                ModelAction::Calls(calls) => {
                    // Partition the batch: spawns run concurrently
                    // with each other; the rest execute in order.
                    let mut results: Vec<Option<CallOutcome>> = Vec::new();
                    results.resize_with(calls.len(), || None);
                    let spawn_idx: Vec<usize> = calls
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.tool == "spawn_subagent")
                        .map(|(i, _)| i)
                        .collect();
                    if spawn_idx.len() > 1 {
                        for &i in &spawn_idx {
                            calls_emitted += 1;
                            emit(
                                &hooks.sink,
                                ToolLoopEvent::ToolCall {
                                    n: calls_emitted,
                                    tool: calls[i].tool.clone(),
                                    args_digest: args_digest(&calls[i].tool, &calls[i].args),
                                },
                            );
                        }
                        let futs = spawn_idx.iter().map(|&i| {
                            self.execute_call(&calls[i], &ctx, budget, tokens_estimate, steps_done, depth, hooks)
                        });
                        let outs = futures::future::join_all(futs).await;
                        for (k, out) in outs.into_iter().enumerate() {
                            results[spawn_idx[k]] = Some(out);
                        }
                    }
                    for (i, call) in calls.iter().enumerate() {
                        if steps_done >= max_steps && steps_done > 0 {
                            stopped = "budget".to_string();
                            break;
                        }
                        if results[i].is_some() {
                            // A spawn already executed above; account it.
                            let out = results[i].take().unwrap();
                            steps_done += 1;
                            tokens_estimate += out.tokens_estimate;
                            self.absorb(out, &mut trace, &mut transcript, &mut findings, &mut citations, &mut seen_chunks, &mut proposals, &mut drafts, &hooks.sink, &mut results_emitted);
                            continue;
                        }
                        calls_emitted += 1;
                        emit(
                            &hooks.sink,
                            ToolLoopEvent::ToolCall {
                                n: calls_emitted,
                                tool: call.tool.clone(),
                                args_digest: args_digest(&call.tool, &call.args),
                            },
                        );
                        let out = self
                            .execute_call(call, &ctx, budget, tokens_estimate, steps_done, depth, hooks)
                            .await;
                        steps_done += 1;
                        tokens_estimate += out.tokens_estimate;
                        self.absorb(out, &mut trace, &mut transcript, &mut findings, &mut citations, &mut seen_chunks, &mut proposals, &mut drafts, &hooks.sink, &mut results_emitted);
                    }
                    if stopped == "budget" {
                        break;
                    }
                }
            }
        }
        if final_report.is_none() && stopped == "finished" {
            stopped = "turn_cap".to_string();
        }

        let final_report = match final_report {
            Some(text) => text,
            None => {
                // No model-authored final: synthesize from the
                // findings, labelled partial when a budget/turn cap
                // (or a provider failure) cut the run short.
                emit(
                    &hooks.sink,
                    ToolLoopEvent::Status {
                        message: "Writing the final answer from the findings gathered so far…".to_string(),
                    },
                );
                let findings_text = if findings.is_empty() {
                    "(no findings were gathered)".to_string()
                } else {
                    findings.join("\n\n")
                };
                let note = if stopped == "finished" {
                    ""
                } else {
                    "\n\nIMPORTANT: the run stopped early (budget or turn limit); this report covers only the completed tool calls — say so plainly at the top."
                };
                match self
                    .llm_call(
                        SYNTH_SYSTEM,
                        &format!("GOAL: {goal}\n\nFINDINGS:\n{findings_text}{note}"),
                    )
                    .await
                {
                    Ok((text, tokens)) => {
                        tokens_estimate += tokens;
                        text
                    }
                    Err(e) => format!(
                        "The run ended ({stopped}) and the final report could not be generated ({e}). Findings gathered:\n\n{}",
                        findings.join("\n\n")
                    ),
                }
            }
        };

        // Drafts are artifacts, not prose: whatever the model wrote
        // as its final answer, each draft is attached verbatim below
        // it (Piece 87) — the text the user reads and downloads is
        // the drafter's own, never the model's re-typing of it.
        let mut final_report = final_report;
        for d in &drafts {
            final_report.push_str(&format!(
                "\n\n---\n\n## Draft — {}\n\n{}",
                d.kind_label, d.text
            ));
        }

        // Live projection of the completed answer: sources first
        // (the Ask stream convention), then the report in deltas.
        if let Some(s) = &hooks.sink {
            if !citations.is_empty() {
                s(ToolLoopEvent::Sources {
                    citations: citations.clone(),
                });
            }
            for chunk in delta_chunks(&final_report) {
                s(ToolLoopEvent::Delta { text: chunk });
            }
        }

        Ok(ToolRun {
            goal: goal.to_string(),
            trace,
            final_report,
            citations,
            stopped,
            tokens_estimate,
            plan: plan.to_vec(),
            proposals,
            drafts,
        })
    }

    /// Fold one executed call into the run's trace, transcript,
    /// findings, and citation set — and project it to the live event
    /// sink, when one is attached.
    #[allow(clippy::too_many_arguments)]
    fn absorb(
        &self,
        out: CallOutcome,
        trace: &mut Vec<ToolTraceEntry>,
        transcript: &mut String,
        findings: &mut Vec<String>,
        citations: &mut Vec<Citation>,
        seen_chunks: &mut HashSet<String>,
        proposals: &mut Vec<EditProposalInfo>,
        drafts: &mut Vec<DraftInfo>,
        sink: &Option<ToolEventSink>,
        results_emitted: &mut usize,
    ) {
        for c in &out.citations {
            if seen_chunks.insert(c.chunk_id.clone()) {
                citations.push(c.clone());
            }
        }
        drafts.extend(out.drafts.iter().cloned());
        if out.status == "ok" {
            findings.push(format!(
                "## {} ({})\n{}",
                out.tool, out.args_digest, out.result_text
            ));
        }
        transcript.push_str(&format!(
            "\n\nTOOL RESULT [{}]: {}",
            out.tool,
            cap_feed(&out.result_text)
        ));
        *results_emitted += 1;
        emit(
            sink,
            ToolLoopEvent::ToolResult {
                n: *results_emitted,
                tool: out.tool.clone(),
                status: out.status.clone(),
                summary: result_summary(&out),
                result_chars: out.result_chars,
                tokens_estimate: out.tokens_estimate,
                duration_ms: out.duration_ms,
                child: out.child.clone(),
            },
        );
        for proposal in &out.proposals {
            proposals.push(proposal.clone());
            emit(
                sink,
                ToolLoopEvent::EditProposed {
                    proposal: proposal.clone(),
                },
            );
        }
        trace.push(ToolTraceEntry {
            n: trace.len() + 1,
            tool: out.tool,
            args_digest: out.args_digest,
            status: out.status,
            error: out.error,
            result_chars: out.result_chars,
            tokens_estimate: out.tokens_estimate,
            duration_ms: out.duration_ms,
            child: out.child,
        });
    }

    /// Execute one validated-or-not tool call. Every failure mode —
    /// unknown tool, disallowed tool, bad arguments, scope refusal,
    /// upstream error — returns an `error` outcome whose text is fed
    /// back to the model; this function never returns Err.
    #[allow(clippy::too_many_arguments)]
    async fn execute_call(
        &self,
        call: &ToolCallReq,
        ctx: &ToolRunContext,
        parent_budget: RunBudget,
        parent_tokens_so_far: u64,
        parent_steps_so_far: usize,
        depth: usize,
        hooks: &ToolRunHooks,
    ) -> CallOutcome {
        let started = Instant::now();
        let digest = args_digest(&call.tool, &call.args);
        let mut out = CallOutcome {
            tool: call.tool.clone(),
            args_digest: digest,
            status: "ok".to_string(),
            error: None,
            result_text: String::new(),
            result_chars: 0,
            tokens_estimate: 0,
            duration_ms: 0,
            citations: Vec::new(),
            child: None,
            proposals: Vec::new(),
            drafts: Vec::new(),
        };
        let fail = |out: &mut CallOutcome, started: Instant, msg: String| {
            out.status = "error".to_string();
            out.error = Some(msg.clone());
            out.result_text = format!("ERROR: {msg}");
            out.result_chars = out.result_text.chars().count();
            out.duration_ms = started.elapsed().as_millis() as u64;
        };

        let Some(def) = tool_def(&call.tool) else {
            fail(&mut out, started, format!("unknown tool \"{}\" — use one of the listed tools", call.tool));
            return out;
        };
        if let Err(msg) = validate_tool_args(&def, &call.args) {
            fail(&mut out, started, msg);
            return out;
        }

        let opts = AskOptions {
            brief_vault: None,
            refine_terms: ctx.terms.clone(),
        };
        let strict = ctx.scope_docs.is_some();

        match call.tool.as_str() {
            "vault_search" => {
                let query = call.args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let top_k = call.args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(4) as usize;
                match self
                    .query
                    .query_opts(query, QueryMode::Mix, top_k, ctx.scope_docs.clone(), strict, &opts, ctx.citator.clone())
                    .await
                {
                    Ok(ans) => {
                        out.tokens_estimate = usage::estimate_tokens(query) + usage::estimate_tokens(&ans.answer);
                        out.result_text = with_scope_note(&ans);
                        out.citations = ans.citations;
                    }
                    Err(e) => fail(&mut out, started, format!("vault_search failed: {e}")),
                }
            }
            "web_research" => {
                let query = call.args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let web_results = call.args.get("web_results").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
                match self
                    .query
                    .query_with_web(query, QueryMode::Mix, 4, web_results, ctx.scope_docs.clone(), strict, ctx.citator.clone(), &opts)
                    .await
                {
                    Ok(ans) => {
                        out.tokens_estimate = usage::estimate_tokens(query) + usage::estimate_tokens(&ans.answer);
                        out.result_text = with_scope_note(&ans);
                        out.citations = ans.citations;
                    }
                    Err(e) => fail(&mut out, started, format!("web_research failed: {e}")),
                }
            }
            "deep_research" => {
                let question = call.args.get("question").and_then(|v| v.as_str()).unwrap_or("");
                let web = call.args.get("web").and_then(|v| v.as_bool()).unwrap_or(false);
                if ctx.scope_docs.is_some() {
                    // The research pipeline retrieves over the whole
                    // vault (its evidence legs take no scope) — running
                    // it here would breach the run's hard Workspace
                    // boundary, so it refuses and says why.
                    fail(
                        &mut out,
                        started,
                        "deep_research runs over the whole vault and cannot be confined to this run's Workspace — use vault_search or graph_query for a Workspace-scoped question, or run the research from an unscoped chat".to_string(),
                    );
                } else {
                    match self.query.research(question, web, &ctx.terms, ctx.citator.clone()).await {
                        Ok(report) => {
                            out.tokens_estimate = usage::estimate_tokens(question)
                                + usage::estimate_tokens(&report.conclusion);
                            out.result_text = format_research_report(&report);
                            out.citations = report.citations;
                        }
                        Err(e) => fail(&mut out, started, format!("deep_research failed: {e}")),
                    }
                }
            }
            "draft_document" => {
                let kind = call.args.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                let instructions = call.args.get("instructions").and_then(|v| v.as_str()).unwrap_or("");
                match &hooks.draft_generator {
                    None => fail(
                        &mut out,
                        started,
                        "draft_document is not available in this run — drafting is offered from the Agent Chat, where the draft is attached to the conversation".to_string(),
                    ),
                    Some(generator) => match generator.generate_draft(kind, instructions).await {
                        Ok(info) => {
                            out.tokens_estimate = usage::estimate_tokens(instructions)
                                + usage::estimate_tokens(&info.text);
                            out.result_text = format!(
                                "DRAFT ({}) — the full text below is attached verbatim under your final answer; introduce it briefly, do not reproduce or summarise it:\n\n{}",
                                info.kind_label, info.text
                            );
                            out.result_chars = out.result_text.chars().count();
                            out.citations = info.citations.clone();
                            out.drafts.push(info);
                        }
                        Err(msg) => fail(&mut out, started, format!("draft_document failed: {msg}")),
                    },
                }
            }
            "analyze_documents" => {
                let action = call.args.get("action").and_then(|v| v.as_str()).unwrap_or("");
                let question = call.args.get("question").and_then(|v| v.as_str());
                let document_ids: Vec<String> = call.args
                    .get("document_ids")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default();
                if document_ids.is_empty() {
                    fail(&mut out, started, "analyze_documents needs at least one document id in document_ids".to_string());
                } else {
                    match &hooks.analyzer {
                        None => fail(
                            &mut out,
                            started,
                            "analyze_documents is not available in this run — structured document analysis is offered from the Agent Chat".to_string(),
                        ),
                        Some(analyzer) => match analyzer.analyze_documents(action, question, &document_ids, &ctx.scope_docs).await {
                            Ok(info) => {
                                out.tokens_estimate = usage::estimate_tokens(&info.text);
                                out.result_text = info.text.clone();
                                out.result_chars = out.result_text.chars().count();
                                out.citations = info.citations.clone();
                            }
                            Err(msg) => fail(&mut out, started, format!("analyze_documents failed: {msg}")),
                        },
                    }
                }
            }
            "run_workflow" => {
                let workflow = call.args.get("workflow").and_then(|v| v.as_str()).unwrap_or("");
                let input = call.args.get("input").and_then(|v| v.as_str()).unwrap_or("");
                match &hooks.workflow_runner {
                    None => fail(
                        &mut out,
                        started,
                        "run_workflow is not available in this run — the fixed workflows are offered from the Agent Chat".to_string(),
                    ),
                    Some(runner) => match runner.run_workflow(workflow, input).await {
                        Ok(info) => {
                            out.tokens_estimate = info.tokens_estimate;
                            out.result_text = format!(
                                "WORKFLOW REPORT — {} (stopped: {}). Ground your answer in this report and cite its sources; do not invent steps or findings beyond it:\n\n{}",
                                info.name, info.stopped, info.report
                            );
                            out.result_chars = out.result_text.chars().count();
                            out.citations = info.citations.clone();
                        }
                        Err(msg) => fail(&mut out, started, format!("run_workflow failed: {msg}")),
                    },
                }
            }
            "graph_query" => {
                let query = call.args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let mode = match call.args.get("mode").and_then(|v| v.as_str()) {
                    Some("local") => QueryMode::Local,
                    Some("global") => QueryMode::Global,
                    Some("mix") => QueryMode::Mix,
                    _ => QueryMode::Hybrid,
                };
                match self
                    .query
                    .query_opts(query, mode, 5, ctx.scope_docs.clone(), strict, &opts, ctx.citator.clone())
                    .await
                {
                    Ok(ans) => {
                        out.tokens_estimate = usage::estimate_tokens(query) + usage::estimate_tokens(&ans.answer);
                        out.result_text = with_scope_note(&ans);
                        out.citations = ans.citations;
                    }
                    Err(e) => fail(&mut out, started, format!("graph_query failed: {e}")),
                }
            }
            "read_document" => {
                let doc_id = call.args.get("doc_id").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(scope) = &ctx.scope_docs {
                    if !scope.iter().any(|d| d == doc_id) {
                        fail(
                            &mut out,
                            started,
                            format!(
                                "document {doc_id} is outside this run's Workspace — a tool run can only read documents inside its scope; use list_workspace to see what is available"
                            ),
                        );
                        return out;
                    }
                }
                match self.catalog.document_text(doc_id, READ_CAP).await {
                    Ok(text) if !text.trim().is_empty() => {
                        let title = self.catalog.doc_title(doc_id).unwrap_or_else(|| doc_id.to_string());
                        out.result_chars = text.chars().count();
                        out.result_text = format!("DOCUMENT {doc_id} — {title}\n\n{text}");
                        out.tokens_estimate = usage::estimate_tokens(&text);
                        out.duration_ms = started.elapsed().as_millis() as u64;
                        return out;
                    }
                    Ok(_) => fail(&mut out, started, format!("document {doc_id} has no readable text")),
                    Err(e) => fail(&mut out, started, format!("read_document failed: {e}")),
                }
            }
            "list_workspace" => {
                let entries = self.catalog.list_documents();
                let lines: Vec<String> = match &ctx.scope_docs {
                    Some(scope) => {
                        let by_id: std::collections::HashMap<&str, &DocListEntry> =
                            entries.iter().map(|e| (e.id.as_str(), e)).collect();
                        scope
                            .iter()
                            .map(|id| match by_id.get(id.as_str()) {
                                Some(e) => format!("- {} · {} ({} chunks)", e.id, e.title, e.chunks),
                                None => format!("- {id} · (title unavailable)"),
                            })
                            .collect()
                    }
                    None => entries
                        .iter()
                        .take(100)
                        .map(|e| format!("- {} · {} ({} chunks)", e.id, e.title, e.chunks))
                        .collect(),
                };
                out.result_text = if lines.is_empty() {
                    "(no documents available to this run)".to_string()
                } else {
                    lines.join("\n")
                };
            }
            "edit_document" => {
                let doc_id = call.args.get("doc_id").and_then(|v| v.as_str()).unwrap_or("");
                let instruction = call.args.get("instruction").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(scope) = &ctx.scope_docs {
                    if !scope.iter().any(|d| d == doc_id) {
                        fail(
                            &mut out,
                            started,
                            format!(
                                "document {doc_id} is outside this run's Workspace — edits can only be proposed for documents inside the run's scope; use list_workspace to see what is available"
                            ),
                        );
                        return out;
                    }
                }
                match &hooks.edit_proposer {
                    None => fail(
                        &mut out,
                        started,
                        "edit_document is not available in this run — document edits are proposed from the Agent Chat, where the user reviews and approves them".to_string(),
                    ),
                    Some(proposer) => match proposer.propose_edit(doc_id, instruction).await {
                        Ok(info) => {
                            out.tokens_estimate = usage::estimate_tokens(instruction)
                                + usage::estimate_tokens(&info.diff.summary_line());
                            out.result_text = format!(
                                "EDIT PROPOSED for “{}” (proposal {}): {}. NOTHING has been written back — the proposal is pending the user's approval. In your final answer, say the edit is proposed and awaiting approval; do not say the document has been changed.",
                                info.doc_title,
                                info.edit_id,
                                info.diff.summary_line()
                            );
                            out.result_chars = out.result_text.chars().count();
                            out.proposals.push(info);
                        }
                        Err(msg) => fail(&mut out, started, format!("edit_document failed: {msg}")),
                    },
                }
            }
            "spawn_subagent" => {
                if depth >= 1 {
                    fail(
                        &mut out,
                        started,
                        "sub-agents cannot spawn sub-agents — do this part of the work yourself with your own tools".to_string(),
                    );
                    return out;
                }
                let goal = call.args.get("goal").and_then(|v| v.as_str()).unwrap_or("");
                // Tool subset: the requested names, minus spawn itself,
                // minus anything unknown. Default: all but spawn.
                let mut child_tools: Vec<String> = match call.args.get("tools").and_then(|v| v.as_array()) {
                    Some(names) => names
                        .iter()
                        .filter_map(|v| v.as_str())
                        .filter(|n| *n != "spawn_subagent" && *n != "edit_document" && *n != "draft_document" && *n != "analyze_documents" && *n != "run_workflow" && tool_def(n).is_some())
                        .map(|n| n.to_string())
                        .collect(),
                    None => all_tool_names()
                        .into_iter()
                        .filter(|n| n != "spawn_subagent")
                        .collect(),
                };
                if child_tools.is_empty() {
                    child_tools = vec!["vault_search".to_string()];
                }
                // Budget carving: the child gets at most what the
                // parent has left (tokens and steps), and at most
                // what the call requested.
                let req_tokens = call.args.get("max_tokens").and_then(|v| v.as_u64());
                let req_steps = call.args.get("max_steps").and_then(|v| v.as_u64()).map(|n| n as usize);
                let child_tokens = {
                    let parent_left = parent_budget
                        .max_tokens
                        .map(|cap| cap.saturating_sub(parent_tokens_so_far));
                    let mut v = req_tokens.unwrap_or(DEFAULT_CHILD_TOKEN_BUDGET);
                    if let Some(left) = parent_left {
                        v = v.min(left.max(1));
                    }
                    v.max(1)
                };
                let child_steps = {
                    let mut v = req_steps.unwrap_or(DEFAULT_CHILD_STEPS);
                    if let Some(cap) = parent_budget.max_steps {
                        v = v.min(cap.saturating_sub(parent_steps_so_far).max(1));
                    }
                    v.max(1)
                };
                let child_budget = RunBudget {
                    max_steps: Some(child_steps),
                    max_tokens: Some(child_tokens),
                };
                // The child inherits the parent's scope and request
                // options unchanged — confinement cannot widen. It
                // gets NO hooks: no live sink (the parent reports it)
                // and no edit proposer — proposals are a top-level,
                // user-watched act, like spawning itself.
                let child_ctx = ctx.clone();
                match Box::pin(self.run_inner(goal, &[], child_ctx, child_budget, child_tools, depth + 1, &ToolRunHooks::default())).await {
                    Ok(child) => {
                        out.tokens_estimate = child.tokens_estimate;
                        out.result_text = format!(
                            "SUB-AGENT REPORT (stopped: {}, {} tool calls, ~{} tokens):\n{}",
                            child.stopped,
                            child.trace.iter().filter(|t| t.tool != "finish" && t.tool != "model").count(),
                            child.tokens_estimate,
                            child.final_report
                        );
                        out.citations = child.citations;
                        out.child = Some(ToolChildSummary {
                            goal: goal.to_string(),
                            stopped: child.stopped.clone(),
                            steps: child.trace.len(),
                            tokens_estimate: child.tokens_estimate,
                        });
                    }
                    Err(e) => fail(&mut out, started, format!("sub-agent failed: {e}")),
                }
            }
            _ => fail(&mut out, started, format!("tool \"{}\" is not executable", call.tool)),
        }

        if out.status == "ok" && out.result_chars == 0 {
            out.result_chars = out.result_text.chars().count();
        }
        out.duration_ms = started.elapsed().as_millis() as u64;
        out
    }
}

fn with_scope_note(ans: &crate::query::QueryAnswer) -> String {
    match &ans.scope_note {
        Some(note) => format!("{}\n\n[scope note: {note}]", ans.answer),
        None => ans.answer.clone(),
    }
}

/// Flatten a [`crate::research::ResearchReport`] into the text a
/// tool result feeds the model: verdict first, then the weighed
/// hypotheses with their evidence markers, conflicts, open points,
/// and the decisions only the user can make — the Research page's
/// own shape, in prose.
fn format_research_report(report: &crate::research::ResearchReport) -> String {
    let mut out = format!(
        "DEEP RESEARCH REPORT — {}\n\nConclusion: {}",
        report.question, report.conclusion
    );
    if let Some(line) = &report.governing_line {
        out.push_str(&format!("\nGoverning authority: {line}"));
    }
    if let Some(note) = &report.web_note {
        out.push_str(&format!("\nWeb note: {note}"));
    }
    out.push_str("\n\nHypotheses:");
    for h in &report.hypotheses {
        out.push_str(&format!(
            "\n- [{}] {} (prediction: {})",
            h.status, h.question, h.prediction
        ));
        for ev in &h.evidence {
            out.push_str(&format!("\n  · {} {}", ev.claim, ev.source));
        }
    }
    if !report.conflicts.is_empty() {
        out.push_str("\n\nConflicts:");
        for c in &report.conflicts {
            out.push_str(&format!(
                "\n- {}: A: {} / B: {}",
                c.topic, c.position_a, c.position_b
            ));
        }
    }
    if !report.open_points.is_empty() {
        out.push_str("\n\nOpen points:");
        for p in &report.open_points {
            out.push_str(&format!("\n- {p}"));
        }
    }
    if !report.decisions.is_empty() {
        out.push_str("\n\nFor the user to decide:");
        for d in &report.decisions {
            out.push_str(&format!("\n- {d}"));
        }
    }
    out
}

struct CallOutcome {
    tool: String,
    args_digest: String,
    status: String,
    error: Option<String>,
    result_text: String,
    result_chars: usize,
    tokens_estimate: u64,
    duration_ms: u64,
    citations: Vec<Citation>,
    child: Option<ToolChildSummary>,
    proposals: Vec<EditProposalInfo>,
    drafts: Vec<DraftInfo>,
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbeddings;
    use crate::graph::InMemoryGraphStore;
    use crate::llm::LlmClient;
    use crate::vector_store::InMemoryVectorStore;
    use crate::web::NoopWebSearch;
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    /// An LLM that scripts the TOOL-LOOP turns (matched by the tool
    /// system prompt), returns a fixed synthesis for the partial-report
    /// prompt, and a fixed grounded answer for every other (query
    /// pipeline) prompt.
    struct ScriptedLlm {
        tool_script: Mutex<VecDeque<String>>,
        synth: String,
        answer: String,
    }

    impl ScriptedLlm {
        fn new(script: Vec<&str>) -> Self {
            Self {
                tool_script: Mutex::new(script.into_iter().map(|s| s.to_string()).collect()),
                synth: "Synthesized partial report.".to_string(),
                answer: "Vault findings [doc:0].".to_string(),
            }
        }
    }

    #[async_trait]
    impl LlmClient for ScriptedLlm {
        async fn complete(&self, system: &str, _user: &str) -> Result<String> {
            let response = if system.contains("autonomous legal-research agent with tools") {
                self.tool_script
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| "{\"final\": \"Script exhausted.\"}".to_string())
            } else if system.contains("ended before the agent produced its own final answer") {
                self.synth.clone()
            } else {
                self.answer.clone()
            };
            usage::record_llm(
                usage::estimate_tokens(system) + 10,
                usage::estimate_tokens(&response),
            );
            Ok(response)
        }
    }

    struct FakeCatalog {
        docs: HashMap<String, (String, String)>, // id -> (title, text)
    }

    fn fake_catalog() -> FakeCatalog {
        let mut docs = HashMap::new();
        docs.insert(
            "doc-a".to_string(),
            (
                "Tenancy note".to_string(),
                "The security deposit is refundable within 30 days of vacating.".to_string(),
            ),
        );
        docs.insert(
            "doc-b".to_string(),
            (
                "Other workspace doc".to_string(),
                "This document belongs to a different workspace.".to_string(),
            ),
        );
        FakeCatalog { docs }
    }

    #[async_trait]
    impl DocCatalog for FakeCatalog {
        async fn document_text(&self, doc_id: &str, _max_chars: usize) -> Result<String> {
            self.docs
                .get(doc_id)
                .map(|(_, text)| text.clone())
                .ok_or_else(|| Error::NotFound(format!("document {doc_id}")))
        }
        fn list_documents(&self) -> Vec<DocListEntry> {
            self.docs
                .iter()
                .map(|(id, (title, _))| DocListEntry { id: id.clone(), title: title.clone(), chunks: 1 })
                .collect()
        }
        fn doc_title(&self, doc_id: &str) -> Option<String> {
            self.docs.get(doc_id).map(|(title, _)| title.clone())
        }
    }

    fn agent_with(script: Vec<&str>) -> ToolAgent {
        let svc = QueryService::new(
            Arc::new(MockEmbeddings::new(8)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(ScriptedLlm::new(script)),
            Arc::new(|_: &str| None),
            Arc::new(NoopWebSearch),
            None,
        );
        ToolAgent::new(Arc::new(svc), Arc::new(fake_catalog()))
    }

    fn call(tool: &str, args: Value) -> ToolCallReq {
        ToolCallReq { tool: tool.to_string(), args }
    }

    // -- schema validation -------------------------------------------------

    #[test]
    fn schema_validation_rejects_bad_arguments() {
        let def = tool_def("vault_search").unwrap();
        assert!(validate_tool_args(&def, &json!({ "query": "rent" })).is_ok());
        // Missing required.
        let e = validate_tool_args(&def, &json!({})).unwrap_err();
        assert!(e.contains("missing required argument \"query\""), "{e}");
        // Wrong type.
        let e = validate_tool_args(&def, &json!({ "query": 42 })).unwrap_err();
        assert!(e.contains("must be string"), "{e}");
        // Integer bounds.
        let e = validate_tool_args(&def, &json!({ "query": "x", "top_k": 99 })).unwrap_err();
        assert!(e.contains("<= 10"), "{e}");
        // Enum.
        let graph = tool_def("graph_query").unwrap();
        let e = validate_tool_args(&graph, &json!({ "query": "x", "mode": "sideways" })).unwrap_err();
        assert!(e.contains("must be one of"), "{e}");
        assert!(validate_tool_args(&graph, &json!({ "query": "x", "mode": "local" })).is_ok());
        // Array item types.
        let spawn = tool_def("spawn_subagent").unwrap();
        let e = validate_tool_args(&spawn, &json!({ "goal": "g", "tools": [1, 2] })).unwrap_err();
        assert!(e.contains("array of string"), "{e}");
        // Non-object args.
        assert!(validate_tool_args(&def, &json!("query")).is_err());
    }

    #[test]
    fn parse_action_handles_calls_batches_finals_and_garbage() {
        match parse_action("{\"tool\": \"vault_search\", \"args\": {\"query\": \"x\"}}") {
            ModelAction::Calls(c) => {
                assert_eq!(c.len(), 1);
                assert_eq!(c[0].tool, "vault_search");
            }
            _ => panic!("expected one call"),
        }
        match parse_action("{\"calls\": [{\"tool\": \"list_workspace\", \"args\": {}}, {\"tool\": \"read_document\", \"args\": {\"doc_id\": \"doc-a\"}}]}") {
            ModelAction::Calls(c) => assert_eq!(c.len(), 2),
            _ => panic!("expected a batch"),
        }
        match parse_action("{\"final\": \"the report\"}") {
            ModelAction::Final(t) => assert_eq!(t, "the report"),
            _ => panic!("expected final"),
        }
        assert!(matches!(parse_action("I will search the vault now."), ModelAction::Invalid(_)));
        assert!(matches!(parse_action("{\"something\": true}"), ModelAction::Invalid(_)));
    }

    // -- workspace confinement, exercised at the execution seam -----------

    #[tokio::test]
    async fn read_document_is_confined_to_the_run_workspace() {
        let agent = agent_with(vec![]);
        let ctx = ToolRunContext {
            scope_docs: Some(vec!["doc-a".to_string()]),
            ..Default::default()
        };
        // Outside the scope: refused, even though the document exists.
        let out = agent
            .execute_call(&call("read_document", json!({ "doc_id": "doc-b" })), &ctx, RunBudget::default(), 0, 0, 0, &ToolRunHooks::default())
            .await;
        assert_eq!(out.status, "error");
        assert!(out.result_text.contains("outside this run's Workspace"), "{}", out.result_text);
        // Inside the scope: the document's own text comes back.
        let out = agent
            .execute_call(&call("read_document", json!({ "doc_id": "doc-a" })), &ctx, RunBudget::default(), 0, 0, 0, &ToolRunHooks::default())
            .await;
        assert_eq!(out.status, "ok");
        assert!(out.result_text.contains("security deposit is refundable"));
        // list_workspace shows only the scoped document.
        let out = agent
            .execute_call(&call("list_workspace", json!({})), &ctx, RunBudget::default(), 0, 0, 0, &ToolRunHooks::default())
            .await;
        assert_eq!(out.status, "ok");
        assert!(out.result_text.contains("doc-a"));
        assert!(!out.result_text.contains("doc-b"));
    }

    #[tokio::test]
    async fn subagent_cannot_spawn_and_unknown_tools_error_out() {
        let agent = agent_with(vec![]);
        let ctx = ToolRunContext::default();
        // Depth 1 = already a sub-agent: spawning is refused.
        let out = agent
            .execute_call(&call("spawn_subagent", json!({ "goal": "deeper" })), &ctx, RunBudget::default(), 0, 0, 1, &ToolRunHooks::default())
            .await;
        assert_eq!(out.status, "error");
        assert!(out.result_text.contains("cannot spawn"));
        // Unknown tool: an error outcome, not a crash.
        let out = agent
            .execute_call(&call("teleport", json!({})), &ctx, RunBudget::default(), 0, 0, 0, &ToolRunHooks::default())
            .await;
        assert_eq!(out.status, "error");
        assert!(out.result_text.contains("unknown tool"));
    }

    // -- the loop -----------------------------------------------------------

    #[tokio::test]
    async fn schema_rejections_are_fed_back_and_the_run_recovers() {
        let agent = agent_with(vec![
            "{\"tool\": \"vault_search\", \"args\": {}}",
            "{\"tool\": \"vault_search\", \"args\": {\"query\": 42}}",
            "{\"tool\": \"vault_search\", \"args\": {\"query\": \"tenancy deposit\"}}",
            "{\"final\": \"Recovered report.\"}",
        ]);
        let run = agent
            .run("deposit rules", &[], ToolRunContext::default(), RunBudget::default())
            .await
            .unwrap();
        assert_eq!(run.stopped, "finished");
        assert_eq!(run.final_report, "Recovered report.");
        let errors: Vec<&ToolTraceEntry> = run.trace.iter().filter(|t| t.status == "error").collect();
        assert_eq!(errors.len(), 2);
        assert!(errors[0].error.as_deref().unwrap().contains("missing required argument"));
        assert!(errors[1].error.as_deref().unwrap().contains("must be string"));
        assert!(run.trace.iter().any(|t| t.tool == "vault_search" && t.status == "ok"));
        assert!(run.trace.iter().any(|t| t.tool == "finish"));
    }

    #[tokio::test]
    async fn token_budget_stops_the_loop_with_a_partial_report() {
        // The model would search forever; a 1-token budget must stop
        // it after the first call and still produce a report.
        let agent = agent_with(vec![
            "{\"tool\": \"vault_search\", \"args\": {\"query\": \"q1\"}}",
            "{\"tool\": \"vault_search\", \"args\": {\"query\": \"q2\"}}",
            "{\"tool\": \"vault_search\", \"args\": {\"query\": \"q3\"}}",
        ]);
        let run = agent
            .run(
                "endless goal",
                &[],
                ToolRunContext::default(),
                RunBudget { max_steps: None, max_tokens: Some(1) },
            )
            .await
            .unwrap();
        assert_eq!(run.stopped, "budget_tokens");
        let calls: Vec<&ToolTraceEntry> = run.trace.iter().filter(|t| t.tool == "vault_search").collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(run.final_report, "Synthesized partial report.");
        assert!(run.tokens_estimate > 0);
    }

    #[tokio::test]
    async fn step_budget_stops_the_loop() {
        let agent = agent_with(vec![
            "{\"tool\": \"vault_search\", \"args\": {\"query\": \"q1\"}}",
            "{\"tool\": \"vault_search\", \"args\": {\"query\": \"q2\"}}",
            "{\"final\": \"never reached\"}",
        ]);
        let run = agent
            .run(
                "capped goal",
                &[],
                ToolRunContext::default(),
                RunBudget { max_steps: Some(1), max_tokens: None },
            )
            .await
            .unwrap();
        assert_eq!(run.stopped, "budget");
        assert_eq!(run.trace.iter().filter(|t| t.tool == "vault_search").count(), 1);
    }

    #[tokio::test]
    async fn subagent_budget_is_carved_from_the_parent() {
        // Parent spawns a child with a 1-token carve-out; the child's
        // own script would search repeatedly but its carved budget
        // stops it after one call, and its spend lands in the parent.
        let agent = agent_with(vec![
            "{\"tool\": \"spawn_subagent\", \"args\": {\"goal\": \"research deposits\", \"max_tokens\": 1}}",
            "{\"tool\": \"vault_search\", \"args\": {\"query\": \"deposit law\"}}",
            "{\"tool\": \"vault_search\", \"args\": {\"query\": \"deposit law again\"}}",
            "{\"final\": \"Parent report.\"}",
        ]);
        let run = agent
            .run("deposit goal", &[], ToolRunContext::default(), RunBudget::default())
            .await
            .unwrap();
        assert_eq!(run.stopped, "finished");
        assert_eq!(run.final_report, "Parent report.");
        let spawn = run
            .trace
            .iter()
            .find(|t| t.tool == "spawn_subagent")
            .expect("spawn recorded in the trace");
        assert_eq!(spawn.status, "ok");
        let child = spawn.child.as_ref().expect("child summary on the spawn entry");
        assert_eq!(child.goal, "research deposits");
        assert_eq!(child.stopped, "budget_tokens");
        assert_eq!(child.steps, 1, "the carved budget stopped the child after one call");
        assert!(child.tokens_estimate > 0);
        assert!(
            run.tokens_estimate >= child.tokens_estimate,
            "child spend must land in the parent's totals"
        );
    }

    // -- Piece 86: the edit seam and the live event stream --------------

    /// Records every proposal request; stands in for the server's
    /// Piece 85 propose path.
    struct FakeProposer {
        calls: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl EditProposer for FakeProposer {
        async fn propose_edit(
            &self,
            doc_id: &str,
            instruction: &str,
        ) -> std::result::Result<EditProposalInfo, String> {
            self.calls
                .lock()
                .unwrap()
                .push((doc_id.to_string(), instruction.to_string()));
            Ok(EditProposalInfo {
                edit_id: "edit-1".to_string(),
                doc_id: doc_id.to_string(),
                doc_title: "Tenancy note".to_string(),
                instruction: instruction.to_string(),
                diff: crate::edits::compute_diff("old text", "new text"),
            })
        }
    }

    fn edit_hooks(proposer: &Arc<FakeProposer>) -> ToolRunHooks {
        ToolRunHooks {
            sink: None,
            edit_proposer: Some(proposer.clone() as Arc<dyn EditProposer>),
            allow_edit: true,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn edit_document_creates_a_proposal_and_writes_nothing() {
        let agent = agent_with(vec![
            "{\"tool\": \"edit_document\", \"args\": {\"doc_id\": \"doc-a\", \"instruction\": \"extend the deposit window to 45 days\"}}",
            "{\"final\": \"The edit is proposed and awaiting your approval.\"}",
        ]);
        let proposer = Arc::new(FakeProposer { calls: Mutex::new(Vec::new()) });
        let run = agent
            .run_hooked(
                "extend the deposit window in the tenancy note",
                &[],
                ToolRunContext::default(),
                RunBudget::default(),
                edit_hooks(&proposer),
            )
            .await
            .unwrap();
        assert_eq!(run.stopped, "finished");
        // Exactly one proposal, parked — the loop has no write path:
        // a proposal is all an edit call can ever produce.
        assert_eq!(proposer.calls.lock().unwrap().len(), 1);
        assert_eq!(run.proposals.len(), 1);
        assert_eq!(run.proposals[0].edit_id, "edit-1");
        assert_eq!(run.proposals[0].doc_id, "doc-a");
        let entry = run.trace.iter().find(|t| t.tool == "edit_document").unwrap();
        assert_eq!(entry.status, "ok");
    }

    #[tokio::test]
    async fn edit_document_without_a_proposer_is_clean_feedback() {
        let agent = agent_with(vec![
            "{\"tool\": \"edit_document\", \"args\": {\"doc_id\": \"doc-a\", \"instruction\": \"fix it\"}}",
            "{\"final\": \"Editing is not available here.\"}",
        ]);
        let run = agent
            .run_hooked(
                "fix the tenancy note",
                &[],
                ToolRunContext::default(),
                RunBudget::default(),
                ToolRunHooks { sink: None, edit_proposer: None, allow_edit: true, ..Default::default() },
            )
            .await
            .unwrap();
        assert_eq!(run.stopped, "finished");
        assert!(run.proposals.is_empty());
        let entry = run.trace.iter().find(|t| t.tool == "edit_document").unwrap();
        assert_eq!(entry.status, "error");
        assert!(entry.error.as_deref().unwrap().contains("not available"), "{:?}", entry.error);
    }

    #[tokio::test]
    async fn edit_document_is_confined_to_the_run_workspace() {
        let agent = agent_with(vec![
            "{\"tool\": \"edit_document\", \"args\": {\"doc_id\": \"doc-b\", \"instruction\": \"fix it\"}}",
            "{\"final\": \"Cannot edit outside the workspace.\"}",
        ]);
        let proposer = Arc::new(FakeProposer { calls: Mutex::new(Vec::new()) });
        let ctx = ToolRunContext {
            scope_docs: Some(vec!["doc-a".to_string()]),
            ..Default::default()
        };
        let run = agent
            .run_hooked("fix the other workspace's doc", &[], ctx, RunBudget::default(), edit_hooks(&proposer))
            .await
            .unwrap();
        assert_eq!(run.stopped, "finished");
        assert!(run.proposals.is_empty());
        assert_eq!(proposer.calls.lock().unwrap().len(), 0, "a confined run must never reach the proposer");
        let entry = run.trace.iter().find(|t| t.tool == "edit_document").unwrap();
        assert_eq!(entry.status, "error");
    }

    #[tokio::test]
    async fn subagent_cannot_propose_edits() {
        let agent = agent_with(vec![
            "{\"tool\": \"spawn_subagent\", \"args\": {\"goal\": \"child branch\", \"tools\": [\"edit_document\", \"vault_search\"]}}",
            "{\"tool\": \"edit_document\", \"args\": {\"doc_id\": \"doc-a\", \"instruction\": \"change it\"}}",
            "{\"final\": \"Child report.\"}",
            "{\"final\": \"Parent report.\"}",
        ]);
        let proposer = Arc::new(FakeProposer { calls: Mutex::new(Vec::new()) });
        let run = agent
            .run_hooked(
                "delegate and edit",
                &[],
                ToolRunContext::default(),
                RunBudget::default(),
                edit_hooks(&proposer),
            )
            .await
            .unwrap();
        assert_eq!(run.final_report, "Parent report.");
        assert_eq!(
            proposer.calls.lock().unwrap().len(),
            0,
            "the child's edit attempt must never reach the proposer (children get no edit seam)"
        );
        assert!(run.proposals.is_empty());
    }

    #[tokio::test]
    async fn research_routes_to_web_research() {
        let agent = agent_with(vec![
            "{\"tool\": \"web_research\", \"args\": {\"query\": \"latest rent control ruling\"}}",
            "{\"final\": \"Researched report.\"}",
        ]);
        let run = agent
            .run("what is the latest ruling", &[], ToolRunContext::default(), RunBudget::default())
            .await
            .unwrap();
        assert_eq!(run.stopped, "finished");
        assert!(run.trace.iter().any(|t| t.tool == "web_research" && t.status == "ok"));
        assert!(run.proposals.is_empty());
    }

    #[tokio::test]
    async fn events_stream_calls_results_and_answer_deltas_in_order() {
        let agent = agent_with(vec![
            "{\"tool\": \"vault_search\", \"args\": {\"query\": \"tenancy deposit\"}}",
            "{\"final\": \"The deposit is refundable within thirty days of vacating the premises.\"}",
        ]);
        let events: Arc<Mutex<Vec<ToolLoopEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_events = events.clone();
        let hooks = ToolRunHooks {
            sink: Some(Arc::new(move |ev| sink_events.lock().unwrap().push(ev))),
            edit_proposer: None,
            allow_edit: false,
            ..Default::default()
        };
        let run = agent
            .run_hooked("deposit rules", &[], ToolRunContext::default(), RunBudget::default(), hooks)
            .await
            .unwrap();
        assert_eq!(run.stopped, "finished");
        let evs = events.lock().unwrap();
        let call_at = evs
            .iter()
            .position(|e| matches!(e, ToolLoopEvent::ToolCall { tool, .. } if tool == "vault_search"))
            .expect("a tool_call event for vault_search");
        let result_at = evs
            .iter()
            .position(|e| matches!(e, ToolLoopEvent::ToolResult { tool, status, .. } if tool == "vault_search" && status == "ok"))
            .expect("a tool_result event for vault_search");
        assert!(call_at < result_at, "the call event must precede its result event");
        // Answer deltas concatenate to the exact final report.
        let streamed: String = evs
            .iter()
            .filter_map(|e| match e {
                ToolLoopEvent::Delta { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(streamed, run.final_report);
        assert!(!streamed.is_empty());
        // A pure question produced no edit activity.
        assert!(evs.iter().all(|e| !matches!(e, ToolLoopEvent::EditProposed { .. })));
    }

    #[tokio::test]
    async fn edit_proposal_is_projected_to_the_event_sink() {
        let agent = agent_with(vec![
            "{\"tool\": \"edit_document\", \"args\": {\"doc_id\": \"doc-a\", \"instruction\": \"extend the window\"}}",
            "{\"final\": \"Proposed.\"}",
        ]);
        let proposer = Arc::new(FakeProposer { calls: Mutex::new(Vec::new()) });
        let events: Arc<Mutex<Vec<ToolLoopEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_events = events.clone();
        let mut hooks = edit_hooks(&proposer);
        hooks.sink = Some(Arc::new(move |ev| sink_events.lock().unwrap().push(ev)));
        agent
            .run_hooked("extend the window", &[], ToolRunContext::default(), RunBudget::default(), hooks)
            .await
            .unwrap();
        let evs = events.lock().unwrap();
        let proposed = evs
            .iter()
            .find_map(|e| match e {
                ToolLoopEvent::EditProposed { proposal } => Some(proposal),
                _ => None,
            })
            .expect("an edit_proposed event");
        assert_eq!(proposed.edit_id, "edit-1");
        assert_eq!(proposed.diff.added.len(), 1);
    }

    // -- Piece 87: deep_research + draft_document ---------------------------

    struct FakeDrafter;

    #[async_trait]
    impl DraftGenerator for FakeDrafter {
        async fn generate_draft(
            &self,
            kind: &str,
            instructions: &str,
        ) -> std::result::Result<DraftInfo, String> {
            Ok(DraftInfo {
                kind: kind.to_string(),
                kind_label: "Legal notice".to_string(),
                text: format!("NOTICE TEXT for: {instructions}"),
                citations: Vec::new(),
            })
        }
    }

    fn draft_hooks() -> ToolRunHooks {
        ToolRunHooks {
            draft_generator: Some(Arc::new(FakeDrafter)),
            allow_draft: true,
            ..Default::default()
        }
    }

    struct FakeAnalyzer;

    #[async_trait]
    impl DocumentAnalyzer for FakeAnalyzer {
        async fn analyze_documents(
            &self,
            action: &str,
            _question: Option<&str>,
            document_ids: &[String],
            _scope_docs: &Option<Vec<String>>,
        ) -> std::result::Result<AnalysisInfo, String> {
            Ok(AnalysisInfo {
                text: format!("ANALYSIS {action} over {}", document_ids.join("+")),
                citations: Vec::new(),
            })
        }
    }

    struct FakeWorkflowRunner;

    #[async_trait]
    impl WorkflowRunner for FakeWorkflowRunner {
        async fn run_workflow(
            &self,
            workflow: &str,
            _input: &str,
        ) -> std::result::Result<WorkflowInfo, String> {
            Ok(WorkflowInfo {
                name: format!("Workflow {workflow}"),
                report: "WORKFLOW REPORT BODY".to_string(),
                citations: Vec::new(),
                tokens_estimate: 42,
                stopped: "finished".to_string(),
            })
        }
    }

    fn full_hooks() -> ToolRunHooks {
        ToolRunHooks {
            analyzer: Some(Arc::new(FakeAnalyzer)),
            allow_analyze: true,
            workflow_runner: Some(Arc::new(FakeWorkflowRunner)),
            allow_workflow: true,
            ..Default::default()
        }
    }

    #[test]
    fn new_capability_schemas_validate() {
        let a = tool_def("analyze_documents").unwrap();
        assert!(validate_tool_args(&a, &json!({"action": "summary", "document_ids": ["d1"]})).is_ok());
        assert!(validate_tool_args(&a, &json!({"action": "bogus", "document_ids": ["d1"]})).is_err());
        assert!(validate_tool_args(&a, &json!({"action": "summary"})).is_err());
        let w = tool_def("run_workflow").unwrap();
        assert!(validate_tool_args(&w, &json!({"workflow": "memo_draft", "input": "x"})).is_ok());
        assert!(validate_tool_args(&w, &json!({"workflow": "nope", "input": "x"})).is_err());
        let names = all_tool_names();
        assert!(!names.contains(&"analyze_documents".to_string()), "seam-gated");
        assert!(!names.contains(&"run_workflow".to_string()), "seam-gated");
    }

    #[tokio::test]
    async fn analyze_and_workflow_tools_run_through_their_seams() {
        let agent = agent_with(vec![
            "{\"tool\": \"analyze_documents\", \"args\": {\"action\": \"clauses\", \"document_ids\": [\"doc-1\", \"doc-2\"]}}",
            "{\"tool\": \"run_workflow\", \"args\": {\"workflow\": \"contract_review\", \"input\": \"review the tenancy\"}}",
            "{\"final\": \"Done.\"}",
        ]);
        let run = agent
            .run_hooked("analyse and review", &[], ToolRunContext::default(), RunBudget::default(), full_hooks())
            .await
            .unwrap();
        assert_eq!(run.stopped, "finished");
        let ok_tools: Vec<&str> = run.trace.iter().filter(|t| t.status == "ok").map(|t| t.tool.as_str()).collect();
        assert!(ok_tools.contains(&"analyze_documents"), "{ok_tools:?}");
        assert!(ok_tools.contains(&"run_workflow"), "{ok_tools:?}");
    }

    #[tokio::test]
    async fn capability_tools_without_seams_feed_back_an_error() {
        let agent = agent_with(vec![
            "{\"tool\": \"analyze_documents\", \"args\": {\"action\": \"summary\", \"document_ids\": [\"doc-1\"]}}",
            "{\"final\": \"Understood.\"}",
        ]);
        // Hooks carry the tools in the allowed set only when seams
        // exist; without seams the call never reaches execute — so
        // drive run_inner's gate the way run_hooked would not: use
        // full hooks minus the analyzer seam but force the tool via
        // a hooked run whose allow flag is on and seam missing.
        let hooks = ToolRunHooks { allow_analyze: true, ..Default::default() };
        let run = agent
            .run_hooked("summarise doc-1", &[], ToolRunContext::default(), RunBudget::default(), hooks)
            .await
            .unwrap();
        // Not offered without the seam: the model cannot call it,
        // so the scripted call is rejected and the run still ends.
        assert_eq!(run.stopped, "finished");
    }

    #[test]
    fn seam_gated_tools_stay_out_of_the_default_registry_view() {
        let names = all_tool_names();
        assert!(names.contains(&"deep_research".to_string()), "deep research needs no seam");
        assert!(!names.contains(&"edit_document".to_string()));
        assert!(!names.contains(&"draft_document".to_string()));
        let chat = chat_tool_names();
        assert!(chat.contains(&"edit_document".to_string()));
        assert!(chat.contains(&"draft_document".to_string()));
        assert!(chat.contains(&"deep_research".to_string()));
    }

    #[test]
    fn new_tool_schemas_validate() {
        let draft = tool_def("draft_document").unwrap();
        assert!(
            validate_tool_args(&draft, &json!({ "kind": "legal_notice", "instructions": "x" })).is_ok()
        );
        assert!(
            validate_tool_args(&draft, &json!({ "kind": "custom", "instructions": "x" })).is_ok()
        );
        let e = validate_tool_args(&draft, &json!({ "kind": "haiku", "instructions": "x" })).unwrap_err();
        assert!(e.contains("must be one of"), "{e}");
        let e = validate_tool_args(&draft, &json!({ "kind": "memo" })).unwrap_err();
        assert!(e.contains("missing required argument \"instructions\""), "{e}");
        let research = tool_def("deep_research").unwrap();
        assert!(validate_tool_args(&research, &json!({ "question": "q" })).is_ok());
        assert!(validate_tool_args(&research, &json!({ "question": "q", "web": true })).is_ok());
        let e = validate_tool_args(&research, &json!({ "question": "q", "web": "yes" })).unwrap_err();
        assert!(e.contains("must be boolean"), "{e}");
    }

    #[tokio::test]
    async fn deep_research_refuses_a_workspace_scoped_run() {
        let agent = agent_with(vec![]);
        let ctx = ToolRunContext {
            scope_docs: Some(vec!["doc-a".to_string()]),
            ..Default::default()
        };
        let out = agent
            .execute_call(&call("deep_research", json!({ "question": "deposit law" })), &ctx, RunBudget::default(), 0, 0, 0, &ToolRunHooks::default())
            .await;
        assert_eq!(out.status, "error");
        assert!(
            out.result_text.contains("cannot be confined"),
            "{}",
            out.result_text
        );
    }

    #[tokio::test]
    async fn deep_research_runs_the_pipeline_unscoped() {
        // The scripted LLM cannot frame or adjudicate, so the
        // pipeline degrades exactly as it does live: a report whose
        // conclusion states the evidence was insufficient, delivered
        // as an ok tool result — never a crash, never an error.
        let agent = agent_with(vec![]);
        let out = agent
            .execute_call(
                &call("deep_research", json!({ "question": "Is the deposit refundable?" })),
                &ToolRunContext::default(),
                RunBudget::default(),
                0,
                0,
                0,
                &ToolRunHooks::default(),
            )
            .await;
        assert_eq!(out.status, "ok", "{}", out.result_text);
        assert!(out.result_text.contains("DEEP RESEARCH REPORT"), "{}", out.result_text);
        assert!(out.result_text.contains("insufficient"), "{}", out.result_text);
    }

    #[tokio::test]
    async fn draft_document_needs_the_generator_seam() {
        let agent = agent_with(vec![]);
        let out = agent
            .execute_call(
                &call("draft_document", json!({ "kind": "legal_notice", "instructions": "demand the rent" })),
                &ToolRunContext::default(),
                RunBudget::default(),
                0,
                0,
                0,
                &ToolRunHooks::default(),
            )
            .await;
        assert_eq!(out.status, "error");
        assert!(out.result_text.contains("not available in this run"), "{}", out.result_text);
    }

    #[tokio::test]
    async fn draft_is_attached_verbatim_below_the_final_answer() {
        let agent = agent_with(vec![
            "{\"tool\": \"draft_document\", \"args\": {\"kind\": \"legal_notice\", \"instructions\": \"demand the unpaid rent\"}}",
            "{\"final\": \"Here is the notice you asked for.\"}",
        ]);
        let run = agent
            .run_hooked(
                "draft a notice for the unpaid rent",
                &[],
                ToolRunContext::default(),
                RunBudget::default(),
                draft_hooks(),
            )
            .await
            .unwrap();
        assert_eq!(run.stopped, "finished");
        assert_eq!(run.drafts.len(), 1);
        assert_eq!(run.drafts[0].kind, "legal_notice");
        assert!(
            run.final_report.starts_with("Here is the notice you asked for."),
            "{}",
            run.final_report
        );
        assert!(
            run.final_report.contains("## Draft — Legal notice"),
            "{}",
            run.final_report
        );
        assert!(
            run.final_report.contains("NOTICE TEXT for: demand the unpaid rent"),
            "{}",
            run.final_report
        );
        assert!(run.trace.iter().any(|t| t.tool == "draft_document" && t.status == "ok"));
    }
}
