//! `/api/v1/admin/golden/*` — the golden ruleset: curated evaluation
//! questions promoted from real user queries (or written by hand), run
//! against the live RAG to measure retrieval quality over time.

use axum::extract::{Path, State};
use axum::Json;
use chrono::Utc;
use legal_rag_auth::AdminUser;
use legal_rag_rag::usage::{new_scope, scope_usage};
use legal_rag_rag::QueryMode;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::golden::{
    aggregate_stages, evaluate, stage_scores, GoldenRule, RuleResult, RunSummary, StageAggregate,
    StageScore,
};
use crate::state::AppState;

#[derive(Serialize)]
pub struct RulesView {
    pub rules: Vec<GoldenRule>,
    pub last_run: Option<RunSummary>,
}

pub async fn list_rules(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Json<RulesView> {
    Json(RulesView { rules: state.golden.list(), last_run: state.golden.last_run() })
}

#[derive(Deserialize)]
pub struct AddRuleRequest {
    pub question: String,
    pub expected_terms: Option<Vec<String>>,
    pub expected_sources: Option<Vec<String>>,
    pub notes: Option<String>,
}

pub async fn add_rule(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Json(body): Json<AddRuleRequest>,
) -> Result<Json<GoldenRule>, ApiError> {
    if body.question.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("question is empty".into()).into());
    }
    let rule = GoldenRule {
        id: legal_rag_common::new_id(),
        question: body.question.trim().to_string(),
        expected_terms: body.expected_terms.unwrap_or_default(),
        expected_sources: body.expected_sources.unwrap_or_default(),
        notes: body.notes.unwrap_or_default(),
        source: "manual".into(),
        created_at: Utc::now(),
        last_result: None,
    };
    Ok(Json(state.golden.add(rule)))
}

#[derive(Deserialize)]
pub struct FromActivityRequest {
    pub activity_id: String,
    pub expected_terms: Option<Vec<String>>,
    pub expected_sources: Option<Vec<String>>,
}

/// Promote a captured user query into a golden rule — the feedback loop
/// that turns production traffic into an evaluation set.
pub async fn from_activity(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Json(body): Json<FromActivityRequest>,
) -> Result<Json<GoldenRule>, ApiError> {
    let event = state
        .activity
        .by_id(&body.activity_id)
        .ok_or_else(|| legal_rag_common::Error::NotFound("activity event not found".into()))?;
    let rule = GoldenRule {
        id: legal_rag_common::new_id(),
        question: event.title.clone(),
        expected_terms: body.expected_terms.unwrap_or_default(),
        expected_sources: body.expected_sources.unwrap_or_default(),
        notes: format!("captured from {} by {}", event.kind, event.user_email),
        source: "captured".into(),
        created_at: Utc::now(),
        last_result: None,
    };
    Ok(Json(state.golden.add(rule)))
}

pub async fn delete_rule(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.golden.delete(&id) {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(legal_rag_common::Error::NotFound("rule not found".into()).into())
    }
}

#[derive(Serialize)]
pub struct RuleRunView {
    pub rule_id: String,
    pub question: String,
    pub result: RuleResult,
}

#[derive(Serialize)]
pub struct RunResponse {
    pub ran_at: chrono::DateTime<Utc>,
    pub total: usize,
    pub passed: usize,
    pub results: Vec<RuleRunView>,
}

/// Run every rule against the live RAG and score the answers. The token
/// cost of the evaluation is billed to the admin running it.
pub async fn run_rules(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
) -> Result<Json<RunResponse>, ApiError> {
    let rules = state.golden.list();
    let scope = new_scope();
    let mut results = Vec::new();
    let mut passed = 0usize;
    for rule in &rules {
        let answer = scope_usage(scope.clone(), async {
            state.query.query(&rule.question, QueryMode::Mix, state.config.default_top_k).await
        })
        .await;
        let result = match answer {
            Ok(a) => {
                let labels: Vec<String> = a
                    .citations
                    .iter()
                    .map(|c| c.doc_title.clone().unwrap_or_else(|| c.doc_id.clone()))
                    .collect();
                evaluate(rule, &a.answer, &labels, a.citations.len())
            }
            Err(e) => RuleResult {
                passed: false,
                missing_terms: rule.expected_terms.clone(),
                missing_sources: rule.expected_sources.clone(),
                answer_excerpt: format!("query failed: {e}"),
                citations: 0,
                ran_at: Utc::now(),
            },
        };
        if result.passed {
            passed += 1;
        }
        state.golden.set_result(&rule.id, result.clone());
        results.push(RuleRunView { rule_id: rule.id.clone(), question: rule.question.clone(), result });
    }
    if let Ok(totals) = scope.lock() {
        state.billing.add_usage(&admin.id, &totals);
    }
    let summary = RunSummary { ran_at: Utc::now(), total: rules.len(), passed };
    state.golden.set_run_summary(summary.clone());
    Ok(Json(RunResponse { ran_at: summary.ran_at, total: summary.total, passed, results }))
}

#[derive(Serialize)]
pub struct RuleStagesView {
    pub rule_id: String,
    pub question: String,
    pub stages: Vec<StageScore>,
}

#[derive(Serialize)]
pub struct StagesResponse {
    pub ran_at: chrono::DateTime<Utc>,
    pub rules: usize,
    pub aggregate: Vec<StageAggregate>,
    pub results: Vec<RuleStagesView>,
}

#[derive(Deserialize)]
pub struct StagesRequest {
    pub top_k: Option<usize>,
}

/// Per-stage retrieval evaluation: every rule's question through the
/// retrieval stages only (vector → BM25 → fused → reranked), scored for
/// expected-source recall / MRR at each stage. No answer generation —
/// one embedding per rule plus the local reranker — so fusion and
/// ranking configurations can be A/B-compared at zero LLM cost.
pub async fn run_stages(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    body: Option<Json<StagesRequest>>,
) -> Result<Json<StagesResponse>, ApiError> {
    let top_k = body
        .and_then(|b| b.top_k)
        .unwrap_or(10)
        .clamp(1, 20);
    let rules = state.golden.list();
    let mut results = Vec::new();
    let mut all: Vec<Vec<StageScore>> = Vec::new();
    for rule in &rules {
        let scores = match state.query.hit_test(&rule.question, 10).await {
            Ok(report) => stage_scores(&report, &rule.expected_sources, top_k),
            Err(_) => Vec::new(),
        };
        all.push(scores.clone());
        results.push(RuleStagesView {
            rule_id: rule.id.clone(),
            question: rule.question.clone(),
            stages: scores,
        });
    }
    Ok(Json(StagesResponse {
        ran_at: Utc::now(),
        rules: rules.len(),
        aggregate: aggregate_stages(&all),
        results,
    }))
}
