//! Vector-engine bench: brute-force in-memory vs native HNSW
//! (Spice/DuckDB-engine design) vs SereneDB (when SERENEDB_URL is
//! set), on synthetic clustered vectors at production dimensionality.
//!
//! Run: `cargo run --release -p legal-rag-rag --example vector_bench`
//! Knobs: `BENCH_N` (default 20000), `BENCH_DIMS` (default 1024),
//! `BENCH_QUERIES` (default 100).

use std::collections::HashSet;
use std::time::Instant;

use legal_rag_rag::vector_store::{ChunkRecord, VectorStore};
use legal_rag_rag::{HnswParams, HnswVectorStore, InMemoryVectorStore};

struct Rng(u64);
impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let v = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (v >> 40) as f32 / (1u64 << 24) as f32
    }
    fn vec(&mut self, dims: usize) -> Vec<f32> {
        (0..dims).map(|_| self.next_f32() - 0.5).collect()
    }
}

fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    for x in v.iter_mut() {
        *x /= n;
    }
}

fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f32 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[tokio::main]
async fn main() {
    let n: usize = std::env::var("BENCH_N").ok().and_then(|v| v.parse().ok()).unwrap_or(20_000);
    let dims: usize =
        std::env::var("BENCH_DIMS").ok().and_then(|v| v.parse().ok()).unwrap_or(1024);
    let nq: usize =
        std::env::var("BENCH_QUERIES").ok().and_then(|v| v.parse().ok()).unwrap_or(100);
    let clusters = 200usize;

    let mut rng = Rng(0xABCD_EF01_2345_6789);
    let centers: Vec<Vec<f32>> = (0..clusters)
        .map(|_| {
            let mut c = rng.vec(dims);
            normalize(&mut c);
            c
        })
        .collect();
    let mut records = Vec::with_capacity(n);
    for i in 0..n {
        let c = &centers[i % clusters];
        let noise = rng.vec(dims);
        let mut v: Vec<f32> =
            c.iter().zip(noise.iter()).map(|(a, b)| a + 0.35 * b).collect();
        normalize(&mut v);
        records.push(ChunkRecord {
            id: format!("v{i}"),
            doc_id: format!("doc{}", i % 500),
            ordinal: i,
            text: format!("bench {i}"),
            embedding: v,
            metadata: Default::default(),
        });
    }
    let mut queries = Vec::with_capacity(nq);
    for i in 0..nq {
        let c = &centers[(i * 7) % clusters];
        let noise = rng.vec(dims);
        let mut v: Vec<f32> =
            c.iter().zip(noise.iter()).map(|(a, b)| a + 0.3 * b).collect();
        normalize(&mut v);
        queries.push(v);
    }
    println!("corpus: {n} vectors x {dims} dims, {clusters} clusters; queries: {nq}");

    // Ground truth via brute force.
    let brute = InMemoryVectorStore::new();
    let t = Instant::now();
    brute.upsert(records.clone()).await.unwrap();
    let brute_build = t.elapsed();
    let truth: Vec<HashSet<String>> = {
        let mut out = Vec::with_capacity(nq);
        for q in &queries {
            out.push(
                brute
                    .search(q, 10)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|h| h.record.id)
                    .collect(),
            );
        }
        out
    };

    async fn measure(
        name: &str,
        store: &dyn VectorStore,
        queries: &[Vec<f32>],
        truth: &[HashSet<String>],
        build: std::time::Duration,
    ) {
        // Warmup.
        for q in queries.iter().take(5) {
            let _ = store.search(q, 10).await.unwrap();
        }
        let mut lat_ms: Vec<f32> = Vec::with_capacity(queries.len());
        let mut recall = 0.0f32;
        for (i, q) in queries.iter().enumerate() {
            let t = Instant::now();
            let hits = store.search(q, 10).await.unwrap();
            lat_ms.push(t.elapsed().as_secs_f32() * 1000.0);
            let got: HashSet<String> = hits.into_iter().map(|h| h.record.id).collect();
            recall += truth[i].intersection(&got).count() as f32 / 10.0;
        }
        lat_ms.sort_by(|a, b| a.total_cmp(b));
        let mean = lat_ms.iter().sum::<f32>() / lat_ms.len() as f32;
        println!(
            "{name:<28} build {:>7.2}s | recall@10 {:.3} | mean {:>7.2} ms | p95 {:>7.2} ms",
            build.as_secs_f32(),
            recall / queries.len() as f32,
            mean,
            percentile(&lat_ms, 0.95)
        );
    }

    let only = std::env::var("BENCH_ONLY").unwrap_or_default();
    if only.is_empty() || only == "brute" {
        measure("in-memory brute force", &brute, &queries, &truth, brute_build).await;
    }

    for ef in [64usize, 128] {
        if !only.is_empty() && only != "hnsw" {
            break;
        }
        let store = HnswVectorStore::in_memory(HnswParams {
            m: 16,
            ef_construction: 128,
            ef_search: ef,
        });
        let t = Instant::now();
        store.upsert(records.clone()).await.unwrap();
        measure(&format!("hnsw (ef_search={ef})"), &store, &queries, &truth, t.elapsed()).await;
    }

    if let Ok(url) = std::env::var("SERENEDB_URL") {
        if !url.is_empty() {
            match legal_rag_rag::SereneDbVectorStore::connect(&url, "vector_bench", dims).await {
                Ok(store) => {
                    let t = Instant::now();
                    store.upsert(records.clone()).await.unwrap();
                    measure("serenedb (ivf)", &store, &queries, &truth, t.elapsed()).await;
                }
                Err(e) => println!("serenedb: connect failed: {e}"),
            }
        }
    } else {
        println!("serenedb: skipped (SERENEDB_URL not set)");
    }
}
