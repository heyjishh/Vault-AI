//! Application-level semantic cache — our answer to "KV cache" when the
//! models live behind APIs.
//!
//! True KV-cache lives inside the model server; we cannot touch it through
//! Cloudflare/Mistral/OpenAI HTTP APIs. What we *can* do, and what this
//! module does:
//!
//! 1. **Extraction cache** — graph extraction is one LLM call per chunk with
//!    temperature ~0. The chunk text is the cache key (SHA-256), so
//!    re-ingesting a document (or ingesting overlapping corpora, extremely
//!    common with legal boilerplate) costs zero LLM calls.
//! 2. **Embedding cache** — same idea for embeddings; headers, footers and
//!    standard clauses repeat across legal documents constantly.
//! 3. **Answer cache** — the query answer prompt already embeds the retrieved
//!    context, so (system, user) hashing is corpus-sensitive for free: if the
//!    corpus changes, the prompt changes, the key changes.
//! 4. **Stable prompt prefixes** — every prompt in this crate puts the static
//!    system instruction first, which maximizes hit rates on providers that
//!    do server-side prefix (KV) caching.
//!
//! All caches are bounded in-memory LRU-ish maps with hit/miss stats.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use legal_rag_common::{Error, Result};
use sha2::{Digest, Sha256};

use crate::embeddings::Embeddings;
use crate::llm::LlmClient;

/// SHA-256 hex of the joined parts; the canonical cache key.
pub fn hash_key(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"\x00");
    }
    format!("{:x}", h.finalize())
}

struct Inner {
    map: HashMap<String, Vec<u8>>,
    order: VecDeque<String>,
}

pub struct SemanticCache {
    inner: Mutex<Inner>,
    capacity: usize,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl SemanticCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner { map: HashMap::new(), order: VecDeque::new() }),
            capacity: capacity.max(16),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let inner = self.inner.lock().ok()?;
        let v = inner.map.get(key).cloned();
        if v.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
        v
    }

    pub fn put(&self, key: String, value: Vec<u8>) {
        let Ok(mut inner) = self.inner.lock() else { return };
        if inner.map.contains_key(&key) {
            inner.map.insert(key, value);
            return;
        }
        while inner.map.len() >= self.capacity {
            if let Some(old) = inner.order.pop_front() {
                inner.map.remove(&old);
            } else {
                break;
            }
        }
        inner.order.push_back(key.clone());
        inner.map.insert(key, value);
    }

    pub fn remove(&self, key: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.map.remove(key);
            inner.order.retain(|k| k != key);
        }
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            entries: self.inner.lock().map(|i| i.map.len()).unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
}

/// LLM decorator that caches `complete` responses by (system, user) hash.
pub struct CachedLlm {
    inner: Arc<dyn LlmClient>,
    cache: Arc<SemanticCache>,
}

impl CachedLlm {
    pub fn new(inner: Arc<dyn LlmClient>, cache: Arc<SemanticCache>) -> Self {
        Self { inner, cache }
    }

    pub fn stats(&self) -> CacheStats {
        self.cache.stats()
    }
}

impl CachedLlm {
    /// Decode a cached completion. A stored provider failure page
    /// (see `crate::llm::is_failure_output`) is PURGED and reported
    /// as a miss: poison written before the guard existed — or by an
    /// unguarded inner client — must never be served a second time.
    fn cached_completion(&self, key: &str) -> Option<Result<String>> {
        let hit = self.cache.get(key)?;
        match String::from_utf8(hit) {
            Ok(text) if crate::llm::is_failure_output(&text) => {
                self.cache.remove(key);
                None
            }
            Ok(text) => Some(Ok(text)),
            Err(e) => Some(Err(Error::Storage(e.to_string()))),
        }
    }

    /// Store a completion — unless it is a failure page. A transient
    /// provider failure must never become a permanent fake answer:
    /// the next identical request has to reach the provider again.
    fn store_completion(&self, key: &str, out: &str) {
        if !crate::llm::is_failure_output(out) {
            self.cache.put(key.to_string(), out.as_bytes().to_vec());
        }
    }
}

#[async_trait]
impl LlmClient for CachedLlm {
    async fn complete(&self, system: &str, user: &str) -> Result<String> {
        let key = format!("llm:{}", hash_key(&[system, user]));
        if let Some(hit) = self.cached_completion(&key) {
            return hit;
        }
        let out = self.inner.complete(system, user).await?;
        self.store_completion(&key, &out);
        Ok(out)
    }

