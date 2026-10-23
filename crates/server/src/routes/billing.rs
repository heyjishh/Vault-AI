//! `/api/v1/billing/*` — plans, subscriptions, checkout, webhooks;
//! `/api/v1/admin/billing/*` — payment settings + spend insights.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use legal_rag_auth::{AdminUser, AuthUser};
use serde::{Deserialize, Serialize};

use crate::billing::{
    plan_by_id, plan_catalog, youpi_create_session, youpi_get_status, verify_youpi_webhook,
    Payment, PaymentSettings, Plan, Subscription,
};
use crate::error::ApiError;
use crate::state::AppState;

// ------------------------------------------------------------ metering

/// Reject the request with HTTP 402 when the user's period quota is used up.
pub fn ensure_quota(state: &AppState, user_id: &str) -> Result<(), ApiError> {
    if state.billing.quota_exceeded(user_id) {
        let (plan, _) = state.billing.current_plan(user_id);
        return Err(legal_rag_common::Error::Quota(format!(
            "token quota exhausted on the {} plan — upgrade to continue",
            plan.name
        ))
        .into());
    }
    Ok(())
}

/// Settle a finished metered operation: bill its tokens to the user and
/// capture it in the activity log (dashboards, insights, golden loop).
#[allow(clippy::too_many_arguments)]
pub fn settle(
    state: &AppState,
    user: &legal_rag_auth::User,
    scope: &legal_rag_rag::usage::UsageScope,
    kind: &str,
    title: &str,
    detail: &str,
    result: &str,
    citations: usize,
) {
    settle_matter(state, user, scope, kind, title, detail, result, citations, None)
}

/// `settle` with a restorable payload attached to the activity event.
#[allow(clippy::too_many_arguments)]
pub fn settle_with_payload(
    state: &AppState,
    user: &legal_rag_auth::User,
    scope: &legal_rag_rag::usage::UsageScope,
    kind: &str,
    title: &str,
    detail: &str,
    result: &str,
    citations: usize,
    payload: Option<serde_json::Value>,
) {
    settle_matter_payload(state, user, scope, kind, title, detail, result, citations, None, payload)
}

/// `settle` for work done inside a matter: the activity record carries the
/// matter id, so it lands in the matter's living history.
#[allow(clippy::too_many_arguments)]
pub fn settle_matter(
    state: &AppState,
    user: &legal_rag_auth::User,
    scope: &legal_rag_rag::usage::UsageScope,
    kind: &str,
    title: &str,
    detail: &str,
    result: &str,
    citations: usize,
    matter_id: Option<String>,
) {
    settle_matter_payload(state, user, scope, kind, title, detail, result, citations, matter_id, None)
}

/// `settle_matter` with a restorable payload on the activity event.
#[allow(clippy::too_many_arguments)]
pub fn settle_matter_payload(
    state: &AppState,
    user: &legal_rag_auth::User,
    scope: &legal_rag_rag::usage::UsageScope,
    kind: &str,
    title: &str,
    detail: &str,
    result: &str,
    citations: usize,
    matter_id: Option<String>,
    payload: Option<serde_json::Value>,
) {
    let totals = scope.lock().map(|t| *t).unwrap_or_default();
    state.billing.add_usage(&user.id, &totals);
    state.activity.record(
        crate::activity::ActivityEvent::new(
            &user.id,
            &user.email,
            kind,
            title,
            detail,
            result,
            citations,
            totals.total(),
        )
        .with_matter(matter_id)
        .with_payload(payload),
    );
}

// ------------------------------------------------------------ user side

pub async fn list_plans(
    AuthUser(_user): AuthUser,
) -> Json<Vec<Plan>> {
    Json(plan_catalog())
}

#[derive(Serialize)]
pub struct MyBilling {
    pub plan: Plan,
    pub subscription: Subscription,
    pub tokens_used: u64,
    pub token_quota: u64,
    pub llm_prompt_tokens: u64,
    pub llm_completion_tokens: u64,
    pub embedding_tokens: u64,
    pub payments: Vec<Payment>,
}

