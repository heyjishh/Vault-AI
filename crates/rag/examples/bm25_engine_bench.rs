//! BM25 engine bench: native linear-scan index vs Tantivy inverted
//! index, over synthetic legal-ish corpora at the eval corpus size
//! (324 chunks) and at scale (50,000 chunks).
//!
//! Run: `cargo run --release -p legal-rag-rag --example bm25_engine_bench`
//! Reports per corpus: build time per engine, mean search latency
//! over 100 deterministic queries, and mean top-10 id overlap
//! between the two engines (ranking agreement; raw scores are not
//! comparable across engines — Tantivy k1=1.2 vs native k1=1.5).

use std::collections::HashSet;
use std::time::Instant;

use legal_rag_rag::bm25::Bm25Index;
use legal_rag_rag::tantivy_bm25::TantivyBm25Index;
use legal_rag_rag::ChunkRecord;

struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

/// Weighted token pool: statute vocabulary with realistic skew, plus
/// load-bearing section-number tokens.
fn token_pool() -> Vec<&'static str> {
    let mut pool = Vec::new();
    let common = [
        "section", "act", "income", "tax", "court", "appeal", "clause", "contract", "the", "of",
        "and", "under", "shall", "person", "year", "assessment", "company", "payment", "amount",
    ];
    let mid = [
        "deduction", "petitioner", "respondent", "jurisdiction", "liability", "penalty",
        "exemption", "dividend", "interest", "salary", "capital", "gains", "transfer", "property",
        "registration", "hindu", "marriage", "arbitration", "limitation", "evidence", "witness",
        "burden", "proof", "theft", "dishonestly", "moves", "consent",
    ];
    let numbers = ["86", "80c", "378", "420", "11", "10", "73", "101", "32", "226", "40", "115jb"];
    for t in common {
        pool.extend(std::iter::repeat_n(t, 8));
    }
    for t in mid {
        pool.extend(std::iter::repeat_n(t, 3));
    }
    for t in numbers {
        pool.extend(std::iter::repeat_n(t, 2));
    }
    pool
}

fn gen_corpus(rng: &mut Rng, pool: &[&str], n: usize) -> Vec<ChunkRecord> {
    (0..n)
        .map(|i| {
            let len = 40 + rng.below(50);
            let mut words = Vec::with_capacity(len);
            for _ in 0..len {
                words.push(pool[rng.below(pool.len())].trim());
            }
            ChunkRecord {
                id: format!("c{i}"),
                doc_id: format!("doc{}", i / 20),
                ordinal: i % 20,
                text: words.join(" "),
                embedding: vec![],
                metadata: Default::default(),
            }
        })
        .collect()
}

fn gen_queries(rng: &mut Rng, pool: &[&str]) -> Vec<String> {
    let mut queries = Vec::with_capacity(100);
    for i in 0..100 {
        if i % 3 == 0 {
            // Citation-style: "section <num>" + topic terms — the
            // query shape BM25 exists for.
            let num = ["86", "378", "11", "73", "101", "80c"][rng.below(6)];
            let a = pool[rng.below(pool.len())].trim();
            let b = pool[rng.below(pool.len())].trim();
            queries.push(format!("section {num} {a} {b}"));
        } else {
            let n = 3 + rng.below(3);
            let terms: Vec<&str> =
                (0..n).map(|_| pool[rng.below(pool.len())].trim()).collect();
            queries.push(terms.join(" "));
        }
    }
    queries
}

fn bench(n: usize) {
    let pool = token_pool();
    let mut rng = Rng(0x5EED_2026_1024_0001);
    let corpus = gen_corpus(&mut rng, &pool, n);
    let queries = gen_queries(&mut rng, &pool);

    let t = Instant::now();
    let native = Bm25Index::build(&corpus);
    let native_build = t.elapsed();
    let t = Instant::now();
    let tantivy = TantivyBm25Index::build(&corpus);
    let tantivy_build = t.elapsed();
    assert_eq!(native.len(), tantivy.len());

    // Warmup (first-touch costs land here, not in the means).
    for q in queries.iter().take(5) {
        let _ = native.search(q, 10);
        let _ = tantivy.search(q, 10);
    }

    let mut native_total = std::time::Duration::ZERO;
    let mut tantivy_total = std::time::Duration::ZERO;
    let mut overlap_sum = 0usize;
    for q in &queries {
        let t = Instant::now();
        let a = native.search(q, 10);
        native_total += t.elapsed();
        let t = Instant::now();
        let b = tantivy.search(q, 10);
        tantivy_total += t.elapsed();
        let a_ids: HashSet<&str> = a.iter().map(|(id, _)| id.as_str()).collect();
        overlap_sum += b.iter().filter(|(id, _)| a_ids.contains(id.as_str())).count();
    }
    let qn = queries.len() as f32;
    println!(
        "corpus {n:>6} chunks | build native {:>8.1} ms, tantivy {:>8.1} ms | \
         mean query native {:>7.3} ms, tantivy {:>7.3} ms | mean top-10 overlap {:.1}/10",
        native_build.as_secs_f32() * 1000.0,
        tantivy_build.as_secs_f32() * 1000.0,
        native_total.as_secs_f32() * 1000.0 / qn,
        tantivy_total.as_secs_f32() * 1000.0 / qn,
        overlap_sum as f32 / qn,
    );
}

fn main() {
    bench(324);
    bench(50_000);
}