    /// Streaming pass-through: cache hits replay in word chunks (the answer
    /// is already local, so it renders instantly); misses stream live from
    /// the provider and populate the cache with the assembled text.
    async fn complete_stream(
        &self,
        system: &str,
        user: &str,
        sink: &mut crate::llm::ChunkSink<'_>,
    ) -> Result<String> {
        let key = format!("llm:{}", hash_key(&[system, user]));
        if let Some(hit) = self.cached_completion(&key) {
            let text = hit?;
            for chunk in text.split_inclusive(' ') {
                if !chunk.is_empty() {
                    sink(chunk);
                }
            }
            return Ok(text);
        }
        let out = self.inner.complete_stream(system, user, sink).await?;
        self.store_completion(&key, &out);
        Ok(out)
    }
}

/// Embeddings decorator that caches per-text vectors. Misses are batched
/// into a single upstream call.
pub struct CachedEmbeddings {
    inner: Arc<dyn Embeddings>,
    cache: Arc<SemanticCache>,
}

impl CachedEmbeddings {
    pub fn new(inner: Arc<dyn Embeddings>, cache: Arc<SemanticCache>) -> Self {
        Self { inner, cache }
    }

    pub fn stats(&self) -> CacheStats {
        self.cache.stats()
    }
}

#[async_trait]
impl Embeddings for CachedEmbeddings {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<String> =
            texts.iter().map(|t| format!("emb:{}", hash_key(&[t]))).collect();
        let mut out: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
        let mut miss_idx = Vec::new();
        let mut miss_texts = Vec::new();
        for (i, (key, text)) in keys.iter().zip(texts.iter()).enumerate() {
            if let Some(hit) = self.cache.get(key) {
                let v: Vec<f32> = serde_json::from_slice(&hit)
                    .map_err(|e| Error::Storage(format!("bad cached embedding: {e}")))?;
                out[i] = Some(v);
            } else {
                miss_idx.push(i);
                miss_texts.push(text.clone());
            }
        }
        if !miss_texts.is_empty() {
            let fresh = self.inner.embed(&miss_texts).await?;
            for (pos, vec) in miss_idx.into_iter().zip(fresh.into_iter()) {
                let bytes = serde_json::to_vec(&vec)
                    .map_err(|e| Error::Storage(format!("cache encode: {e}")))?;
                self.cache.put(keys[pos].clone(), bytes);
                out[pos] = Some(vec);
            }
        }
        Ok(out.into_iter().map(|o| o.expect("embedding cache filled")).collect())
    }

    fn dims(&self) -> usize {
        self.inner.dims()
    }
}

/// Semantic answer cache: whole grounded answers indexed by the
/// *question's embedding*. Where [`CachedLlm`] only hits on an identical
/// (system, prompt) pair — which requires identical retrieval — this cache
/// hits on a *paraphrase*: if a new question's embedding lands within the
/// similarity threshold of a stored question asked under the same scope
/// (mode, top_k, brief, corpus fingerprint), the stored answer is served
/// at zero LLM cost.
///
/// Safety rails, by construction:
/// - The scope key embeds the corpus fingerprint, so any ingest or delete
///   makes every prior entry unreachable (belt: entries also age out FIFO).
/// - Callers never store or look up answers for web-augmented or private
///   (confidential-vault) queries; only answers with ≥1 citation are
///   stored, so refusals and ungrounded text never enter the cache.
/// - Provider failure pages are refused at `store` and purged at
///   `lookup` (Piece 14): a transient generation failure can never
///   enter, or be served from, this cache as though it were an answer.
/// - The threshold is deliberately high (default 0.97): this trades hit
///   rate for near-zero false-serve risk on fact-sensitive questions.
pub struct SemanticAnswerCache {
    entries: Mutex<VecDeque<SemanticAnswerEntry>>,
    capacity: usize,
    threshold: f32,
    hits: AtomicU64,
    misses: AtomicU64,
}

struct SemanticAnswerEntry {
    qvec: Vec<f32>,
    scope_key: String,
    answer: crate::query::QueryAnswer,
}