pub async fn my_billing(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Json<MyBilling> {
    let (plan, subscription) = state.billing.current_plan(&user.id);
    let usage = state.billing.usage_of(&user.id);
    Json(MyBilling {
        token_quota: plan.monthly_tokens,
        plan,
        subscription,
        tokens_used: usage.total(),
        llm_prompt_tokens: usage.llm_prompt_tokens,
        llm_completion_tokens: usage.llm_completion_tokens,
        embedding_tokens: usage.embedding_tokens,
        payments: state.billing.payments_of(&user.id),
    })
}

#[derive(Deserialize)]
pub struct CheckoutRequest {
    pub plan_id: String,
}

#[derive(Serialize)]
pub struct CheckoutResponse {
    pub payment_id: String,
    /// mock | redirect
    pub mode: String,
    pub amount_inr: u64,
    pub checkout_url: Option<String>,
}

/// Start a subscription purchase for a paid plan.
pub async fn checkout(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<CheckoutRequest>,
) -> Result<Json<CheckoutResponse>, ApiError> {
    let plan = plan_by_id(&body.plan_id)
        .ok_or_else(|| legal_rag_common::Error::BadRequest("unknown plan".into()))?;
    if plan.price_inr == 0 {
        return Err(legal_rag_common::Error::BadRequest(
            "the Free plan needs no purchase".into(),
        )
        .into());
    }
    let settings = state.billing.settings();
    let payment = state.billing.create_payment(&user.id, &user.email, &plan, &settings.provider);
    if settings.provider == "youpi" {
        let name = user.email.split('@').next().unwrap_or("Customer").to_string();
        match youpi_create_session(&settings, &payment, &name, &user.email).await {
            Ok((provider_payment_id, checkout_url)) => {
                state.billing.set_provider_session(&payment.id, &provider_payment_id, &checkout_url);
                Ok(Json(CheckoutResponse {
                    payment_id: payment.id,
                    mode: "redirect".into(),
                    amount_inr: plan.price_inr,
                    checkout_url: Some(checkout_url),
                }))
            }
            Err(e) => {
                state.billing.set_payment_status(&payment.id, "failed");
                Err(legal_rag_common::Error::Upstream(e).into())
            }
        }
    } else {
        Ok(Json(CheckoutResponse {
            payment_id: payment.id,
            mode: "mock".into(),
            amount_inr: plan.price_inr,
            checkout_url: None,
        }))
    }
}

/// Development/test provider: simulate a completed payment.
pub async fn complete_mock(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Payment>, ApiError> {
    let payment = state
        .billing
        .payment_by_id(&id)
        .filter(|p| p.user_id == user.id)
        .ok_or_else(|| legal_rag_common::Error::NotFound("payment not found".into()))?;
    if payment.provider != "mock" {
        return Err(legal_rag_common::Error::BadRequest(
            "this payment uses the live gateway; complete it at checkout".into(),
        )
        .into());
    }
    let paid = state
        .billing
        .mark_paid(&id, None)
        .ok_or_else(|| legal_rag_common::Error::NotFound("payment not found".into()))?;
    Ok(Json(paid))
}

/// Payment status; for gateway payments this polls YouPI-PG and settles
/// the subscription the moment the gateway reports SUCCESS.
pub async fn payment_status(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Payment>, ApiError> {
    let payment = state
        .billing
        .payment_by_id(&id)
        .filter(|p| p.user_id == user.id)
        .ok_or_else(|| legal_rag_common::Error::NotFound("payment not found".into()))?;
    if payment.provider == "youpi" && payment.status == "initiated" {
        if let Some(provider_id) = &payment.provider_payment_id {
            let settings = state.billing.settings();
            if let Ok(status) = youpi_get_status(&settings, provider_id).await {
                match status.as_str() {
                    "SUCCESS" => {
                        state.billing.mark_paid(&id, None);
                    }
                    "FAILED" | "EXPIRED" | "CANCELLED" => {
                        state.billing.set_payment_status(&id, &status.to_lowercase());
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(Json(state.billing.payment_by_id(&id).unwrap_or(payment)))
}

pub async fn my_payments(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Json<Vec<Payment>> {
    Json(state.billing.payments_of(&user.id))
}

// ------------------------------------------------------------ webhook

/// YouPI-PG authoritative confirmation (HMAC-SHA256 over the raw body).
pub async fn youpi_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let settings = state.billing.settings();
    let signature = headers
        .get("x-gateway-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !verify_youpi_webhook(&settings.youpi_webhook_secret, &body, signature) {
        return Err(legal_rag_common::Error::Auth("invalid webhook signature".into()).into());
    }
    let payload: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| legal_rag_common::Error::BadRequest(format!("bad webhook body: {e}")))?;
    let event = payload.get("event").and_then(|v| v.as_str()).unwrap_or("");
    let payment_id = payload
        .get("metadata")
        .and_then(|m| m.get("payment_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            payload
                .get("orderId")
                .and_then(|v| v.as_str())
                .and_then(|order| state.billing.payment_id_by_order(order))
        });
    if let Some(payment_id) = payment_id {
        match event {
            "SUCCESS" => {
                let utr = payload
                    .get("utr")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                state.billing.mark_paid(&payment_id, utr);
            }
            "FAILED" | "EXPIRED" | "CANCELLED" => {
                state.billing.set_payment_status(&payment_id, &event.to_lowercase());
            }
            _ => {}
        }
    }
    Ok(Json(serde_json::json!({ "received": true })))
}

// ------------------------------------------------------------ admin

pub async fn admin_overview(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Result<Json<crate::billing::BillingOverview>, ApiError> {
    let users = state
        .auth
        .list_users()?
        .into_iter()
        .map(|u| (u.id, u.email, u.role))
        .collect::<Vec<_>>();
    Ok(Json(state.billing.overview(&users)))
}

#[derive(Serialize)]
pub struct SettingsView {
    pub provider: String,
    pub youpi_base_url: String,
    pub youpi_merchant_id: String,
    pub webhook_secret_set: bool,
}

impl From<PaymentSettings> for SettingsView {
    fn from(s: PaymentSettings) -> Self {
        Self {
            provider: s.provider,
            youpi_base_url: s.youpi_base_url,
            youpi_merchant_id: s.youpi_merchant_id,
            webhook_secret_set: !s.youpi_webhook_secret.is_empty(),
        }
    }
}

pub async fn admin_get_settings(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Json<SettingsView> {
    Json(state.billing.settings().into())
}

#[derive(Deserialize)]
pub struct SettingsUpdate {
    pub provider: String,
    pub youpi_base_url: String,
    pub youpi_merchant_id: String,
    /// Empty string keeps the stored secret.
    pub youpi_webhook_secret: Option<String>,
}

pub async fn admin_update_settings(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Json(body): Json<SettingsUpdate>,
) -> Result<Json<SettingsView>, ApiError> {
    if body.provider != "mock" && body.provider != "youpi" {
        return Err(legal_rag_common::Error::BadRequest("provider must be mock or youpi".into()).into());
    }
    let mut settings = state.billing.settings();
    settings.provider = body.provider;
    settings.youpi_base_url = body.youpi_base_url.trim().to_string();
    settings.youpi_merchant_id = body.youpi_merchant_id.trim().to_string();
    if let Some(secret) = body.youpi_webhook_secret {
        if !secret.is_empty() {
            settings.youpi_webhook_secret = secret;
        }
    }
    state.billing.update_settings(settings.clone());
    Ok(Json(settings.into()))
}
