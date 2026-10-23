//! Tantivy-backed BM25 keyword retrieval — an alternative sparse leg
//! with the same operational shape as the native [`crate::bm25`] index.
//!
//! The native index is a linear scan over per-chunk term maps: fine at
//! hundreds of chunks, but its search cost grows linearly with the
//! corpus. This engine keeps the native index's *semantics* (same
//! tokenizer rules, same non-negative Robertson/Spärck Jones IDF,
//! OR-of-terms scoring, best-first `(chunk_id, score)` results) on top
//! of Tantivy's inverted index, so search cost tracks the posting
//! lists of the query terms instead of the corpus size. Which engine
//! serves the query layer is selected elsewhere; this module only
//! mirrors the native interface (`build` / `search` / `len`).
//!
//! ## Tokenizer parity with the native index
//!
//! Native [`crate::bm25::tokenize`]: lowercase, split on
//! non-alphanumeric runs, keep tokens of length ≥ 2 **or** pure
//! numbers of any length ("86" is a load-bearing token in statutory
//! text). The Tantivy analyzer registered as `legal_bm25` reproduces
//! that chain:
//!
//! - `SimpleTokenizer` — splits on non-alphanumeric runs, exactly the
//!   native split rule (both are Unicode `char::is_alphanumeric`).
//! - `LowerCaser` — the native lowercase step.
//! - [`ShortTokenFilter`] — drops the 1-char tokens the native rule
//!   drops, keeping 1-char pure digits.
//!
//! ## Where it differs from the native index (documented, deliberate)
//!
//! - **BM25 k1**: Tantivy hardcodes k1 = 1.2 (its `query::bm25`
//!   constants); the native index uses k1 = 1.5. b (0.75) and the IDF
//!   formula are identical, so rankings agree closely but raw scores
//!   are *not* comparable across engines — never mix scores from both
//!   in one pool; fuse on ranks (as the query layer's RRF does).
//! - **Duplicate chunk ids**: the native index scores each entry
//!   independently; Tantivy indexes them as separate documents too,
//!   but TopDocs may return the same id twice from two documents.
//!   Callers already dedup downstream.
//! - **Tie order**: unspecified in both engines; near-equal scores
//!   may order differently run-to-run across engines.

use std::collections::HashSet;

use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, TermQuery};
use tantivy::schema::{
    IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, STRING,
};
use tantivy::tokenizer::{
    LowerCaser, SimpleTokenizer, TextAnalyzer, Token, TokenFilter, TokenStream, Tokenizer,
};
use tantivy::{Index, IndexReader, Searcher, TantivyDocument, Term};

use crate::vector_store::ChunkRecord;

/// Name the analyzer is registered under (schema references it).
const TOKENIZER_NAME: &str = "legal_bm25";

/// Which sparse engine this module provides, for selection wiring and
/// diagnostics. (The native engine reports "native".)
pub fn engine_name() -> &'static str {
    "tantivy"
}

/// Drops tokens shorter than 2 bytes unless they are pure ASCII
/// digits — the native tokenizer's keep-rule, as a Tantivy token
/// filter. Runs after `LowerCaser`, matching the native order
/// (lowercase first, then filter).
#[derive(Clone, Default)]
pub struct ShortTokenFilter;

impl ShortTokenFilter {
    fn keeps(text: &str) -> bool {
        text.len() >= 2 || text.bytes().all(|b| b.is_ascii_digit())
    }
}

impl TokenFilter for ShortTokenFilter {
    type Tokenizer<T: Tokenizer> = ShortTokenFilterWrapper<T>;

    fn transform<T: Tokenizer>(self, tokenizer: T) -> ShortTokenFilterWrapper<T> {
        ShortTokenFilterWrapper { inner: tokenizer }
    }
}

#[derive(Clone)]
pub struct ShortTokenFilterWrapper<T: Tokenizer> {
    inner: T,
}

impl<T: Tokenizer> Tokenizer for ShortTokenFilterWrapper<T> {
    type TokenStream<'a> = ShortTokenFilterStream<T::TokenStream<'a>>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        ShortTokenFilterStream { tail: self.inner.token_stream(text) }
    }
}

pub struct ShortTokenFilterStream<T: TokenStream> {
    tail: T,
}

impl<T: TokenStream> TokenStream for ShortTokenFilterStream<T> {
    fn advance(&mut self) -> bool {
        while self.tail.advance() {
            if ShortTokenFilter::keeps(&self.tail.token().text) {
                return true;
            }
        }
        false
    }

    fn token(&self) -> &Token {
        self.tail.token()
    }

    fn token_mut(&mut self) -> &mut Token {
        self.tail.token_mut()
    }
}

/// The analyzer chain replicating the native tokenizer (see module docs).
fn legal_analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(LowerCaser)
        .filter(ShortTokenFilter)
        .build()
}

/// Tantivy-backed BM25 index over chunk texts. Built once per corpus
/// snapshot (in-memory index; the corpus is the source of truth and
/// rebuilds are the native index's lifecycle too).
pub struct TantivyBm25Index {
    searcher: Searcher,
    // Keeps the reader (and its segments) alive for `searcher`.
    _reader: IndexReader,
    analyzer: TextAnalyzer,
    id_field: tantivy::schema::Field,
    text_field: tantivy::schema::Field,
    len: usize,
}

