//! Dashboards & insights.
//! `GET /api/v1/dashboard/me` — the user's own home: plan, usage, counts,
//! recent research and documents.
//! `GET /api/v1/admin/insights` — every user insight in one place for the
//! Command Center: totals, 14-day series, plan mix, per-user activity,
//! top queries, and the live activity feed.

use std::collections::HashMap;

use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use legal_rag_auth::{AdminUser, AuthUser};
use serde::Serialize;

use crate::activity::ActivityEvent;
use crate::error::ApiError;
use crate::state::AppState;

// ------------------------------------------------------------ user dashboard

#[derive(Serialize)]
pub struct KindCounts {
    pub queries: usize,
    pub researches: usize,
    pub workflows: usize,
    pub interacts: usize,
    pub drafts: usize,
    pub uploads: usize,
}

#[derive(Serialize)]
pub struct MyDashboard {
    pub plan_name: String,
    pub plan_id: String,
    pub tokens_used: u64,
    pub token_quota: u64,
    pub period_end: DateTime<Utc>,
    pub counts: KindCounts,
    pub recent_activity: Vec<ActivityEvent>,
    pub documents: Vec<legal_rag_rag::DocumentInfo>,
    pub corpus_documents: usize,
    pub corpus_chunks: usize,
}

pub async fn my_dashboard(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Json<MyDashboard> {
    let (plan, sub) = state.billing.current_plan(&user.id);
    let usage = state.billing.usage_of(&user.id);
    let mine = state.activity.for_user(&user.id, 500);
    let count = |kind: &str| mine.iter().filter(|e| e.kind == kind).count();
    let mut documents = state.rag.list_documents();
    documents.sort_by(|a, b| b.ingested_at.cmp(&a.ingested_at));
    documents.truncate(6);
    Json(MyDashboard {
        plan_name: plan.name.clone(),
        plan_id: plan.id.clone(),
        tokens_used: usage.total(),
        token_quota: plan.monthly_tokens,
        period_end: sub.period_end,
        counts: KindCounts {
            queries: count("query"),
            researches: count("research"),
            workflows: count("workflow"),
            interacts: count("interact"),
            drafts: count("draft"),
            uploads: count("upload"),
        },
        recent_activity: state.activity.for_user(&user.id, 60),
        documents,
        corpus_documents: state.rag.list_documents().len(),
        corpus_chunks: state.rag.corpus_size().await,
    })
}

// ------------------------------------------------------------ admin insights

#[derive(Serialize)]
pub struct DayPoint {
    pub date: String,
    pub events: u64,
    pub tokens: u64,
}

#[derive(Serialize)]
pub struct PlanSlice {
    pub plan: String,
    pub users: usize,
}

#[derive(Serialize)]
pub struct UserInsight {
    pub user_id: String,
    pub email: String,
    pub role: String,
    pub plan_name: String,
    pub events: usize,
    pub queries: usize,
    pub researches: usize,
    pub tokens_activity: u64,
    pub tokens_billed: u64,
    pub token_quota: u64,
    pub spent_inr: u64,
    pub last_active: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
pub struct TopQuery {
    pub question: String,
    pub count: usize,
    pub last_asked: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct AdminInsights {
    pub total_users: usize,
    pub total_events: usize,
    pub total_queries: usize,
    pub total_tokens_billed: u64,
    pub documents: usize,
    pub chunks: usize,
    pub mrr_inr: u64,
    pub collected_inr: u64,
    pub active_subscriptions: usize,
    pub series_14d: Vec<DayPoint>,
    pub plan_distribution: Vec<PlanSlice>,
    pub per_user: Vec<UserInsight>,
    pub top_queries: Vec<TopQuery>,
    pub recent_activity: Vec<ActivityEvent>,
}

/// One user's full activity feed (admin drill-down from Insights).
pub async fn admin_user_activity(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    axum::extract::Path(user_id): axum::extract::Path<String>,
) -> Json<Vec<ActivityEvent>> {
    Json(state.activity.for_user(&user_id, 50))
}

pub async fn admin_insights(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Result<Json<AdminInsights>, ApiError> {
    let users = state.auth.list_users()?;
    let user_tuples: Vec<(String, String, String)> =
        users.iter().map(|u| (u.id.clone(), u.email.clone(), u.role.clone())).collect();
    let overview = state.billing.overview(&user_tuples);
    let events = state.activity.all();
    let now = Utc::now();

    // 14-day series (oldest → newest).
    let mut series = Vec::new();
    for i in (0..14).rev() {
        let day = (now - Duration::days(i)).format("%Y-%m-%d").to_string();
        let day_events: Vec<&ActivityEvent> = events
            .iter()
            .filter(|e| e.created_at.format("%Y-%m-%d").to_string() == day)
            .collect();
        series.push(DayPoint {
            date: day,
            events: day_events.len() as u64,
            tokens: day_events.iter().map(|e| e.tokens).sum(),
        });
    }

    // Plan distribution from the billing overview rows.
    let mut plan_counts: HashMap<String, usize> = HashMap::new();
    for row in &overview.users {
        *plan_counts.entry(row.plan_name.clone()).or_default() += 1;
    }
    let plan_distribution: Vec<PlanSlice> = plan_counts
        .into_iter()
        .map(|(plan, users)| PlanSlice { plan, users })
        .collect();

    // Per-user merge: billing rows + activity aggregates.
    let mut per_user = Vec::new();
    for row in &overview.users {
        let mine: Vec<&ActivityEvent> =
            events.iter().filter(|e| e.user_id == row.user_id).collect();
        per_user.push(UserInsight {
            user_id: row.user_id.clone(),
            email: row.email.clone(),
            role: row.role.clone(),
            plan_name: row.plan_name.clone(),
            events: mine.len(),
            queries: mine.iter().filter(|e| e.kind == "query").count(),
            researches: mine.iter().filter(|e| e.kind == "research").count(),
            tokens_activity: mine.iter().map(|e| e.tokens).sum(),
            tokens_billed: row.tokens_used,
            token_quota: row.token_quota,
            spent_inr: row.spent_inr,
            last_active: mine.iter().map(|e| e.created_at).max(),
        });
    }
    per_user.sort_by(|a, b| b.events.cmp(&a.events));

    // Top queries by normalized question text.
    let mut query_counts: HashMap<String, (usize, DateTime<Utc>, String)> = HashMap::new();
    for e in events.iter().filter(|e| e.kind == "query") {
        let key = e.title.trim().to_lowercase();
        let entry = query_counts.entry(key).or_insert((0, e.created_at, e.title.clone()));
        entry.0 += 1;
        if e.created_at > entry.1 {
            entry.1 = e.created_at;
        }
    }
    let mut top_queries: Vec<TopQuery> = query_counts
        .into_values()
        .map(|(count, last_asked, question)| TopQuery { question, count, last_asked })
        .collect();
    top_queries.sort_by(|a, b| b.count.cmp(&a.count));
    top_queries.truncate(10);

    Ok(Json(AdminInsights {
        total_users: users.len(),
        total_events: events.len(),
        total_queries: events.iter().filter(|e| e.kind == "query").count(),
        total_tokens_billed: overview.users.iter().map(|u| u.tokens_used).sum(),
        documents: state.rag.list_documents().len(),
        chunks: state.rag.corpus_size().await,
        mrr_inr: overview.mrr_inr,
        collected_inr: overview.collected_inr,
        active_subscriptions: overview.active_subscriptions,
        series_14d: series,
        plan_distribution,
        per_user,
        top_queries,
        recent_activity: state.activity.recent(30),
    }))
}
