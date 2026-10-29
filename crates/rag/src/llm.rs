//! LLM clients: OpenAI-compatible chat completions + mock.

use async_trait::async_trait;
use legal_rag_common::{Error, Result};
use serde::{Deserialize, Serialize};

/// Callback receiving generated text chunks during streaming.
pub type ChunkSink<'a> = dyn FnMut(&str) + Send + 'a;

/// Text generation backend.
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, system: &str, user: &str) -> Result<String>;

    /// Streaming completion: forwards text chunks to `sink` as they are
    /// generated and returns the full assembled text. Default: one chunk.
    async fn complete_stream(
        &self,
        system: &str,
        user: &str,
        sink: &mut ChunkSink<'_>,
    ) -> Result<String> {
        let full = self.complete(system, user).await?;
        if !full.is_empty() {
            sink(&full);
        }
        Ok(full)
    }
}

/// Whole-output provider failure pages, lowercased. Web-backed model
/// bridges return their failure page as an ordinary HTTP 200 completion
/// (the Gemini web bridge behind the test instance is the observed
/// case: a transient Gemini failure arrives as the assistant text
/// "Sorry, something went wrong. Please try your request again.").
/// Nothing in that text marks it as an error — it must be recognised
/// here, at the provider boundary, or it is indistinguishable from a
/// real answer and every downstream cache will store and re-serve it
/// as one (Piece 14, pinned live 2026-10-29: an Interact chat's
/// one-off bridge failure was cached and replayed in 0s forever).
const FAILURE_OUTPUT_PREFIXES: &[&str] = &["sorry, something went wrong"];

/// True when a completion is a provider failure page rather than a
/// generated answer. Deliberately conservative: the ENTIRE trimmed
/// output must be short (≤ 200 chars) and begin with a known failure
/// prefix — a genuine answer that merely discusses a failure, or
/// quotes one inside a longer analysis, never matches.
pub fn is_failure_output(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.chars().count() > 200 {
        return false;
    }
    let lower = t.to_lowercase();
    FAILURE_OUTPUT_PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// The error a failure page becomes: an upstream failure, so callers
/// surface an honest error status and a retry reaches the provider.
/// (The wording deliberately contains no HTTP status code —
/// `retry_upstream` pattern-matches codes in error text, and a
/// failure page is not a retryable HTTP transient at this layer.)
pub(crate) fn failure_output_error() -> Error {
    Error::Upstream(
        "llm provider returned a failure message instead of a completion".into(),
    )
}

/// Retry an upstream call on 429 / transient 5xx with exponential backoff.
/// Mistral's test/free tiers rate-limit aggressively, so this is essential.
pub(crate) async fn retry_upstream<T, F, Fut>(op: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut delay_ms: u64 = 2000;
    for attempt in 1..=4 {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let msg = e.to_string();
                let retryable = ["429", "500", "502", "503", "504"]
                    .iter()
                    .any(|code| msg.contains(code));
                if !retryable || attempt == 4 {
                    return Err(e);
                }
                tracing::warn!(attempt, delay_ms, error = %msg, "upstream busy; backing off");
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                delay_ms *= 3;
            }
        }
    }
    unreachable!("retry loop always returns")
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
}

/// OpenAI-compatible `/chat/completions` endpoint (OpenAI, Azure, Ollama,
/// vLLM, Cloudflare Workers AI...).
pub struct OpenAiLlm {
    client: reqwest::Client,
    url: String,
    api_key: Option<String>,
    model: String,
    max_tokens: Option<u32>,
}

impl OpenAiLlm {
    pub fn new(url: &str, api_key: Option<&str>, model: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.to_string(),
            api_key: api_key.map(|s| s.to_string()),
            model: model.to_string(),
            max_tokens: None,
        }
    }

    /// Cap on generated tokens (important: some providers default to tiny
    /// limits that truncate JSON extraction output mid-string).
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n);
        self
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Deserialize, Clone, Copy)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessageContent,
}

#[derive(Deserialize)]
struct ChatMessageContent {
    content: Option<String>,
}

