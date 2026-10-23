//! Reranking — a precision pass over retrieved candidates.
//!
//! Neither RAG-Anything nor LightRAG ships a reranker; both expose only a
//! hook (`rerank_model_func`). For legal QA, precision in the top results
//! matters more than recall, so retrieved candidates get a second scoring
//! pass before the best `top_k` reach the answer prompt.
//!
//! Two implementations behind the `Reranker` trait:
//!   * `HttpReranker` — the default decision layer: a local cross-encoder
//!     service (bge-reranker-v2-m3 via `tools/rerank/bge_reranker_sidecar.py`).
//!     Deterministic, offline, and — decisively for the token plans — zero
//!     billable tokens per query. Benchmark (2026-10-20,
//!     ~/workspace/rerank-bench): quality tied with the LLM reranker
//!     (MRR 0.944 each on a 9-query EN/HI legal set with near-miss traps),
//!     while the LLM path burned ~4,093 tokens per rerank.
//!   * `LlmReranker` — listwise LLM scoring, kept as an optional deep judge
//!     (LEGAL_RAG_RERANKER=llm), e.g. for Research mode.
//!
//! Flow: retrieve a wide candidate set (default 20) → score → keep the best
//! `top_k`. Failures degrade gracefully to the original retrieval order.

use std::sync::Arc;

use async_trait::async_trait;
use legal_rag_common::Result;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::graph::parse_llm_json;
use crate::llm::LlmClient;

const RERANK_SYSTEM: &str = "You are a relevance judge for legal retrieval. Score each passage \
0-10 for how directly it helps answer the query (10 = answers it outright, 0 = irrelevant). \
Respond with ONLY a JSON object: {\"scores\":[{\"index\":0,\"score\":8},{\"index\":1,\"score\":3}]}.";

#[derive(Deserialize)]
struct ScoreEntry {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    score: f32,
}

#[derive(Deserialize)]
struct ScoreOutput {
    #[serde(default)]
    scores: Vec<ScoreEntry>,
}

/// Ranks candidate indices best-first for a query.
#[async_trait]
pub trait Reranker: Send + Sync {
    async fn rerank(&self, query: &str, docs: &[String], top_n: usize) -> Result<Vec<usize>> {
        Ok(self
            .rerank_scored(query, docs, top_n)
            .await?
            .into_iter()
            .map(|(i, _)| i)
            .collect())
    }

    /// The same ranking with the underlying relevance scores, when the
    /// implementation has them (cross-encoder logits, LLM 0–10 scores).
    /// Scores feed the score-gap cutoff (Autocut): a ranking whose scores
    /// fall off a cliff should not be padded out to a fixed top-k.
    /// Implementations without scores return NaN per index.
    async fn rerank_scored(
        &self,
        query: &str,
        docs: &[String],
        top_n: usize,
    ) -> Result<Vec<(usize, f32)>>;
}

/// Order indices by descending score, keeping at most `top_n`.
fn order_by_scores(scores: &[f32], top_n: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    idx.sort_by(|&a, &b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    idx.truncate(top_n);
    idx
}

/// Listwise LLM reranker using the configured chat model.
pub struct LlmReranker {
    llm: Arc<dyn LlmClient>,
}

impl LlmReranker {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self { llm }
    }
}

#[async_trait]
impl Reranker for LlmReranker {
    async fn rerank_scored(
        &self,
        query: &str,
        docs: &[String],
        top_n: usize,
    ) -> Result<Vec<(usize, f32)>> {
        if docs.is_empty() || top_n == 0 {
            return Ok(Vec::new());
        }
        let passages = docs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                format!("[{i}]\n{}", d.chars().take(900).collect::<String>())
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let raw = self
            .llm
            .complete(RERANK_SYSTEM, &format!("Query: {query}\n\nPassages:\n{passages}"))
            .await?;
        let parsed: ScoreOutput = match parse_llm_json(&raw) {
            Some(p) => p,
            None => return Ok(Vec::new()), // caller falls back to original order
        };
        // Dense score vector; unscored passages sink to the end in order.
        let mut dense = vec![f32::NEG_INFINITY; docs.len()];
        for s in parsed.scores {
            if s.index < docs.len() {
                dense[s.index] = s.score;
            }
        }
        Ok(order_by_scores(&dense, top_n)
            .into_iter()
            .map(|i| (i, dense[i]))
            .collect())
    }
}