impl TantivyBm25Index {
    pub fn build(records: &[ChunkRecord]) -> Self {
        let mut schema_builder = Schema::builder();
        let id_field = schema_builder.add_text_field("id", STRING.set_stored());
        let text_field = schema_builder.add_text_field(
            "text",
            TextOptions::default().set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(TOKENIZER_NAME)
                    .set_index_option(IndexRecordOption::WithFreqs),
            ),
        );
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema);
        let analyzer = legal_analyzer();
        index.tokenizers().register(TOKENIZER_NAME, analyzer.clone());

        let mut writer = index
            .writer::<TantivyDocument>(50_000_000)
            .expect("tantivy: create index writer");
        for rec in records {
            let mut doc = TantivyDocument::default();
            doc.add_field_value(id_field, &rec.id);
            doc.add_field_value(text_field, &rec.text);
            writer.add_document(doc).expect("tantivy: add document");
        }
        writer.commit().expect("tantivy: commit index");

        let reader = index.reader().expect("tantivy: create reader");
        let searcher = reader.searcher();
        Self {
            searcher,
            _reader: reader,
            analyzer,
            id_field,
            text_field,
            len: records.len(),
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Unique query terms via the indexing analyzer, so query-side
    /// tokenization can never drift from index-side tokenization.
    fn query_terms(&self, query: &str) -> Vec<String> {
        let mut analyzer = self.analyzer.clone();
        let mut stream = analyzer.token_stream(query);
        let mut seen = HashSet::new();
        let mut terms = Vec::new();
        while stream.advance() {
            let text = &stream.token().text;
            if seen.insert(text.clone()) {
                terms.push(text.clone());
            }
        }
        terms
    }

    /// Rank chunks for a query. Returns (chunk_id, score) with
    /// score > 0, best first, at most `top_k` entries — the native
    /// index's contract. Scores are Tantivy's BM25 (k1 = 1.2); do not
    /// compare them against native-engine scores.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(String, f32)> {
        if top_k == 0 || self.len == 0 {
            return Vec::new();
        }
        let terms = self.query_terms(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = terms
            .iter()
            .map(|t| {
                let q = TermQuery::new(
                    Term::from_field_text(self.text_field, t),
                    IndexRecordOption::WithFreqs,
                );
                (Occur::Should, Box::new(q) as Box<dyn tantivy::query::Query>)
            })
            .collect();
        let query = BooleanQuery::new(clauses);
        let top_docs = TopDocs::with_limit(top_k).order_by_score();
        let results = match self.searcher.search(&query, &top_docs) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        results
            .into_iter()
            .filter_map(|(score, addr)| {
                if score <= 0.0 {
                    return None;
                }
                let doc: TantivyDocument = self.searcher.doc(addr).ok()?;
                let id = doc.get_first(self.id_field)?.as_str()?.to_string();
                Some((id, score))
            })
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
    fn number_tokens_are_searchable() {
        let idx = TantivyBm25Index::build(&[
            chunk("a", "the weather in delhi is pleasant today and the parks are full of people"),
            chunk("b", "section 86 of the income tax act deals with share of member of association"),
            chunk("c", "section 80c deductions for life insurance premiums and provident fund contributions"),
        ]);
        // "86" is a pure-number token: the native tokenizer keeps it,
        // and the Tantivy analyzer must too.
        let hits = idx.search("86", 3);
        assert_eq!(hits.len(), 1, "only the Section 86 chunk carries token 86");
        assert_eq!(hits[0].0, "b");
        // Single non-digit letters are dropped natively ("(b)" -> no
        // token), so a lone "b" query yields no terms and no hits.
        assert!(idx.search("b", 3).is_empty());
    }

    #[test]
    fn ranking_sanity_on_legal_corpus() {
        let idx = TantivyBm25Index::build(&[
            chunk("a", "the weather in delhi is pleasant today and the parks are full of people"),
            chunk("b", "section 86 of the income tax act deals with share of member of association"),
            chunk("c", "section 80c deductions for life insurance premiums and provident fund contributions"),
        ]);
        assert_eq!(idx.len(), 3);
        assert_eq!(engine_name(), "tantivy");
        let hits = idx.search("section 86 share of member", 3);
        assert_eq!(hits[0].0, "b", "the chunk carrying the exact terms must rank first");
        assert!(hits[0].1 > 0.0);
        // A query with no shared terms scores nothing.
        assert!(idx.search("quantum entanglement photons", 3).is_empty());
    }

    #[test]
    fn empty_index_is_safe() {
        let idx = TantivyBm25Index::build(&[]);
        assert_eq!(idx.len(), 0);
        assert!(idx.is_empty());
        assert!(idx.search("section 86", 5).is_empty());
        assert!(idx.search("", 5).is_empty());
        // Zero top_k on a non-empty index is also a no-op.
        let idx = TantivyBm25Index::build(&[chunk("x", "section 86 income tax")]);
        assert!(idx.search("section 86", 0).is_empty());
    }

    #[test]
    fn analyzer_matches_native_tokenizer() {
        // Spot-check parity on the cases the native tests pin down.
        let native = crate::bm25::tokenize("Section 86 and rule 40(b) of the Act");
        let mut analyzer = legal_analyzer();
        let mut stream = analyzer.token_stream("Section 86 and rule 40(b) of the Act");
        let mut ours = Vec::new();
        while stream.advance() {
            ours.push(stream.token().text.clone());
        }
        assert_eq!(ours, native);
    }
}
