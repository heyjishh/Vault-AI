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
    ]
}

pub fn tool_def(name: &str) -> Option<ToolDef> {
    tool_defs().into_iter().find(|d| d.name == name)
}

/// The tools a fresh top-level run may use.
pub fn all_tool_names() -> Vec<String> {
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
When the goal is covered, answer with {\"final\": ...} instead of calling more tools.";

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
        "read_document" => pick(&["doc_id"]),
        "spawn_subagent" => pick(&["goal"]),
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
        if goal.trim().is_empty() {
            return Err(Error::BadRequest("goal is empty".to_string()));
        }
        self.run_inner(goal, plan, ctx, budget, all_tool_names(), 0).await
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
                        let futs = spawn_idx.iter().map(|&i| {
                            self.execute_call(&calls[i], &ctx, budget, tokens_estimate, steps_done, depth)
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
                            self.absorb(out, &mut trace, &mut transcript, &mut findings, &mut citations, &mut seen_chunks);
                            continue;
                        }
                        let out = self
                            .execute_call(call, &ctx, budget, tokens_estimate, steps_done, depth)
                            .await;
                        steps_done += 1;
                        tokens_estimate += out.tokens_estimate;
                        self.absorb(out, &mut trace, &mut transcript, &mut findings, &mut citations, &mut seen_chunks);
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

        Ok(ToolRun {
            goal: goal.to_string(),
            trace,
            final_report,
            citations,
            stopped,
            tokens_estimate,
            plan: plan.to_vec(),
        })
    }

    /// Fold one executed call into the run's trace, transcript,
    /// findings, and citation set.
    fn absorb(
        &self,
        out: CallOutcome,
        trace: &mut Vec<ToolTraceEntry>,
        transcript: &mut String,
        findings: &mut Vec<String>,
        citations: &mut Vec<Citation>,
        seen_chunks: &mut HashSet<String>,
    ) {
        for c in out.citations {
            if seen_chunks.insert(c.chunk_id.clone()) {
                citations.push(c);
            }
        }
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
                        .filter(|n| *n != "spawn_subagent" && tool_def(n).is_some())
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
                // options unchanged — confinement cannot widen.
                let child_ctx = ctx.clone();
                match Box::pin(self.run_inner(goal, &[], child_ctx, child_budget, child_tools, depth + 1)).await {
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
            .execute_call(&call("read_document", json!({ "doc_id": "doc-b" })), &ctx, RunBudget::default(), 0, 0, 0)
            .await;
        assert_eq!(out.status, "error");
        assert!(out.result_text.contains("outside this run's Workspace"), "{}", out.result_text);
        // Inside the scope: the document's own text comes back.
        let out = agent
            .execute_call(&call("read_document", json!({ "doc_id": "doc-a" })), &ctx, RunBudget::default(), 0, 0, 0)
            .await;
        assert_eq!(out.status, "ok");
        assert!(out.result_text.contains("security deposit is refundable"));
        // list_workspace shows only the scoped document.
        let out = agent
            .execute_call(&call("list_workspace", json!({})), &ctx, RunBudget::default(), 0, 0, 0)
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
            .execute_call(&call("spawn_subagent", json!({ "goal": "deeper" })), &ctx, RunBudget::default(), 0, 0, 1)
            .await;
        assert_eq!(out.status, "error");
        assert!(out.result_text.contains("cannot spawn"));
        // Unknown tool: an error outcome, not a crash.
        let out = agent
            .execute_call(&call("teleport", json!({})), &ctx, RunBudget::default(), 0, 0, 0)
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
}
