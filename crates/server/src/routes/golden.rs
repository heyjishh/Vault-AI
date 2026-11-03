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

#[derive(Deserialize)]
pub struct UpdateRuleRequest {
    pub question: Option<String>,
    pub expected_terms: Option<Vec<String>>,
    pub expected_sources: Option<Vec<String>>,
    pub notes: Option<String>,
}

/// Edit an existing rule in place: only the fields present in the body
/// change, so fixing a term or a source no longer means deleting the
/// rule (and its run history) and re-adding it.
pub async fn update_rule(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateRuleRequest>,
) -> Result<Json<GoldenRule>, ApiError> {
    if let Some(q) = &body.question {
        if q.trim().is_empty() {
            return Err(legal_rag_common::Error::BadRequest("question is empty".into()).into());
        }
    }
    let question = body.question.map(|q| q.trim().to_string());
    match state.golden.update(&id, question, body.expected_terms, body.expected_sources, body.notes) {
        Some(rule) => Ok(Json(rule)),
        None => Err(legal_rag_common::Error::NotFound("rule not found".into()).into()),
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

/// The outcome of one full ruleset run (Piece 81): the stored
/// summary plus every rule's fresh result.
pub struct RulesetRun {
    pub summary: RunSummary,
    pub results: Vec<RuleRunView>,
}

/// Run every rule against the live RAG and score the answers,
/// storing each result and the run summary. Shared by the admin
/// endpoint (which bills the admin) and the background auto-run
/// loop (unbilled system evaluation, Piece 81).
pub async fn run_ruleset(
    state: &AppState,
    scope: &legal_rag_rag::usage::UsageScope,
) -> RulesetRun {
    let rules = state.golden.list();
    let mut results = Vec::new();
    let mut passed = 0usize;
    for rule in &rules {
        let answer = scope_usage(scope.clone(), async {
            state.query.query(&rule.question, QueryMode::Mix, state.config.default_top_k, legal_rag_rag::CitatorContext::default()).await
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
    let summary = RunSummary { ran_at: Utc::now(), total: rules.len(), passed };
    state.golden.set_run_summary(summary.clone());
    RulesetRun { summary, results }
}

/// Run every rule against the live RAG and score the answers. The token
/// cost of the evaluation is billed to the admin running it.
pub async fn run_rules(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
) -> Result<Json<RunResponse>, ApiError> {
    let scope = new_scope();
    let run = run_ruleset(&state, &scope).await;
    if let Ok(totals) = scope.lock() {
        state.billing.add_usage(&admin.id, &totals);
    }
    Ok(Json(RunResponse {
        ran_at: run.summary.ran_at,
        total: run.summary.total,
        passed: run.summary.passed,
        results: run.results,
    }))
}

/// Auto-capture a golden rule from a downvoted answer (Piece 81):
/// the feedback loop closes itself. The rule is deduped on the
/// normalized question, its expected terms are the statutory
/// provisions the question itself names, and its expected sources
/// are pre-filled from the asker's own activity record for that
/// question — labelled `feedback` so an admin can refine it in the
/// Golden page like any other rule.
pub fn capture_from_downvote(state: &AppState, user: &legal_rag_auth::User, question: &str) {
    let norm = crate::golden::normalize_question(question);
    if norm.is_empty() {
        return;
    }
    if state
        .golden
        .list()
        .iter()
        .any(|r| crate::golden::normalize_question(&r.question) == norm)
    {
        return;
    }
    let mut expected_sources: Vec<String> = Vec::new();
    for ev in state.activity.for_user(&user.id, 100) {
        if ev.kind != "query" || crate::golden::normalize_question(&ev.title) != norm {
            continue;
        }
        if let Some(citations) = ev
            .payload
            .as_ref()
            .and_then(|p| p.get("citations"))
            .and_then(|c| c.as_array())
        {
            for c in citations {
                let label = c
                    .get("doc_title")
                    .and_then(|t| t.as_str())
                    .filter(|s| !s.is_empty())
                    .or_else(|| c.get("doc_id").and_then(|t| t.as_str()));
                if let Some(l) = label {
                    let l = l.to_string();
                    if !expected_sources.contains(&l) {
                        expected_sources.push(l);
                    }
                }
            }
        }
        break;
    }
    let rule = GoldenRule {
        id: legal_rag_common::new_id(),
        question: question.to_string(),
        expected_terms: crate::golden::statute_terms(question),
        expected_sources,
        notes: format!(
            "auto-captured from a downvote by {} — refine the expected terms to what a good answer must contain",
            user.email
        ),
        source: "feedback".into(),
        created_at: Utc::now(),
        last_result: None,
    };
    state.golden.add(rule);
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
