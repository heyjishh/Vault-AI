//! Billing: token-based subscription plans, usage metering, payments.
//!
//! Plans grant a monthly token quota; every metered AI operation (query,
//! research, workflows, ingest) bills its LLM + embedding tokens against
//! the user's current period. Payments run through a pluggable provider:
//! `mock` for development, or a self-hosted **YouPI-PG** UPI gateway
//! (admin-configured) for real INR collection. All state persists to
//! `billing.json` in the data directory (atomic writes on every mutation).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Duration, Utc};
use legal_rag_rag::usage::UsageTotals;
use serde::{Deserialize, Serialize};

pub const PERIOD_DAYS: i64 = 30;

// ---------------------------------------------------------------- plans

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Plan {
    pub id: String,
    pub name: String,
    /// Price per period in whole INR (0 = free).
    pub price_inr: u64,
    /// Tokens included per period (LLM prompt + completion + embeddings).
    pub monthly_tokens: u64,
    pub features: Vec<String>,
}

pub fn plan_catalog() -> Vec<Plan> {
    vec![
        Plan {
            id: "free".into(),
            name: "Free".into(),
            price_inr: 0,
            monthly_tokens: 50_000,
            features: vec![
                "50,000 tokens / month".into(),
                "Assistant with citations".into(),
                "Vault: personal documents".into(),
                "Community support".into(),
            ],
        },
        Plan {
            id: "basic".into(),
            name: "Basic".into(),
            price_inr: 2_999,
            monthly_tokens: 2_000_000,
            features: vec![
                "2,000,000 tokens / month".into(),
                "Research mode + web evidence".into(),
                "Agent workflows (contract review, due diligence, memos)".into(),
                "Routed Indic OCR (13 languages)".into(),
                "Email support".into(),
            ],
        },
        Plan {
            id: "premium".into(),
            name: "Premium".into(),
            price_inr: 9_999,
            monthly_tokens: 10_000_000,
            features: vec![
                "10,000,000 tokens / month".into(),
                "Everything in Basic".into(),
                "Priority processing".into(),
                "Admin Command Center seats".into(),
                "Priority support".into(),
            ],
        },
    ]
}

pub fn plan_by_id(id: &str) -> Option<Plan> {
    plan_catalog().into_iter().find(|p| p.id == id)
}

// ---------------------------------------------------------------- state

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub user_id: String,
    pub plan_id: String,
    /// active | cancelled | expired
    pub status: String,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsagePeriod {
    pub period_start: DateTime<Utc>,
    pub llm_prompt_tokens: u64,
    pub llm_completion_tokens: u64,
    pub embedding_tokens: u64,
}

