//! Autonomous agent — plan-first research over the query tools.
//!
//! Piece 80 rework. The original design planned reactively (one
//! planner call per step, seeing only 700-char briefs of earlier
//! findings) and drifted: no global line of inquiry, repeated
//! queries, wandering steps. The agent now plans ONCE, up front:
//!
//! 1. A scoping retrieval on the goal shows what the vault already
//!    holds, so the plan is grounded in real material.
//! 2. One planner call produces the whole plan — validated,
//!    deduplicated (case/space-insensitive), and bounded (≤6 steps).
//!    An unusable plan falls back to a single vault search on the
//!    goal; a bad plan never kills the run.
//! 3. The plan executes without re-planning. A per-run budget
//!    (max steps and/or estimated tokens) can stop execution early;
//!    the report then covers only the completed steps and says so.
//!
//! Every tool result is a real grounded query through the same
//! QueryService the Ask surface uses, so citations, the
//! citation-integrity audit and brief confinement apply unchanged.

use std::collections::HashSet;
use std::sync::Arc;

use legal_rag_common::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::query::{QueryMode, QueryService};
use crate::Citation;

#[derive(Debug, Clone, Serialize)]
pub struct AgentStepTrace {
    /// 1-based step number.
    pub n: usize,
    /// search_vault | search_web
    pub action: String,
    /// The query the planner sent to the tool.
    pub input: String,
    /// The planner's stated reason for this call.
    pub reason: String,
    /// The tool's grounded answer (full text).
    pub findings: String,
    pub citations: Vec<Citation>,
}

/// One step of an up-front plan. Returned by `plan_upfront` for the
/// plan-approval UI, and accepted back (possibly edited) by
/// `execute_plan`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedStep {
    /// search_vault | search_web
    pub action: String,
    pub query: String,
    #[serde(default)]
    pub reason: String,
}

/// Per-run budget (Piece 80). Either bound may be unset. Token
/// enforcement uses the codebase's ≈4 chars/token estimate, checked
/// between steps; a run always completes at least one planned step.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunBudget {
    pub max_steps: Option<usize>,
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutonomousRun {
    pub goal: String,
    pub steps: Vec<AgentStepTrace>,
    pub final_report: String,
    pub citations: Vec<Citation>,
    /// finished | budget | budget_tokens
    pub stopped: String,
    pub max_steps: usize,
    /// Estimated tokens spent executing (estimate-based, like the
    /// budget; provider-reported usage is billed separately).
    pub tokens_estimate: u64,
}

const MAX_PLAN_STEPS: usize = 6;

const PLAN_SYSTEM: &str = "You are the planner of an autonomous legal-research agent. \
Given a GOAL and a scoping pass over the user's vault (what it already holds on this topic), \
produce the COMPLETE research plan in one response: a JSON array of steps, no other text. \
Each step: {\"action\": \"search_vault\" | \"search_web\", \"query\": \"<specific search query>\", \
\"reason\": \"<one short sentence>\"}. Rules: 2 to 6 steps; every query specific, distinct, and \
answerable on its own; prefer the vault for documents the scoping pass shows it holds and the \
web for recent or external facts; build on the scoping material instead of re-asking what it \
already answers; the steps together must cover the goal.";

const SYNTH_SYSTEM: &str = "You are a senior legal researcher writing the final report of an \
autonomous research run. Write a clear, structured report that answers the GOAL from the FINDINGS \
only: verdict first, then analysis, citing sources inline as [doc:N] / [web:N] exactly as they \
appear in the findings. Say plainly what the findings do not establish. Do not invent facts, \
cases, or citations.";

#[derive(Debug, Deserialize)]
struct RawStep {
    #[serde(default)]
    action: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    reason: String,
}

