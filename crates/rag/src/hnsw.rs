//! In-process HNSW vector store — the DuckDB-VSS-style engine from
//! Spice.ai's vector-engine lineup, rewritten natively (ADR-0020).
//!
//! Spice offers three vector engines: `s3_vectors` (external, see
//! [`crate::s3vectors`]), `elasticsearch` (Enterprise-only), and
//! `duckdb` — an embedded HNSW index living in the accelerator file,
//! tuned by `m` / `ef_construction` / `ef_search`. This store takes
//! that design (single-process HNSW, cosine, the same default
//! parameters) without the DuckDB dependency: a pure-Rust HNSW graph
//! behind the same [`VectorStore`] trait as the in-memory and
//! SereneDB backends.
//!
//! Records persist to a JSON file (atomic tmp+rename on mutation) and
//! the graph is rebuilt from them at open — the same durability model
//! as the in-memory store's snapshots. Updates and deletions unlink
//! the node from its (bidirectional) neighbor lists, so mutations
//! never trigger a rebuild.
//!
//! Tiny graphs stay exact: `search` falls back to a brute-force scan
//! whenever the graph holds fewer nodes than the pool the caller
//! needs to see ranked, so small corpora behave identically to the
//! in-memory store.

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::RwLock;

use async_trait::async_trait;
use legal_rag_common::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::vector_store::{cosine, ChunkRecord, ScoredChunk, VectorStore};

/// HNSW tuning, mirroring Spice's DuckDB vector-engine parameters
/// (`duckdb_hnsw_m`, `duckdb_hnsw_ef_construction`,
/// `duckdb_hnsw_ef_search`).
#[derive(Debug, Clone, Copy)]
pub struct HnswParams {
    pub m: usize,
    pub ef_construction: usize,
    pub ef_search: usize,
}

impl Default for HnswParams {
    fn default() -> Self {
        Self { m: 16, ef_construction: 128, ef_search: 64 }
    }
}

impl HnswParams {
    pub fn from_env() -> Self {
        let mut p = Self::default();
        for (var, slot) in [
            ("LEGAL_RAG_HNSW_M", &mut p.m),
            ("LEGAL_RAG_HNSW_EF_CONSTRUCTION", &mut p.ef_construction),
            ("LEGAL_RAG_HNSW_EF_SEARCH", &mut p.ef_search),
        ] {
            if let Ok(v) = std::env::var(var) {
                if let Ok(n) = v.parse() {
                    *slot = n;
                }
            }
        }
        p.m = p.m.clamp(2, 64);
        p.ef_construction = p.ef_construction.clamp(4, 1024);
        p.ef_search = p.ef_search.clamp(1, 1024);
        p
    }

    fn max_conn(&self, layer: usize) -> usize {
        if layer == 0 {
            self.m * 2
        } else {
            self.m
        }
    }
}

/// f32 score with a total order, for heap use.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Score(f32);
impl Eq for Score {}
impl PartialOrd for Score {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Score {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[derive(Debug, Default)]
struct Node {
    /// Neighbor ids per layer (index = layer).
    layers: Vec<Vec<String>>,
}

#[derive(Debug)]
struct Inner {
    records: HashMap<String, ChunkRecord>,
    /// L2-normalized embeddings (cosine = dot product).
    vecs: HashMap<String, Vec<f32>>,
    nodes: HashMap<String, Node>,
    entry: Option<String>,
    max_level: usize,
    rng: u64,
}

impl Inner {
    fn new() -> Self {
        Self {
            records: HashMap::new(),
            vecs: HashMap::new(),
            nodes: HashMap::new(),
            entry: None,
            max_level: 0,
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }

    fn next_uniform(&mut self) -> f32 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        let v = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        // (0, 1]
        1.0 - ((v >> 40) as f32 / (1u64 << 24) as f32)
    }

    fn random_level(&mut self, ml: f32) -> usize {
        let u = self.next_uniform().max(1e-9);
        (-u.ln() * ml).floor() as usize
    }

