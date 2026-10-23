//! Retrieval-tier results cache — the Spice.ai caching pattern,
//! rewritten natively: Spice checks a results cache *before query
//! planning* and serves stale entries while a single background
//! revalidation refreshes them (stale-while-revalidate). Here the cached
//! unit is one `retrieve()` result (ranked hits + graph context), keyed
//! by question, mode, fetch size, brief scope and the corpus
//! fingerprint — a corpus change changes the fingerprint, so stale law
//! is never served from an old corpus state.
//!
//! The semantic answer cache sits one tier above (it replays whole
//! answers); this tier replays the retrieval underneath answers whose
//! surface form differs (streaming vs blocking, different top_k at the
//! generation step, research/agent re-retrieval of the same question).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use crate::vector_store::ScoredChunk;

pub type RetrievalValue = (Vec<ScoredChunk>, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    /// Inside the fresh TTL — serve as-is.
    Fresh,
    /// Past fresh but inside the SWR window — serve now, refresh in
    /// the background.
    Stale,
    Miss,
}

struct Entry {
    stored_at: Instant,
    value: RetrievalValue,
}

pub struct RetrievalCache {
    entries: RwLock<HashMap<String, Entry>>,
    fresh_ttl: Duration,
    swr_ttl: Duration,
    capacity: usize,
    fresh_hits: AtomicU64,
    stale_hits: AtomicU64,
    misses: AtomicU64,
    revalidations: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RetrievalCacheStats {
    pub fresh_hits: u64,
    pub stale_hits: u64,
    pub misses: u64,
    pub revalidations: u64,
    pub entries: usize,
}

impl RetrievalCache {
    pub fn new(fresh_ttl: Duration, swr_ttl: Duration, capacity: usize) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            fresh_ttl,
            swr_ttl: swr_ttl.max(fresh_ttl),
            capacity: capacity.max(1),
            fresh_hits: AtomicU64::new(0),
            stale_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            revalidations: AtomicU64::new(0),
        }
    }

    /// From the environment: `LEGAL_RAG_RETRIEVAL_CACHE_TTL_SECS`
    /// (fresh, default 120), `LEGAL_RAG_RETRIEVAL_CACHE_SWR_SECS`
    /// (default 600), `LEGAL_RAG_RETRIEVAL_CACHE_MAX` (default 512).
    pub fn from_env() -> Self {
        let secs = |key: &str, default: u64| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(default)
        };
        Self::new(
            Duration::from_secs(secs("LEGAL_RAG_RETRIEVAL_CACHE_TTL_SECS", 120)),
            Duration::from_secs(secs("LEGAL_RAG_RETRIEVAL_CACHE_SWR_SECS", 600)),
            std::env::var("LEGAL_RAG_RETRIEVAL_CACHE_MAX")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(512),
        )
    }

    pub fn lookup(&self, key: &str) -> (Lookup, Option<RetrievalValue>) {
        let entries = self.entries.read().expect("lock poisoned");
        let Some(entry) = entries.get(key) else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return (Lookup::Miss, None);
        };
        let age = entry.stored_at.elapsed();
        if age <= self.fresh_ttl {
            self.fresh_hits.fetch_add(1, Ordering::Relaxed);
            (Lookup::Fresh, Some(entry.value.clone()))
        } else if age <= self.swr_ttl {
            self.stale_hits.fetch_add(1, Ordering::Relaxed);
            (Lookup::Stale, Some(entry.value.clone()))
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            (Lookup::Miss, None)
        }
    }

    pub fn store(&self, key: String, value: RetrievalValue) {
        let mut entries = self.entries.write().expect("lock poisoned");
        if !entries.contains_key(&key) && entries.len() >= self.capacity {
            // Evict the single oldest entry (bounded, tiny map).
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, e)| e.stored_at)
                .map(|(k, _)| k.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(key, Entry { stored_at: Instant::now(), value });
    }

    pub fn note_revalidation(&self) {
        self.revalidations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn stats(&self) -> RetrievalCacheStats {
        RetrievalCacheStats {
            fresh_hits: self.fresh_hits.load(Ordering::Relaxed),
            stale_hits: self.stale_hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            revalidations: self.revalidations.load(Ordering::Relaxed),
            entries: self.entries.read().expect("lock poisoned").len(),
        }
    }
}

/// Stable cache key for one retrieval: normalized question, mode,
/// fetch size, sorted scope doc ids, corpus fingerprint.
pub fn retrieval_key(
    question: &str,
    mode: &str,
    top_k: usize,
    scope_docs: Option<&[String]>,
    corpus_fp: &str,
) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    question.trim().to_lowercase().hash(&mut h);
    mode.hash(&mut h);
    top_k.hash(&mut h);
    corpus_fp.hash(&mut h);
    match scope_docs {
        Some(docs) => {
            let mut sorted: Vec<&String> = docs.iter().collect();
            sorted.sort();
            for d in sorted {
                d.hash(&mut h);
            }
        }
        None => "unscoped".hash(&mut h),
    }
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector_store::ChunkRecord;

    fn value() -> RetrievalValue {
        (
            vec![ScoredChunk {
                record: ChunkRecord {
                    id: "c1".into(),
                    doc_id: "d1".into(),
                    ordinal: 0,
                    text: "text".into(),
                    embedding: vec![],
                    metadata: Default::default(),
                },
                score: 0.9,
            }],
            "ctx".to_string(),
        )
    }

    #[test]
    fn fresh_stale_miss_windows() {
        let cache = RetrievalCache::new(Duration::from_millis(60), Duration::from_millis(200), 4);
        assert_eq!(cache.lookup("k").0, Lookup::Miss);
        cache.store("k".into(), value());
        assert_eq!(cache.lookup("k").0, Lookup::Fresh);
        std::thread::sleep(Duration::from_millis(90));
        assert_eq!(cache.lookup("k").0, Lookup::Stale);
        std::thread::sleep(Duration::from_millis(160));
        assert_eq!(cache.lookup("k").0, Lookup::Miss);
        let s = cache.stats();
        assert_eq!(s.fresh_hits, 1);
        assert_eq!(s.stale_hits, 1);
        assert_eq!(s.misses, 2);
    }

    #[test]
    fn key_distinguishes_scope_mode_and_corpus() {
        let a = retrieval_key("What is bail?", "mix", 20, None, "fp1");
        let b = retrieval_key("  what is bail? ", "mix", 20, None, "fp1");
        assert_eq!(a, b, "normalization makes surface variants collide");
        assert_ne!(a, retrieval_key("What is bail?", "mix", 20, None, "fp2"));
        assert_ne!(
            a,
            retrieval_key("What is bail?", "mix", 20, Some(&["d1".to_string()]), "fp1")
        );
        assert_ne!(a, retrieval_key("What is bail?", "local", 20, None, "fp1"));
    }

    #[test]
    fn capacity_evicts_oldest() {
        let cache = RetrievalCache::new(Duration::from_secs(60), Duration::from_secs(120), 2);
        cache.store("a".into(), value());
        std::thread::sleep(Duration::from_millis(5));
        cache.store("b".into(), value());
        cache.store("c".into(), value());
        assert_eq!(cache.stats().entries, 2);
        assert_eq!(cache.lookup("a").0, Lookup::Miss);
        assert_eq!(cache.lookup("c").0, Lookup::Fresh);
    }
}
