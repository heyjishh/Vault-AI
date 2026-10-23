//! Query service — mirrors `raganything.query.QueryMixin.aquery`.
//!
//! Modes are LightRAG's exact six, which RAG-Anything passes straight through:
//! - `naive`: vector search over chunks only.
//! - `local`: query entities → graph neighborhood (entity descriptions +
//!   related chunks).
//! - `global`: relation-level graph knowledge (relations + entity descriptions).
//! - `hybrid`: local + global combined.
//! - `mix`: hybrid blended with naive vector chunks (RAG-Anything's default).
//! - `bypass`: no retrieval; the LLM answers from its own knowledge.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use legal_rag_common::Result;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::bm25::Bm25Index;
use crate::cache::{hash_key, SemanticAnswerCache};
use crate::embeddings::Embeddings;
use crate::graph::{Entity, GraphStore, Relation};
use crate::llm::LlmClient;
use crate::rerank::Reranker;
use crate::vector_store::{is_searchable, parent_id, ChunkRecord, ScoredChunk, VectorStore};
use crate::web::{FetchedPage, WebSearch};

/// Retrieval strategy — LightRAG query modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryMode {
    Naive,
    Local,
    Global,
    Hybrid,
    Mix,
    Bypass,
}

impl Default for QueryMode {
    fn default() -> Self {
        Self::Mix
    }
}

/// A source citation attached to an answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub doc_id: String,
    pub doc_title: Option<String>,
    pub chunk_id: String,
    pub ordinal: usize,
    pub score: f32,
    pub snippet: String,
    /// Pinpoint: the single sentence inside the cited passage that best
    /// matches the question (sentence-level citation). `None` when no
    /// sentence clears the relevance bar — the snippet then stands alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentence: Option<String>,
    /// Staleness note when the cited document is known to have been
    /// superseded (e.g. "Superseded by the 2023 Amendment Act").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<String>,
    /// Origin URL for web citations (the authority card's link).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Publication / judgment date (YYYY-MM-DD) for web citations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_date: Option<String>,
}

/// Format contract appended to the system prompt for web-augmented
/// answers: verdict first (the way counsel reads), structured analysis,
/// and a hard rule against citing a source for what it does not say —
/// the citation-integrity audit enforces the same rule mechanically.
pub const WEB_FORMAT_ADDENDUM: &str = "\n\nWEB-AUGMENTED ANSWER FORMAT — follow exactly:\n\
## Verdict\nThe direct answer in 2-4 sentences. Every factual claim cited inline.\n\
## Analysis\nThe reasoning, organised with short sub-headings where the material warrants it. \
Cite every factual claim inline as [web:N] or [doc:N], attached to the sentence it supports.\n\
## Caveats\nInclude this section ONLY if sources conflict, a key source is dated, or the position may have changed since publication.\n\
Cite a source ONLY for propositions its text actually states — never for general background. \
Prefer court and government sources over commentary; when sources disagree, say so and cite both. \
One bracket per source: write [web:1] [web:2], not [web:1, web:2].";

/// Pick the most question-relevant passages from a fetched page rather
/// than its head (usually navigation/boilerplate). Paragraphs are scored
/// by distinct query-term coverage; the best are re-joined in original
/// order up to `budget` chars.
pub fn best_passages(text: &str, question: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_string();
    }
    let mut qterms = crate::bm25::tokenize(question);
    qterms.sort();
    qterms.dedup();
    if qterms.is_empty() {
        return text.chars().take(budget).collect();
    }
    let paras: Vec<&str> = text.split('\n').filter(|l| !l.trim().is_empty()).collect();
    let mut scored: Vec<(usize, usize)> = Vec::new();
    for (i, p) in paras.iter().enumerate() {
        let toks = crate::bm25::tokenize(p);
        let set: std::collections::HashSet<&String> = toks.iter().collect();
        let score = qterms.iter().filter(|t| set.contains(t)).count();
        if score > 0 {
            scored.push((i, score));
        }
    }
    if scored.is_empty() {
        return text.chars().take(budget).collect();
    }
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut chosen: Vec<usize> = Vec::new();
    let mut used = 0usize;
    for (i, _) in &scored {
        let len = paras[*i].chars().count();
        if used + len > budget && !chosen.is_empty() {
            continue;
        }
        chosen.push(*i);
        used += len + 8;
        if used >= budget {
            break;
        }
    }
    chosen.sort_unstable();
    chosen.iter().map(|&i| paras[i]).collect::<Vec<_>>().join("\n...\n")
}

/// Pinpoint extraction: the single sentence in `text` that best supports a
/// citation for `question`, scored by distinct query-term coverage (the
/// same signal `best_passages` uses). Returns `None` when nothing clears
/// the bar (≥2 distinct terms, or ≥1 for very short questions) or the best
/// sentence is unusably short/long — a wrong pinpoint is worse than none.
pub fn best_sentence(text: &str, question: &str) -> Option<String> {
    let mut qterms = crate::bm25::tokenize(question);
    qterms.sort();
    qterms.dedup();
    if qterms.is_empty() {
        return None;
    }
    let need = if qterms.len() <= 2 { 1 } else { 2 };
    let mut best: Option<(usize, String)> = None;
    for raw in crate::pipeline::split_sentences_flowing(text) {
        let s = raw.trim();
        let n = s.chars().count();
        if !(40..=700).contains(&n) {
            continue;
        }
        // A pinpoint must be a complete sentence, not a wrapped fragment:
        // it has to end on terminal punctuation.
        if !matches!(s.chars().last(), Some('.' | '!' | '?' | '।')) {
            continue;
        }
        let toks = crate::bm25::tokenize(s);
        let set: std::collections::HashSet<&String> = toks.iter().collect();
        let score = qterms.iter().filter(|t| set.contains(t)).count();
        if score < need {
            continue;
        }
        match &best {
            Some((bs, _)) if *bs >= score => {}
            _ => best = Some((score, s.to_string())),
        }
    }
    best.map(|(_, s)| s)
}

/// A source blended into the answer that lives outside the shared corpus —
/// the owner's confidential documents, decrypted for one request only.
/// Cited as `[private:N]`.
#[derive(Debug, Clone)]
pub struct ExtraSource {
    pub title: String,
    pub text: String,
    pub score: f32,
}

/// A grounded answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryAnswer {
    pub answer: String,
    pub citations: Vec<Citation>,
    pub mode: QueryMode,
    /// Set when a matter-scoped query found nothing inside the matter and
    /// fell back to the full vault — the UI surfaces this so the widening
    /// is never silent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_note: Option<String>,
    /// True when this answer is a human-verified annotation, served
    /// verbatim — the UI badges it so its authority is visible.
    #[serde(default)]
    pub verified: bool,
    /// Citation-integrity receipt: how many markers were mechanically
    /// checked against their sources, verified, and stripped as
    /// unsupported. None when no audit ran (verified annotations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation_audit: Option<crate::audit::CitationAudit>,
}

/// A human-verified answer (annotation): a curator corrected or authored
/// the answer to a question, and semantically matching future questions
/// are served it verbatim. Annotations bind to the corpus fingerprint
/// and (optionally) a brief's document set — change either and the
/// annotation stops serving until a curator re-verifies it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub question: String,
    pub qvec: Vec<f32>,
    pub answer: String,
    pub citations: Vec<Citation>,
    /// None = serves unscoped queries; Some = only that document set.
    pub scope_docs: Option<Vec<String>>,
    pub corpus_fp: String,
    pub enabled: bool,
}

/// Document-id set a query is scoped to (a matter's documents).
pub type DocScope = std::collections::HashSet<String>;

/// One ranked hit in a hit-test stage.
#[derive(Debug, Clone, Serialize)]
pub struct HitView {
    pub chunk_id: String,
    pub doc_id: String,
    pub doc_title: Option<String>,
    pub score: f32,
    pub snippet: String,
    pub is_child: bool,
}

/// One retrieval stage's ranking in a hit test.
#[derive(Debug, Clone, Serialize)]
pub struct HitStage {
    pub name: String,
    pub hits: Vec<HitView>,
}

/// The full hit-test report: the same question through every stage.
#[derive(Debug, Clone, Serialize)]
pub struct HitTestReport {
    pub question: String,
    pub stages: Vec<HitStage>,
}

/// One event in a streaming answer: status updates, the citation set
/// (emitted before generation so the UI can render sources early),
/// answer deltas, and the completed answer.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueryEvent {
    Status { message: String },
    Sources {
        citations: Vec<Citation>,
        mode: QueryMode,
        /// True when the answer being streamed is a verified annotation.
        #[serde(default)]
        verified: bool,
    },
    Delta { text: String },
    Done {
        answer: String,
        /// Citation-integrity receipt for the completed answer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audit: Option<crate::audit::CitationAudit>,
    },
}