    fn sim(&self, q: &[f32], id: &str) -> f32 {
        match self.vecs.get(id) {
            Some(v) => dot(q, v),
            None => f32::NEG_INFINITY,
        }
    }

    /// Beam search within one layer; returns up to `ef` (id, score)
    /// pairs, best first.
    fn search_layer(
        &self,
        q: &[f32],
        entry_points: &[String],
        ef: usize,
        layer: usize,
    ) -> Vec<(String, f32)> {
        let mut visited: HashSet<String> = HashSet::new();
        // Candidates: max-heap by score (expand best first).
        let mut candidates: BinaryHeap<(Score, String)> = BinaryHeap::new();
        // Results: min-heap by score via Reverse (worst on top).
        let mut results: BinaryHeap<std::cmp::Reverse<(Score, String)>> = BinaryHeap::new();
        for ep in entry_points {
            if visited.insert(ep.clone()) {
                let s = self.sim(q, ep);
                candidates.push((Score(s), ep.clone()));
                results.push(std::cmp::Reverse((Score(s), ep.clone())));
            }
        }
        while let Some((Score(cs), cid)) = candidates.pop() {
            if let Some(worst) = results.peek().map(|rev| rev.0 .0 .0) {
                if cs < worst && results.len() >= ef {
                    break;
                }
            }
            let neighbors: Vec<String> = self
                .nodes
                .get(&cid)
                .and_then(|n| n.layers.get(layer))
                .cloned()
                .unwrap_or_default();
            for nid in neighbors {
                if !visited.insert(nid.clone()) {
                    continue;
                }
                let s = self.sim(q, &nid);
                let worst = results.peek().map(|rev| rev.0 .0 .0);
                if results.len() < ef || worst.map(|w| s > w).unwrap_or(true) {
                    candidates.push((Score(s), nid.clone()));
                    results.push(std::cmp::Reverse((Score(s), nid)));
                    if results.len() > ef {
                        results.pop();
                    }
                }
            }
        }
        let mut out: Vec<(String, f32)> =
            results.into_iter().map(|rev| (rev.0 .1, rev.0 .0 .0)).collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }

    /// Remove a node and every (bidirectional) edge pointing at it.
    fn unlink(&mut self, id: &str) {
        let Some(node) = self.nodes.remove(id) else {
            return;
        };
        for (layer, neighbors) in node.layers.iter().enumerate() {
            for nid in neighbors {
                if let Some(n) = self.nodes.get_mut(nid) {
                    if let Some(list) = n.layers.get_mut(layer) {
                        list.retain(|x| x != id);
                    }
                }
            }
        }
        if self.entry.as_deref() == Some(id) {
            let mut best: Option<(usize, String)> = None;
            for (nid, n) in self.nodes.iter() {
                let lvl = n.layers.len().saturating_sub(1);
                if best.as_ref().map(|(bl, _)| lvl > *bl).unwrap_or(true) {
                    best = Some((lvl, nid.clone()));
                }
            }
            match best {
                Some((lvl, nid)) => {
                    self.max_level = lvl;
                    self.entry = Some(nid);
                }
                None => {
                    self.max_level = 0;
                    self.entry = None;
                }
            }
        }
    }