impl SemanticAnswerCache {
    pub fn new(capacity: usize, threshold: f32) -> Self {
        Self {
            entries: Mutex::new(VecDeque::new()),
            capacity: capacity.max(16),
            threshold: threshold.clamp(0.80, 0.999),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// Best-matching stored answer for this question vector under this
    /// scope key, if its cosine clears the threshold.
    pub fn lookup(&self, qvec: &[f32], scope_key: &str) -> Option<crate::query::QueryAnswer> {
        let mut entries = self.entries.lock().ok()?;
        // Purge-on-read: a stored provider failure page (Piece 14) is
        // not an answer — drop any such entry before matching, so it
        // can never be served, even if it was written by a code path
        // that predates the store-side refusal below.
        entries.retain(|e| !crate::llm::is_failure_output(&e.answer.answer));
        let mut best: Option<(f32, &SemanticAnswerEntry)> = None;
        for e in entries.iter() {
            if e.scope_key != scope_key || e.qvec.len() != qvec.len() {
                continue;
            }
            let sim = crate::vector_store::cosine(qvec, &e.qvec);
            if sim >= self.threshold && best.is_none_or(|(s, _)| sim > s) {
                best = Some((sim, e));
            }
        }
        match best {
            Some((_, e)) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(e.answer.clone())
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    pub fn store(&self, qvec: Vec<f32>, scope_key: String, answer: crate::query::QueryAnswer) {
        // A provider failure page is never an answer (Piece 14):
        // refuse the write, whatever the caller believed it generated.
        if crate::llm::is_failure_output(&answer.answer) {
            return;
        }
        let Ok(mut entries) = self.entries.lock() else { return };
        // Same question under the same scope: replace, don't duplicate.
        entries.retain(|e| !(e.scope_key == scope_key && crate::vector_store::cosine(&e.qvec, &qvec) >= 0.999));
        while entries.len() >= self.capacity {
            entries.pop_front();
        }
        entries.push_back(SemanticAnswerEntry { qvec, scope_key, answer });
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            entries: self.entries.lock().map(|e| e.len()).unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlm;

    #[tokio::test]
    async fn llm_cache_hit_avoids_second_call() {
        let inner = Arc::new(MockLlm::new("resp"));
        let cached = CachedLlm::new(inner, Arc::new(SemanticCache::new(16)));
        let a = cached.complete("sys", "user").await.unwrap();
        let b = cached.complete("sys", "user").await.unwrap();
        assert_eq!((a, b), ("resp".to_string(), "resp".to_string()));
        let s = cached.stats();
        assert_eq!((s.hits, s.misses), (1, 1));
    }

    #[tokio::test]
    async fn embeddings_cache_batches_misses() {
        use crate::embeddings::MockEmbeddings;
        let inner = Arc::new(MockEmbeddings::new(8));
        let cached = CachedEmbeddings::new(inner, Arc::new(SemanticCache::new(16)));
        let texts = vec!["a".to_string(), "b".to_string()];
        let v1 = cached.embed(&texts).await.unwrap();
        let v2 = cached.embed(&texts).await.unwrap();
        assert_eq!(v1, v2);
        assert_eq!(cached.stats().hits, 2);
    }

    #[test]
    fn cache_evicts_oldest_over_capacity() {
        // Note: SemanticCache floors capacity at 16.
        let c = SemanticCache::new(16);
        for i in 0..17 {
            c.put(format!("k{i}"), b"v".to_vec());
        }
        assert!(c.get("k0").is_none());
        assert!(c.get("k16").is_some());
        assert_eq!(c.stats().entries, 16);
    }

    /// The failure page the Gemini web bridge returns as an HTTP 200
    /// completion (the :3001 poison, 2026-10-29).
    const FAILURE_PAGE: &str = "Sorry, something went wrong. Please try your request again.";

    /// LLM that plays a fixed script of `Ok` responses and counts how
    /// often it was actually called — a cache serve never increments.
    struct ScriptedLlm {
        script: std::sync::Mutex<VecDeque<String>>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl ScriptedLlm {
        fn new(script: Vec<&str>) -> Self {
            Self {
                script: std::sync::Mutex::new(
                    script.into_iter().map(str::to_string).collect(),
                ),
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl LlmClient for ScriptedLlm {
        async fn complete(&self, _system: &str, _user: &str) -> Result<String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted llm ran out of responses"))
        }
    }

    #[tokio::test]
    async fn llm_failure_output_is_never_cached_and_retry_reaches_provider() {
        // An unguarded provider delivers the failure page as Ok(..)
        // once, then recovers. The failure must not be written to the
        // cache: the identical retry goes back to the provider.
        let inner = Arc::new(ScriptedLlm::new(vec![FAILURE_PAGE, "real answer"]));
        let cached = CachedLlm::new(inner.clone(), Arc::new(SemanticCache::new(16)));
        let first = cached.complete("sys", "user").await.unwrap();
        assert_eq!(first, FAILURE_PAGE, "unguarded text passes through once");
        let second = cached.complete("sys", "user").await.unwrap();
        assert_eq!(second, "real answer");
        assert_eq!(inner.calls(), 2, "retry reached the provider, not the cache");
        // A successful answer still caches normally.
        let third = cached.complete("sys", "user").await.unwrap();
        assert_eq!(third, "real answer");
        assert_eq!(inner.calls(), 2, "success serves from cache");
    }

    #[tokio::test]
    async fn llm_error_is_never_cached_and_retry_reaches_provider() {
        struct FlakyLlm {
            calls: std::sync::atomic::AtomicUsize,
        }
        #[async_trait]
        impl LlmClient for FlakyLlm {
            async fn complete(&self, _system: &str, _user: &str) -> Result<String> {
                let n = self.calls.fetch_add(1, Ordering::Relaxed);
                if n == 0 {
                    Err(Error::Upstream("transient boom".into()))
                } else {
                    Ok("recovered".to_string())
                }
            }
        }
        let inner = Arc::new(FlakyLlm { calls: std::sync::atomic::AtomicUsize::new(0) });
        let cached = CachedLlm::new(inner.clone(), Arc::new(SemanticCache::new(16)));
        assert!(cached.complete("sys", "user").await.is_err());
        assert_eq!(cached.complete("sys", "user").await.unwrap(), "recovered");
        assert_eq!(inner.calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn llm_cache_purges_a_stored_failure_on_read() {
        // Poison written before the guard existed: it sits in the
        // cache under the request's key. The next identical request
        // must purge it and reach the provider instead of serving it.
        let cache = Arc::new(SemanticCache::new(16));
        let key = format!("llm:{}", hash_key(&["sys", "user"]));
        cache.put(key.clone(), FAILURE_PAGE.as_bytes().to_vec());
        let inner = Arc::new(ScriptedLlm::new(vec!["real answer"]));
        let cached = CachedLlm::new(inner.clone(), cache.clone());
        let out = cached.complete("sys", "user").await.unwrap();
        assert_eq!(out, "real answer");
        assert_eq!(inner.calls(), 1, "provider was consulted, poison not served");
        let stored = String::from_utf8(cache.get(&key).expect("entry present")).unwrap();
        assert_eq!(stored, "real answer", "poison replaced by the real completion");
    }

    #[tokio::test]
    async fn llm_stream_failure_output_is_never_cached() {
        let inner = Arc::new(ScriptedLlm::new(vec![FAILURE_PAGE, "streamed answer"]));
        let cached = CachedLlm::new(inner.clone(), Arc::new(SemanticCache::new(16)));
        let mut sink = |_chunk: &str| {};
        let first = cached.complete_stream("sys", "user", &mut sink).await.unwrap();
        assert_eq!(first, FAILURE_PAGE);
        let second = cached.complete_stream("sys", "user", &mut sink).await.unwrap();
        assert_eq!(second, "streamed answer");
        assert_eq!(inner.calls(), 2, "streamed retry reached the provider");
        let third = cached.complete_stream("sys", "user", &mut sink).await.unwrap();
        assert_eq!(third, "streamed answer");
        assert_eq!(inner.calls(), 2, "streamed success serves from cache");
    }

    fn qa(answer: &str) -> crate::query::QueryAnswer {
        crate::query::QueryAnswer {
            answer: answer.to_string(),
            citations: Vec::new(),
            mode: crate::query::QueryMode::Mix,
            scope_note: None,
            verified: false,
            citation_audit: None,
            expansions: Vec::new(),
        }
    }

    #[test]
    fn answer_cache_refuses_failure_answers_but_keeps_real_ones() {
        let cache = SemanticAnswerCache::new(16, 0.97);
        let qvec = vec![1.0, 0.0];
        cache.store(qvec.clone(), "scope".to_string(), qa(FAILURE_PAGE));
        assert!(
            cache.lookup(&qvec, "scope").is_none(),
            "a failure page must never enter the answer cache"
        );
        assert_eq!(cache.stats().entries, 0);
        cache.store(qvec.clone(), "scope".to_string(), qa("grounded answer"));
        let hit = cache.lookup(&qvec, "scope").expect("real answer serves");
        assert_eq!(hit.answer, "grounded answer");
    }

    #[test]
    fn answer_cache_purges_poisoned_entries_on_lookup() {
        // Poison inserted directly (as a pre-guard build would have
        // stored it): lookup must not serve it, and must evict it.
        let cache = SemanticAnswerCache::new(16, 0.97);
        cache.entries.lock().unwrap().push_back(SemanticAnswerEntry {
            qvec: vec![1.0, 0.0],
            scope_key: "scope".to_string(),
            answer: qa(FAILURE_PAGE),
        });
        assert!(cache.lookup(&[1.0, 0.0], "scope").is_none());
        assert_eq!(cache.stats().entries, 0, "poisoned entry evicted on read");
    }
}