pub const ANSWER_SYSTEM: &str = "You are a precise legal research assistant. Answer the user's question \
using ONLY the provided source excerpts. Cite every factual claim with the source id in brackets, \
e.g. [doc:3]. If the sources do not contain the answer, say so explicitly and do not invent details.";

const BYPASS_SYSTEM: &str = "You are a precise legal research assistant. Answer the user's question \
from your own knowledge. Be clear about what you know and what you are unsure of.";

pub struct QueryService {
    embeddings: Arc<dyn Embeddings>,
    store: Arc<dyn VectorStore>,
    graph: Arc<dyn GraphStore>,
    llm: Arc<dyn LlmClient>,
    doc_titles: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
    /// Legal-validity metadata lookup by document id, pushed in by the
    /// server (defaults to a no-metadata source).
    doc_meta: std::sync::RwLock<Arc<dyn Fn(&str) -> Option<crate::pipeline::DocMeta> + Send + Sync>>,
    web: Arc<dyn WebSearch>,
    reranker: Option<Arc<dyn Reranker>>,
    rerank_candidates: usize,
    /// Admin-set system prompt override (None = built-in default).
    answer_system: std::sync::RwLock<Option<String>>,
    /// Admin-set research prompt overrides (planner, adjudicator).
    research_systems: std::sync::RwLock<(Option<String>, Option<String>)>,
    /// Semantic answer cache (paraphrase-level hits at zero LLM cost).
    answer_cache: Arc<SemanticAnswerCache>,
    answer_cache_enabled: bool,
    /// Human-verified annotations, pushed in by the server (the verified
    /// tier — checked before the automatic semantic cache).
    annotations: std::sync::RwLock<Vec<Annotation>>,
    annotation_threshold: f32,
    /// BM25 keyword index, rebuilt lazily when the corpus fingerprint
    /// changes: (fingerprint, index).
    bm25: std::sync::RwLock<Option<(String, Arc<Bm25Index>)>>,
    /// Full-text cache for the in-app source viewer, keyed by URL.
    source_cache: std::sync::Mutex<std::collections::HashMap<String, FetchedPage>>,
}