    fn insert(&mut self, params: &HnswParams, id: &str, q: &[f32]) {
        if self.nodes.contains_key(id) {
            self.unlink(id);
        }
        let ml = 1.0 / (params.m as f32).ln();
        let level = self.random_level(ml);
        self.nodes.insert(id.to_string(), Node { layers: vec![Vec::new(); level + 1] });
        self.vecs.insert(id.to_string(), q.to_vec());

        let Some(mut ep) = self.entry.clone() else {
            self.entry = Some(id.to_string());
            self.max_level = level;
            return;
        };
        let cur_max = self.max_level;

        // Greedy descent through layers above the node's own level.
        let mut layer = cur_max;
        while layer > level {
            let res = self.search_layer(q, std::slice::from_ref(&ep), 1, layer);
            if let Some((best, _)) = res.first() {
                ep = best.clone();
            }
            layer -= 1;
        }
        // Connect at layers min(level, cur_max)..=0.
        let mut l = level.min(cur_max) + 1;
        while l > 0 {
            l -= 1;
            let found =
                self.search_layer(q, std::slice::from_ref(&ep), params.ef_construction, l);
            if let Some((best, _)) = found.first() {
                ep = best.clone();
            }
            let max_conn = params.max_conn(l);
            let neighbors: Vec<String> =
                found.into_iter().take(max_conn).map(|(nid, _)| nid).collect();
            if let Some(n) = self.nodes.get_mut(id) {
                n.layers[l] = neighbors.clone();
            }
            // Back edges, pruning each neighbor's list to its closest.
            for nid in neighbors {
                let Some(nvec) = self.vecs.get(&nid).cloned() else {
                    continue;
                };
                let mut list = self
                    .nodes
                    .get(&nid)
                    .and_then(|n| n.layers.get(l))
                    .cloned()
                    .unwrap_or_default();
                if !list.iter().any(|x| x == id) {
                    list.push(id.to_string());
                }
                if list.len() > max_conn {
                    let mut scored: Vec<(String, f32)> = list
                        .into_iter()
                        .map(|x| {
                            let s = self.vecs.get(&x).map(|v| dot(&nvec, v)).unwrap_or(f32::NEG_INFINITY);
                            (x, s)
                        })
                        .collect();
                    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
                    scored.truncate(max_conn);
                    list = scored.into_iter().map(|(x, _)| x).collect();
                }
                if let Some(n) = self.nodes.get_mut(&nid) {
                    if n.layers.len() <= l {
                        n.layers.resize_with(l + 1, Vec::new);
                    }
                    n.layers[l] = list;
                }
            }
        }
        if level > cur_max {
            self.entry = Some(id.to_string());
            self.max_level = level;
        }
    }