/// Local cross-encoder reranker over HTTP — the default decision layer.
///
/// Talks to a rerank service (our `tools/rerank/bge_reranker_sidecar.py`,
/// or any TEI-compatible endpoint):
///   POST {url}  {"query": "...", "documents": ["...", ...]}
///   → {"scores": [0.91, ...]}            (sidecar shape)
///   → [{"index": 0, "score": 0.91}, ...] (TEI shape)
///   → {"results": [{"index": 0, "relevance_score": 0.91}]} (Cohere shape)
/// Any transport/parse failure returns an empty ranking, and the caller
/// keeps the retrieval order — reranking must never break a query.
pub struct HttpReranker {
    client: reqwest::Client,
    url: String,
}

impl HttpReranker {
    pub fn new(url: impl Into<String>) -> Self {
        // Reranking is a precision pass, never a gate: if the service is
        // slow (cold model, loaded CPU), the caller falls back to the
        // retrieval order. Cap the wait so a query never stalls on it.
        let timeout_ms = std::env::var("LEGAL_RAG_RERANK_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(12_000);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .unwrap_or_default();
        Self { client, url: url.into() }
    }
}

#[derive(Serialize)]
struct HttpRerankRequest<'a> {
    query: &'a str,
    documents: Vec<String>,
}

/// Extract a dense score vector (length `n`) from any supported response
/// shape. `None` when the body matches none of them.
fn parse_rerank_scores(body: &str, n: usize) -> Option<Vec<f32>> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let mut dense = vec![f32::NEG_INFINITY; n];
    if let Some(scores) = v.get("scores").and_then(|s| s.as_array()) {
        if scores.len() != n {
            return None;
        }
        for (i, s) in scores.iter().enumerate() {
            dense[i] = s.as_f64()? as f32;
        }
        return Some(dense);
    }
    let entries = if let Some(arr) = v.as_array() {
        arr.clone()
    } else if let Some(arr) = v.get("results").and_then(|r| r.as_array()) {
        arr.clone()
    } else {
        return None;
    };
    let mut seen = 0usize;
    for e in &entries {
        let idx = e.get("index")?.as_u64()? as usize;
        if idx >= n {
            return None;
        }
        let score = e
            .get("score")
            .or_else(|| e.get("relevance_score"))?
            .as_f64()? as f32;
        dense[idx] = score;
        seen += 1;
    }
    if seen == 0 {
        return None;
    }
    Some(dense)
}

