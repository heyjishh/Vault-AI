//! Agentic workflows — Harvey-style multi-step legal agents.
//!
//! A workflow is a named sequence of steps. Each step runs grounded
//! retrieval (corpus, optionally web) guided by a step-specific focus,
//! producing cited findings. Step findings feed forward as context into
//! later steps, and a final report is synthesized from all of them.
//! Every claim keeps its citations; the report never invents sources.
//!
//! Built-ins: contract risk review, due diligence, legal memo drafting.
//! Workflows are data (`Workflow` structs), so new ones can be added
//! without touching the engine.

use std::collections::HashSet;
use std::sync::Arc;

use legal_rag_common::{Error, Result};
use serde::Serialize;

use crate::query::{Citation, QueryMode, QueryService};

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowStepDef {
    pub id: &'static str,
    pub name: &'static str,
    pub query_template: &'static str,
    pub focus: &'static str,
    pub mode: QueryMode,
    pub top_k: usize,
    pub use_web: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowDef {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub input_label: &'static str,
    pub report_instruction: &'static str,
    pub steps: &'static [WorkflowStepDef],
}

#[derive(Debug, Clone, Serialize)]
pub struct StepResult {
    pub step_id: String,
    pub name: String,
    pub findings: String,
    pub citations: Vec<Citation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowRun {
    pub workflow_id: String,
    pub workflow_name: String,
    pub input: String,
    pub steps: Vec<StepResult>,
    pub final_report: String,
    pub citations: Vec<Citation>,
}

// ---------------------------------------------------------------------------
// Built-in workflows
// ---------------------------------------------------------------------------

const REVIEW_STEPS: &[WorkflowStepDef] = &[
    WorkflowStepDef {
        id: "obligations",
        name: "Key obligations",
        query_template: "What are the key obligations of each party in {input}?",
        focus: "List each obligation with the obligated party and any deadlines.",
        mode: QueryMode::Mix,
        top_k: 5,
        use_web: false,
    },
    WorkflowStepDef {
        id: "termination",
        name: "Termination",
        query_template: "What are the termination provisions in {input}?",
        focus: "Notice periods, termination for convenience vs cause, and post-termination duties.",
        mode: QueryMode::Mix,
        top_k: 5,
        use_web: false,
    },
    WorkflowStepDef {
        id: "liability",
        name: "Liability & indemnity",
        query_template: "What liability caps, indemnification and limitation clauses exist in {input}?",
        focus: "Caps, carve-outs, indemnity triggers, and anything uncapped.",
        mode: QueryMode::Mix,
        top_k: 5,
        use_web: false,
    },
    WorkflowStepDef {
        id: "payment",
        name: "Payment terms",
        query_template: "What are the payment terms in {input}?",
        focus: "Amounts, schedule, late-payment consequences, and audit rights.",
        mode: QueryMode::Mix,
        top_k: 5,
        use_web: false,
    },
    WorkflowStepDef {
        id: "governing_law",
        name: "Governing law & disputes",
        query_template: "What governing law and dispute resolution provisions apply in {input}?",
        focus: "Jurisdiction, arbitration vs litigation, and venue.",
        mode: QueryMode::Mix,
        top_k: 5,
        use_web: false,
    },
];

const DD_STEPS: &[WorkflowStepDef] = &[
    WorkflowStepDef {
        id: "facts",
        name: "Key facts",
        query_template: "What are the key facts about {input} established in the corpus?",
        focus: "Dates, parties, amounts, and operative provisions. Distinguish established facts from assertions.",
        mode: QueryMode::Mix,
        top_k: 6,
        use_web: false,
    },
    WorkflowStepDef {
        id: "risks",
        name: "Risks & red flags",
        query_template: "What risks, red flags, or adverse provisions relate to {input}?",
        focus: "Anything a cautious reviewer would escalate: uncapped liability, change-of-control, non-competes, litigation exposure.",
        mode: QueryMode::Mix,
        top_k: 6,
        use_web: false,
    },
    WorkflowStepDef {
        id: "external",
        name: "External check",
        query_template: "Recent news, litigation, or regulatory action concerning {input}",
        focus: "Only report items with identifiable sources; mark speculation as such.",
        mode: QueryMode::Naive,
        top_k: 3,
        use_web: true,
    },
    WorkflowStepDef {
        id: "gaps",
        name: "Missing information",
        query_template: "Given the review so far, what is missing or ambiguous about {input}?",
        focus: "Concrete follow-up questions for counsel, not generic hedging.",
        mode: QueryMode::Mix,
        top_k: 4,
        use_web: false,
    },
];

const MEMO_STEPS: &[WorkflowStepDef] = &[
    WorkflowStepDef {
        id: "issues",
        name: "Issues",
        query_template: "What legal issues does this question raise: {input}?",
        focus: "Frame each issue as a crisp question a memo would answer.",
        mode: QueryMode::Mix,
        top_k: 5,
        use_web: false,
    },
    WorkflowStepDef {
        id: "rules",
        name: "Applicable rules",
        query_template: "What statutes, sections, or rules govern {input}?",
        focus: "Cite the exact provisions with their operative language.",
        mode: QueryMode::Mix,
        top_k: 6,
        use_web: false,
    },
    WorkflowStepDef {
        id: "analysis",
        name: "Analysis",
        query_template: "How do the applicable rules apply to the facts of {input}? Context from earlier steps: {context}",
        focus: "Apply rule to facts step by step; note counterarguments.",
        mode: QueryMode::Mix,
        top_k: 6,
        use_web: false,
    },
    WorkflowStepDef {
        id: "conclusion",
        name: "Conclusion",
        query_template: "What is the bottom-line answer to {input}? Context from earlier steps: {context}",
        focus: "A direct answer first, then the two or three reasons that carry it.",
        mode: QueryMode::Mix,
        top_k: 4,
        use_web: false,
    },
];

pub const BUILTIN_WORKFLOWS: &[WorkflowDef] = &[
    WorkflowDef {
        id: "contract_review",
        name: "Contract risk review",
        description: "Extracts obligations, termination, liability, payment and governing-law provisions, then synthesizes a risk report.",
        input_label: "Contract or document to review",
        report_instruction: "You are a senior contracts lawyer. Write a risk review report with sections: Executive summary, Key obligations, Termination, Liability & indemnity, Payment terms, Governing law & disputes, Red flags. Every factual claim must carry its citation id like [doc:0]. Be direct about what is risky.",
        steps: REVIEW_STEPS,
    },
    WorkflowDef {
        id: "due_diligence",
        name: "Due diligence",
        description: "Establishes key facts, hunts for risks and red flags, checks external sources, and lists what is still missing.",
        input_label: "Subject of diligence",
        report_instruction: "You are a due-diligence lead. Write a diligence memo with sections: Executive summary, Established facts, Risks & red flags, External findings, Open questions for counsel. Distinguish established facts from inconclusive items. Every claim needs its citation id.",
        steps: DD_STEPS,
    },
    WorkflowDef {
        id: "memo_draft",
        name: "Legal memo",
        description: "Frames the issues, finds the applicable rules, applies them to the facts, and drafts a structured IRAC-style memo.",
        input_label: "Legal question",
        report_instruction: "You are a senior associate drafting a legal memo. Structure: Question presented, Brief answer, Applicable rules, Analysis, Conclusion. Apply rules to facts explicitly and note counterarguments. Every legal proposition needs its citation id like [doc:0].",
        steps: MEMO_STEPS,
    },
];

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

pub struct WorkflowEngine {
    query: Arc<QueryService>,
    /// Admin-set standing instructions, injected into every workflow
    /// run's synthesis (workflows keep their own step structure).
    instruction: std::sync::RwLock<Option<String>>,
}

impl WorkflowEngine {
    pub fn new(query: Arc<QueryService>) -> Self {
        Self { query, instruction: std::sync::RwLock::new(None) }
    }

    /// Set (or clear) the admin's standing instructions for agent runs.
    pub fn set_instruction(&self, instruction: Option<String>) {
        let instruction = instruction.filter(|s| !s.trim().is_empty());
        *self.instruction.write().unwrap() = instruction;
    }

    fn instruction(&self) -> Option<String> {
        self.instruction.read().unwrap().clone()
    }

    pub fn list(&self) -> Vec<WorkflowDef> {
        BUILTIN_WORKFLOWS.to_vec()
    }

    fn find(id: &str) -> Option<&'static WorkflowDef> {
        BUILTIN_WORKFLOWS.iter().find(|w| w.id == id)
    }

    /// Run a workflow end to end.
    pub async fn run(&self, workflow_id: &str, input: &str) -> Result<WorkflowRun> {
        let def = Self::find(workflow_id)
            .ok_or_else(|| Error::BadRequest(format!("unknown workflow: {workflow_id}")))?;
        if def.steps.is_empty() {
            return Err(Error::Other(anyhow::anyhow!("workflow has no steps")));
        }

        let mut step_results: Vec<StepResult> = Vec::new();
        let mut context_acc = String::new();

        for step in def.steps {
            let question = step
                .query_template
                .replace("{input}", input)
                .replace("{context}", &context_acc);
            let focused = if step.focus.is_empty() {
                question
            } else {
                format!("{question}\n\nFocus: {}", step.focus)
            };
            let answer = if step.use_web {
                self.query.query_with_web(&focused, step.mode, step.top_k, 2, None, false, crate::query::CitatorContext::default()).await?
            } else {
                self.query.query(&focused, step.mode, step.top_k, crate::query::CitatorContext::default()).await?
            };
            // Feed a compact summary forward so later steps build on earlier ones.
            let summary: String = answer.answer.chars().take(800).collect();
            if !context_acc.is_empty() {
                context_acc.push_str("\n\n");
            }
            context_acc.push_str(&format!("{}: {}", step.name, summary));

            step_results.push(StepResult {
                step_id: step.id.to_string(),
                name: step.name.to_string(),
                findings: answer.answer,
                citations: answer.citations,
            });
        }

        // Final synthesis.
        let steps_text = step_results
            .iter()
            .map(|s| format!("## {}\n{}", s.name, s.findings))
            .collect::<Vec<_>>()
            .join("\n\n");
        let report_system = match self.instruction() {
            Some(extra) => format!(
                "{}\n\nStanding instructions from the workspace admin (follow them unless they conflict with this workflow's structure):\n{extra}",
                def.report_instruction
            ),
            None => def.report_instruction.to_string(),
        };
        let final_report = self
            .query
            .llm()
            .complete(
                &report_system,
                &format!("Subject: {input}\n\nStep findings:\n{steps_text}"),
            )
            .await?;

        // Deduplicate citations across steps.
        let mut seen = HashSet::new();
        let mut citations: Vec<Citation> = Vec::new();
        for s in &step_results {
            for c in &s.citations {
                if seen.insert(c.chunk_id.clone()) {
                    citations.push(c.clone());
                }
            }
        }

        Ok(WorkflowRun {
            workflow_id: def.id.to_string(),
            workflow_name: def.name.to_string(),
            input: input.to_string(),
            steps: step_results,
            final_report,
            citations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbeddings;
    use crate::graph::InMemoryGraphStore;
    use crate::llm::MockLlm;
    use crate::query::QueryService;
    use crate::vector_store::InMemoryVectorStore;
    use crate::web::NoopWebSearch;

    fn engine() -> WorkflowEngine {
        let svc = QueryService::new(
            Arc::new(MockEmbeddings::new(8)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(MockLlm::new("Mock findings [doc:0].")),
            Arc::new(|_: &str| None),
            Arc::new(NoopWebSearch),
            None,
        );
        WorkflowEngine::new(Arc::new(svc))
    }

    #[test]
    fn builtins_list_three_workflows() {
        let e = engine();
        let list = e.list();
        assert_eq!(list.len(), 3);
        assert!(list.iter().all(|w| !w.steps.is_empty()));
    }

    #[tokio::test]
    async fn memo_workflow_runs_end_to_end() {
        let e = engine();
        let run = e.run("memo_draft", "test question").await.unwrap();
        assert_eq!(run.workflow_id, "memo_draft");
        assert_eq!(run.steps.len(), 4);
        assert!(!run.final_report.is_empty());
    }

    #[tokio::test]
    async fn unknown_workflow_errors() {
        let e = engine();
        assert!(e.run("nope", "x").await.is_err());
    }
}