impl QueryService {
    pub fn new(
        embeddings: Arc<dyn Embeddings>,
        store: Arc<dyn VectorStore>,
        graph: Arc<dyn GraphStore>,
        llm: Arc<dyn LlmClient>,
        doc_titles: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
        web: Arc<dyn WebSearch>,
        reranker: Option<Arc<dyn Reranker>>,
    ) -> Self {
        let rerank_candidates = std::env::var("LEGAL_RAG_RERANK_CANDIDATES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20)
            .clamp(5, 60);
        let threshold = std::env::var("LEGAL_RAG_SEMANTIC_THRESHOLD")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.97);
        Self {
            embeddings,
            store,
            graph,
            llm,
            doc_titles,
            doc_meta: std::sync::RwLock::new(Arc::new(|_| None)),
            web,
            reranker,
            rerank_candidates,
            answer_system: std::sync::RwLock::new(None),
            research_systems: std::sync::RwLock::new((None, None)),
            answer_cache: Arc::new(SemanticAnswerCache::new(256, threshold)),
            answer_cache_enabled: std::env::var("LEGAL_RAG_SEMANTIC_CACHE")
                .map(|v| v != "0")
                .unwrap_or(true),
            annotations: std::sync::RwLock::new(Vec::new()),
            annotation_threshold: std::env::var("LEGAL_RAG_ANNOTATION_THRESHOLD")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.95)
                .clamp(0.85, 0.999),
            bm25: std::sync::RwLock::new(None),
            source_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Full text of a web source, for the in-app source viewer. Cached
    /// per URL for the life of the process: a user re-opens the same
    /// source repeatedly while reading one answer, and judgment PDFs
    /// are slow to fetch and extract.
    pub async fn fetch_web_source(&self, url: &str) -> Result<FetchedPage> {
        if let Some(page) = self.source_cache.lock().unwrap().get(url).cloned() {
            return Ok(page);
        }
        let page = self.web.fetch(url).await?;
        let mut cache = self.source_cache.lock().unwrap();
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(url.to_string(), page.clone());
        Ok(page)
    }

    /// Stats for the semantic answer cache (admin/cache visibility).
    pub fn answer_cache_stats(&self) -> crate::cache::CacheStats {
        self.answer_cache.stats()
    }

    /// Retrieval hit-testing (admin): run one question through every
    /// retrieval stage — vector, BM25, RRF fusion, rerank — and report
    /// each stage's ranking, without generating an answer. One embedding
    /// call; the reranker is local.
    pub async fn hit_test(&self, question: &str, top_k: usize) -> Result<HitTestReport> {
        let top_k = top_k.clamp(1, 20);
        let view = |h: &ScoredChunk| HitView {
            chunk_id: h.record.id.clone(),
            doc_id: h.record.doc_id.clone(),
            doc_title: (self.doc_titles)(&h.record.doc_id),
            score: h.score,
            snippet: h.record.text.chars().take(240).collect(),
            is_child: parent_id(&h.record).is_some(),
        };
        let qvec = self.embeddings.embed(&[question.to_string()]).await?.remove(0);
        let wide = (top_k * 4).clamp(top_k, 200);
        let raw = self.store.search(&qvec, wide).await?;
        let mut vec_hits: Vec<ScoredChunk> =
            raw.into_iter().filter(|h| is_searchable(&h.record)).collect();
        vec_hits.truncate(top_k);
        let mut stages = vec![HitStage {
            name: "vector".to_string(),
            hits: vec_hits.iter().map(&view).collect(),
        }];
        // BM25 stage on its own (records fetched for display + fusion).
        let fp = self.corpus_fingerprint().await;
        let kw = match self.bm25_index(&fp).await {
            Some(idx) if !idx.is_empty() => idx.search(question, top_k),
            _ => Vec::new(),
        };
        let kw_ids: Vec<String> = kw.iter().map(|(id, _)| id.clone()).collect();
        let kw_records = if kw_ids.is_empty() {
            Vec::new()
        } else {
            self.store.get_by_ids(&kw_ids).await.unwrap_or_default()
        };
        let kw_by_id: HashMap<&str, &ChunkRecord> =
            kw_records.iter().map(|r| (r.id.as_str(), r)).collect();
        stages.push(HitStage {
            name: "bm25".to_string(),
            hits: kw
                .iter()
                .filter_map(|(id, score)| {
                    kw_by_id.get(id.as_str()).map(|r| HitView {
                        chunk_id: r.id.clone(),
                        doc_id: r.doc_id.clone(),
                        doc_title: (self.doc_titles)(&r.doc_id),
                        score: *score,
                        snippet: r.text.chars().take(240).collect(),
                        is_child: parent_id(r).is_some(),
                    })
                })
                .collect(),
        });
        // Fused pool (exactly what production retrieval builds), then
        // the reranker's verdict on it.
        let vec_ids: HashSet<&str> = vec_hits.iter().map(|h| h.record.id.as_str()).collect();
        let extra: Vec<ChunkRecord> = kw_records
            .iter()
            .filter(|r| !vec_ids.contains(r.id.as_str()))
            .cloned()
            .collect();
        let fused = fuse_rrf(vec_hits, &kw, extra, top_k);
        stages.push(HitStage {
            name: "fused".to_string(),
            hits: fused.iter().map(&view).collect(),
        });
        if self.reranker.is_some() {
            let ranked = self.maybe_rerank(question, fused, top_k).await?;
            stages.push(HitStage {
                name: "reranked".to_string(),
                hits: ranked.iter().map(&view).collect(),
            });
        }
        Ok(HitTestReport { question: question.to_string(), stages })
    }

    /// Replace the verified-annotation set (server pushes its store in
    /// at boot and on every mutation).
    pub fn set_annotations(&self, annotations: Vec<Annotation>) {
        *self.annotations.write().unwrap() = annotations;
    }

    /// The verified tier: a human-curated answer whose question embedding
    /// matches this one, under the same corpus state and brief scope.
    /// Checked before the automatic semantic cache — a verified answer
    /// always wins over a machine-cached one.
    fn annotation_lookup(
        &self,
        qvec: &[f32],
        mode: QueryMode,
        scope: Option<&DocScope>,
        corpus_fp: &str,
    ) -> Option<QueryAnswer> {
        let annotations = self.annotations.read().unwrap();
        let mut best: Option<(f32, &Annotation)> = None;
        for a in annotations.iter() {
            if !a.enabled || a.corpus_fp != corpus_fp || a.qvec.len() != qvec.len() {
                continue;
            }
            let scope_ok = match (&a.scope_docs, scope) {
                (None, None) => true,
                (Some(docs), Some(set)) => {
                    docs.len() == set.len() && docs.iter().all(|d| set.contains(d))
                }
                _ => false,
            };
            if !scope_ok {
                continue;
            }
            let sim = crate::vector_store::cosine(qvec, &a.qvec);
            if sim >= self.annotation_threshold && best.is_none_or(|(s, _)| sim > s) {
                best = Some((sim, a));
            }
        }
        best.map(|(_, a)| QueryAnswer {
            answer: a.answer.clone(),
            citations: a.citations.clone(),
            mode,
            scope_note: None,
            verified: true,
            citation_audit: None,
        })
    }

    /// Corpus fingerprint: hash of the sorted chunk-id set. Any ingest,
    /// delete, or re-embed changes it, which (a) invalidates every
    /// semantic-cache scope key and (b) triggers a BM25 index rebuild.
    async fn corpus_fingerprint(&self) -> String {
        match self.store.chunk_ids().await {
            Ok(mut ids) => {
                ids.sort_unstable();
                hash_key(&[&ids.join(",")])
            }
            Err(_) => "unknown".to_string(),
        }
    }

    /// Scope key for the semantic answer cache: two questions may only
    /// share an answer under identical mode, pool size, brief scope, and
    /// corpus state.
    fn answer_scope_key(
        &self,
        mode: QueryMode,
        top_k: usize,
        scope: Option<&DocScope>,
        corpus_fp: &str,
    ) -> String {
        let scope_part = match scope {
            Some(docs) => {
                let mut v: Vec<&str> = docs.iter().map(|s| s.as_str()).collect();
                v.sort_unstable();
                v.join(",")
            }
            None => "all".to_string(),
        };
        hash_key(&[&format!("{mode:?}"), &top_k.to_string(), &scope_part, corpus_fp])
    }

    /// The current BM25 index, rebuilt when the corpus has changed.
    async fn bm25_index(&self, corpus_fp: &str) -> Option<Arc<Bm25Index>> {
        {
            let guard = self.bm25.read().unwrap();
            if let Some((fp, idx)) = guard.as_ref() {
                if fp == corpus_fp {
                    return Some(idx.clone());
                }
            }
        }
        let chunks = self.store.all_chunks().await.ok()?;
        // Keyword search runs over the same match units as vector search:
        // child chunks + un-split parents, never context-only parents.
        let searchable: Vec<ChunkRecord> =
            chunks.into_iter().filter(|c| is_searchable(c)).collect();
        let idx = Arc::new(Bm25Index::build(&searchable));
        *self.bm25.write().unwrap() = Some((corpus_fp.to_string(), idx.clone()));
        Some(idx)
    }

    /// Replace the research prompts at runtime (admin customization):
    /// the planner that frames hypotheses and the adjudicator that
    /// weighs evidence and writes the report. `None` = built-in.
    pub fn set_research_prompts(&self, frame: Option<String>, adjudicate: Option<String>) {
        let clean = |p: Option<String>| p.filter(|s| !s.trim().is_empty());
        *self.research_systems.write().unwrap() = (clean(frame), clean(adjudicate));
    }

    pub(crate) fn research_frame_system(&self, default: &str) -> String {
        self.research_systems
            .read()
            .unwrap()
            .0
            .clone()
            .unwrap_or_else(|| default.to_string())
    }

    pub(crate) fn research_adjudicate_system(&self, default: &str) -> String {
        self.research_systems
            .read()
            .unwrap()
            .1
            .clone()
            .unwrap_or_else(|| default.to_string())
    }

    /// Replace the answer system prompt at runtime (admin customization).
    /// `None` restores the built-in default.
    pub fn set_answer_system(&self, prompt: Option<String>) {
        let prompt = prompt.filter(|p| !p.trim().is_empty());
        *self.answer_system.write().unwrap() = prompt;
    }

    fn answer_system(&self) -> String {
        self.answer_system
            .read()
            .unwrap()
            .clone()
            .unwrap_or_else(|| ANSWER_SYSTEM.to_string())
    }

    pub(crate) fn llm(&self) -> &Arc<dyn LlmClient> {
        &self.llm
    }

    pub(crate) fn doc_titles(&self) -> &Arc<dyn Fn(&str) -> Option<String> + Send + Sync> {
        &self.doc_titles
    }

    /// Push in the legal-validity metadata source (server wiring: the
    /// document registry). Called once at startup.
    pub fn set_doc_meta_source(
        &self,
        f: Arc<dyn Fn(&str) -> Option<crate::pipeline::DocMeta> + Send + Sync>,
    ) {
        *self.doc_meta.write().expect("lock poisoned") = f;
    }

    /// Staleness note for one document, when it is known superseded.
    pub(crate) fn stale_of(&self, doc_id: &str) -> Option<String> {
        let f = self.doc_meta.read().expect("lock poisoned").clone();
        f(doc_id).and_then(|m| m.stale_note())
    }

    /// Run one query end to end — like `aquery`.
    pub async fn query(&self, question: &str, mode: QueryMode, top_k: usize) -> Result<QueryAnswer> {
        self.query_inner(question, mode, top_k, &[], None, &[], false, None).await
    }

    /// Full query: optional brief scope plus the owner's confidential
    /// sources (decrypted by the caller for this request only), cited as
    /// `[private:N]`.
    pub async fn query_full(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        scope_docs: Option<Vec<String>>,
        extra: Vec<ExtraSource>,
        strict: bool,
        llm_override: Option<Arc<dyn LlmClient>>,
    ) -> Result<QueryAnswer> {
        let scope: Option<DocScope> = scope_docs.map(|docs| docs.into_iter().collect());
        self.query_inner(question, mode, top_k, &[], scope, &extra, strict, llm_override).await
    }

    /// Query scoped to a set of documents (a matter). Retrieval and graph
    /// context are limited to those documents; if nothing in scope matches,
    /// the query falls back to the full vault and says so in `scope_note`.
    pub async fn query_scoped(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        scope_docs: Vec<String>,
    ) -> Result<QueryAnswer> {
        let scope: DocScope = scope_docs.into_iter().collect();
        self.query_inner(question, mode, top_k, &[], Some(scope), &[], false, None).await
    }

    /// Strictly scoped query for legal-validity filters (as-of date /
    /// jurisdiction): the document set is a hard boundary — when nothing
    /// inside it matches, the answer refuses instead of falling back to
    /// the full vault (which could silently serve out-of-force law).
    pub async fn query_scoped_strict(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        scope_docs: Vec<String>,
    ) -> Result<QueryAnswer> {
        let scope: DocScope = scope_docs.into_iter().collect();
        self.query_inner(question, mode, top_k, &[], Some(scope), &[], true, None).await
    }

    /// Query with live web augmentation: local retrieval plus stealth-fetched
    /// web pages, cited as `[web:N]` alongside corpus `[doc:N]` citations.
    pub async fn query_with_web(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        web_results: usize,
        scope_docs: Option<Vec<String>>,
        strict: bool,
    ) -> Result<QueryAnswer> {
        let pages = self.gather_web(question, web_results).await;
        let scope: Option<DocScope> = scope_docs.map(|docs| docs.into_iter().collect());
        self.query_inner(question, mode, top_k, &pages, scope, &[], strict, None).await
    }

    /// Fetch + extract top web pages for a question (best-effort; failures
    /// are skipped, never fatal). The search sidecar returns up to 2x
    /// deduped, authority-ranked candidates. Fetching runs in two waves:
    /// the top-3 candidates (primary sources — court PDFs are slow under
    /// load) get a protected wave with a full budget, then the rest of
    /// the window races to fill the remaining slots, keeping the first
    /// `max_results` successes in ranked order — one bot-guarded site can
    /// neither stall the leg nor sink it (no fallback candidates).
    pub(crate) async fn gather_web(&self, question: &str, max_results: usize) -> Vec<FetchedPage> {
        let results = match self.web.search(question, max_results).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "web search failed");
                return Vec::new();
            }
        };
        let window = (max_results + 3).min(results.len());
        let results: Vec<crate::web::WebResult> =
            results.into_iter().take(window).collect();
        let mut ok: Vec<(usize, FetchedPage)> = Vec::new();
        let mut done: Vec<bool> = vec![false; window];

        // Phase A — protected fetch for the top-ranked candidates.
        // Search returns authority-ranked results (court PDFs first for
        // legal questions), but a judgment PDF is slow under load: in an
        // all-at-once race it loses to fast news pages and gets aborted
        // by the grace exit below, leaving the answer citing secondary
        // sources (news, Wikipedia) for a primary-source question. The
        // top 3 candidates get their own wave and a full budget before
        // the pack starts.
        let priority = 3.min(window);
        {
            let mut set = tokio::task::JoinSet::new();
            for (i, r) in results.iter().take(priority).enumerate() {
                let web = self.web.clone();
                let url = r.url.clone();
                set.spawn(async move { (i, web.fetch(&url).await) });
            }
            let deadline =
                tokio::time::Instant::now() + std::time::Duration::from_secs(45);
            while !set.is_empty() {
                match tokio::time::timeout_at(deadline, set.join_next()).await {
                    Ok(Some(Ok((i, Ok(p))))) => {
                        done[i] = true;
                        if !p.text.trim().is_empty() {
                            ok.push((i, p));
                        }
                    }
                    Ok(Some(Ok((i, Err(e))))) => {
                        done[i] = true;
                        warn!(error = %e, "web fetch failed (priority wave)");
                    }
                    Ok(Some(Err(e))) => {
                        warn!(error = %e, "web fetch task failed (priority wave)")
                    }
                    Ok(None) => break,
                    Err(_) => {
                        set.abort_all();
                        break;
                    }
                }
            }
        }

        // Phase B — the rest of the window races to fill the remaining
        // slots, with the grace/hard-cap exits below.
        let mut set = tokio::task::JoinSet::new();
        if ok.len() < max_results {
            for (i, r) in results.iter().enumerate().skip(priority) {
                let web = self.web.clone();
                let url = r.url.clone();
                set.spawn(async move { (i, web.fetch(&url).await) });
            }
        }
        let started = tokio::time::Instant::now();
        while !set.is_empty() {
            let joined =
                tokio::time::timeout(std::time::Duration::from_secs(8), set.join_next()).await;
            match joined {
                Ok(Some(Ok((i, Ok(p))))) if !p.text.trim().is_empty() => {
                    done[i] = true;
                    ok.push((i, p));
                }
                Ok(Some(Ok((i, Ok(_))))) => done[i] = true,
                Ok(Some(Ok((i, Err(e))))) => {
                    done[i] = true;
                    warn!(error = %e, "web fetch failed");
                }
                Ok(Some(Err(e))) => warn!(error = %e, "web fetch task failed"),
                Ok(None) => break,
                Err(_) => {} // 8s slice elapsed; re-check the exit rules.
            }
            let elapsed = started.elapsed();
            if ok.len() >= max_results {
                // Ideal exit: the first `max_results` successes are in
                // and every candidate ranked above the last of them has
                // finished. Grace exit: after 20s a still-grinding
                // straggler (a bot-guarded site in the Firefox fallback)
                // must not gate the answer either.
                let mut idx: Vec<usize> = ok.iter().map(|(i, _)| *i).collect();
                idx.sort_unstable();
                let cutoff = idx[max_results - 1];
                if done[..=cutoff].iter().all(|d| *d)
                    || elapsed > std::time::Duration::from_secs(20)
                {
                    set.abort_all();
                    break;
                }
            }
            if elapsed > std::time::Duration::from_secs(45) && !ok.is_empty() {
                set.abort_all();
                break;
            }
        }
        ok.sort_by_key(|(i, _)| *i);
        let pages: Vec<FetchedPage> =
            ok.into_iter().map(|(_, p)| p).take(max_results).collect();
        info!(pages = pages.len(), "web pages gathered");
        pages
    }

    /// Precision pass: score candidates with the reranker and keep the best
    /// `top_k`. Falls back to retrieval order on any failure.
    async fn maybe_rerank(
        &self,
        question: &str,
        hits: Vec<ScoredChunk>,
        top_k: usize,
    ) -> Result<Vec<ScoredChunk>> {
        let Some(reranker) = &self.reranker else {
            return Ok(hits.into_iter().take(top_k).collect());
        };
        if hits.len() <= 1 {
            return Ok(hits);
        }
        // Latency discipline for the cross-encoder (it is the slowest
        // stage on CPU hosts, and its cost grows with pool × length):
        // 1. Decisive-margin fast path — when the top vector hit clearly
        //    dominates, the ranking is not in doubt; reranking near-ties
        //    is exactly where the cross-encoder earns its keep, and those
        //    are the cases that still take the slow path.
        if hits.len() >= 2 && hits[0].score >= 0.55 && (hits[0].score - hits[1].score) >= 0.12 {
            info!(top = hits[0].score, runner_up = hits[1].score, "rerank skipped: decisive margin");
            return Ok(hits.into_iter().take(top_k).collect());
        }
        //    …or when the top-k SET is decisive: a clear score cliff at
        //    the cut means reranking could only shuffle order inside the
        //    set the answer will cite anyway — not worth seconds of wait.
        if hits.len() > top_k && (hits[top_k - 1].score - hits[top_k].score) >= 0.08 {
            info!(cut = hits[top_k - 1].score, below = hits[top_k].score, "rerank skipped: decisive top-k set");
            return Ok(hits.into_iter().take(top_k).collect());
        }
        // 2. Cap the scored pool (env-tunable; raise it on GPU hosts).
        let pool = std::env::var("LEGAL_RAG_RERANK_POOL")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(12)
            .clamp(4, 40);
        let hits: Vec<ScoredChunk> = hits.into_iter().take(pool).collect();
        // 3. Score on a tighter excerpt — cross-encoders front-weight
        //    the passage, and shorter sequences are much cheaper.
        let docs: Vec<String> = hits
            .iter()
            .map(|h| h.record.text.chars().take(500).collect())
            .collect();
        let order = match reranker.rerank(question, &docs, top_k).await {
            Ok(o) if !o.is_empty() => o,
            _ => {
                warn!("rerank failed; keeping retrieval order");
                return Ok(hits.into_iter().take(top_k).collect());
            }
        };
        let mut ranked: Vec<ScoredChunk> = Vec::with_capacity(top_k);
        for &i in &order {
            if let Some(h) = hits.get(i) {
                ranked.push(h.clone());
            }
        }
        for (i, h) in hits.iter().enumerate() {
            if ranked.len() >= top_k {
                break;
            }
            if !order.contains(&i) {
                ranked.push(h.clone());
            }
        }
        info!(candidates = hits.len(), kept = ranked.len(), "reranked");
        Ok(ranked)
    }

    /// Retrieval + rerank + prompt assembly. Returns (system, prompt,
    /// citations, scope_note) so both the blocking and streaming paths share
    /// identical grounding.
    async fn prepare_context(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        web_pages: &[FetchedPage],
        scope: Option<DocScope>,
        extra: &[ExtraSource],
        strict: bool,
    ) -> Result<(String, String, Vec<Citation>, Option<String>)> {
        let top_k = top_k.clamp(1, 50);

        if mode == QueryMode::Bypass {
            return Ok((
                BYPASS_SYSTEM.to_string(),
                format!("Question: {question}"),
                Vec::new(),
                None,
            ));
        }

        // With a reranker, retrieve a wide candidate set and let the
        // precision pass select the best top_k. When scoped to a matter,
        // fetch wider still: the in-scope pool is a subset of the corpus.
        let fetch_k = if scope.is_some() {
            (top_k * 6).max(self.rerank_candidates).clamp(top_k, 100)
        } else if self.reranker.is_some() {
            self.rerank_candidates.max(top_k)
        } else {
            top_k
        };
        let (mut hits, mut graph_context) =
            self.retrieve(question, mode, fetch_k, scope.as_ref()).await?;
        let mut scope_note = None;
        if scope.is_some() && hits.is_empty() && graph_context.is_empty() {
            if strict {
                // A legal-validity scope (as-of date / jurisdiction) is a
                // hard boundary: serving out-of-scope law silently would
                // be a correctness defect, so there is no fallback — the
                // answer proceeds with no sources and refuses, grounded.
                scope_note = Some(
                    "No documents in force for the requested date and jurisdiction matched — nothing outside that scope was used."
                        .to_string(),
                );
            } else {
                // Nothing inside the matter matched: widen to the full vault,
                // but never silently — the answer carries the note.
                let (h2, g2) = self.retrieve(question, mode, fetch_k, None).await?;
                hits = h2;
                graph_context = g2;
                scope_note =
                    Some("No documents in this brief matched — answered from the full vault.".to_string());
            }
        }
        let hits = self.maybe_rerank(question, hits, top_k).await?;
        let hits = apply_per_doc_cap(hits);
        // Parent-child: the reranker judged the precise child spans; the
        // LLM now gets each hit's parent — the whole section — as
        // context, and citations point at the parent.
        let hits = self.expand_parents(hits).await;

        let mut context_parts = Vec::new();
        if !graph_context.is_empty() {
            context_parts.push(format!("Knowledge graph context:\n{graph_context}"));
        }
        let chunks_text = hits
            .iter()
            .enumerate()
            .map(|(i, h)| {
                format!(
                    "[doc:{}] (chunk {}):\n{}",
                    i,
                    h.record.id,
                    h.record.text.chars().take(1500).collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if !chunks_text.is_empty() {
            context_parts.push(format!("Source excerpts:\n{chunks_text}"));
        }
        if !web_pages.is_empty() {
            let web_text = web_pages
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    format!(
                        "[web:{}] {} ({})\n{}",
                        i,
                        p.title,
                        p.url,
                        best_passages(&p.text, question, 1500)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            context_parts.push(format!(
                "Live web sources (verify recency; may be less authoritative than the corpus):\n{web_text}"
            ));
        }
        if !extra.is_empty() {
            let priv_text = extra
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    format!(
                        "[private:{}] {}\n{}",
                        i,
                        s.title,
                        s.text.chars().take(1500).collect::<String>()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            context_parts.push(format!(
                "Confidential sources (the user's own private documents, visible to them alone):\n{priv_text}"
            ));
        }
        let user_prompt = format!("Question: {question}\n\n{}", context_parts.join("\n\n"));

        let mut citations: Vec<Citation> = hits
            .into_iter()
            .enumerate()
            .map(|(i, h)| Citation {
                doc_title: (self.doc_titles)(&h.record.doc_id),
                stale: self.stale_of(&h.record.doc_id),
                sentence: best_sentence(&h.record.text, question),
                snippet: h.record.text.chars().take(1500).collect(),
                doc_id: format!("doc:{i}"),
                chunk_id: h.record.id,
                ordinal: h.record.ordinal,
                score: h.score,
                source_url: None,
                source_date: None,
            })
            .collect();
        for (i, p) in web_pages.iter().enumerate() {
            let snippet = best_passages(&p.text, question, 1500);
            let sentence = best_sentence(&snippet, question);
            citations.push(Citation {
                doc_title: Some(p.title.clone()),
                stale: None,
                sentence,
                snippet,
                doc_id: format!("web:{i}"),
                chunk_id: p.url.clone(),
                ordinal: i,
                score: 0.7,
                source_url: Some(p.url.clone()),
                source_date: p.date.clone(),
            });
        }
        for (i, s) in extra.iter().enumerate() {
            citations.push(Citation {
                doc_title: Some(s.title.clone()),
                stale: None,
                sentence: best_sentence(&s.text, question),
                snippet: s.text.chars().take(1500).collect(),
                doc_id: format!("private:{i}"),
                chunk_id: format!("private:{i}"),
                ordinal: i,
                score: s.score,
                source_url: None,
                source_date: None,
            });
        }
        let mut system = self.answer_system();
        if !web_pages.is_empty() {
            system.push_str(WEB_FORMAT_ADDENDUM);
        }
        Ok((system, user_prompt, citations, scope_note))
    }

    async fn query_inner(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        web_pages: &[FetchedPage],
        scope: Option<DocScope>,
        extra: &[ExtraSource],
        strict: bool,
        llm_override: Option<Arc<dyn LlmClient>>,
    ) -> Result<QueryAnswer> {
        // Verified annotations + semantic answer cache — corpus-only,
        // grounded work only. Web answers go stale and private answers
        // are confidential: neither tier is ever looked up or stored for
        // them. Bypass has no grounding to reuse. The verified tier is
        // checked first and always wins.
        let rails_ok = web_pages.is_empty() && extra.is_empty() && mode != QueryMode::Bypass;
        let mut cache_slot: Option<(Vec<f32>, String)> = None;
        if rails_ok {
            if let Ok(qvec) = self
                .embeddings
                .embed(&[question.to_string()])
                .await
                .map(|mut v| v.remove(0))
            {
                let fp = self.corpus_fingerprint().await;
                if let Some(hit) = self.annotation_lookup(&qvec, mode, scope.as_ref(), &fp) {
                    info!("verified annotation served (zero LLM cost)");
                    return Ok(hit);
                }
                if self.answer_cache_enabled {
                    let key = self.answer_scope_key(mode, top_k, scope.as_ref(), &fp);
                    if let Some(hit) = self.answer_cache.lookup(&qvec, &key) {
                        info!("semantic answer cache hit (zero LLM cost)");
                        return Ok(hit);
                    }
                    cache_slot = Some((qvec, key));
                }
            }
        }
        let (system, user_prompt, citations, scope_note) =
            self.prepare_context(question, mode, top_k, web_pages, scope, extra, strict).await?;
        // Only this final call ever sees confidential (extra) source
        // text, so only it honors the private-LLM override.
        let gen_llm = llm_override.as_ref().unwrap_or(&self.llm);
        let answer = gen_llm.complete(&system, &user_prompt).await?;
        // Citation-integrity audit: strip markers the cited source does
        // not support, before the answer (or its cached copy) ships.
        let audit_sources: Vec<(String, String)> = citations
            .iter()
            .map(|c| (c.doc_id.clone(), c.snippet.clone()))
            .collect();
        let (answer, audit) = crate::audit::audit_answer(&answer, &audit_sources);
        if audit.stripped > 0 {
            warn!(checked = audit.checked, stripped = audit.stripped, "citation audit stripped unsupported markers");
        }
        info!(mode = ?mode, citations = citations.len(), web_pages = web_pages.len(), "answered query");
        let out = QueryAnswer {
            answer,
            citations,
            mode,
            scope_note,
            verified: false,
            citation_audit: Some(audit),
        };
        if let Some((qvec, key)) = cache_slot {
            if !out.citations.is_empty() {
                self.answer_cache.store(qvec, key, out.clone());
            }
        }
        Ok(out)
    }

    /// Streaming query: emits retrieval status, then the citation set,
    /// then answer deltas as the LLM generates them, then the full answer.
    /// The channel closing without `Done` signals failure to the client.
    pub async fn query_stream(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        web: bool,
        web_results: usize,
        tx: tokio::sync::mpsc::UnboundedSender<QueryEvent>,
    ) -> Result<()> {
        self.query_stream_scoped(question, mode, top_k, web, web_results, None, &[], false, None, tx)
            .await
    }

    /// Streaming query scoped to a set of documents (a brief), optionally
    /// blended with the owner's confidential sources.
    #[allow(clippy::too_many_arguments)]
    pub async fn query_stream_scoped(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        web: bool,
        web_results: usize,
        scope_docs: Option<Vec<String>>,
        extra: &[ExtraSource],
        strict: bool,
        llm_override: Option<Arc<dyn LlmClient>>,
        tx: tokio::sync::mpsc::UnboundedSender<QueryEvent>,
    ) -> Result<()> {
        let _ = tx.send(QueryEvent::Status {
            message: if web {
                "Searching the vault and the web…".to_string()
            } else {
                "Searching the vault…".to_string()
            },
        });
        let pages = if web {
            self.gather_web(question, web_results).await
        } else {
            Vec::new()
        };
        let scope: Option<DocScope> = scope_docs.map(|docs| docs.into_iter().collect());
        // Verified annotations + semantic answer cache — same rails as
        // the blocking path (never web, never private, never bypass).
        // A hit replays instantly: sources first, then the answer in
        // word chunks, then Done. Verified beats automatic.
        let mut cache_slot: Option<(Vec<f32>, String)> = None;
        if !web && extra.is_empty() && mode != QueryMode::Bypass {
            if let Ok(qvec) = self
                .embeddings
                .embed(&[question.to_string()])
                .await
                .map(|mut v| v.remove(0))
            {
                let fp = self.corpus_fingerprint().await;
                if let Some(hit) = self.annotation_lookup(&qvec, mode, scope.as_ref(), &fp) {
                    let _ = tx.send(QueryEvent::Status {
                        message: "Serving a human-verified answer for this question.".to_string(),
                    });
                    let _ = tx.send(QueryEvent::Sources {
                        citations: hit.citations.clone(),
                        mode,
                        verified: true,
                    });
                    for chunk in hit.answer.split_inclusive(' ') {
                        if !chunk.is_empty() {
                            let _ = tx.send(QueryEvent::Delta { text: chunk.to_string() });
                        }
                    }
                    let _ = tx.send(QueryEvent::Done { answer: hit.answer, audit: None });
                    return Ok(());
                }
                if self.answer_cache_enabled {
                    let key = self.answer_scope_key(mode, top_k, scope.as_ref(), &fp);
                    if let Some(hit) = self.answer_cache.lookup(&qvec, &key) {
                        let _ = tx.send(QueryEvent::Status {
                            message: "Matched an earlier answer — serving it instantly.".to_string(),
                        });
                        let _ = tx.send(QueryEvent::Sources {
                            citations: hit.citations.clone(),
                            mode,
                            verified: false,
                        });
                        for chunk in hit.answer.split_inclusive(' ') {
                            if !chunk.is_empty() {
                                let _ = tx.send(QueryEvent::Delta { text: chunk.to_string() });
                            }
                        }
                        let _ = tx.send(QueryEvent::Done {
                            answer: hit.answer.clone(),
                            audit: hit.citation_audit,
                        });
                        return Ok(());
                    }
                    cache_slot = Some((qvec, key));
                }
            }
        }
        // Retrieval + rerank can take seconds on a loaded box; keep the
        // status line moving while it runs so the wait never reads as a
        // frozen "connecting" — the user sees work happening throughout.
        let prep = self.prepare_context(question, mode, top_k, &pages, scope, extra, strict);
        tokio::pin!(prep);
        let mut beat = tokio::time::interval(std::time::Duration::from_millis(1600));
        beat.tick().await; // consume the immediate first tick
        let heartbeats = [
            "Ranking the most relevant passages…",
            "Checking the knowledge graph…",
            "Weighing the evidence…",
        ];
        let mut beat_no = 0usize;
        let (system, user_prompt, citations, scope_note) = loop {
            tokio::select! {
                done = &mut prep => break done?,
                _ = beat.tick() => {
                    let _ = tx.send(QueryEvent::Status {
                        message: heartbeats[beat_no % heartbeats.len()].to_string(),
                    });
                    beat_no += 1;
                }
            }
        };
        if let Some(note) = &scope_note {
            let _ = tx.send(QueryEvent::Status { message: note.clone() });
        }
        let _ = tx.send(QueryEvent::Sources { citations: citations.clone(), mode, verified: false });
        let _ = tx.send(QueryEvent::Status { message: "Drafting your answer…".to_string() });

        let sink_tx = tx.clone();
        let mut sink = move |text: &str| {
            let _ = sink_tx.send(QueryEvent::Delta { text: text.to_string() });
        };
        // Only this generation call sees confidential (extra) source
        // text, so only it honors the private-LLM override.
        let gen_llm = llm_override.as_ref().unwrap_or(&self.llm);
        let answer = gen_llm.complete_stream(&system, &user_prompt, &mut sink).await?;
        // Same integrity audit as the blocking path. Deltas already
        // streamed raw; the Done event carries the audited answer,
        // which is what the client persists and re-renders.
        let audit_sources: Vec<(String, String)> = citations
            .iter()
            .map(|c| (c.doc_id.clone(), c.snippet.clone()))
            .collect();
        let (answer, audit) = crate::audit::audit_answer(&answer, &audit_sources);
        if let Some((qvec, key)) = cache_slot {
            if !citations.is_empty() {
                self.answer_cache.store(
                    qvec,
                    key,
                    QueryAnswer {
                        answer: answer.clone(),
                        citations,
                        mode,
                        scope_note,
                        verified: false,
                        citation_audit: Some(audit),
                    },
                );
            }
        }
        let _ = tx.send(QueryEvent::Done { answer, audit: Some(audit) });
        Ok(())
    }

    /// Retrieve chunks + graph context for a mode. When `scope` is set,
    /// chunk hits and graph context are limited to the scoped documents.
    pub(crate) async fn retrieve(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        scope: Option<&DocScope>,
    ) -> Result<(Vec<ScoredChunk>, String)> {
        let mut hits: Vec<ScoredChunk> = Vec::new();
        let mut graph_context = String::new();

        // The three legs are independent — run them concurrently so the
        // local leg's entity-extraction LLM call overlaps the embedding +
        // vector search instead of queueing behind them.
        let vec_leg = async {
            if matches!(mode, QueryMode::Naive | QueryMode::Mix) {
                let qvec = self.embeddings.embed(&[question.to_string()]).await?.remove(0);
                // Search wide, then keep searchable units only: context-
                // only parents (pc=parent) keep embeddings for storage
                // parity but are not match units — their children are.
                let wide = (top_k * 4).clamp(top_k, 200);
                let raw = self.store.search(&qvec, wide).await?;
                let mut vec_hits: Vec<ScoredChunk> =
                    raw.into_iter().filter(|h| is_searchable(&h.record)).collect();
                vec_hits.truncate(top_k);
                self.fuse_keyword_leg(question, vec_hits, top_k).await
            } else {
                Ok(Vec::new())
            }
        };
        let local_leg = async {
            if matches!(mode, QueryMode::Local | QueryMode::Hybrid | QueryMode::Mix) {
                self.local_retrieve(question, top_k, scope).await
            } else {
                Ok((Vec::new(), String::new()))
            }
        };
        let global_leg = async {
            if matches!(mode, QueryMode::Global | QueryMode::Hybrid | QueryMode::Mix) {
                self.global_retrieve(top_k, scope).await
            } else {
                Ok((Vec::new(), String::new()))
            }
        };
        let (vec_res, local_res, global_res) = tokio::join!(vec_leg, local_leg, global_leg);
        hits.extend(vec_res?);
        let (local_hits, local_ctx) = local_res?;
        hits.extend(local_hits);
        if !local_ctx.is_empty() {
            graph_context.push_str(&local_ctx);
        }
        let (global_hits, global_ctx) = global_res?;
        hits.extend(global_hits);
        if !graph_context.is_empty() && !global_ctx.is_empty() {
            graph_context.push_str("\n\n");
        }
        graph_context.push_str(&global_ctx);

        dedup_hits(&mut hits);
        if let Some(scope) = scope {
            hits.retain(|h| scope.contains(&h.record.doc_id));
        }
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(top_k);
        Ok((hits, graph_context))
    }

    /// Replace child-chunk hits with their parent records (score kept),
    /// collapsing siblings of one parent to the best-scored child. Hits
    /// that are not children pass through untouched.
    async fn expand_parents(&self, hits: Vec<ScoredChunk>) -> Vec<ScoredChunk> {
        let parent_ids: Vec<String> =
            hits.iter().filter_map(|h| parent_id(&h.record).map(|s| s.to_string())).collect();
        if parent_ids.is_empty() {
            return hits;
        }
        let parents = self.store.get_by_ids(&parent_ids).await.unwrap_or_default();
        let by_id: HashMap<&str, &ChunkRecord> =
            parents.iter().map(|r| (r.id.as_str(), r)).collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<ScoredChunk> = Vec::with_capacity(hits.len());
        for h in hits {
            let record = match parent_id(&h.record) {
                Some(pid) => match by_id.get(pid) {
                    Some(p) => (*p).clone(),
                    None => h.record.clone(),
                },
                None => h.record.clone(),
            };
            if seen.insert(record.id.clone()) {
                out.push(ScoredChunk { record, score: h.score });
            }
        }
        out
    }

    /// Hybrid fusion: fold the BM25 keyword ranking into the vector
    /// ranking so exact-term chunks the vector leg ranked low (or missed)
    /// still reach the rerank pool. Disable with `LEGAL_RAG_BM25=0`.
    async fn fuse_keyword_leg(
        &self,
        question: &str,
        vec_hits: Vec<ScoredChunk>,
        top_k: usize,
    ) -> Result<Vec<ScoredChunk>> {
        if std::env::var("LEGAL_RAG_BM25").map(|v| v == "0").unwrap_or(false) {
            return Ok(vec_hits);
        }
        let fp = self.corpus_fingerprint().await;
        let Some(index) = self.bm25_index(&fp).await else {
            return Ok(vec_hits);
        };
        if index.is_empty() {
            return Ok(vec_hits);
        }
        let kw = index.search(question, top_k);
        if kw.is_empty() {
            return Ok(vec_hits);
        }
        let vec_ids: HashSet<&str> = vec_hits.iter().map(|h| h.record.id.as_str()).collect();
        let missing: Vec<String> = kw
            .iter()
            .map(|(id, _)| id.clone())
            .filter(|id| !vec_ids.contains(id.as_str()))
            .collect();
        let extra: Vec<ChunkRecord> = if missing.is_empty() {
            Vec::new()
        } else {
            self.store.get_by_ids(&missing).await.unwrap_or_default()
        };
        Ok(fuse_rrf(vec_hits, &kw, extra, top_k))
    }

    /// Local mode: entities in the query → their graph neighborhoods.
    async fn local_retrieve(
        &self,
        question: &str,
        top_k: usize,
        scope: Option<&DocScope>,
    ) -> Result<(Vec<ScoredChunk>, String)> {
        let query_entities = extract_query_entities(self.llm.as_ref(), question).await;
        if query_entities.is_empty() {
            return Ok((Vec::new(), String::new()));
        }
        let all_entities = self.graph.all_entities().await?;
        let mut matched: Vec<&Entity> = Vec::new();
        for qe in &query_entities {
            let ql = qe.to_lowercase();
            for entity in &all_entities {
                let el = entity.name.to_lowercase();
                if el == ql || el.contains(&ql) || ql.contains(&el) {
                    if !matched.iter().any(|m| m.name == entity.name) {
                        matched.push(entity);
                    }
                }
            }
        }

        // Gather (entity, neighbours) pairs and the union of their chunks.
        let mut pairs: Vec<(&Entity, Vec<(Entity, Relation)>)> = Vec::new();
        let mut chunk_ids: Vec<String> = Vec::new();
        for entity in matched.iter().take(10) {
            let neighbors = self.graph.neighbors(&entity.name).await?;
            chunk_ids.extend(entity.chunk_ids.iter().cloned());
            for (neighbor, _) in &neighbors {
                chunk_ids.extend(neighbor.chunk_ids.iter().cloned());
            }
            pairs.push((*entity, neighbors));
        }

        let records = self.store.get_by_ids(&chunk_ids).await?;
        let records: Vec<_> = match scope {
            Some(s) => records
                .into_iter()
                .filter(|r| s.contains(&r.doc_id))
                .collect(),
            None => records,
        };
        // Graph context lines only for entities that touch a kept chunk.
        let kept: std::collections::HashSet<&str> =
            records.iter().map(|r| r.id.as_str()).collect();
        let touches = |ids: &[String]| ids.iter().any(|id| kept.contains(id.as_str()));
        let mut ctx_lines = Vec::new();
        for (entity, neighbors) in &pairs {
            if scope.is_some() && !touches(&entity.chunk_ids) {
                continue;
            }
            ctx_lines.push(format!("Entity '{}': {}", entity.name, entity.description));
            for (neighbor, rel) in neighbors {
                if scope.is_some() && !touches(&neighbor.chunk_ids) {
                    continue;
                }
                ctx_lines.push(format!(
                    "  {} --({})--> {}: {}",
                    entity.name, rel.description, neighbor.name, neighbor.description
                ));
            }
        }

        let hits = records
            .into_iter()
            .take(top_k * 3)
            .map(|record| ScoredChunk { record, score: 0.85 })
            .collect();
        Ok((hits, ctx_lines.join("\n")))
    }

    /// Global mode: relation-level knowledge across the whole graph.
    async fn global_retrieve(
        &self,
        top_k: usize,
        scope: Option<&DocScope>,
    ) -> Result<(Vec<ScoredChunk>, String)> {
        let relations = self.graph.all_relations().await?;
        if relations.is_empty() {
            return Ok((Vec::new(), String::new()));
        }
        let mut chunk_ids: Vec<String> = Vec::new();
        for r in relations.iter().take(60) {
            chunk_ids.extend(r.chunk_ids.iter().cloned());
        }
        let records = self.store.get_by_ids(&chunk_ids).await?;
        let records: Vec<_> = match scope {
            Some(s) => records
                .into_iter()
                .filter(|r| s.contains(&r.doc_id))
                .collect(),
            None => records,
        };
        if records.is_empty() {
            return Ok((Vec::new(), String::new()));
        }
        let kept: std::collections::HashSet<&str> =
            records.iter().map(|r| r.id.as_str()).collect();
        let ctx_lines: Vec<String> = relations
            .iter()
            .take(60)
            .filter(|r| {
                scope.is_none() || r.chunk_ids.iter().any(|id| kept.contains(id.as_str()))
            })
            .map(|r: &Relation| format!("{} --({})--> {}", r.from, r.description, r.to))
            .collect();
        let hits = records
            .into_iter()
            .take(top_k * 3)
            .map(|record| ScoredChunk { record, score: 0.8 })
            .collect();
        if ctx_lines.is_empty() {
            return Ok((hits, String::new()));
        }
        Ok((hits, format!("Key relationships:\n{}", ctx_lines.join("\n"))))
    }
}

/// Extract entity names from the query via the LLM (best-effort).
async fn extract_query_entities(llm: &dyn LlmClient, question: &str) -> Vec<String> {
    let system = "Extract the named entities (people, organizations, clauses, dates, \
        legal concepts) from the user's question. Respond with ONLY a JSON array of strings.";
    let raw = match llm.complete(system, question).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "query entity extraction failed");
            return Vec::new();
        }
    };
    let t = raw.trim().trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();
    serde_json::from_str::<Vec<String>>(t).unwrap_or_default()
}

/// Build a query service over a `RagAnything`'s backends.
pub fn query_service_for(
    rag: &crate::pipeline::RagAnything,
    doc_titles: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
    web: Arc<dyn WebSearch>,
    reranker: Option<Arc<dyn Reranker>>,
) -> QueryService {
    let (embeddings, store, graph, llm) = rag.backends();
    QueryService::new(embeddings, store, graph, llm, doc_titles, web, reranker)
}

fn dedup_hits(hits: &mut Vec<ScoredChunk>) {
    let mut seen = HashSet::new();
    hits.retain(|h| seen.insert(h.record.id.clone()));
}

/// Per-document cap on the final answer context (env
/// `LEGAL_RAG_PER_DOC_CAP`, 0/unset = off): no single document
/// contributes more than N chunks. Fights hub-document saturation in
/// larger corpora — on the 2026-10-23 expanded eval corpus a few long
/// documents filled the whole pool and case documents never surfaced,
/// and raising top_k alone measured no improvement (11/20 vs 12/20).
/// Rank order is preserved; the context simply gets more diverse.
fn apply_per_doc_cap(hits: Vec<ScoredChunk>) -> Vec<ScoredChunk> {
    let cap = std::env::var("LEGAL_RAG_PER_DOC_CAP")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if cap == 0 {
        return hits;
    }
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut kept = Vec::with_capacity(hits.len());
    for h in hits {
        let c = counts.entry(h.record.doc_id.clone()).or_insert(0);
        if *c < cap {
            *c += 1;
            kept.push(h);
        }
    }
    kept
}

/// Reciprocal-rank fusion (k=60) of a vector ranking with a BM25 ranking.
/// The fused ORDER decides the candidate pool; vector hits keep their
/// cosine scores, and keyword-only hits (records supplied via `extra`)
/// inherit the cosine of the vector hit at their fused rank (× 0.98), so
/// downstream margin gates and citation scores stay on the cosine scale.
fn fuse_rrf(
    vec_hits: Vec<ScoredChunk>,
    kw_ranked: &[(String, f32)],
    extra: Vec<ChunkRecord>,
    top_k: usize,
) -> Vec<ScoredChunk> {
    const K: f32 = 60.0;
    let vec_cosines: Vec<f32> = vec_hits.iter().map(|h| h.score).collect();
    let mut rrf: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    for (rank, h) in vec_hits.iter().enumerate() {
        *rrf.entry(h.record.id.clone()).or_default() += 1.0 / (K + rank as f32 + 1.0);
    }
    for (rank, (id, _)) in kw_ranked.iter().enumerate() {
        *rrf.entry(id.clone()).or_default() += 1.0 / (K + rank as f32 + 1.0);
    }
    let mut by_id: std::collections::HashMap<String, ScoredChunk> =
        vec_hits.into_iter().map(|h| (h.record.id.clone(), h)).collect();
    let mut ids: Vec<&String> = rrf.keys().collect();
    ids.sort_by(|a, b| {
        rrf[*b].partial_cmp(&rrf[*a]).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut fused: Vec<ScoredChunk> = Vec::with_capacity(top_k);
    for (pos, id) in ids.into_iter().take(top_k).enumerate() {
        if let Some(h) = by_id.remove(id) {
            fused.push(h);
        } else if let Some(rec) = extra.iter().find(|r| &r.id == id) {
            let base = vec_cosines
                .get(pos)
                .copied()
                .unwrap_or_else(|| vec_cosines.last().copied().unwrap_or(0.3) * 0.95);
            fused.push(ScoredChunk { record: rec.clone(), score: base * 0.98 });
        }
    }
    fused
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RagAnythingConfig;
    use crate::embeddings::MockEmbeddings;
    use crate::graph::InMemoryGraphStore;
    use crate::llm::MockLlm;
    use crate::pipeline::RagAnything;
    use crate::vector_store::InMemoryVectorStore;

    async fn seeded() -> (QueryService, Arc<RagAnything>) {
        let rag = Arc::new(RagAnything::new(
            RagAnythingConfig::default(),
            Arc::new(MockEmbeddings::new(16)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            // First call: query-entity extraction; later: answers.
            Arc::new(MockLlm::new(
                r#"["Acme"]"#,
            )),
            500,
            50,
        ));
        // Index via the pipeline (text path uses a different mock response for graph extraction).
        rag.process_document_complete(
            "nda.txt",
            b"Acme Corp agrees to confidentiality. Acme shall not disclose.",
            None,
            None,
        )
        .await
        .unwrap();
        let rag2 = rag.clone();
        let svc = QueryService::new(
            rag2.backends().0,
            rag2.backends().1,
            rag2.backends().2,
            Arc::new(MockLlm::new("The answer is grounded [doc:0].")),
            Arc::new(move |id: &str| rag2.doc_title(id)),
            Arc::new(crate::web::NoopWebSearch),
            None,
        );
        (svc, rag)
    }

    #[tokio::test]
    async fn all_modes_answer() {
        let (svc, _) = seeded().await;
        for mode in [
            QueryMode::Naive,
            QueryMode::Local,
            QueryMode::Global,
            QueryMode::Hybrid,
            QueryMode::Mix,
        ] {
            let ans = svc.query("What did Acme agree to?", mode, 3).await.unwrap();
            assert!(!ans.answer.is_empty(), "mode {mode:?} gave empty answer");
            assert_eq!(ans.mode, mode);
        }
    }

    #[tokio::test]
    async fn bypass_has_no_citations() {
        let (svc, _) = seeded().await;
        let ans = svc.query("What is a contract?", QueryMode::Bypass, 3).await.unwrap();
        assert!(ans.citations.is_empty());
        assert!(!ans.answer.is_empty());
    }

    /// Embeddings that map every text to the same vector — paraphrases
    /// collide perfectly, which is what the semantic cache keys on.
    struct SameVec;
    #[async_trait::async_trait]
    impl Embeddings for SameVec {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
        fn dims(&self) -> usize {
            2
        }
    }

    /// LLM that answers differently on every call, so a cache hit is
    /// observable: the second answer must repeat the first verbatim.
    struct CountingLlm(std::sync::atomic::AtomicU32);
    #[async_trait::async_trait]
    impl LlmClient for CountingLlm {
        async fn complete(&self, _system: &str, _user: &str) -> Result<String> {
            let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            Ok(format!("Acme agrees to confidentiality (call {n}) [doc:0]"))
        }
    }

    #[tokio::test]
    async fn semantic_cache_serves_paraphrase_without_second_llm_call() {
        let (_svc, rag) = seeded().await;
        let (_emb, store, graph, _llm) = rag.backends();
        let rag2 = rag.clone();
        let svc = QueryService::new(
            Arc::new(SameVec),
            store,
            graph,
            Arc::new(CountingLlm(std::sync::atomic::AtomicU32::new(0))),
            Arc::new(move |id: &str| rag2.doc_title(id)),
            Arc::new(crate::web::NoopWebSearch),
            None,
        );
        let a = svc.query("What did Acme agree to?", QueryMode::Naive, 3).await.unwrap();
        // Different wording, same embedding → semantic hit, no new LLM call.
        let b = svc.query("Tell me Acme's commitments", QueryMode::Naive, 3).await.unwrap();
        assert_eq!(a.answer, "Acme agrees to confidentiality (call 1) [doc:0]");
        assert_eq!(b.answer, a.answer, "paraphrase must be served from the answer cache");
        assert_eq!(svc.answer_cache_stats().hits, 1);
        // A different brief scope is a different cache key → fresh call.
        let doc_id = svc.store.all_chunks().await.unwrap()[0].doc_id.clone();
        let c = svc
            .query_scoped("What did Acme agree to?", QueryMode::Naive, 3, vec![doc_id])
            .await
            .unwrap();
        assert_eq!(c.answer, "Acme agrees to confidentiality (call 2) [doc:0]");
    }

    #[tokio::test]
    async fn verified_annotation_wins_and_respects_corpus() {
        let (_svc, rag) = seeded().await;
        let (_e, store, graph, _l) = rag.backends();
        let rag2 = rag.clone();
        let svc = QueryService::new(
            Arc::new(SameVec),
            store,
            graph,
            Arc::new(CountingLlm(std::sync::atomic::AtomicU32::new(0))),
            Arc::new(move |id: &str| rag2.doc_title(id)),
            Arc::new(crate::web::NoopWebSearch),
            None,
        );
        let fp = svc.corpus_fingerprint().await;
        svc.set_annotations(vec![Annotation {
            id: "a1".into(),
            question: "What did Acme agree to?".into(),
            qvec: vec![1.0, 0.0],
            answer: "VERIFIED: Acme agreed to confidentiality.".into(),
            citations: vec![],
            scope_docs: None,
            corpus_fp: fp.clone(),
            enabled: true,
        }]);
        let a = svc.query("What did Acme agree to?", QueryMode::Naive, 3).await.unwrap();
        assert!(a.verified);
        assert_eq!(a.answer, "VERIFIED: Acme agreed to confidentiality.");
        // Corpus drift retires the annotation: it must stop serving.
        svc.set_annotations(vec![Annotation {
            id: "a1".into(),
            question: "What did Acme agree to?".into(),
            qvec: vec![1.0, 0.0],
            answer: "VERIFIED: stale.".into(),
            citations: vec![],
            scope_docs: None,
            corpus_fp: "some-other-corpus".into(),
            enabled: true,
        }]);
        let b = svc.query("What did Acme agree to?", QueryMode::Naive, 3).await.unwrap();
        assert!(!b.verified);
        assert_eq!(b.answer, "Acme agrees to confidentiality (call 1) [doc:0]", "stale annotation must not serve");
    }

    #[tokio::test]
    async fn child_hits_expand_to_parent_and_collapse() {
        let store = Arc::new(InMemoryVectorStore::new());
        let rec = |id: &str, text: &str, meta: &[(&str, &str)]| ChunkRecord {
            id: id.into(),
            doc_id: "d".into(),
            ordinal: 0,
            text: text.into(),
            embedding: vec![1.0, 0.0],
            metadata: meta.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        };
        let parent = rec("p1", "THE WHOLE SECTION TEXT", &[("pc", "parent")]);
        let child_a = rec("p1#c0", "first span", &[("pc", "child"), ("parent_id", "p1")]);
        let child_b = rec("p1#c1", "second span", &[("pc", "child"), ("parent_id", "p1")]);
        store.upsert(vec![parent, child_a.clone(), child_b.clone()]).await.unwrap();
        let svc = QueryService::new(
            Arc::new(MockEmbeddings::new(2)),
            store,
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(MockLlm::new("x")),
            Arc::new(|_: &str| None),
            Arc::new(crate::web::NoopWebSearch),
            None,
        );
        let hits = vec![
            ScoredChunk { record: child_a, score: 0.9 },
            ScoredChunk { record: child_b, score: 0.8 },
        ];
        let out = svc.expand_parents(hits).await;
        assert_eq!(out.len(), 1, "sibling children collapse to one parent hit");
        assert_eq!(out[0].record.text, "THE WHOLE SECTION TEXT");
        assert_eq!(out[0].score, 0.9, "best child score is kept");
    }

    #[test]
    fn rrf_fusion_promotes_keyword_only_chunk() {
        let rec = |id: &str| ChunkRecord {
            id: id.into(),
            doc_id: "d".into(),
            ordinal: 0,
            text: id.into(),
            embedding: vec![],
            metadata: Default::default(),
        };
        let scored = |id: &str, s: f32| ScoredChunk { record: rec(id), score: s };
        let vec_hits = vec![scored("a", 0.9), scored("b", 0.8), scored("c", 0.7)];
        // BM25 ranks d (vector-invisible) first, then c, then a.
        let kw = vec![("d".to_string(), 9.0), ("c".to_string(), 5.0), ("a".to_string(), 3.0)];
        let fused = fuse_rrf(vec_hits, &kw, vec![rec("d")], 4);
        let ids: Vec<&str> = fused.iter().map(|h| h.record.id.as_str()).collect();
        assert_eq!(ids[0], "a");
        assert_eq!(ids[1], "c");
        assert!(ids.contains(&"d"), "keyword-only chunk must enter the pool");
        let d = fused.iter().find(|h| h.record.id == "d").unwrap();
        assert!((d.score - 0.7 * 0.98).abs() < 1e-4, "imputed score stays on the cosine scale");
    }

    #[test]
    fn pinpoint_picks_the_supporting_sentence() {
        let text = "The Court heard the parties at length. \
                    Income-tax is not payable on interest due on any security of the Central Government issued tax free. \
                    The appeal is accordingly dismissed with costs.";
        let s = best_sentence(text, "When is income-tax not payable on Central Government securities?")
            .expect("a pinpoint sentence");
        assert!(s.contains("Income-tax is not payable"), "got: {s}");
    }

    #[test]
    fn pinpoint_handles_hindi_danda_sentences() {
        let text = "यह अधिनियम भारत भर में लागू होता है। धारा 86 के अंतर्गत केंद्र सरकार की प्रतिभूतियों पर आयकर देय नहीं है। यह प्रावधान 1961 से प्रभावी है।";
        let s = best_sentence(text, "धारा 86 आयकर प्रतिभूति").expect("hindi pinpoint");
        assert!(s.contains("धारा 86"), "got: {s}");
    }

    #[test]
    fn pinpoint_survives_hard_wrapped_lines() {
        // Statute text extracted from PDFs is wrapped mid-sentence; the
        // pinpoint must be the complete sentence, not a line fragment.
        let text = "Where a member of a Hindu undivided\nfamily receives income from a firm,\nno income-tax shall be payable by him\nin respect of that share of the income.";
        let s = best_sentence(text, "Is income-tax payable by a Hindu undivided family member on firm income?")
            .expect("pinpoint from wrapped text");
        assert!(s.starts_with("Where a member"), "got: {s}");
        assert!(s.ends_with('.'), "got: {s}");
        assert!(!s.contains('\n'), "got: {s}");
    }

    #[test]
    fn pinpoint_is_none_without_support() {
        let text = "The weather in Delhi remained dry throughout the week with mild winds.";
        assert!(best_sentence(text, "What does Section 86 say about income tax?").is_none());
    }
}