#[async_trait]
impl Reranker for HttpReranker {
    async fn rerank_scored(
        &self,
        query: &str,
        docs: &[String],
        top_n: usize,
    ) -> Result<Vec<(usize, f32)>> {
        if docs.is_empty() || top_n == 0 {
            return Ok(Vec::new());
        }
        let req = HttpRerankRequest {
            query,
            documents: docs
                .iter()
                .map(|d| d.chars().take(4000).collect())
                .collect(),
        };
        let resp = match self.client.post(&self.url).json(&req).send().await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "rerank service unreachable; keeping retrieval order");
                return Ok(Vec::new());
            }
        };
        let body = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                warn!(error = %e, "rerank response read failed; keeping retrieval order");
                return Ok(Vec::new());
            }
        };
        match parse_rerank_scores(&body, docs.len()) {
            Some(scores) => Ok(order_by_scores(&scores, top_n)
                .into_iter()
                .map(|i| (i, scores[i]))
                .collect()),
            None => {
                warn!("rerank response unparseable; keeping retrieval order");
                Ok(Vec::new())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlm;

    #[tokio::test]
    async fn rerank_orders_by_score() {
        let llm = Arc::new(MockLlm::new(
            r#"{"scores":[{"index":0,"score":2},{"index":1,"score":9},{"index":2,"score":5}]}"#,
        ));
        let r = LlmReranker::new(llm);
        let docs = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(r.rerank("q", &docs, 3).await.unwrap(), vec![1, 2, 0]);
        // The scored variant carries the underlying scores (Autocut input).
        let scored = r.rerank_scored("q", &docs, 3).await.unwrap();
        assert_eq!(scored[0], (1, 9.0));
        assert_eq!(scored[1], (2, 5.0));
    }

    #[tokio::test]
    async fn rerank_bad_json_falls_back_empty() {
        let llm = Arc::new(MockLlm::new("not json at all"));
        let r = LlmReranker::new(llm);
        let docs = vec!["a".to_string()];
        assert!(r.rerank("q", &docs, 1).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rerank_truncates_to_top_n() {
        let llm = Arc::new(MockLlm::new(
            r#"{"scores":[{"index":0,"score":1},{"index":1,"score":2},{"index":2,"score":3}]}"#,
        ));
        let r = LlmReranker::new(llm);
        let docs = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(r.rerank("q", &docs, 2).await.unwrap(), vec![2, 1]);
    }

    #[test]
    fn http_scores_sidecar_shape() {
        let s = parse_rerank_scores(r#"{"scores":[0.1,0.9,0.5]}"#, 3).unwrap();
        assert_eq!(order_by_scores(&s, 3), vec![1, 2, 0]);
        assert_eq!(order_by_scores(&s, 2), vec![1, 2]);
    }

    #[test]
    fn http_scores_tei_shape() {
        let s = parse_rerank_scores(
            r#"[{"index":0,"score":0.2},{"index":1,"score":0.7},{"index":2,"score":0.4}]"#,
            3,
        )
        .unwrap();
        assert_eq!(order_by_scores(&s, 3), vec![1, 2, 0]);
    }

    #[test]
    fn http_scores_results_shape() {
        let s = parse_rerank_scores(
            r#"{"results":[{"index":2,"relevance_score":0.9},{"index":0,"relevance_score":0.1}]}"#,
            3,
        )
        .unwrap();
        assert_eq!(order_by_scores(&s, 3), vec![2, 0, 1]);
    }

    #[test]
    fn http_scores_garbage_is_none() {
        assert!(parse_rerank_scores("not json", 2).is_none());
        assert!(parse_rerank_scores(r#"{"scores":[0.1]}"#, 2).is_none());
        assert!(parse_rerank_scores(r#"{"other":true}"#, 2).is_none());
    }

    #[test]
    fn late_scores_parse() {
        let s = parse_late_scores(
            r#"{"scores":[{"id":"a","score":1.5},{"id":"b","score":-0.5}]}"#,
        )
        .unwrap();
        assert_eq!(s, vec![("a".to_string(), 1.5), ("b".to_string(), -0.5)]);
        assert!(parse_late_scores("nope").is_none());
    }
}

/// Late interaction (ColBERT-style MaxSim): score a query against
/// documents at TOKEN level — each query token takes its best match
/// among the document's tokens, summed. Where a cross-encoder judges
/// the passage as a whole, MaxSim rewards exact term-level evidence
/// (section numbers, defined terms), which complements it.
#[async_trait::async_trait]
pub trait LateInteractor: Send + Sync {
    /// Returns (doc_id, maxsim_score) for the given documents.
    async fn maxsim_scores(
        &self,
        query: &str,
        docs: &[(String, String)],
    ) -> Result<Vec<(String, f32)>>;
}

/// HTTP client for the late-interaction sidecar
/// (`tools/colbert/colbert_sidecar.py`):
///   POST {url}/late-interaction {"query": "...", "documents": [{"id", "text"}]}
///   → {"scores": [{"id": "...", "score": 12.3}]}
/// Failures return Err and the caller keeps the existing order —
/// like reranking, this stage must never break a query.
pub struct HttpLateInteractor {
    client: reqwest::Client,
    url: String,
}

impl HttpLateInteractor {
    pub fn new(base_url: impl Into<String>) -> Self {
        let timeout_ms = std::env::var("LEGAL_RAG_COLBERT_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(15_000);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .unwrap_or_default();
        let base = base_url.into();
        Self {
            client,
            url: format!("{}/late-interaction", base.trim_end_matches('/')),
        }
    }
}

#[derive(Serialize)]
struct LateRequest<'a> {
    query: &'a str,
    documents: Vec<LateDoc<'a>>,
}

#[derive(Serialize)]
struct LateDoc<'a> {
    id: &'a str,
    text: &'a str,
}

fn parse_late_scores(body: &str) -> Option<Vec<(String, f32)>> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let arr = v.get("scores")?.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for e in arr {
        out.push((
            e.get("id")?.as_str()?.to_string(),
            e.get("score")?.as_f64()? as f32,
        ));
    }
    Some(out)
}

#[async_trait::async_trait]
impl LateInteractor for HttpLateInteractor {
    async fn maxsim_scores(
        &self,
        query: &str,
        docs: &[(String, String)],
    ) -> Result<Vec<(String, f32)>> {
        let body = LateRequest {
            query,
            documents: docs
                .iter()
                .map(|(id, text)| LateDoc { id: id.as_str(), text: text.as_str() })
                .collect(),
        };
        let resp = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| legal_rag_common::Error::Upstream(format!("colbert sidecar: {e}")))?;
        let text = resp
            .text()
            .await
            .map_err(|e| legal_rag_common::Error::Upstream(format!("colbert sidecar: {e}")))?;
        parse_late_scores(&text).ok_or_else(|| {
            legal_rag_common::Error::Upstream("colbert sidecar: unparseable scores".to_string())
        })
    }
}
