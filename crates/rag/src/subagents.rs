//! True sub-agent orchestration (P1 sequential + P2 parallel research).
//!
//! A lead agent decomposes a goal into independent research branches,
//! dispatches one Researcher per branch concurrently, then a Drafter
//! writes the final answer from the researchers' findings + sources
//! only, and a Checker — independent by construction — verifies the
//! draft against the cited sources only. The lead reconciles: on
//! conflict it runs one narrowed re-query and, if the conflict stands,
//! FLAGS it in the answer. It never invents a resolution.
//!
//! Hard boundaries enforced here, not by prompt alone:
//! - Brief scoping: every grounded leg call carries the caller's
//!   `scope_docs`, and scoped calls are STRICT (no full-vault fallback),
//!   so no sub-agent can ever work across briefs.
//! - Checker independence: the checker prompt is built from the draft +
//!   the cited sources only — never the drafter's rationale, never the
//!   researchers' raw findings beyond what is cited.
//! - No sub-sub-agents: the lead dispatches specialists only; the
//!   reconciliation re-query is a direct leg call, not a new agent.
//! - Budgets: per-sub-agent token budget and a per-run cap (admin knob
//!   via the server settings). Budgets are enforced at stage
//!   boundaries — a single provider call cannot be interrupted
//!   mid-generation, so an overshoot is flagged on its receipt and the
//!   run cap stops later stages. Tokens are measured with child usage
//!   scopes per sub-agent (see [`crate::usage`]) and merged into the
//!   caller's scope, so runs bill through the existing usage path
//!   exactly like any other LLM call.
//! - Timeouts: each sub-agent stage has its own deadline; a timeout
//!   aborts that specialist and the run continues with the rest
//!   (stopped = "partial"), so one runaway specialist cannot hang a run.
//!
//! No new tool surface: specialists use only the existing grounded
//! query legs (corpus + web). User-invoked only; nothing here runs on
//! a schedule, and runs leave no memory across briefs.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use legal_rag_common::Result;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::audit::{audit_answer, CitationAudit};
use crate::query::{Citation, QueryEvent, QueryMode, QueryService};
use crate::usage;

/// Orchestrator defaults (the commissioned values; the per-run cap is
/// overridable by the admin settings knob on the server).
pub const DEFAULT_MAX_PARALLEL_RESEARCHERS: usize = 3;
pub const DEFAULT_PER_SUBAGENT_TOKEN_BUDGET: u64 = 25_000;
pub const DEFAULT_PER_RUN_TOKEN_CAP: u64 = 100_000;
pub const DEFAULT_PER_SUBAGENT_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    pub max_parallel_researchers: usize,
    pub per_subagent_token_budget: u64,
    pub per_run_token_cap: u64,
    pub per_subagent_timeout_secs: u64,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_parallel_researchers: DEFAULT_MAX_PARALLEL_RESEARCHERS,
            per_subagent_token_budget: DEFAULT_PER_SUBAGENT_TOKEN_BUDGET,
            per_run_token_cap: DEFAULT_PER_RUN_TOKEN_CAP,
            per_subagent_timeout_secs: DEFAULT_PER_SUBAGENT_TIMEOUT_SECS,
        }
    }
}

/// One sub-agent's receipt: what it did, what it contributed, what it
/// cost, and whether it finished cleanly. Receipts are the provenance
/// record — they ride the SSE Done event and the JSON response alike.
#[derive(Debug, Clone, Serialize)]
pub struct SubAgentReceipt {
    /// lead | researcher | drafter | checker | reconciler
    pub role: String,
    /// e.g. "researcher-1", "drafter", "checker".
    pub name: String,
    /// The task this sub-agent was given (branch query, or role task).
    pub input: String,
    /// What it contributed (findings excerpt, draft summary, issues).
    pub contribution: String,
    pub citations: Vec<Citation>,
    pub tokens_used: u64,
    /// completed | timeout | failed | budget_exceeded | skipped
    pub status: String,
    pub elapsed_ms: u64,
}

