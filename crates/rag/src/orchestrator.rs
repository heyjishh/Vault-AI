//! Autonomous agent — a planner-executor loop over the query tools.
//!
//! Unlike the built-in workflows (fixed step lists), this agent plans
//! dynamically: at every iteration the planner LLM sees the goal and
//! the findings gathered so far, then chooses the next tool call —
//! `search_vault` (optionally brief-scoped), `search_web`, or `finish`.
//! Execution stops at `finish` or when the step budget runs out, and a
//! final synthesis writes the report from the gathered findings with
//! their citations. Every tool result is a real grounded query through
//! the same QueryService the Ask surface uses, so citations, the
//! citation-integrity audit and brief confinement all apply unchanged.

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

#[derive(Debug, Clone, Serialize)]
pub struct AutonomousRun {
    pub goal: String,
    pub steps: Vec<AgentStepTrace>,
    pub final_report: String,
    pub citations: Vec<Citation>,
    /// finished | budget | planner_error
    pub stopped: String,
    pub max_steps: usize,
}

const PLANNER_SYSTEM: &str = "You are the planner of an autonomous legal-research agent. \
You pursue a GOAL by calling tools, one at a time. Available tools:\n\
- search_vault: search the user's document vault (statutes, judgments, filed documents). Use for anything the vault may hold.\n\
- search_web: search the live web (recent developments, primary sources online). Use when the vault is unlikely to hold it or currency matters.\n\
- finish: stop gathering and write the final report from the findings so far.\n\
Respond with EXACTLY one JSON object, no other text: \
{\"action\": \"search_vault\" | \"search_web\" | \"finish\", \"query\": \"<the search query, for search actions>\", \"reason\": \"<one short sentence>\"}\n\
Rules: make each query specific and different from earlier ones; do not repeat a query; \
prefer the vault for documents and the web for recent or external facts; finish when the findings \
can answer the goal, or when further searches are repeating themselves.";

const SYNTH_SYSTEM: &str = "You are a senior legal researcher writing the final report of an \
autonomous research run. Write a clear, structured report that answers the GOAL from the FINDINGS \
only: verdict first, then analysis, citing sources inline as [doc:N] / [web:N] exactly as they \
appear in the findings. Say plainly what the findings do not establish. Do not invent facts, \
cases, or citations.";

#[derive(Debug, Deserialize)]
struct Plan {
    action: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    reason: String,
}

/// Extract the planner's JSON object from a raw completion (tolerant of
/// code fences and surrounding prose).
fn parse_plan(raw: &str) -> Option<Plan> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Plan>(&raw[start..=end]).ok()
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

    /// Run the planner loop for one goal. `scope_docs` confines vault
    /// searches to a brief (the caller enforces space membership);
    /// `max_steps` is clamped to 1..=10. `terms` (Piece 13) pool into
    /// every step's retrieval expansions.
    pub async fn run(
        &self,
        goal: &str,
        scope_docs: Option<Vec<String>>,
        max_steps: usize,
        terms: &[String],
    ) -> Result<AutonomousRun> {
        let max_steps = max_steps.clamp(1, 10);
        let mut steps: Vec<AgentStepTrace> = Vec::new();
        let mut stopped = "budget".to_string();

        for n in 1..=max_steps {
            let plan = match self.plan(goal, &steps, max_steps + 1 - n).await {
                Some(p) => p,
                None => {
                    stopped = "planner_error".to_string();
                    break;
                }
            };
            match plan.action.as_str() {
                "finish" => {
                    stopped = "finished".to_string();
                    break;
                }
                "search_vault" | "search_web" => {
                    let q = if plan.query.trim().is_empty() {
                        goal.to_string()
                    } else {
                        plan.query.trim().to_string()
                    };
                    let step_opts = crate::query::AskOptions {
                        brief_vault: None,
                        refine_terms: terms.to_vec(),
                    };
                    let answer = if plan.action == "search_web" {
                        self.query
                            .query_with_web(&q, QueryMode::Mix, 4, 3, scope_docs.clone(), false, crate::query::CitatorContext::default(), &step_opts)
                            .await?
                    } else if let Some(docs) = &scope_docs {
                        self.query
                            .query_opts(&q, QueryMode::Mix, 4, Some(docs.clone()), false, &step_opts, crate::query::CitatorContext::default())
                            .await?
                    } else {
                        self.query.query_opts(&q, QueryMode::Mix, 4, None, false, &step_opts, crate::query::CitatorContext::default()).await?
                    };
                    steps.push(AgentStepTrace {
                        n,
                        action: plan.action.clone(),
                        input: q,
                        reason: plan.reason.clone(),
                        findings: answer.answer,
                        citations: answer.citations,
                    });
                }
                _ => {
                    stopped = "planner_error".to_string();
                    break;
                }
            }
        }

        // Synthesis from whatever was gathered (possibly nothing — the
        // report must then say so, not invent).
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
        let final_report = self
            .query
            .llm()
            .complete(
                &synth_system,
                &format!("GOAL: {goal}\n\nFINDINGS:\n{findings_text}"),
            )
            .await?;

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
        })
    }

    async fn plan(
        &self,
        goal: &str,
        steps: &[AgentStepTrace],
        budget_left: usize,
    ) -> Option<Plan> {
        let findings = if steps.is_empty() {
            "(nothing gathered yet)".to_string()
        } else {
            steps
                .iter()
                .map(|s| {
                    let brief: String = s.findings.chars().take(700).collect();
                    format!("Step {} [{}] q=\"{}\"\n{}", s.n, s.action, s.input, brief)
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        let raw = self
            .query
            .llm()
            .complete(
                PLANNER_SYSTEM,
                &format!(
                    "GOAL: {goal}\n\nFINDINGS SO FAR:\n{findings}\n\nTool calls remaining (including this decision): {budget_left}. \
                     If the findings already answer the goal, choose finish."
                ),
            )
            .await
            .ok()?;
        parse_plan(&raw)
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

    #[tokio::test]
    async fn planner_that_finishes_immediately_synthesizes_with_no_steps() {
        let agent = agent_with(r#"{"action":"finish","reason":"nothing to gather"}"#);
        let run = agent.run("a goal", None, 5, &[]).await.unwrap();
        assert_eq!(run.stopped, "finished");
        assert!(run.steps.is_empty());
        assert!(!run.final_report.is_empty());
    }

    #[tokio::test]
    async fn step_budget_bounds_the_loop() {
        // The mock always asks for another vault search; the budget must stop it.
        let agent = agent_with(r#"{"action":"search_vault","query":"q","reason":"digging"}"#);
        let run = agent.run("a goal", None, 3, &[]).await.unwrap();
        assert_eq!(run.stopped, "budget");
        assert_eq!(run.steps.len(), 3);
        assert!(run.steps.iter().all(|s| s.action == "search_vault"));
    }

    #[test]
    fn parse_plan_tolerates_fences_and_prose() {
        let raw = "Here is my decision:\n```json\n{\"action\":\"search_web\",\"query\":\"DPDP rules 2026\",\"reason\":\"recent\"}\n```";
        let p = parse_plan(raw).expect("plan");
        assert_eq!(p.action, "search_web");
        assert_eq!(p.query, "DPDP rules 2026");
        assert!(parse_plan("no json here").is_none());
    }
}
