//! Embedding providers: OpenAI-compatible HTTP API + deterministic mock.

use async_trait::async_trait;
use legal_rag_common::{Error, Result};
use serde::{Deserialize, Serialize};

/// Converts texts to dense vectors.
#[async_trait]
pub trait Embeddings: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn dims(&self) -> usize;
}

/// OpenAI-compatible `/embeddings` endpoint (OpenAI, Azure, Ollama, vLLM...).
pub struct OpenAiEmbeddings {
    client: reqwest::Client,
    url: String,
    api_key: Option<String>,
    model: String,
    dims: usize,
}

impl OpenAiEmbeddings {
    pub fn new(url: &str, api_key: Option<&str>, model: &str, dims: usize) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.to_string(),
            api_key: api_key.map(|s| s.to_string()),
            model: model.to_string(),
            dims,
        }
    }
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedDatum>,
    usage: Option<EmbedUsage>,
}

#[derive(Deserialize, Clone, Copy)]
struct EmbedUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct EmbedDatum {
    embedding: Vec<f32>,
}

#[async_trait]
impl Embeddings for OpenAiEmbeddings {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let est_total: u64 = texts.iter().map(|t| crate::usage::estimate_tokens(t)).sum();
        crate::llm::retry_upstream(|| async {
            let mut req = self.client.post(&self.url).json(&EmbedRequest {
                model: &self.model,
                input: texts,
            });
            if let Some(key) = &self.api_key {
                req = req.bearer_auth(key);
            }
            let resp: EmbedResponse = req
                .send()
                .await
                .map_err(|e| Error::Upstream(format!("embedding request failed: {e}")))?
                .error_for_status()
                .map_err(|e| Error::Upstream(format!("embedding request failed: {e}")))?
                .json()
                .await
                .map_err(|e| Error::Upstream(format!("bad embedding response: {e}")))?;
            let reported = resp
                .usage
                .and_then(|u| u.prompt_tokens.or(u.total_tokens));
            crate::usage::record_embeddings(reported.unwrap_or(est_total));
            Ok(resp.data.into_iter().map(|d| d.embedding).collect())
        })
        .await
    }

    fn dims(&self) -> usize {
        self.dims
    }
}

/// Deterministic mock embeddings for tests and offline development.
pub struct MockEmbeddings {
    dims: usize,
}

impl MockEmbeddings {
    pub fn new(dims: usize) -> Self {
        Self { dims }
    }

    fn vector_for(&self, text: &str) -> Vec<f32> {
        // FNV-1a seeded xorshift: deterministic per text.
        let mut h: u64 = 0xcbf29ce484222325;
        for b in text.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        let mut x = h.max(1);
        let mut v = Vec::with_capacity(self.dims);
        for _ in 0..self.dims {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            v.push((x as f64 / u64::MAX as f64) as f32 * 2.0 - 1.0);
        }
        // L2 normalize so cosine == dot product.
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        v.iter().map(|x| x / norm).collect()
    }
}

#[async_trait]
impl Embeddings for MockEmbeddings {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        crate::usage::record_embeddings(
            texts.iter().map(|t| crate::usage::estimate_tokens(t)).sum(),
        );
        Ok(texts.iter().map(|t| self.vector_for(t)).collect())
    }

    fn dims(&self) -> usize {
        self.dims
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_embeddings_are_deterministic_and_normalized() {
        let e = MockEmbeddings::new(16);
        let a = e.embed(&["indemnification clause".to_string()]).await.unwrap();
        let b = e.embed(&["indemnification clause".to_string()]).await.unwrap();
        assert_eq!(a, b);
        let norm: f32 = a[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }
}