impl UsagePeriod {
    pub fn total(&self) -> u64 {
        self.llm_prompt_tokens + self.llm_completion_tokens + self.embedding_tokens
    }
    fn add(&mut self, t: &UsageTotals) {
        self.llm_prompt_tokens += t.llm_prompt_tokens;
        self.llm_completion_tokens += t.llm_completion_tokens;
        self.embedding_tokens += t.embedding_tokens;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payment {
    pub id: String,
    /// Merchant order id sent to the gateway (`JV-…`).
    pub order_id: String,
    pub user_id: String,
    pub user_email: String,
    pub plan_id: String,
    pub amount_inr: u64,
    /// mock | youpi
    pub provider: String,
    pub provider_payment_id: Option<String>,
    pub checkout_url: Option<String>,
    /// initiated | success | failed | expired | cancelled
    pub status: String,
    /// Bank UTR once the gateway confirms (YouPI webhooks).
    pub utr: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentSettings {
    /// mock | youpi
    pub provider: String,
    /// Base URL of the admin's self-hosted YouPI-PG (e.g. https://pay.firm.in).
    pub youpi_base_url: String,
    /// Optional public merchant id (X-Merchant-Id header).
    pub youpi_merchant_id: String,
    /// Webhook HMAC secret configured in YouPI-PG (write-only via API).
    pub youpi_webhook_secret: String,
}

impl Default for PaymentSettings {
    fn default() -> Self {
        Self {
            provider: "mock".into(),
            youpi_base_url: String::new(),
            youpi_merchant_id: String::new(),
            youpi_webhook_secret: String::new(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BillingData {
    subscriptions: HashMap<String, Subscription>,
    usage: HashMap<String, UsagePeriod>,
    payments: Vec<Payment>,
    settings: PaymentSettings,
}

// ---------------------------------------------------------------- store

pub struct BillingStore {
    data: RwLock<BillingData>,
    path: PathBuf,
}

impl BillingStore {
    /// Load from disk (or start fresh), seeding payment settings from env
    /// on first boot: `LEGAL_RAG_PAYMENT_PROVIDER`, `LEGAL_RAG_YOUPI_URL`,
    /// `LEGAL_RAG_YOUPI_MERCHANT_ID`, `LEGAL_RAG_YOUPI_WEBHOOK_SECRET`.
    pub fn load(path: PathBuf) -> Self {
        let mut data: BillingData = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if data.settings.provider == "mock"
            && data.settings.youpi_base_url.is_empty()
            && std::env::var("LEGAL_RAG_YOUPI_URL").is_ok()
        {
            data.settings.provider =
                std::env::var("LEGAL_RAG_PAYMENT_PROVIDER").unwrap_or_else(|_| "youpi".into());
            data.settings.youpi_base_url =
                std::env::var("LEGAL_RAG_YOUPI_URL").unwrap_or_default();
            data.settings.youpi_merchant_id =
                std::env::var("LEGAL_RAG_YOUPI_MERCHANT_ID").unwrap_or_default();
            data.settings.youpi_webhook_secret =
                std::env::var("LEGAL_RAG_YOUPI_WEBHOOK_SECRET").unwrap_or_default();
        }
        let store = Self { data: RwLock::new(data), path };
        store.save();
        store
    }

    fn save(&self) {
        if let Ok(data) = self.data.read() {
            if let Ok(bytes) = serde_json::to_vec_pretty(&*data) {
                let tmp = self.path.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(tmp, &self.path);
                }
            }
        }
    }

    // ----- subscriptions & quota -----

    /// The user's effective subscription: their paid plan while its period
    /// is live, otherwise an implicit Free subscription.
    fn effective_subscription(&self, data: &BillingData, user_id: &str) -> Subscription {
        let now = Utc::now();
        if let Some(sub) = data.subscriptions.get(user_id) {
            if sub.status == "active" && sub.period_end > now {
                return sub.clone();
            }
        }
        Subscription {
            user_id: user_id.to_string(),
            plan_id: "free".into(),
            status: "active".into(),
            period_start: now - Duration::days(1),
            period_end: now + Duration::days(PERIOD_DAYS),
            updated_at: now,
        }
    }

    pub fn current_plan(&self, user_id: &str) -> (Plan, Subscription) {
        let data = self.data.read().unwrap();
        let sub = self.effective_subscription(&data, user_id);
        let plan = plan_by_id(&sub.plan_id).unwrap_or_else(|| plan_by_id("free").unwrap());
        (plan, sub)
    }

    /// Usage totals for the user's current period (aligned to the
    /// subscription window; resets when a new period begins).
    pub fn usage_of(&self, user_id: &str) -> UsagePeriod {
        let data = self.data.read().unwrap();
        let sub = self.effective_subscription(&data, user_id);
        match data.usage.get(user_id) {
            Some(u) if u.period_start >= sub.period_start - Duration::seconds(1) => u.clone(),
            _ => UsagePeriod { period_start: sub.period_start, ..Default::default() },
        }
    }

    pub fn quota_exceeded(&self, user_id: &str) -> bool {
        let (plan, _) = self.current_plan(user_id);
        self.usage_of(user_id).total() >= plan.monthly_tokens
    }

    /// Bill a completed operation's tokens to the user.
    pub fn add_usage(&self, user_id: &str, totals: &UsageTotals) {
        if totals.total() == 0 {
            return;
        }
        {
            let mut data = self.data.write().unwrap();
            let sub = self.effective_subscription(&data, user_id);
            let entry = data.usage.entry(user_id.to_string()).or_insert(UsagePeriod {
                period_start: sub.period_start,
                ..Default::default()
            });
            if entry.period_start < sub.period_start - Duration::seconds(1) {
                *entry = UsagePeriod { period_start: sub.period_start, ..Default::default() };
            }
            entry.add(totals);
        }
        self.save();
    }

    // ----- payments -----

    pub fn create_payment(&self, user_id: &str, email: &str, plan: &Plan, provider: &str) -> Payment {
        let id = legal_rag_common::new_id();
        let payment = Payment {
            order_id: format!("JV-{}", &id[..8]),
            id,
            user_id: user_id.to_string(),
            user_email: email.to_string(),
            plan_id: plan.id.clone(),
            amount_inr: plan.price_inr,
            provider: provider.to_string(),
            provider_payment_id: None,
            checkout_url: None,
            status: "initiated".into(),
            utr: None,
            created_at: Utc::now(),
            completed_at: None,
        };
        {
            let mut data = self.data.write().unwrap();
            data.payments.push(payment.clone());
        }
        self.save();
        payment
    }

    pub fn payment_by_id(&self, id: &str) -> Option<Payment> {
        self.data.read().unwrap().payments.iter().find(|p| p.id == id).cloned()
    }

    pub fn payment_id_by_order(&self, order_id: &str) -> Option<String> {
        self.data
            .read()
            .unwrap()
            .payments
            .iter()
            .find(|p| p.order_id == order_id)
            .map(|p| p.id.clone())
    }

    pub fn set_provider_session(&self, id: &str, provider_payment_id: &str, checkout_url: &str) {
        {
            let mut data = self.data.write().unwrap();
            if let Some(p) = data.payments.iter_mut().find(|p| p.id == id) {
                p.provider_payment_id = Some(provider_payment_id.to_string());
                p.checkout_url = Some(checkout_url.to_string());
            }
        }
        self.save();
    }

    pub fn set_payment_status(&self, id: &str, status: &str) {
        {
            let mut data = self.data.write().unwrap();
            if let Some(p) = data.payments.iter_mut().find(|p| p.id == id) {
                if p.status != "success" {
                    p.status = status.to_string();
                }
            }
        }
        self.save();
    }

    /// Mark a payment successful and activate/extend the buyer's
    /// subscription for one period. Idempotent; returns the payment.
    pub fn mark_paid(&self, payment_id: &str, utr: Option<String>) -> Option<Payment> {
        let updated = {
            let mut data = self.data.write().unwrap();
            let payment = data.payments.iter_mut().find(|p| p.id == payment_id)?;
            if payment.status == "success" {
                return Some(payment.clone());
            }
            payment.status = "success".into();
            payment.completed_at = Some(Utc::now());
            if utr.is_some() {
                payment.utr = utr;
            }
            let payment = payment.clone();
            let now = Utc::now();
            // Extend from the current period end when renewing the same
            // plan early; otherwise start a fresh period today.
            let existing = data.subscriptions.get(&payment.user_id);
            let start = match existing {
                Some(s) if s.plan_id == payment.plan_id && s.period_end > now => s.period_start,
                _ => now,
            };
            let end = match existing {
                Some(s) if s.plan_id == payment.plan_id && s.period_end > now => {
                    s.period_end + Duration::days(PERIOD_DAYS)
                }
                _ => now + Duration::days(PERIOD_DAYS),
            };
            data.subscriptions.insert(
                payment.user_id.clone(),
                Subscription {
                    user_id: payment.user_id.clone(),
                    plan_id: payment.plan_id.clone(),
                    status: "active".into(),
                    period_start: start,
                    period_end: end,
                    updated_at: now,
                },
            );
            // New paid period: reset the usage window.
            data.usage.insert(
                payment.user_id.clone(),
                UsagePeriod { period_start: start, ..Default::default() },
            );
            Some(payment)
        };
        self.save();
        updated
    }

    pub fn payments_of(&self, user_id: &str) -> Vec<Payment> {
        let mut payments: Vec<Payment> = self
            .data
            .read()
            .unwrap()
            .payments
            .iter()
            .filter(|p| p.user_id == user_id)
            .cloned()
            .collect();
        payments.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        payments
    }

    // ----- settings -----

    pub fn settings(&self) -> PaymentSettings {
        self.data.read().unwrap().settings.clone()
    }

    pub fn update_settings(&self, settings: PaymentSettings) {
        {
            let mut data = self.data.write().unwrap();
            data.settings = settings;
        }
        self.save();
    }

    // ----- admin overview -----

    pub fn overview(&self, users: &[(String, String, String)]) -> BillingOverview {
        let data = self.data.read().unwrap();
        let now = Utc::now();
        let mut rows = Vec::new();
        let mut active_subs = 0usize;
        let mut mrr_inr = 0u64;
        for (id, email, role) in users {
            let sub = self.effective_subscription(&data, id);
            let plan = plan_by_id(&sub.plan_id).unwrap_or_else(|| plan_by_id("free").unwrap());
            let is_paid = plan.price_inr > 0 && sub.period_end > now;
            if is_paid {
                active_subs += 1;
                mrr_inr += plan.price_inr;
            }
            let usage = match data.usage.get(id) {
                Some(u) if u.period_start >= sub.period_start - Duration::seconds(1) => u.total(),
                _ => 0,
            };
            let spent: u64 = data
                .payments
                .iter()
                .filter(|p| p.user_id == *id && p.status == "success")
                .map(|p| p.amount_inr)
                .sum();
            rows.push(UserBillingRow {
                user_id: id.clone(),
                email: email.clone(),
                role: role.clone(),
                plan_id: plan.id.clone(),
                plan_name: plan.name.clone(),
                subscription_status: if is_paid || plan.id == "free" {
                    "active".into()
                } else {
                    sub.status.clone()
                },
                period_end: sub.period_end,
                tokens_used: usage,
                token_quota: plan.monthly_tokens,
                spent_inr: spent,
            });
        }
        rows.sort_by(|a, b| b.spent_inr.cmp(&a.spent_inr));
        let collected_inr: u64 = data
            .payments
            .iter()
            .filter(|p| p.status == "success")
            .map(|p| p.amount_inr)
            .sum();
        let mut recent: Vec<Payment> = data.payments.clone();
        recent.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        recent.truncate(20);
        BillingOverview {
            total_users: users.len(),
            active_subscriptions: active_subs,
            mrr_inr,
            collected_inr,
            payments_count: data.payments.iter().filter(|p| p.status == "success").count(),
            users: rows,
            recent_payments: recent,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct UserBillingRow {
    pub user_id: String,
    pub email: String,
    pub role: String,
    pub plan_id: String,
    pub plan_name: String,
    pub subscription_status: String,
    pub period_end: DateTime<Utc>,
    pub tokens_used: u64,
    pub token_quota: u64,
    pub spent_inr: u64,
}

#[derive(Debug, Serialize)]
pub struct BillingOverview {
    pub total_users: usize,
    pub active_subscriptions: usize,
    pub mrr_inr: u64,
    pub collected_inr: u64,
    pub payments_count: usize,
    pub users: Vec<UserBillingRow>,
    pub recent_payments: Vec<Payment>,
}

// ---------------------------------------------------------------- YouPI-PG

/// Create a payment session on the admin's self-hosted YouPI-PG gateway.
/// API per the project's OpenAPI spec: `POST /api/v1/payments` returns a
/// hosted `checkoutUrl`; confirmation arrives via HMAC-signed webhook and
/// can also be polled via `GET /api/v1/payments/{id}`.
pub async fn youpi_create_session(
    settings: &PaymentSettings,
    payment: &Payment,
    customer_name: &str,
    customer_email: &str,
) -> Result<(String, String), String> {
    let base = settings.youpi_base_url.trim_end_matches('/');
    if base.is_empty() {
        return Err("YouPI-PG base URL is not configured (admin: Billing settings)".into());
    }
    let client = reqwest::Client::new();
    let mut req = client
        .post(format!("{base}/api/v1/payments"))
        .header("Idempotency-Key", payment.id.clone())
        .json(&serde_json::json!({
            "orderId": payment.order_id,
            "amount": format!("{}.00", payment.amount_inr),
            "currency": "INR",
            "customer": { "name": customer_name, "email": customer_email },
            "description": format!("Juris Vault AI {} plan (monthly)", payment.plan_id),
            "metadata": { "user_id": payment.user_id, "plan_id": payment.plan_id, "payment_id": payment.id },
        }));
    if !settings.youpi_merchant_id.is_empty() {
        req = req.header("X-Merchant-Id", settings.youpi_merchant_id.clone());
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("cannot reach YouPI-PG gateway: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("YouPI-PG rejected the payment session (HTTP {})", resp.status()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("bad response from YouPI-PG: {e}"))?;
    let payment_id = body
        .get("paymentId")
        .and_then(|v| v.as_str())
        .ok_or("YouPI-PG response missing paymentId")?
        .to_string();
    let checkout_url = body
        .get("checkoutUrl")
        .and_then(|v| v.as_str())
        .ok_or("YouPI-PG response missing checkoutUrl")?
        .to_string();
    Ok((payment_id, checkout_url))
}

/// Poll a payment's status on YouPI-PG; returns the gateway status string
/// (INITIATED/ACTIVE/QUEUED/PROCESSING/SUCCESS/FAILED/EXPIRED/CANCELLED).
pub async fn youpi_get_status(settings: &PaymentSettings, provider_payment_id: &str) -> Result<String, String> {
    let base = settings.youpi_base_url.trim_end_matches('/');
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/api/v1/payments/{provider_payment_id}"))
        .send()
        .await
        .map_err(|e| format!("cannot reach YouPI-PG gateway: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("YouPI-PG status check failed (HTTP {})", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| format!("bad response: {e}"))?;
    body.get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "YouPI-PG response missing status".to_string())
}

/// Verify a YouPI-PG webhook: `X-Gateway-Signature: sha256=<hex>` where
/// the hex is HMAC-SHA256(webhook_secret, raw_body). Compared in constant
/// time-ish fashion (length check + XOR fold).
pub fn verify_youpi_webhook(secret: &str, raw_body: &[u8], signature_header: &str) -> bool {
    if secret.is_empty() {
        return true; // no secret configured in YouPI: nothing to verify against
    }
    let Some(hex_part) = signature_header.strip_prefix("sha256=") else {
        return false;
    };
    let expected = hmac_sha256_hex(secret.as_bytes(), raw_body);
    if hex_part.len() != expected.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in hex_part.bytes().zip(expected.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut key_block = [0u8; 64];
    if key.len() > 64 {
        let hashed = Sha256::digest(key);
        key_block[..hashed.len()].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let ipad: Vec<u8> = key_block.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = key_block.iter().map(|b| b ^ 0x5c).collect();
    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(inner_hash);
    outer.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> BillingStore {
        BillingStore {
            data: RwLock::new(BillingData::default()),
            path: PathBuf::from("/dev/null"),
        }
    }

    #[test]
    fn catalog_is_sane() {
        let plans = plan_catalog();
        assert_eq!(plans.len(), 3);
        assert_eq!(plans[0].price_inr, 0);
        assert!(plans[1].monthly_tokens > plans[0].monthly_tokens);
        assert!(plans[2].monthly_tokens > plans[1].monthly_tokens);
    }

    #[test]
    fn free_plan_default_and_quota() {
        let s = store();
        let (plan, _) = s.current_plan("u1");
        assert_eq!(plan.id, "free");
        assert!(!s.quota_exceeded("u1"));
        s.add_usage("u1", &UsageTotals { llm_prompt_tokens: 60_000, ..Default::default() });
        assert!(s.quota_exceeded("u1"));
    }

    #[test]
    fn payment_activates_plan_and_resets_usage() {
        let s = store();
        s.add_usage("u1", &UsageTotals { llm_prompt_tokens: 40_000, ..Default::default() });
        let basic = plan_by_id("basic").unwrap();
        let p = s.create_payment("u1", "a@b.c", &basic, "mock");
        let paid = s.mark_paid(&p.id, None).unwrap();
        assert_eq!(paid.status, "success");
        let (plan, sub) = s.current_plan("u1");
        assert_eq!(plan.id, "basic");
        assert!(sub.period_end > Utc::now());
        assert_eq!(s.usage_of("u1").total(), 0, "usage resets on plan activation");
        // Idempotent.
        assert!(s.mark_paid(&p.id, None).is_some());
    }

    #[test]
    fn hmac_matches_known_vector() {
        // RFC 4231 test case 2.
        let hex = hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert!(verify_youpi_webhook(
            "Jefe",
            b"what do ya want for nothing?",
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        ));
        assert!(!verify_youpi_webhook("Jefe", b"tampered", "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"));
    }
}