#[async_trait]
impl LlmClient for OpenAiLlm {
    async fn complete(&self, system: &str, user: &str) -> Result<String> {
        let est_prompt = crate::usage::estimate_tokens(system) + crate::usage::estimate_tokens(user);
        retry_upstream(|| async {
            let mut req = self.client.post(&self.url).json(&ChatRequest {
                model: &self.model,
                messages: vec![
                    ChatMessage { role: "system", content: system },
                    ChatMessage { role: "user", content: user },
                ],
                temperature: 0.1,
                max_tokens: self.max_tokens,
                stream: false,
            });
            if let Some(key) = &self.api_key {
                req = req.bearer_auth(key);
            }
            let resp: ChatResponse = req
                .send()
                .await
                .map_err(|e| Error::Upstream(format!("llm request failed: {e}")))?
                .error_for_status()
                .map_err(|e| Error::Upstream(format!("llm request failed: {e}")))?
                .json()
                .await
                .map_err(|e| Error::Upstream(format!("bad llm response: {e}")))?;
            let usage = resp.usage;
            let content = resp
                .choices
                .into_iter()
                .next()
                .and_then(|c| c.message.content)
                .ok_or_else(|| Error::Upstream("llm returned no content".into()))?;
            // A provider failure page delivered as HTTP 200 content
            // is a failure, not a completion (see is_failure_output):
            // never meter it, cache it, or serve it as an answer.
            if is_failure_output(&content) {
                return Err(failure_output_error());
            }
            // Metering: provider-reported usage wins; estimate otherwise.
            match usage {
                Some(u) => crate::usage::record_llm(u.prompt_tokens, u.completion_tokens),
                None => crate::usage::record_llm(
                    est_prompt,
                    crate::usage::estimate_tokens(&content),
                ),
            }
            Ok(content)
        })
        .await
    }

    /// Real token streaming over the provider's SSE channel: each generated
    /// chunk is forwarded to `sink` the moment it arrives, so the UI can
    /// render word-by-word with no perceived latency.
    async fn complete_stream(
        &self,
        system: &str,
        user: &str,
        sink: &mut ChunkSink<'_>,
    ) -> Result<String> {
        use futures::StreamExt as _;

        let est_prompt = crate::usage::estimate_tokens(system) + crate::usage::estimate_tokens(user);
        let mut req = self.client.post(&self.url).json(&ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage { role: "system", content: system },
                ChatMessage { role: "user", content: user },
            ],
            temperature: 0.1,
            max_tokens: self.max_tokens,
            stream: true,
        });
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Upstream(format!("llm stream request failed: {e}")))?
            .error_for_status()
            .map_err(|e| Error::Upstream(format!("llm stream request failed: {e}")))?;

        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut full = String::new();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk
                .map_err(|e| Error::Upstream(format!("llm stream read failed: {e}")))?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));
            // SSE events are newline-delimited; process complete lines only.
            while let Some(nl) = buffer.find('\n') {
                let line: String = buffer.drain(..=nl).collect();
                let line = line.trim();
                let Some(payload) = line.strip_prefix("data:") else { continue };
                let payload = payload.trim();
                if payload == "[DONE]" {
                    crate::usage::record_llm(est_prompt, crate::usage::estimate_tokens(&full));
                    return finish_stream(full);
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) else { continue };
                if let Some(delta) = v
                    .pointer("/choices/0/delta/content")
                    .and_then(|c| c.as_str())
                {
                    if !delta.is_empty() {
                        full.push_str(delta);
                        sink(delta);
                    }
                }
            }
        }
        crate::usage::record_llm(est_prompt, crate::usage::estimate_tokens(&full));
        finish_stream(full)
    }
}

fn finish_stream(full: String) -> Result<String> {
    if full.is_empty() {
        Err(Error::Upstream("llm stream returned no content".into()))
    } else if is_failure_output(&full) {
        // A streamed provider failure page is a failure, not an
        // answer (see is_failure_output).
        Err(failure_output_error())
    } else {
        Ok(full)
    }
}

