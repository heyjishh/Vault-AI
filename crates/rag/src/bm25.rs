//! BM25 keyword retrieval — the sparse leg of hybrid search.
//!
//! Dense (vector) retrieval is weakest exactly where law is most precise:
//! "Section 86", "40(b)", citation strings, defined terms. BM25 over the
//! chunk texts covers that flank with exact-term statistics. The query
//! layer fuses this ranking with the vector ranking (RRF) to build the
//! candidate pool the reranker then judges on merit.
//!
//! The index is rebuilt lazily when the corpus fingerprint changes; at
//! current corpus scales a full rebuild is milliseconds-to-seconds and a
//! search is a linear scan over per-chunk term maps.

use std::collections::HashMap;

use crate::vector_store::ChunkRecord;

/// Split text into search tokens: lowercase alphanumeric runs. Words of
/// 2+ chars are kept, and so are pure numbers of any length ("86" is a
/// load-bearing token in statutory text).
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tok in text.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        if tok.is_empty() {
            continue;
        }
        if tok.len() >= 2 || tok.bytes().all(|b| b.is_ascii_digit()) {
            out.push(tok.to_string());
        }
    }
    out
}

const K1: f32 = 1.5;
const B: f32 = 0.75;

pub struct Bm25Index {
    chunk_ids: Vec<String>,
    doc_len: Vec<f32>,
    avgdl: f32,
    idf: HashMap<String, f32>,
    tf: Vec<HashMap<String, u32>>,
}

impl Bm25Index {
    pub fn build(chunks: &[ChunkRecord]) -> Self {
        let n = chunks.len().max(1) as f32;
        let mut df: HashMap<String, u32> = HashMap::new();
        let mut tf: Vec<HashMap<String, u32>> = Vec::with_capacity(chunks.len());
        let mut doc_len: Vec<f32> = Vec::with_capacity(chunks.len());
        let mut total_len = 0f32;
        for chunk in chunks {
            let tokens = tokenize(&chunk.text);
            doc_len.push(tokens.len() as f32);
            total_len += tokens.len() as f32;
            let mut freq: HashMap<String, u32> = HashMap::new();
            for t in tokens {
                *freq.entry(t).or_insert(0) += 1;
            }
            for term in freq.keys() {
                *df.entry(term.clone()).or_insert(0) += 1;
            }
            tf.push(freq);
        }
        // Non-negative IDF (Robertson/Spärck Jones): rare terms weigh most,
        // corpus-wide terms approach zero but never go negative.
        let idf: HashMap<String, f32> = df
            .into_iter()
            .map(|(term, d)| {
                let d = d as f32;
                (term, (1.0 + (n - d + 0.5) / (d + 0.5)).ln())
            })
            .collect();
        Self {
            chunk_ids: chunks.iter().map(|c| c.id.clone()).collect(),
            doc_len,
            avgdl: if chunks.is_empty() { 1.0 } else { (total_len / n).max(1.0) },
            idf,
            tf,
        }
    }

    pub fn len(&self) -> usize {
        self.chunk_ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunk_ids.is_empty()
    }

    /// Rank chunks for a query. Returns (chunk_id, score) with score > 0,
    /// best first, at most `top_k` entries.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(String, f32)> {
        let mut terms: Vec<String> = tokenize(query);
        terms.sort();
        terms.dedup();
        if terms.is_empty() || self.chunk_ids.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(usize, f32)> = Vec::new();
        for (i, freq) in self.tf.iter().enumerate() {
            let mut score = 0.0f32;
            for term in &terms {
                let (Some(&idf), Some(&f)) = (self.idf.get(term), freq.get(term)) else {
                    continue;
                };
                let f = f as f32;
                let denom = f + K1 * (1.0 - B + B * self.doc_len[i] / self.avgdl);
                score += idf * (f * (K1 + 1.0)) / denom;
            }
            if score > 0.0 {
                scored.push((i, score));
            }
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .take(top_k)
            .map(|(i, s)| (self.chunk_ids[i].clone(), s))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap as Map;

    fn chunk(id: &str, text: &str) -> ChunkRecord {
        ChunkRecord {
            id: id.into(),
            doc_id: "d".into(),
            ordinal: 0,
            text: text.into(),
            embedding: vec![],
            metadata: Map::new(),
        }
    }

    #[test]
    fn exact_statute_term_wins() {
        let idx = Bm25Index::build(&[
            chunk("a", "the weather in delhi is pleasant today and the parks are full of people"),
            chunk("b", "section 86 of the income tax act deals with share of member of association"),
            chunk("c", "section 80c deductions for life insurance premiums and provident fund contributions"),
        ]);
        let hits = idx.search("section 86 share of member", 3);
        assert_eq!(hits[0].0, "b", "the chunk carrying the exact terms must rank first");
        // A query with no shared terms scores nothing.
        assert!(idx.search("quantum entanglement photons", 3).is_empty());
    }

    #[test]
    fn tokenize_keeps_section_numbers() {
        let toks = tokenize("Section 86 and rule 40(b) of the Act");
        assert!(toks.contains(&"86".to_string()));
        assert!(toks.contains(&"section".to_string()));
        assert!(toks.contains(&"40".to_string()));
    }
}