/// Normalize a query for duplicate detection (case/space-insensitive).
fn norm_query(q: &str) -> String {
    q.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse + validate a plan from a raw planner completion. Tolerant of
/// code fences and surrounding prose. Returns the validated steps
/// (possibly empty — the caller applies the fallback).
fn parse_plan_array(raw: &str) -> Vec<PlannedStep> {
    let (Some(start), Some(end)) = (raw.find('['), raw.rfind(']')) else {
        return Vec::new();
    };
    if end <= start {
        return Vec::new();
    }
    let Ok(raw_steps) = serde_json::from_str::<Vec<RawStep>>(&raw[start..=end]) else {
        return Vec::new();
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<PlannedStep> = Vec::new();
    for s in raw_steps {
        let query = s.query.trim().to_string();
        if query.is_empty() || !seen.insert(norm_query(&query)) {
            continue;
        }
        let action = match s.action.trim() {
            "search_web" => "search_web",
            _ => "search_vault",
        };
        out.push(PlannedStep {
            action: action.to_string(),
            query,
            reason: s.reason.trim().to_string(),
        });
        if out.len() >= MAX_PLAN_STEPS {
            break;
        }
    }
    out
}

pub struct AutonomousAgent {
    query: Arc<QueryService>,
    /// Admin-set standing instructions for synthesis (settings seam,
    /// shared wording with the workflow engine's).
    instruction: std::sync::RwLock<Option<String>>,
}

impl AutonomousAgent {
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

    /// Produce the up-front plan for a goal (Piece 80): a scoping
    /// retrieval grounds the planner in what the vault holds, then a
    /// single planner call returns the whole plan, validated and
    /// deduplicated. Never fails — an unusable plan falls back to one
    /// vault search on the goal itself.
    pub async fn plan_upfront(
        &self,
        goal: &str,
        scope_docs: Option<Vec<String>>,
        terms: &[String],
    ) -> Vec<PlannedStep> {
        // Scoping pass: what does the vault already hold on this?
        let step_opts = crate::query::AskOptions {
            brief_vault: None,
            refine_terms: terms.to_vec(),
        };
        let scoping = self
            .query
            .query_opts(
                goal,
                QueryMode::Mix,
                4,
                scope_docs.clone(),
                false,
                &step_opts,
                crate::query::CitatorContext::default(),
            )
            .await;
        let scoping_text = match &scoping {
            Ok(answer) if !answer.citations.is_empty() => answer
                .citations
                .iter()
                .map(|c| {
                    let title = c.doc_title.as_deref().unwrap_or("Untitled source");
                    let snippet: String = c.snippet.chars().take(220).collect();
                    format!("- {title}: {snippet}")
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => "(the scoping pass found nothing in the vault on this goal)".to_string(),
        };

        let planned = self
            .query
            .llm()
            .complete(
                PLAN_SYSTEM,
                &format!("GOAL: {goal}\n\nSCOPING PASS — what the vault already holds:\n{scoping_text}"),
            )
            .await
            .map(|raw| parse_plan_array(&raw))
            .unwrap_or_default();
        if planned.is_empty() {
            vec![PlannedStep {
                action: "search_vault".to_string(),
                query: goal.to_string(),
                reason: "Fallback: the goal as a single vault search.".to_string(),
            }]
        } else {
            planned
        }
    }

    /// Execute a plan (the planner's, or a user-approved edited one)
    /// under a budget, then synthesize the report from the findings.
    pub async fn execute_plan(
        &self,
        goal: &str,
        plan: &[PlannedStep],
        scope_docs: Option<Vec<String>>,
        terms: &[String],
        budget: RunBudget,
    ) -> Result<AutonomousRun> {
        if plan.is_empty() {
            return Err(Error::BadRequest("plan has no steps".to_string()));
        }
        let max_steps = budget
            .max_steps
            .unwrap_or(plan.len())
            .clamp(1, 10);
        let mut steps: Vec<AgentStepTrace> = Vec::new();
        let mut tokens_estimate: u64 = 0;
        let mut stopped = "finished".to_string();

        for (i, pstep) in plan.iter().enumerate() {
            if i >= max_steps {
                stopped = "budget".to_string();
                break;
            }
            if let Some(cap) = budget.max_tokens {
                if !steps.is_empty() && tokens_estimate >= cap {
                    stopped = "budget_tokens".to_string();
                    break;
                }
            }
            let n = i + 1;
            let step_opts = crate::query::AskOptions {
                brief_vault: None,
                refine_terms: terms.to_vec(),
            };
            let answer = if pstep.action == "search_web" {
                self.query
                    .query_with_web(&pstep.query, QueryMode::Mix, 4, 3, scope_docs.clone(), false, crate::query::CitatorContext::default(), &step_opts)
                    .await?
            } else if let Some(docs) = &scope_docs {
                self.query
                    .query_opts(&pstep.query, QueryMode::Mix, 4, Some(docs.clone()), false, &step_opts, crate::query::CitatorContext::default())
                    .await?
            } else {
                self.query.query_opts(&pstep.query, QueryMode::Mix, 4, None, false, &step_opts, crate::query::CitatorContext::default()).await?
            };
            tokens_estimate += crate::usage::estimate_tokens(&pstep.query)
                + crate::usage::estimate_tokens(&answer.answer);
            steps.push(AgentStepTrace {
                n,
                action: pstep.action.clone(),
                input: pstep.query.clone(),
                reason: pstep.reason.clone(),
                findings: answer.answer,
                citations: answer.citations,
            });
        }
        if stopped == "finished" && steps.len() < plan.len() {
            stopped = "budget".to_string();
        }

        let final_report = self.synthesize(goal, &steps, &stopped).await?;
        tokens_estimate += crate::usage::estimate_tokens(&final_report);

        let mut seen = HashSet::new();
        let mut citations: Vec<Citation> = Vec::new();
        for s in &steps {
            for c in &s.citations {
                if seen.insert(c.chunk_id.clone()) {
                    citations.push(c.clone());
                }
            }
        }

        Ok(AutonomousRun {
            goal: goal.to_string(),
            steps,
            final_report,
            citations,
            stopped,
            max_steps,
            tokens_estimate,
        })
    }

    /// Write the final report from gathered findings (possibly none —
    /// the report must then say so, not invent).
    async fn synthesize(
        &self,
        goal: &str,
        steps: &[AgentStepTrace],
        stopped: &str,
    ) -> Result<String> {
        let findings_text = if steps.is_empty() {
            "(no findings were gathered)".to_string()
        } else {
            steps
                .iter()
                .map(|s| {
                    format!(
                        "## Step {} — {} ({})\n{}",
                        s.n, s.action, s.input, s.findings
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        let synth_system = match self.instruction() {
            Some(extra) => format!(
                "{SYNTH_SYSTEM}\n\nStanding instructions from the workspace admin (follow them unless they conflict with the report's grounding rules):\n{extra}"
            ),
            None => SYNTH_SYSTEM.to_string(),
        };
        let budget_note = if stopped == "budget" || stopped == "budget_tokens" {
            "\n\nNOTE: the run stopped early on its budget; the report covers only the steps that ran — say so plainly."
        } else {
            ""
        };
        self.query
            .llm()
            .complete(
                &synth_system,
                &format!("GOAL: {goal}\n\nFINDINGS:\n{findings_text}{budget_note}"),
            )
            .await
    }

    /// One-shot run: plan up front, then execute. `max_steps` (the
    /// legacy step budget, clamped 1..=10) caps execution.
    pub async fn run(
        &self,
        goal: &str,
        scope_docs: Option<Vec<String>>,
        max_steps: usize,
        terms: &[String],
    ) -> Result<AutonomousRun> {
        let plan = self.plan_upfront(goal, scope_docs.clone(), terms).await;
        self.execute_plan(
            goal,
            &plan,
            scope_docs,
            terms,
            RunBudget {
                max_steps: Some(max_steps),
                max_tokens: None,
            },
        )
        .await
    }

    /// One-shot run with a full budget (steps and/or tokens).
    pub async fn run_budgeted(
        &self,
        goal: &str,
        scope_docs: Option<Vec<String>>,
        budget: RunBudget,
        terms: &[String],
    ) -> Result<AutonomousRun> {
        let plan = self.plan_upfront(goal, scope_docs.clone(), terms).await;
        self.execute_plan(goal, &plan, scope_docs, terms, budget).await
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

    const PLAN_JSON: &str = r#"[
      {"action":"search_vault","query":"rent agreement efflux of time","reason":"core doctrine"},
      {"action":"search_web","query":"Section 106 notice recent rulings","reason":"currency"},
      {"action":"search_vault","query":"Rent  Agreement   Efflux Of Time","reason":"duplicate of step 1"}
    ]"#;

    fn agent_with(llm_response: &str) -> AutonomousAgent {
        let svc = QueryService::new(
            Arc::new(MockEmbeddings::new(8)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(MockLlm::new(llm_response)),
            Arc::new(|_: &str| None),
            Arc::new(NoopWebSearch),
            None,
        );
        AutonomousAgent::new(Arc::new(svc))
    }

    #[test]
    fn parse_plan_array_validates_and_dedupes() {
        let steps = parse_plan_array(PLAN_JSON);
        // The third step is a case/space-variant duplicate of the first.
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].action, "search_vault");
        assert_eq!(steps[1].action, "search_web");
        // Fences and prose are tolerated; garbage yields no steps.
        let fenced = format!("Plan:\n```json\n{PLAN_JSON}\n```");
        assert_eq!(parse_plan_array(&fenced).len(), 2);
        assert!(parse_plan_array("no plan here").is_empty());
        assert!(parse_plan_array(r#"[{"query":""}]"#).is_empty());
    }

    #[tokio::test]
    async fn plan_upfront_dedupes_the_planners_output() {
        let agent = agent_with(PLAN_JSON);
        let plan = agent.plan_upfront("eviction after expiry", None, &[]).await;
        assert_eq!(plan.len(), 2);
    }

    #[tokio::test]
    async fn run_executes_the_plan_and_finishes() {
        let agent = agent_with(PLAN_JSON);
        let run = agent.run("eviction after expiry", None, 6, &[]).await.unwrap();
        assert_eq!(run.stopped, "finished");
        assert_eq!(run.steps.len(), 2);
        assert!(!run.final_report.is_empty());
        assert!(run.tokens_estimate > 0);
    }

    #[tokio::test]
    async fn step_and_token_budgets_stop_execution_early() {
        let agent = agent_with(PLAN_JSON);
        let plan = agent.plan_upfront("eviction after expiry", None, &[]).await;
        let by_steps = agent
            .execute_plan("g", &plan, None, &[], RunBudget { max_steps: Some(1), max_tokens: None })
            .await
            .unwrap();
        assert_eq!(by_steps.steps.len(), 1);
        assert_eq!(by_steps.stopped, "budget");
        let by_tokens = agent
            .execute_plan("g", &plan, None, &[], RunBudget { max_steps: None, max_tokens: Some(1) })
            .await
            .unwrap();
        assert_eq!(by_tokens.steps.len(), 1);
        assert_eq!(by_tokens.stopped, "budget_tokens");
    }

    #[tokio::test]
    async fn unusable_plan_falls_back_to_a_single_goal_search() {
        let agent = agent_with("the model rambled without any json");
        let plan = agent.plan_upfront("some goal", None, &[]).await;
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].query, "some goal");
        let run = agent.run("some goal", None, 5, &[]).await.unwrap();
        assert_eq!(run.steps.len(), 1);
        assert_eq!(run.stopped, "finished");
    }
}