impl SubAgentReceipt {
    fn new(role: &str, name: &str, input: &str) -> Self {
        Self {
            role: role.to_string(),
            name: name.to_string(),
            input: input.to_string(),
            contribution: String::new(),
            citations: Vec::new(),
            tokens_used: 0,
            status: "completed".to_string(),
            elapsed_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OrchestratedRun {
    pub goal: String,
    pub final_report: String,
    pub citations: Vec<Citation>,
    /// Per-sub-agent receipts, in run order.
    pub sub_agents: Vec<SubAgentReceipt>,
    /// Unresolved conflicts, flagged in the answer (never hidden).
    pub conflicts: Vec<String>,
    /// What reconciliation did (re-query evidence, narrowing, flags).
    pub reconciliations: Vec<String>,
    /// Mechanical citation-integrity audit of the draft.
    pub citation_audit: Option<CitationAudit>,
    pub total_tokens: u64,
    pub elapsed_ms: u64,
    /// finished | partial | budget | drafter_error
    pub stopped: String,
}

const LEAD_SYSTEM: &str = "You are the lead agent of a legal-research team. \
Decompose the GOAL into independent research branches that can be investigated \
in parallel. Each branch must be a single specific question answerable from \
documents or the web, and branches must not depend on each other's answers. \
Respond with EXACTLY one JSON object, no other text: \
{\"branches\": [{\"query\": \"<specific research question>\", \"why\": \"<one short sentence>\"}]}. \
Use 1 branch for a narrow goal and at most 3 branches for a broad one; never \
more than 3. Do not answer the goal yourself — only decompose it.";

const DRAFTER_SYSTEM: &str = "You are a senior legal researcher drafting the final answer of a \
research run. Write from the RESEARCHERS' FINDINGS AND CITED SOURCES ONLY: \
verdict first, then analysis, citing sources inline as [doc:N] / [web:N] exactly \
as they appear in the findings. Say plainly what the findings do not establish. \
Do not invent facts, cases, or citations, and do not use any knowledge beyond \
the findings and sources below.";

const CHECKER_SYSTEM: &str = "You are an independent checker. You receive a DRAFT and the CITED \
SOURCES it relies on — nothing else. Judge only whether the draft's verdict \
and claims follow from those cited passages. Respond with EXACTLY one JSON \
object, no other text: {\"verdict_supported\": true|false, \"issues\": [\"<each \
claim that the cited sources do not support, or each place where cited sources \
contradict each other or the draft>\"]}. An empty issues list means the draft \
is fully supported. Do not rewrite the draft and do not use outside knowledge.";

#[derive(Debug, serde::Deserialize)]
struct LeadPlan {
    #[serde(default)]
    branches: Vec<LeadBranch>,
}

#[derive(Debug, serde::Deserialize)]
struct LeadBranch {
    #[serde(default)]
    query: String,
    #[allow(dead_code)]
    #[serde(default)]
    why: String,
}

#[derive(Debug, serde::Deserialize)]
struct CheckerVerdict {
    #[serde(default)]
    verdict_supported: bool,
    #[serde(default)]
    issues: Vec<String>,
}

/// Extract a JSON object from a raw completion (tolerant of fences/prose).
fn parse_json_object<T: serde::de::DeserializeOwned>(raw: &str) -> Option<T> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<T>(&raw[start..=end]).ok()
}

/// Extract the first brace-matched JSON object from `raw`, honouring
/// string literals (so a `}` inside a quoted issue string does not end
/// the object early, and trailing prose after the object is ignored).
fn first_json_object(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    let mut start = None;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if start.is_none() {
            if b == b'{' {
                start = Some(i);
                depth = 1;
            }
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start.unwrap()..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Parse a checker verdict leniently. The checker model sometimes wraps
/// its JSON in prose or emits a malformed issues list; a strict-only
/// parse would discard a usable verdict. Fallback: if a
/// `verdict_supported` boolean can be found at all, trust it, and when
/// it is false but the issues list is unrecoverable, flag exactly that
/// (a flagged unknown is honest; a silently dropped verdict is not).
/// Returns None only when no verdict boolean is present anywhere.
fn parse_checker_verdict(raw: &str) -> Option<CheckerVerdict> {
    if let Some(obj) = first_json_object(raw) {
        if let Ok(v) = serde_json::from_str::<CheckerVerdict>(obj) {
            return Some(v);
        }
    }
    // Lenient scan for the verdict boolean (handles `"true"` strings
    // and prose-wrapped output alike).
    let lower = raw.to_lowercase();
    let key = lower.find("verdict_supported")?;
    let after = &lower[key..];
    let value = after
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|t| *t == "true" || *t == "false")?;
    let supported = value == "true";
    Some(CheckerVerdict {
        verdict_supported: supported,
        issues: if supported {
            Vec::new()
        } else {
            vec!["checker verdict: not supported (detailed issues unparseable — see the cited sources)".to_string()]
        },
    })
}

/// Plan branches from a lead completion; falls back to the goal itself
/// when the plan is unusable (the run must not die on a bad plan).
fn branches_from_plan(raw: &str, goal: &str, max: usize) -> Vec<String> {
    let max = max.clamp(1, 8);
    let mut out: Vec<String> = Vec::new();
    if let Some(plan) = parse_json_object::<LeadPlan>(raw) {
        for b in plan.branches {
            let q = b.query.trim().to_string();
            if !q.is_empty() && !out.contains(&q) {
                out.push(q);
            }
            if out.len() >= max {
                break;
            }
        }
    }
    if out.is_empty() {
        out.push(goal.to_string());
    }
    out
}

/// Is `s` a citation marker key (`doc:3`, `web:0`, `private:1`)?
fn is_marker_key(s: &str) -> bool {
    match s.split_once(':') {
        Some((kind, n)) => {
            matches!(kind, "doc" | "web" | "private")
                && !n.is_empty()
                && n.chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

/// Remap every citation marker in `text` through `map` (old key ->
/// new key). Brackets that are not purely markers pass through
/// untouched, so ordinary bracketed prose is never corrupted.
fn remap_markers(text: &str, map: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find(']') {
            Some(close) => {
                let inner = &after[..close];
                let keys: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
                if !keys.is_empty() && keys.iter().all(|k| is_marker_key(k)) {
                    let remapped: Vec<String> = keys
                        .iter()
                        .map(|k| map.get(*k).cloned().unwrap_or_else(|| k.to_string()))
                        .collect();
                    out.push('[');
                    out.push_str(&remapped.join(", "));
                    out.push(']');
                } else {
                    out.push('[');
                    out.push_str(inner);
                    out.push(']');
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Build the checker input from the draft + cited sources ONLY. This is
/// the independence boundary: no drafter rationale, no researcher
/// findings beyond the cited sources, no plan — the checker judges the
/// draft against its sources and nothing else.
fn checker_input(goal: &str, draft: &str, citations: &[Citation]) -> String {
    let mut sources = String::new();
    for c in citations {
        let title = c.doc_title.clone().unwrap_or_else(|| c.doc_id.clone());
        // The FULL citation snippet, not the one-sentence pinpoint:
        // verdict-consistency needs the passage around the claim, and
        // live eval showed a pinpoint-only excerpt making the checker
        // flag a claim its source does support. Still only the cited
        // sources — no other findings, no rationale.
        sources.push_str(&format!("[{}] {}\n{}\n\n", c.doc_id, title, c.snippet));
    }
    if sources.is_empty() {
        sources.push_str("(no cited sources — every claim in the draft is therefore unsupported)\n");
    }
    format!("GOAL: {goal}\n\nDRAFT:\n{draft}\n\nCITED SOURCES (the ONLY sources the draft may rely on):\n{sources}")
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Fold one researcher's answer into the run's global citation space:
/// dedupe by chunk id, remap the branch's per-answer markers onto
/// global keys, and return (branch citations, remapped answer text).
/// Shared by the parallel wave and the sequential retry so both paths
/// produce identical citation numbering.
#[allow(clippy::too_many_arguments)]
fn incorporate_branch(
    ans: &crate::query::QueryAnswer,
    global_citations: &mut Vec<Citation>,
    by_chunk: &mut HashMap<String, String>,
    doc_counter: &mut usize,
    web_counter: &mut usize,
) -> (Vec<Citation>, String) {
    let mut map: HashMap<String, String> = HashMap::new();
    let mut branch_citations: Vec<Citation> = Vec::new();
    for c in &ans.citations {
        if let Some(existing) = by_chunk.get(&c.chunk_id) {
            map.insert(c.doc_id.clone(), existing.clone());
            continue;
        }
        let (kind, _) = c.doc_id.split_once(':').unwrap_or(("doc", "0"));
        let new_key = if kind == "web" {
            let k = format!("web:{web_counter}");
            *web_counter += 1;
            k
        } else {
            let k = format!("doc:{doc_counter}");
            *doc_counter += 1;
            k
        };
        by_chunk.insert(c.chunk_id.clone(), new_key.clone());
        map.insert(c.doc_id.clone(), new_key.clone());
        let mut gc = c.clone();
        gc.doc_id = new_key;
        global_citations.push(gc.clone());
        branch_citations.push(gc);
    }
    (branch_citations, remap_markers(&ans.answer, &map))
}

pub struct SubAgentOrchestrator {
    query: Arc<QueryService>,
    /// Admin-set standing instructions, injected into the drafter
    /// (the synthesis surface), mirroring the workflow engine seam.
    instruction: std::sync::RwLock<Option<String>>,
}

impl SubAgentOrchestrator {
    pub fn new(query: Arc<QueryService>) -> Self {
        Self {
            query,
            instruction: std::sync::RwLock::new(None),
        }
    }

    pub fn set_instruction(&self, instruction: Option<String>) {
        let instruction = instruction.filter(|s| !s.trim().is_empty());
        *self.instruction.write().unwrap() = instruction;
    }

    fn instruction(&self) -> Option<String> {
        self.instruction.read().unwrap().clone()
    }

    fn progress(&self, tx: &Option<UnboundedSender<QueryEvent>>, message: &str) {
        if let Some(tx) = tx {
            let _ = tx.send(QueryEvent::Status {
                message: message.to_string(),
            });
        }
    }

    /// Run one grounded leg call for a researcher branch (corpus + web,
    /// strictly brief-scoped when `scope_docs` is set — no fallback).
    async fn researcher_query(
        &self,
        branch: &str,
        scope_docs: &Option<Vec<String>>,
    ) -> Result<crate::query::QueryAnswer> {
        match scope_docs {
            Some(docs) => {
                self.query
                    .query_with_web(branch, QueryMode::Mix, 4, 2, Some(docs.clone()), true, crate::query::CitatorContext::default())
                    .await
            }
            None => {
                self.query
                    .query_with_web(branch, QueryMode::Mix, 4, 2, None, false, crate::query::CitatorContext::default())
                    .await
            }
        }
    }

    /// Run the full lead → researchers → drafter → checker →
    /// reconciler pipeline. `progress` receives status events for SSE
    /// streaming; `None` runs silently for the JSON response.
    pub async fn run(
        &self,
        goal: &str,
        scope_docs: Option<Vec<String>>,
        config: OrchestratorConfig,
        progress: Option<UnboundedSender<QueryEvent>>,
    ) -> Result<OrchestratedRun> {
        let started = Instant::now();
        let mut receipts: Vec<SubAgentReceipt> = Vec::new();
        let mut total_tokens: u64 = 0;
        let mut stopped = "finished".to_string();
        let timeout = Duration::from_secs(config.per_subagent_timeout_secs.max(1));

        // --- budget helpers (stage boundaries) -------------------------
        let over_cap = |total: u64| total >= config.per_run_token_cap;

        // --- Stage 1: lead plans branches -------------------------------
        let branches: Vec<String>;
        {
            let t0 = Instant::now();
            let child = usage::new_scope();
            let child2 = child.clone();
            let goal_owned = goal.to_string();
            let fut = usage::scope_usage(child2, async {
                self.query
                    .llm()
                    .complete(LEAD_SYSTEM, &format!("GOAL: {goal_owned}"))
                    .await
            });
            let mut receipt = SubAgentReceipt::new("lead", "lead-plan", goal);
            match tokio::time::timeout(timeout, fut).await {
                Ok(Ok(raw)) => {
                    branches = branches_from_plan(
                        &raw,
                        goal,
                        config.max_parallel_researchers.clamp(1, 8),
                    );
                    receipt.contribution = format!(
                        "Planned {} research branch(es): {}",
                        branches.len(),
                        branches.join(" | ")
                    );
                }
                Ok(Err(e)) => {
                    branches = vec![goal.to_string()];
                    receipt.status = "failed".to_string();
                    receipt.contribution =
                        format!("Lead planning failed ({e}); falling back to the goal as a single branch.");
                }
                Err(_) => {
                    branches = vec![goal.to_string()];
                    receipt.status = "timeout".to_string();
                    receipt.contribution = format!(
                        "Lead planning timed out after {}s; falling back to the goal as a single branch.",
                        config.per_subagent_timeout_secs
                    );
                }
            }
            let tokens = child.lock().map(|t| t.total()).unwrap_or(0);
            receipt.tokens_used = tokens;
            if tokens > config.per_subagent_token_budget {
                receipt.status = "budget_exceeded".to_string();
            }
            receipt.elapsed_ms = t0.elapsed().as_millis() as u64;
            usage::record_llm(
                child.lock().map(|t| t.llm_prompt_tokens).unwrap_or(0),
                child.lock().map(|t| t.llm_completion_tokens).unwrap_or(0),
            );
            usage::record_embeddings(child.lock().map(|t| t.embedding_tokens).unwrap_or(0));
            total_tokens += tokens;
            self.progress(&progress, &format!("lead: planned {} branch(es)", branches.len()));
            receipts.push(receipt);
        }

        if over_cap(total_tokens) {
            stopped = "budget".to_string();
        }

        // --- Stage 2: researchers, concurrently (P2) --------------------
        // Global citation registry: dedupe by chunk id, remap each
        // branch's per-answer markers onto global keys so the drafter
        // and checker share one unambiguous citation space.
        let mut global_citations: Vec<Citation> = Vec::new();
        let mut by_chunk: HashMap<String, String> = HashMap::new();
        let mut doc_counter = 0usize;
        let mut web_counter = 0usize;
        let mut findings_text = String::new();
        let mut any_partial = false;

        if stopped != "budget" {
            let deadline = Instant::now() + timeout;
            let mut handles = Vec::new();
            for (i, branch) in branches.iter().enumerate() {
                let child = usage::new_scope();
                let child2 = child.clone();
                let svc = self.query.clone();
                let scope = scope_docs.clone();
                let branch_owned = branch.clone();
                let handle = tokio::spawn(async move {
                    usage::scope_usage(child2, async move {
                        match &scope {
                            Some(docs) => {
                                svc.query_with_web(
                                    &branch_owned,
                                    QueryMode::Mix,
                                    4,
                                    2,
                                    Some(docs.clone()),
                                    true,
                                    crate::query::CitatorContext::default(),
                                )
                                .await
                            }
                            None => {
                                svc.query_with_web(
                                    &branch_owned,
                                    QueryMode::Mix,
                                    4,
                                    2,
                                    None,
                                    false,
                                    crate::query::CitatorContext::default(),
                                )
                                .await
                            }
                        }
                    })
                    .await
                });
                handles.push((i, branch.clone(), child, handle));
                self.progress(
                    &progress,
                    &format!("researcher-{}: started ({})", i + 1, branches[i]),
                );
            }
            for (i, branch, child, mut handle) in handles {
                let t0 = Instant::now();
                let mut receipt =
                    SubAgentReceipt::new("researcher", &format!("researcher-{}", i + 1), &branch);
                let outcome = tokio::time::timeout_at(deadline.into(), &mut handle).await;
                let answer = match outcome {
                    Ok(Ok(Ok(ans))) => Some(ans),
                    Ok(Ok(Err(e))) => {
                        receipt.status = "failed".to_string();
                        receipt.contribution = format!("Research leg failed: {e}");
                        any_partial = true;
                        None
                    }
                    Ok(Err(e)) => {
                        receipt.status = "failed".to_string();
                        receipt.contribution = format!("Research task failed: {e}");
                        any_partial = true;
                        None
                    }
                    Err(_) => {
                        // Abort: one runaway specialist must not keep
                        // consuming the provider in the background.
                        handle.abort();
                        receipt.status = "timeout".to_string();
                        receipt.contribution = format!(
                            "Research timed out after {}s; branch contributes nothing.",
                            config.per_subagent_timeout_secs
                        );
                        any_partial = true;
                        None
                    }
                };
                // Tokens recorded before the abort are still counted
                // below (the child scope is shared with the task).
                let totals = child.lock().map(|t| *t).unwrap_or_default();
                receipt.tokens_used = totals.total();
                if receipt.status == "completed"
                    && totals.total() > config.per_subagent_token_budget
                {
                    receipt.status = "budget_exceeded".to_string();
                }
                receipt.elapsed_ms = t0.elapsed().as_millis() as u64;
                usage::record_llm(totals.llm_prompt_tokens, totals.llm_completion_tokens);
                usage::record_embeddings(totals.embedding_tokens);
                total_tokens += totals.total();
                if let Some(ans) = answer {
                    let (branch_citations, remapped) = incorporate_branch(
                        &ans,
                        &mut global_citations,
                        &mut by_chunk,
                        &mut doc_counter,
                        &mut web_counter,
                    );
                    receipt.citations = branch_citations.clone();
                    receipt.contribution = truncate(&remapped, 1200);
                    if !findings_text.is_empty() {
                        findings_text.push_str("\n\n");
                    }
                    findings_text.push_str(&format!(
                        "## Branch {} — {}\n{}\n\nCited sources in this branch:\n{}",
                        i + 1,
                        branch,
                        remapped,
                        branch_citations
                            .iter()
                            .map(|c| format!(
                                "[{}] {}",
                                c.doc_id,
                                c.doc_title.clone().unwrap_or_default()
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ));
                }
                self.progress(
                    &progress,
                    &format!("researcher-{}: {}", i + 1, receipt.status),
                );
                receipts.push(receipt);
                if over_cap(total_tokens) {
                    stopped = "budget".to_string();
                    break;
                }
            }

            // Sequential retry for branches whose PARALLEL attempt
            // timed out: on a contended provider the same leg call
            // usually succeeds alone (verified on the anonymous
            // Gemini bridge, where one leg call takes ~80s but three
            // in parallel blow the 120s deadline). Bounded — one
            // retry per timed-out branch, same timeout — so the run
            // still cannot hang, and on a fast provider this never
            // triggers. Failed (non-timeout) branches are not retried.
            if stopped != "budget" {
                let timed_out: Vec<(String, String)> = receipts
                    .iter()
                    .filter(|r| r.role == "researcher" && r.status == "timeout")
                    .filter_map(|r| {
                        r.name
                            .strip_prefix("researcher-")
                            .and_then(|n| n.parse::<usize>().ok())
                            .and_then(|n| branches.get(n - 1).map(|b| (r.name.clone(), b.clone())))
                    })
                    .collect();
                for (name, branch) in timed_out {
                    if over_cap(total_tokens) {
                        stopped = "budget".to_string();
                        break;
                    }
                    self.progress(&progress, &format!("{name}: sequential retry"));
                    let t0 = Instant::now();
                    let child = usage::new_scope();
                    let child2 = child.clone();
                    let branch_owned = branch.clone();
                    let fut = usage::scope_usage(child2, async {
                        self.researcher_query(&branch_owned, &scope_docs).await
                    });
                    let retry = tokio::time::timeout(timeout, fut).await;
                    let totals = child.lock().map(|t| *t).unwrap_or_default();
                    usage::record_llm(totals.llm_prompt_tokens, totals.llm_completion_tokens);
                    usage::record_embeddings(totals.embedding_tokens);
                    total_tokens += totals.total();
                    match retry {
                        Ok(Ok(ans)) => {
                            let idx = branches.iter().position(|b| *b == branch).unwrap_or(0);
                            let (branch_citations, remapped) = incorporate_branch(
                                &ans,
                                &mut global_citations,
                                &mut by_chunk,
                                &mut doc_counter,
                                &mut web_counter,
                            );
                            if !findings_text.is_empty() {
                                findings_text.push_str("\n\n");
                            }
                            findings_text.push_str(&format!(
                                "## Branch {} — {}\n{}\n\nCited sources in this branch:\n{}",
                                idx + 1,
                                branch,
                                remapped,
                                branch_citations
                                    .iter()
                                    .map(|c| format!(
                                        "[{}] {}",
                                        c.doc_id,
                                        c.doc_title.clone().unwrap_or_default()
                                    ))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            ));
                            if let Some(r) =
                                receipts.iter_mut().find(|r| r.name == name)
                            {
                                r.status = "completed".to_string();
                                r.citations = branch_citations;
                                r.contribution = format!(
                                    "Parallel attempt timed out; sequential retry succeeded. {}",
                                    truncate(&remapped, 1200)
                                );
                                r.tokens_used += totals.total();
                                r.elapsed_ms += t0.elapsed().as_millis() as u64;
                            }
                            // A rescued branch is no longer partial.
                            if !receipts.iter().any(|r| {
                                r.role == "researcher"
                                    && (r.status == "timeout" || r.status == "failed")
                            }) {
                                any_partial = false;
                            }
                            self.progress(&progress, &format!("{name}: retry succeeded"));
                        }
                        Ok(Err(e)) => {
                            if let Some(r) =
                                receipts.iter_mut().find(|r| r.name == name)
                            {
                                r.tokens_used += totals.total();
                                r.contribution =
                                    format!("{} Sequential retry also failed: {e}", r.contribution);
                            }
                            self.progress(&progress, &format!("{name}: retry failed"));
                        }
                        Err(_) => {
                            if let Some(r) =
                                receipts.iter_mut().find(|r| r.name == name)
                            {
                                r.tokens_used += totals.total();
                                r.contribution =
                                    format!("{} Sequential retry also timed out.", r.contribution);
                            }
                            self.progress(&progress, &format!("{name}: retry timed out"));
                        }
                    }
                }
            }
        }

        // --- Stage 3: drafter -------------------------------------------
        let mut draft = String::new();
        let mut draft_ok = false;
        if stopped != "budget" {
            let t0 = Instant::now();
            let child = usage::new_scope();
            let child2 = child.clone();
            let drafter_system = match self.instruction() {
                Some(extra) => format!(
                    "{DRAFTER_SYSTEM}\n\nStanding instructions from the workspace admin (follow them unless they conflict with the grounding rules above):\n{extra}"
                ),
                None => DRAFTER_SYSTEM.to_string(),
            };
            let findings = if findings_text.is_empty() {
                "(no researcher findings — no sources were retrieved; say so plainly)".to_string()
            } else {
                findings_text.clone()
            };
            let fut = usage::scope_usage(child2, async {
                self.query.llm().complete(
                    &drafter_system,
                    &format!("GOAL: {goal}\n\nRESEARCHERS' FINDINGS AND CITED SOURCES:\n{findings}"),
                )
                .await
            });
            let mut receipt = SubAgentReceipt::new("drafter", "drafter", goal);
            match tokio::time::timeout(timeout, fut).await {
                Ok(Ok(text)) => {
                    draft = text.clone();
                    draft_ok = true;
                    receipt.contribution = truncate(&text, 1200);
                }
                Ok(Err(e)) => {
                    receipt.status = "failed".to_string();
                    receipt.contribution = format!("Drafting failed: {e}");
                    any_partial = true;
                }
                Err(_) => {
                    receipt.status = "timeout".to_string();
                    receipt.contribution = format!(
                        "Drafting timed out after {}s.",
                        config.per_subagent_timeout_secs
                    );
                    any_partial = true;
                }
            }
            let totals = child.lock().map(|t| *t).unwrap_or_default();
            receipt.tokens_used = totals.total();
            if receipt.status == "completed"
                && totals.total() > config.per_subagent_token_budget
            {
                receipt.status = "budget_exceeded".to_string();
            }
            receipt.elapsed_ms = t0.elapsed().as_millis() as u64;
            usage::record_llm(totals.llm_prompt_tokens, totals.llm_completion_tokens);
            usage::record_embeddings(totals.embedding_tokens);
            total_tokens += totals.total();
            self.progress(&progress, &format!("drafter: {}", receipt.status));
            receipts.push(receipt);
            if over_cap(total_tokens) {
                stopped = "budget".to_string();
            }
        }

        if !draft_ok {
            // Never hard-fail a user-visible run on a drafting error:
            // ship the grounded findings with an explicit flag instead.
            let fallback = if findings_text.is_empty() {
                "No findings could be gathered and drafting failed; no answer can be given from the sources.".to_string()
            } else {
                format!(
                    "Drafting failed; the researchers' grounded findings are reported verbatim below (no synthesized verdict is asserted).\n\n{findings_text}"
                )
            };
            return Ok(OrchestratedRun {
                goal: goal.to_string(),
                final_report: fallback,
                citations: global_citations,
                sub_agents: receipts,
                conflicts: vec!["drafting failed — no synthesized verdict is asserted".to_string()],
                reconciliations: vec!["drafter failed; findings reported verbatim".to_string()],
                citation_audit: None,
                total_tokens,
                elapsed_ms: started.elapsed().as_millis() as u64,
                stopped: if stopped == "budget" {
                    "budget".to_string()
                } else {
                    "drafter_error".to_string()
                },
            });
        }

        // --- Stage 4: checker (independent context by construction) -----
        let mut conflicts: Vec<String> = Vec::new();
        let mut audit_receipt: Option<CitationAudit> = None;
        let mut checker_note = String::new();
        if stopped != "budget" {
            let t0 = Instant::now();
            let child = usage::new_scope();
            let child2 = child.clone();
            // The ONLY inputs: draft + cited sources. The checker never
            // sees the drafter's rationale or the plan.
            let input = checker_input(goal, &draft, &global_citations);
            let fut = usage::scope_usage(child2, async {
                self.query.llm().complete(CHECKER_SYSTEM, &input).await
            });
            let mut receipt = SubAgentReceipt::new("checker", "checker", "draft + cited sources only");
            let mut verdict: Option<CheckerVerdict> = None;
            match tokio::time::timeout(timeout, fut).await {
                Ok(Ok(raw)) => {
                    verdict = parse_checker_verdict(&raw);
                    if verdict.is_none() {
                        receipt.status = "failed".to_string();
                        checker_note =
                            "checker verdict unparseable — verdict-consistency check is inconclusive; citation audit still applies".to_string();
                    }
                }
                Ok(Err(e)) => {
                    receipt.status = "failed".to_string();
                    receipt.contribution = format!("Checker failed: {e}");
                    checker_note = "checker call failed — verdict-consistency check did not run; citation audit still applies".to_string();
                    any_partial = true;
                }
                Err(_) => {
                    receipt.status = "timeout".to_string();
                    receipt.contribution = format!(
                        "Checker timed out after {}s.",
                        config.per_subagent_timeout_secs
                    );
                    checker_note = "checker timed out — verdict-consistency check did not run; citation audit still applies".to_string();
                    any_partial = true;
                }
            }
            // Mechanical citation-integrity audit: always runs, even
            // when the checker's LLM verdict is unavailable.
            let audit_sources: Vec<(String, String)> = global_citations
                .iter()
                .map(|c| (c.doc_id.clone(), c.snippet.clone()))
                .collect();
            let (cleaned, audit) = audit_answer(&draft, &audit_sources);
            draft = cleaned;
            audit_receipt = Some(audit);
            if let Some(v) = &verdict {
                if !v.verdict_supported || !v.issues.is_empty() {
                    conflicts.extend(v.issues.clone());
                }
                receipt.contribution = format!(
                    "verdict_supported={} issues={} audit: {} checked, {} verified, {} stripped",
                    v.verdict_supported,
                    v.issues.len(),
                    audit.checked,
                    audit.verified,
                    audit.stripped
                );
            } else if receipt.status == "completed" {
                receipt.contribution = format!(
                    "audit: {} checked, {} verified, {} stripped",
                    audit.checked, audit.verified, audit.stripped
                );
            }
            if audit.stripped > 0 {
                conflicts.push(format!(
                    "citation audit stripped {} unsupported marker(s) from the draft; those claims ship without the stripped citations",
                    audit.stripped
                ));
            }
            let totals = child.lock().map(|t| *t).unwrap_or_default();
            receipt.tokens_used = totals.total();
            if receipt.status == "completed"
                && totals.total() > config.per_subagent_token_budget
            {
                receipt.status = "budget_exceeded".to_string();
            }
            receipt.elapsed_ms = t0.elapsed().as_millis() as u64;
            receipt.citations = global_citations.clone();
            usage::record_llm(totals.llm_prompt_tokens, totals.llm_completion_tokens);
            usage::record_embeddings(totals.embedding_tokens);
            total_tokens += totals.total();
            self.progress(&progress, &format!("checker: {}", receipt.status));
            receipts.push(receipt);
            if over_cap(total_tokens) {
                stopped = "budget".to_string();
            }
        }

        // --- Stage 5: lead reconciliation --------------------------------
        let mut reconciliations: Vec<String> = Vec::new();
        let mut final_report = draft.clone();
        if !checker_note.is_empty() {
            reconciliations.push(checker_note.clone());
        }
        if stopped == "budget" {
            reconciliations.push(
                "run token cap reached before reconciliation; conflicts below (if any) are flagged unresolved"
                    .to_string(),
            );
        } else if conflicts.is_empty() {
            reconciliations.push(
                "checker found no conflicts and the citation audit stripped nothing; the audited draft stands"
                    .to_string(),
            );
            let receipt = SubAgentReceipt {
                role: "reconciler".to_string(),
                name: "reconciler".to_string(),
                input: "checked draft, no conflicts".to_string(),
                contribution: "No conflicts to reconcile; draft stands as audited.".to_string(),
                citations: Vec::new(),
                tokens_used: 0,
                status: "completed".to_string(),
                elapsed_ms: 0,
            };
            receipts.push(receipt);
        } else {
            // One narrowed re-query on the first disputed claim, then
            // flag: the lead never invents a resolution, and a conflict
            // that survives re-query ships flagged, not hidden.
            let disputed = conflicts.first().cloned().unwrap_or_default();
            let t0 = Instant::now();
            let child = usage::new_scope();
            let child2 = child.clone();
            let requery = format!(
                "Verify this disputed claim against the brief documents, citing sources: {disputed}"
            );
            let fut = usage::scope_usage(child2, async {
                self.researcher_query(&requery, &scope_docs).await
            });
            let mut receipt =
                SubAgentReceipt::new("reconciler", "reconciler", &requery);
            let evidence = match tokio::time::timeout(timeout, fut).await {
                Ok(Ok(ans)) => {
                    receipt.citations = ans.citations.clone();
                    let ev = truncate(&ans.answer, 800);
                    if ev.is_empty() {
                        "(re-query returned no answer text)".to_string()
                    } else {
                        ev
                    }
                }
                Ok(Err(e)) => {
                    receipt.status = "failed".to_string();
                    format!("(re-query failed: {e})")
                }
                Err(_) => {
                    receipt.status = "timeout".to_string();
                    format!(
                        "(re-query timed out after {}s)",
                        config.per_subagent_timeout_secs
                    )
                }
            };
            let totals = child.lock().map(|t| *t).unwrap_or_default();
            receipt.tokens_used = totals.total();
            if receipt.status == "completed"
                && totals.total() > config.per_subagent_token_budget
            {
                receipt.status = "budget_exceeded".to_string();
            }
            receipt.elapsed_ms = t0.elapsed().as_millis() as u64;
            receipt.contribution = evidence.clone();
            usage::record_llm(totals.llm_prompt_tokens, totals.llm_completion_tokens);
            usage::record_embeddings(totals.embedding_tokens);
            total_tokens += totals.total();
            self.progress(&progress, &format!("reconciler: {}", receipt.status));
            receipts.push(receipt);
            reconciliations.push(format!(
                "re-queried the disputed claim; evidence: {evidence}"
            ));
            reconciliations.push(
                "conflict NOT resolved by invention — it is flagged below and the draft's verdict is not asserted for the disputed claim(s)"
                    .to_string(),
            );
            let mut flagged = String::from(
                "\n\n> ⚠ Conflicts flagged: the checked draft disagreed with its cited sources \
                 (or sources disagreed with each other). The verdict above is NOT asserted for the \
                 disputed claim(s) below — read them against the cited sources.\n\n## Reconciliation — conflicts flagged\n",
            );
            for c in &conflicts {
                flagged.push_str(&format!("- {c}\n"));
            }
            flagged.push_str(&format!("\nRe-query evidence: {evidence}\n"));
            flagged.push_str(
                "\nStatus: UNRESOLVED — no resolution is asserted; where sources conflict, both are cited and neither is silently preferred.\n",
            );
            final_report.push_str(&flagged);
        }

        if stopped != "budget" && any_partial {
            stopped = "partial".to_string();
        }

        Ok(OrchestratedRun {
            goal: goal.to_string(),
            final_report,
            citations: global_citations,
            sub_agents: receipts,
            conflicts,
            reconciliations,
            citation_audit: audit_receipt,
            total_tokens,
            elapsed_ms: started.elapsed().as_millis() as u64,
            stopped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbeddings;
    use crate::graph::InMemoryGraphStore;
    use crate::llm::MockLlm;
    use crate::vector_store::InMemoryVectorStore;
    use crate::web::NoopWebSearch;

    fn orchestrator_with(llm_response: &str) -> SubAgentOrchestrator {
        let svc = QueryService::new(
            Arc::new(MockEmbeddings::new(8)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(MockLlm::new(llm_response)),
            Arc::new(|_: &str| None),
            Arc::new(NoopWebSearch),
            None,
        );
        SubAgentOrchestrator::new(Arc::new(svc))
    }

    #[test]
    fn defaults_match_the_commissioned_numbers() {
        let c = OrchestratorConfig::default();
        assert_eq!(c.max_parallel_researchers, 3);
        assert_eq!(c.per_subagent_token_budget, 25_000);
        assert_eq!(c.per_run_token_cap, 100_000);
        assert_eq!(c.per_subagent_timeout_secs, 120);
    }

    #[test]
    fn plan_parsing_caps_branches_and_falls_back() {
        let raw = r#"{"branches": [
            {"query": "q1", "why": "a"}, {"query": "q2", "why": "b"},
            {"query": "q3", "why": "c"}, {"query": "q4", "why": "d"}]}"#;
        let b = branches_from_plan(raw, "goal", 3);
        assert_eq!(b, vec!["q1", "q2", "q3"]);
        assert_eq!(branches_from_plan("no json", "goal", 3), vec!["goal"]);
        assert_eq!(branches_from_plan("", "goal", 3), vec!["goal"]);
    }

    #[test]
    fn marker_remap_rekeys_and_leaves_prose_alone() {
        let mut map = HashMap::new();
        map.insert("doc:0".to_string(), "doc:5".to_string());
        map.insert("web:0".to_string(), "web:2".to_string());
        let out = remap_markers(
            "Held [doc:0] and [web:0, doc:0]; also [see section 4] and [not a marker].",
            &map,
        );
        assert_eq!(
            out,
            "Held [doc:5] and [web:2, doc:5]; also [see section 4] and [not a marker]."
        );
    }

    #[test]
    fn checker_input_contains_only_draft_and_sources() {
        let draft = "The verdict is X [doc:0].";
        let rationale = "SECRET-DRAFTER-RATIONALE: I guessed X because of Y.";
        let citations = vec![Citation {
            doc_id: "doc:0".to_string(),
            doc_title: Some("Source Doc".to_string()),
            chunk_id: "chunk-1".to_string(),
            ordinal: 0,
            score: 1.0,
            snippet: "the source passage with full context around the claim".to_string(),
            sentence: Some("one pinpoint sentence".to_string()),
            stale: None,
            source_url: None,
            source_date: None,
            treatment: None,
        }];
        let input = checker_input("goal?", draft, &citations);
        assert!(input.contains(draft));
        // Full snippet, not just the pinpoint sentence.
        assert!(input.contains("the source passage with full context around the claim"));
        // The rationale is never an input: it is not in the string at
        // all, and no other text beyond draft + sources + goal is.
        assert!(!input.contains(rationale));
        assert!(!input.contains("SECRET"));
    }

    #[test]
    fn checker_verdict_parsing_is_lenient() {
        // Clean JSON, fenced JSON, prose-wrapped JSON, and a verdict
        // boolean surviving a malformed issues list all parse.
        let clean = parse_checker_verdict(
            r#"{"verdict_supported": false, "issues": ["claim A unsupported"]}"#,
        )
        .expect("clean");
        assert!(!clean.verdict_supported);
        assert_eq!(clean.issues.len(), 1);
        let fenced = parse_checker_verdict(
            "Here it is:\n```json\n{\"verdict_supported\": true, \"issues\": []}\n```\nDone.",
        )
        .expect("fenced");
        assert!(fenced.verdict_supported);
        let broken = parse_checker_verdict(
            "verdict_supported: false, issues: [unclosed, malformed",
        )
        .expect("broken still yields a verdict");
        assert!(!broken.verdict_supported);
        assert!(!broken.issues.is_empty());
        assert!(parse_checker_verdict("no verdict here").is_none());
    }

    #[tokio::test]
    async fn full_run_emits_all_role_receipts() {
        // One fixed mock response serves every LLM call: it parses as a
        // valid lead plan (one branch), a draft, and a clean checker
        // verdict, so the pipeline completes end to end.
        let o = orchestrator_with(
            r#"{"branches": [{"query": "q1", "why": "only"}], "verdict_supported": true, "issues": []}"#,
        );
        let run = o
            .run("a goal", None, OrchestratorConfig::default(), None)
            .await
            .unwrap();
        let roles: Vec<&str> = run.sub_agents.iter().map(|r| r.role.as_str()).collect();
        assert!(roles.contains(&"lead"), "roles: {roles:?}");
        assert!(roles.contains(&"researcher"), "roles: {roles:?}");
        assert!(roles.contains(&"drafter"), "roles: {roles:?}");
        assert!(roles.contains(&"checker"), "roles: {roles:?}");
        assert!(roles.contains(&"reconciler"), "roles: {roles:?}");
        assert_eq!(run.stopped, "finished");
        assert!(!run.final_report.is_empty());
        assert!(run.conflicts.is_empty());
    }

    #[tokio::test]
    async fn run_cap_zero_stops_before_research() {
        let o = orchestrator_with(r#"{"branches": [{"query": "q1", "why": "only"}]}"#);
        let mut cfg = OrchestratorConfig::default();
        cfg.per_run_token_cap = 0;
        let run = o.run("a goal", None, cfg, None).await.unwrap();
        // Lead runs first and bills its mock tokens, so the cap trips
        // immediately after it; no researcher receipt exists.
        assert_eq!(run.stopped, "budget");
        assert!(!run.sub_agents.iter().any(|r| r.role == "researcher"));
    }
}
