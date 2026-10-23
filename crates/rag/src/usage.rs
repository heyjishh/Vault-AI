//! Request-scoped token usage metering.
//!
//! Providers record the tokens they consume into a task-local accumulator
//! whenever one is installed (see [`scope_usage`]). The server installs a
//! scope around each user-attributable operation (query, research, workflow,
//! ingest), then bills the accumulated totals against the user's plan.
//! Calls made outside any scope (startup warmup, tests) are simply not
//! recorded. Cache hits record nothing — the wrapped provider is never
//! called — which matches their zero marginal cost.

use std::future::Future;
use std::sync::{Arc, Mutex};

/// Token totals for one metered operation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UsageTotals {
    pub llm_prompt_tokens: u64,
    pub llm_completion_tokens: u64,
    pub embedding_tokens: u64,
}

impl UsageTotals {
    /// The single number plans are billed against: every token counts.
    pub fn total(&self) -> u64 {
        self.llm_prompt_tokens + self.llm_completion_tokens + self.embedding_tokens
    }

    pub fn add(&mut self, other: &UsageTotals) {
        self.llm_prompt_tokens += other.llm_prompt_tokens;
        self.llm_completion_tokens += other.llm_completion_tokens;
        self.embedding_tokens += other.embedding_tokens;
    }
}

/// Shared handle to an operation's accumulator.
pub type UsageScope = Arc<Mutex<UsageTotals>>;

pub fn new_scope() -> UsageScope {
    Arc::new(Mutex::new(UsageTotals::default()))
}

tokio::task_local! {
    static CURRENT_USAGE: UsageScope;
}

/// Run `fut` with `scope` installed as the current usage accumulator.
pub async fn scope_usage<F: Future>(scope: UsageScope, fut: F) -> F::Output {
    CURRENT_USAGE.scope(scope, fut).await
}

/// Record LLM tokens into the current scope, if any.
pub fn record_llm(prompt_tokens: u64, completion_tokens: u64) {
    let _ = CURRENT_USAGE.try_with(|scope| {
        if let Ok(mut totals) = scope.lock() {
            totals.llm_prompt_tokens += prompt_tokens;
            totals.llm_completion_tokens += completion_tokens;
        }
    });
}

/// Record embedding tokens into the current scope, if any.
pub fn record_embeddings(tokens: u64) {
    let _ = CURRENT_USAGE.try_with(|scope| {
        if let Ok(mut totals) = scope.lock() {
            totals.embedding_tokens += tokens;
        }
    });
}

/// Rough token estimate (≈4 chars/token) used when a provider does not
/// report usage. Good enough for quota accounting; provider-reported
/// usage is preferred wherever available.
pub fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64 + 3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_add_up() {
        let mut a = UsageTotals { llm_prompt_tokens: 10, llm_completion_tokens: 5, embedding_tokens: 3 };
        a.add(&UsageTotals { llm_prompt_tokens: 1, llm_completion_tokens: 2, embedding_tokens: 4 });
        assert_eq!(a.total(), 25);
    }

    #[test]
    fn estimate_is_monotonic() {
        assert!(estimate_tokens("hello world, this is a longer prompt") > estimate_tokens("hi"));
        assert_eq!(estimate_tokens(""), 0);
    }

    #[tokio::test]
    async fn scope_collects_records() {
        let scope = new_scope();
        scope_usage(scope.clone(), async {
            record_llm(100, 50);
            record_embeddings(25);
        })
        .await;
        let totals = *scope.lock().unwrap();
        assert_eq!(totals.total(), 175);
    }

    #[tokio::test]
    async fn no_scope_no_panic() {
        record_llm(1, 1);
        record_embeddings(1);
    }
}