/// Fixed-response mock for tests and offline development.
/// An LLM client whose model can be swapped at runtime (admin
/// customization): delegates to the current inner client; `set_model`
/// rebuilds an `OpenAiLlm` against the same endpoint/key with a new model
/// id. In mock mode (no endpoint) swapping is refused.
pub struct DynamicLlm {
    inner: std::sync::RwLock<std::sync::Arc<dyn LlmClient>>,
    endpoint: Option<(String, Option<String>)>,
    model: std::sync::RwLock<String>,
}

impl DynamicLlm {
    pub fn new(
        inner: std::sync::Arc<dyn LlmClient>,
        url: Option<&str>,
        api_key: Option<&str>,
        model: &str,
    ) -> Self {
        Self {
            inner: std::sync::RwLock::new(inner),
            endpoint: url.map(|u| (u.to_string(), api_key.map(str::to_string))),
            model: std::sync::RwLock::new(model.to_string()),
        }
    }

    pub fn model(&self) -> String {
        self.model.read().unwrap().clone()
    }

    pub fn set_model(&self, model: &str) -> Result<()> {
        let (url, key) = self.endpoint.clone().ok_or_else(|| {
            Error::BadRequest("model swapping is unavailable with the mock LLM".to_string())
        })?;
        let client = OpenAiLlm::new(&url, key.as_deref(), model).with_max_tokens(2000);
        *self.inner.write().unwrap() = std::sync::Arc::new(client);
        *self.model.write().unwrap() = model.to_string();
        Ok(())
    }
}

#[async_trait]
impl LlmClient for DynamicLlm {
    async fn complete(&self, system: &str, user: &str) -> Result<String> {
        let inner = self.inner.read().unwrap().clone();
        inner.complete(system, user).await
    }

    async fn complete_stream(
        &self,
        system: &str,
        user: &str,
        sink: &mut ChunkSink<'_>,
    ) -> Result<String> {
        let inner = self.inner.read().unwrap().clone();
        inner.complete_stream(system, user, sink).await
    }
}

pub struct MockLlm {
    response: String,
}

impl MockLlm {
    pub fn new(response: &str) -> Self {
        Self { response: response.to_string() }
    }
}

#[async_trait]
impl LlmClient for MockLlm {
    async fn complete(&self, system: &str, user: &str) -> Result<String> {
        crate::usage::record_llm(
            crate::usage::estimate_tokens(system) + crate::usage::estimate_tokens(user),
            crate::usage::estimate_tokens(&self.response),
        );
        Ok(self.response.clone())
    }

    async fn complete_stream(
        &self,
        _system: &str,
        _user: &str,
        sink: &mut ChunkSink<'_>,
    ) -> Result<String> {
        // Emit word-chunks so the mock path exercises the same UI code.
        for chunk in self.response.split_inclusive(' ') {
            if !chunk.is_empty() {
                sink(chunk);
            }
        }
        crate::usage::record_llm(
            crate::usage::estimate_tokens(&self.response) / 4,
            crate::usage::estimate_tokens(&self.response),
        );
        Ok(self.response.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_output_detects_the_bridge_failure_page() {
        // The exact text the Gemini web bridge delivered as a 200
        // completion on :3001 (2026-10-29), plus trivial variants.
        assert!(is_failure_output(
            "Sorry, something went wrong. Please try your request again."
        ));
        assert!(is_failure_output("  sorry, something went wrong\n"));
        assert!(is_failure_output("Sorry, something went wrong."));
    }

    #[test]
    fn failure_output_ignores_genuine_answers() {
        assert!(!is_failure_output(""));
        assert!(!is_failure_output("   "));
        assert!(!is_failure_output("The notice period is 30 days [doc:0]."));
        // A longer answer that merely opens by discussing a failure
        // is an answer, not a failure page: the whole-output length
        // cap keeps it out.
        let long = format!(
            "Sorry, something went wrong in the transaction. {}",
            "The agreement provides a cure period and a termination right \
             for the non-defaulting party, exercisable on fifteen days' \
             written notice to the defaulting party's registered address. "
                .repeat(3)
        );
        assert!(long.chars().count() > 200);
        assert!(!is_failure_output(&long));
    }
}