    /// Approximate top-`top_k` (id, score) via the graph.
    fn search_ann(&self, params: &HnswParams, query: &[f32], top_k: usize) -> Vec<(String, f32)> {
        let q = normalize(query);
        let Some(mut ep) = self.entry.clone() else {
            return Vec::new();
        };
        let mut layer = self.max_level;
        while layer > 0 {
            let res = self.search_layer(&q, std::slice::from_ref(&ep), 1, layer);
            if let Some((best, _)) = res.first() {
                ep = best.clone();
            }
            layer -= 1;
        }
        let ef = params.ef_search.max(top_k);
        self.search_layer(&q, &[ep], ef, 0).into_iter().take(top_k).collect()
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

fn normalize(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n == 0.0 {
        return v.to_vec();
    }
    v.iter().map(|x| x / n).collect()
}

#[derive(Serialize, Deserialize)]
struct PersistFile {
    records: Vec<ChunkRecord>,
}

/// HNSW-backed vector store (see module docs).
pub struct HnswVectorStore {
    params: HnswParams,
    inner: RwLock<Inner>,
    persist_path: Option<PathBuf>,
}

impl HnswVectorStore {
    /// Open (or create) a store. When `persist_path` is set and the
    /// file exists, records are loaded and the graph rebuilt.
    pub fn open(persist_path: Option<PathBuf>, params: HnswParams) -> Result<Self> {
        let store = Self { params, inner: RwLock::new(Inner::new()), persist_path };
        if let Some(path) = &store.persist_path {
            if path.exists() {
                let bytes = std::fs::read(path).map_err(|e| {
                    Error::Other(anyhow::anyhow!("hnsw: read {}: {e}", path.display()))
                })?;
                let parsed: PersistFile = serde_json::from_slice(&bytes).map_err(|e| {
                    Error::Other(anyhow::anyhow!("hnsw: parse {}: {e}", path.display()))
                })?;
                let mut inner = store.inner.write().expect("lock poisoned");
                for rec in parsed.records {
                    let v = normalize(&rec.embedding);
                    let id = rec.id.clone();
                    inner.records.insert(id.clone(), rec);
                    inner.insert(&store.params, &id, &v);
                }
                tracing::info!(count = inner.records.len(), path = %path.display(), "hnsw index loaded");
            }
        }
        Ok(store)
    }

    /// In-memory only (tests, benches).
    pub fn in_memory(params: HnswParams) -> Self {
        Self { params, inner: RwLock::new(Inner::new()), persist_path: None }
    }

    fn persist(&self, inner: &Inner) -> Result<()> {
        let Some(path) = &self.persist_path else {
            return Ok(());
        };
        let file = PersistFile { records: inner.records.values().cloned().collect() };
        let bytes = serde_json::to_vec(&file)
            .map_err(|e| Error::Other(anyhow::anyhow!("hnsw: serialize: {e}")))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)
            .map_err(|e| Error::Other(anyhow::anyhow!("hnsw: write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| Error::Other(anyhow::anyhow!("hnsw: rename {}: {e}", path.display())))?;
        Ok(())
    }

    fn brute_force(inner: &Inner, query: &[f32], top_k: usize) -> Vec<ScoredChunk> {
        let mut scored: Vec<ScoredChunk> = inner
            .records
            .values()
            .map(|r| ScoredChunk { record: r.clone(), score: cosine(query, &r.embedding) })
            .collect();
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }
}

#[async_trait]
impl VectorStore for HnswVectorStore {
    async fn upsert(&self, records: Vec<ChunkRecord>) -> Result<()> {
        let mut inner = self.inner.write().expect("lock poisoned");
        for rec in records {
            let v = normalize(&rec.embedding);
            let id = rec.id.clone();
            inner.records.insert(id.clone(), rec);
            inner.insert(&self.params, &id, &v);
        }
        self.persist(&inner)
    }

    async fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<ScoredChunk>> {
        let inner = self.inner.read().expect("lock poisoned");
        // Exact on small graphs: callers over-fetch (up to 200) and
        // re-rank anyway, and an approximate pass over a handful of
        // nodes could only lose recall.
        if inner.nodes.len() <= top_k.max(64) {
            return Ok(Self::brute_force(&inner, query, top_k));
        }
        Ok(inner
            .search_ann(&self.params, query, top_k)
            .into_iter()
            .filter_map(|(id, score)| {
                inner.records.get(&id).map(|r| ScoredChunk { record: r.clone(), score })
            })
            .collect())
    }

    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<ChunkRecord>> {
        let inner = self.inner.read().expect("lock poisoned");
        Ok(ids.iter().filter_map(|id| inner.records.get(id).cloned()).collect())
    }

    async fn len(&self) -> usize {
        self.inner.read().expect("lock poisoned").records.len()
    }

    async fn remove_doc(&self, doc_id: &str) -> Result<usize> {
        let mut inner = self.inner.write().expect("lock poisoned");
        let ids: Vec<String> = inner
            .records
            .values()
            .filter(|r| r.doc_id == doc_id)
            .map(|r| r.id.clone())
            .collect();
        let n = ids.len();
        for id in &ids {
            inner.unlink(id);
            inner.records.remove(id);
            inner.vecs.remove(id);
        }
        self.persist(&inner)?;
        Ok(n)
    }

    async fn chunks_of_doc(&self, doc_id: &str) -> Result<Vec<ChunkRecord>> {
        let inner = self.inner.read().expect("lock poisoned");
        let mut chunks: Vec<ChunkRecord> =
            inner.records.values().filter(|r| r.doc_id == doc_id).cloned().collect();
        chunks.sort_by_key(|r| r.ordinal);
        Ok(chunks)
    }

    async fn chunk_ids(&self) -> Result<Vec<String>> {
        let inner = self.inner.read().expect("lock poisoned");
        Ok(inner.records.keys().cloned().collect())
    }

    async fn all_chunks(&self) -> Result<Vec<ChunkRecord>> {
        let inner = self.inner.read().expect("lock poisoned");
        Ok(inner.records.values().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn rec(id: &str, doc: &str, embedding: Vec<f32>) -> ChunkRecord {
        ChunkRecord {
            id: id.into(),
            doc_id: doc.into(),
            ordinal: 0,
            text: id.into(),
            embedding,
            metadata: HashMap::new(),
        }
    }

    /// Deterministic pseudo-random unit vector (xorshift + normalize).
    fn rand_vec(state: &mut u64, dims: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(dims);
        for _ in 0..dims {
            *state ^= *state >> 12;
            *state ^= *state << 25;
            *state ^= *state >> 27;
            let v = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
            out.push(((v >> 40) as f32 / (1u64 << 24) as f32) - 0.5);
        }
        normalize(&out)
    }

    #[tokio::test]
    async fn trait_roundtrip_small() {
        let store = HnswVectorStore::in_memory(HnswParams::default());
        store
            .upsert(vec![
                rec("a", "d1", vec![1.0, 0.0, 0.0]),
                rec("b", "d1", vec![0.0, 1.0, 0.0]),
                rec("c", "d2", vec![0.7, 0.7, 0.0]),
            ])
            .await
            .unwrap();
        assert_eq!(store.len().await, 3);
        let hits = store.search(&[1.0, 0.0, 0.0], 2).await.unwrap();
        assert_eq!(hits[0].record.id, "a");
        // Update in place.
        store.upsert(vec![rec("a", "d1", vec![0.0, 0.0, 1.0])]).await.unwrap();
        assert_eq!(store.len().await, 3);
        let hits = store.search(&[0.0, 0.0, 1.0], 1).await.unwrap();
        assert_eq!(hits[0].record.id, "a");
        assert_eq!(store.remove_doc("d1").await.unwrap(), 2);
        assert_eq!(store.len().await, 1);
        assert_eq!(store.chunks_of_doc("d2").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ann_recall_against_brute_force() {
        use crate::vector_store::InMemoryVectorStore;
        let dims = 32;
        let n = 1500;
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        let mut records = Vec::new();
        for i in 0..n {
            records.push(rec(&format!("r{i}"), "d", rand_vec(&mut state, dims)));
        }
        let hnsw = HnswVectorStore::in_memory(HnswParams::default());
        hnsw.upsert(records.clone()).await.unwrap();
        let brute = InMemoryVectorStore::new();
        brute.upsert(records).await.unwrap();
        let mut total_recall = 0.0f32;
        let queries = 30;
        for _ in 0..queries {
            let q = rand_vec(&mut state, dims);
            let truth: HashSet<String> = brute
                .search(&q, 10)
                .await
                .unwrap()
                .into_iter()
                .map(|h| h.record.id)
                .collect();
            let got: HashSet<String> = hnsw
                .search(&q, 10)
                .await
                .unwrap()
                .into_iter()
                .map(|h| h.record.id)
                .collect();
            total_recall += truth.intersection(&got).count() as f32 / 10.0;
        }
        let avg = total_recall / queries as f32;
        assert!(avg >= 0.85, "recall@10 too low: {avg:.3}");
    }

    #[tokio::test]
    async fn persistence_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "hnsw-test-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let store =
                HnswVectorStore::open(Some(path.clone()), HnswParams::default()).unwrap();
            store
                .upsert(vec![
                    rec("a", "d1", vec![1.0, 0.0]),
                    rec("b", "d1", vec![0.0, 1.0]),
                ])
                .await
                .unwrap();
        }
        let store = HnswVectorStore::open(Some(path.clone()), HnswParams::default()).unwrap();
        assert_eq!(store.len().await, 2);
        let hits = store.search(&[1.0, 0.0], 1).await.unwrap();
        assert_eq!(hits[0].record.id, "a");
        let _ = std::fs::remove_file(&path);
    }
}
