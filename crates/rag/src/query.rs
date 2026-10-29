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
use crate::llm::{is_failure_output, LlmClient};
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
    /// Citator status of this authority (see `crate::citator`): the
    /// treatment a later judgment in the visible scope gave it, with
    /// the passage as provenance — or Unknown when no corpus passage
    /// says. Web / private citations are always Unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub treatment: Option<crate::citator::CitationTreatment>,
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

/// Format contract appended to the system prompt for corpus-only
/// answers: the same verdict-first shape as web answers, but citing
/// corpus sources only. Corpus answers previously got no format
/// contract at all, so their shape was whatever the model chose. The
/// web addendum is kept byte-for-byte for web answers; this one never
/// mentions [web:N], so a corpus answer is never invited to invent a
/// web citation (the audit would strip it anyway, but not inviting it
/// is cleaner). Bypass answers are untouched — they return before any
/// addendum is applied.
pub const CORPUS_FORMAT_ADDENDUM: &str = "\n\nCORPUS ANSWER FORMAT — follow exactly:\n\
## Verdict\nThe direct answer in 2-4 sentences. Every factual claim cited inline.\n\
## Analysis\nThe reasoning, organised with short sub-headings where the material warrants it. \
Cite every factual claim inline as [doc:N], attached to the sentence it supports.\n\
## Caveats\nInclude this section ONLY if sources conflict, a key source is dated, or the position may have changed since publication.\n\
Cite a source ONLY for propositions its text actually states — never for general background. \
Prefer primary sources (statutes, judgments) over commentary; when sources disagree, say so and cite both. \
One bracket per source: write [doc:1] [doc:2], not [doc:1, doc:2].";

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
    /// HyDE / multi-query expansions actually used for retrieval
    /// (empty when the question didn't trigger expansion) — surfaced
    /// so the user can see how a terse citation-style question was
    /// interpreted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expansions: Vec<String>,
}

/// Per-request Ask options (Pieces 12–13), threaded through the query
/// entry points. `Default` reproduces today's behavior exactly.
#[derive(Debug, Clone, Default)]
pub struct AskOptions {
    /// Brief-vault mode (Piece 12): the active brief's display label.
    /// When set (together with a document scope), the corpus leg is a
    /// HARD boundary — retrieval never falls back to the full vault —
    /// and every answer's scope note names the brief, so the user can
    /// always tell the answer came from that brief alone. `None` =
    /// today's behavior (soft brief scoping with a disclosed fallback).
    pub brief_vault: Option<String>,
    /// Retrieval terms produced by prompt refinement (Piece 13): one
    /// pooled query built from these terms joins the same expansion
    /// pool HyDE / multi-query uses — there is no second path.
    pub refine_terms: Vec<String>,
}

impl AskOptions {
    /// True when brief-vault hard scoping is active.
    pub fn brief_vault_on(&self) -> bool {
        self.brief_vault.is_some()
    }
}

/// Scope note for a brief-vault answer that found in-brief evidence.
pub fn brief_vault_note_hit(label: &str) -> String {
    format!("Brief vault: answered only from {label}'s documents — the full vault was not searched.")
}

/// Scope note for a brief-vault answer with no in-brief evidence: the
/// grounded refusal declares the restriction instead of widening.
pub fn brief_vault_note_empty(label: &str) -> String {
    format!(
        "Brief vault: no documents in {label} matched — nothing outside this brief was searched."
    )
}

/// The single pooled retrieval query built from refinement terms
/// (Piece 13). Terms are keywords/phrases (statutes, sections, legal
/// terms, synonyms), so a comma-joined string embeds as one topical
/// query alongside the HyDE variants.
pub fn refine_terms_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
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
        /// HyDE / multi-query expansions used for this answer's
        /// retrieval (skipped in serialization when empty).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        expansions: Vec<String>,
        /// Per-sub-agent receipts for an orchestrated run (extended
        /// Done event): each specialist's contribution, tokens and
        /// status. Empty for ordinary streamed answers.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sub_agents: Vec<crate::subagents::SubAgentReceipt>,
    },
}

pub const ANSWER_SYSTEM: &str = "You are a precise legal research assistant. Answer the user's question \
using ONLY the provided source excerpts. Cite every factual claim with the source id in brackets, \
e.g. [doc:3]. If the sources do not contain the answer, say so explicitly and do not invent details.";

const BYPASS_SYSTEM: &str = "You are a precise legal research assistant. Answer the user's question \
from your own knowledge. Be clear about what you know and what you are unsure of.";

/// Per-request citator context, threaded from the Ask API into V2
/// resolution only. Never stored on the shared QueryService (an Arc
/// serving many requests — that would race); every public query
/// method takes one, and non-HTTP callers pass `default()` (state
/// None, applicant None = today's behavior). `state` is a normalized
/// code ("MH") or None (unknown codes normalize to None at the API
/// boundary, never a crash); `applicant` is trimmed, empty = None.
#[derive(Debug, Clone, Default)]
pub struct CitatorContext {
    pub state: Option<String>,
    pub applicant: Option<String>,
}

/// How a web gather ended. The web leg is failure-isolated — Ask and
/// Research always proceed on the corpus — but never *silently*: any
/// outcome other than `Delivered` becomes a user-visible note, so a
/// corpus-only answer is never mistaken for a web-augmented one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WebOutcome {
    /// Pages were fetched and blended into the answer.
    Delivered,
    /// Search ran but returned no usable results.
    NoResults,
    /// The leg failed (engine error, sidecar crash, timeout, disabled).
    Failed(String),
}

impl WebOutcome {
    /// The user-facing note for a non-delivered outcome.
    pub(crate) fn note(&self) -> Option<String> {
        match self {
            WebOutcome::Delivered => None,
            WebOutcome::NoResults => Some(
                "Live web search found no usable sources for this question; \
                 this answer is based only on your vault."
                    .to_string(),
            ),
            WebOutcome::Failed(reason) => Some(format!(
                "Live web search was unavailable for this answer ({reason}); \
                 it is based only on your vault."
            )),
        }
    }
}

/// Pages plus the leg's outcome — `gather_web`'s return value.
#[derive(Debug, Clone)]
pub(crate) struct WebGather {
    pub pages: Vec<FetchedPage>,
    pub outcome: WebOutcome,
}

pub struct QueryService {
    embeddings: Arc<dyn Embeddings>,
    store: Arc<dyn VectorStore>,
    graph: Arc<dyn GraphStore>,
    llm: Arc<dyn LlmClient>,
    doc_titles: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
    /// Legal-validity metadata lookup by document id, pushed in by the
    /// server (defaults to a no-metadata source).
    doc_meta: std::sync::RwLock<Arc<dyn Fn(&str) -> Option<crate::pipeline::DocMeta> + Send + Sync>>,
    /// Citator treatment store, pushed in by the server (defaults to an
    /// empty store, so every citation resolves Unknown until ingest
    /// or backfill populates it).
    treatments: std::sync::RwLock<Arc<crate::citator::TreatmentStore>>,
    /// Citator V2 edge store (full-text ingest detection), pushed in
    /// by the server; defaults to an empty store. Never read when the
    /// V2 switch is off.
    v2_treatments: std::sync::RwLock<Arc<crate::citator_v2::V2EdgeStore>>,
    web: Arc<dyn WebSearch>,
    reranker: Option<Arc<dyn Reranker>>,
    /// Optional ColBERT-style late-interaction rescorer, bound by the
    /// server when LEGAL_RAG_COLBERT_URL is set (None = stage off).
    late_interactor: std::sync::RwLock<Option<Arc<dyn crate::rerank::LateInteractor>>>,
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
    bm25: std::sync::RwLock<Option<(String, Arc<Bm25Engine>)>>,
    /// Retrieval-tier results cache (Spice pattern): replays a
    /// `retrieve()` result under stale-while-revalidate semantics.
    retrieval_cache: Arc<crate::retrieval_cache::RetrievalCache>,
    /// Self-reference for background revalidation tasks, bound by the
    /// owner right after the service is wrapped in an Arc.
    weak_self: std::sync::RwLock<Option<std::sync::Weak<QueryService>>>,
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
            treatments: std::sync::RwLock::new(Arc::new(crate::citator::TreatmentStore::new())),
            v2_treatments: std::sync::RwLock::new(Arc::new(crate::citator_v2::V2EdgeStore::new())),
            web,
            reranker,
            late_interactor: std::sync::RwLock::new(None),
            rerank_candidates,
            retrieval_cache: Arc::new(crate::retrieval_cache::RetrievalCache::from_env()),
            weak_self: std::sync::RwLock::new(None),
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

    /// Bind the owning Arc so background cache revalidation can run
    /// retrieval without an owner reference cycle. Called once by the
    /// server right after construction.
    pub fn bind_self(&self, arc: &Arc<QueryService>) {
        *self.weak_self.write().expect("lock poisoned") = Some(Arc::downgrade(arc));
    }

    /// Stats for the retrieval-tier results cache.
    pub fn retrieval_cache_stats(&self) -> crate::retrieval_cache::RetrievalCacheStats {
        self.retrieval_cache.stats()
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
        let cfg = FusionConfig::from_env();
        let doc_dates = self.doc_dates_for(&vec_hits, &extra);
        let statute_docs = self.statute_docs_for(question, &vec_hits, &extra);
        let fused =
            fuse_rankings(question, vec_hits, &kw, extra, top_k, &cfg, &doc_dates, &statute_docs);
        // Production applies the metadata boost to the fused pool before
        // the reranker sees it; mirror that here so stages stay honest.
        let fused = self.apply_metadata_boost(question, fused);
        stages.push(HitStage {
            name: "fused".to_string(),
            hits: fused.iter().map(&view).collect(),
        });
        if self.reranker.is_some() {
            let ranked = self.maybe_rerank(question, fused.clone(), top_k).await?;
            stages.push(HitStage {
                name: "reranked".to_string(),
                hits: ranked.iter().map(&view).collect(),
            });
            let blended = self.apply_late_interaction(question, ranked).await;
            let ranked_ids: Vec<&str> =
                stages.last().unwrap().hits.iter().map(|h| h.chunk_id.as_str()).collect();
            if blended.iter().map(|h| h.record.id.as_str()).collect::<Vec<_>>() != ranked_ids {
                stages.push(HitStage {
                    name: "late-interaction".to_string(),
                    hits: blended.iter().map(&view).collect(),
                });
            }
        }
        Ok(HitTestReport { question: question.to_string(), stages })
    }

    /// Bind the late-interaction rescorer (server, when
    /// LEGAL_RAG_COLBERT_URL is configured).
    pub fn set_late_interactor(&self, li: Arc<dyn crate::rerank::LateInteractor>) {
        *self.late_interactor.write().unwrap() = Some(li);
    }

    pub fn late_interaction_enabled(&self) -> bool {
        self.late_interactor.read().unwrap().is_some()
    }

    /// ColBERT MaxSim blend: rescore the reranked hits at token level
    /// and blend (min-max normalized) with the existing scores.
    /// No-ops when the stage is unconfigured, has <2 hits, or the
    /// sidecar fails — the reranker's order is never made worse by an
    /// outage.
    async fn apply_late_interaction(
        &self,
        question: &str,
        hits: Vec<ScoredChunk>,
    ) -> Vec<ScoredChunk> {
        let li = self.late_interactor.read().unwrap().clone();
        let Some(li) = li else { return hits };
        if hits.len() < 2 {
            return hits;
        }
        let docs: Vec<(String, String)> = hits
            .iter()
            .map(|h| {
                (
                    h.record.id.clone(),
                    h.record.text.chars().take(4000).collect::<String>(),
                )
            })
            .collect();
        match li.maxsim_scores(question, &docs).await {
            Ok(scores) if !scores.is_empty() => {
                let weight = std::env::var("LEGAL_RAG_COLBERT_WEIGHT")
                    .ok()
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(0.5)
                    .clamp(0.0, 1.0);
                blend_maxsim(hits, &scores, weight)
            }
            Ok(_) => hits,
            Err(e) => {
                warn!(error = %e, "late interaction failed; keeping reranker order");
                hits
            }
        }
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
            expansions: Vec::new(),
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
    /// corpus state. Brief-vault mode (Piece 12) is part of the key: a
    /// hard-scoped answer (refusal, or brief-only citations) must never
    /// be served to a soft-scoped ask over the same document set, nor
    /// the reverse — the two modes can produce opposite answers to the
    /// same question over the same brief.
    fn answer_scope_key(
        &self,
        mode: QueryMode,
        top_k: usize,
        scope: Option<&DocScope>,
        corpus_fp: &str,
        ctx: &CitatorContext,
        hard_scope: bool,
    ) -> String {
        let scope_part = match scope {
            Some(docs) => {
                let mut v: Vec<&str> = docs.iter().map(|s| s.as_str()).collect();
                v.sort_unstable();
                v.join(",")
            }
            None => "all".to_string(),
        };
        // Context is part of the key: the cached answer text already
        // bakes in the first request's governing line, so a different
        // state/applicant must never replay it.
        hash_key(&[
            &format!("{mode:?}"),
            &top_k.to_string(),
            &scope_part,
            corpus_fp,
            ctx.state.as_deref().unwrap_or(""),
            ctx.applicant.as_deref().unwrap_or(""),
            if hard_scope { "brief-vault" } else { "soft" },
        ])
    }

    /// The current BM25 index, rebuilt when the corpus has changed.
    async fn bm25_index(&self, corpus_fp: &str) -> Option<Arc<Bm25Engine>> {
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
        // Auxiliary units (index-time augmentation, hyperedges) are
        // indexed UNDER their canonical chunk id, so the keyword leg
        // votes for the real chunk with the aux text's terms.
        let searchable: Vec<ChunkRecord> = chunks
            .into_iter()
            .filter(|c| is_searchable(c))
            .map(|mut c| {
                if let Some(target) = canonical_target(&c) {
                    c.id = target;
                }
                c
            })
            .collect();
        let idx = Arc::new(Bm25Engine::build(&searchable));
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

    /// Bind the shared citator treatment store (server, right after
    /// construction — the same store the ingest pipeline writes).
    pub fn set_treatment_store(&self, store: Arc<crate::citator::TreatmentStore>) {
        *self.treatments.write().expect("lock poisoned") = store;
    }

    /// Bind the shared citator V2 edge store (server, right after
    /// construction — the same store the ingest pipeline writes).
    pub fn set_v2_edge_store(&self, store: Arc<crate::citator_v2::V2EdgeStore>) {
        *self.v2_treatments.write().expect("lock poisoned") = store;
    }

    /// V2 resolution over an already-built citation list (research,
    /// orchestrated runs). `real_doc_ids` aligns with `citations` by
    /// index (None for web citations). Does NOT check the kill switch
    /// — callers gate on `citator_v2_enabled()` first, so this core is
    /// unit-testable without env manipulation. Returns the full
    /// resolution (treatments, ranked corpus order, governing line).
    pub(crate) fn v2_resolution_for(
        &self,
        citations: &[Citation],
        real_doc_ids: &[Option<String>],
        scope: Option<&DocScope>,
        ctx: &CitatorContext,
    ) -> crate::citator_v2::V2Resolution {
        let v1 = self.treatments.read().expect("lock poisoned").all();
        let stored = self.v2_treatments.read().expect("lock poisoned").all();
        let mut meta_map: std::collections::HashMap<String, crate::pipeline::DocMeta> =
            std::collections::HashMap::new();
        let mut real_to_positional: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for (c, real) in citations.iter().zip(real_doc_ids.iter()) {
            if let Some(real) = real {
                real_to_positional.insert(real.clone(), c.doc_id.clone());
                if let Some(m) = self.meta_of(real) {
                    meta_map.insert(c.doc_id.clone(), m.clone());
                    meta_map.insert(real.clone(), m);
                }
            }
        }
        crate::citator_v2::resolve_v2(citations, &v1, &stored, scope, &meta_map, ctx, &real_to_positional)
    }

    /// Governing line over citations whose real ids were remapped away
    /// (orchestrated runs: global doc:N ids). Recovers real doc ids
    /// via chunk lookup, then resolves. None when the switch is off or
    /// no authority governs — never invented.
    pub(crate) async fn governing_line_for_citations(
        &self,
        citations: &[Citation],
        scope: Option<&DocScope>,
        ctx: &CitatorContext,
    ) -> Option<String> {
        if !crate::citator_v2::citator_v2_enabled() {
            return None;
        }
        let chunk_ids: Vec<String> = citations
            .iter()
            .filter(|c| c.doc_id.starts_with("doc:"))
            .map(|c| c.chunk_id.clone())
            .collect();
        let records = self.store.get_by_ids(&chunk_ids).await.unwrap_or_default();
        let by_chunk: std::collections::HashMap<&str, &str> = records
            .iter()
            .map(|r| (r.id.as_str(), r.doc_id.as_str()))
            .collect();
        let real_doc_ids: Vec<Option<String>> = citations
            .iter()
            .map(|c| {
                if c.doc_id.starts_with("doc:") {
                    by_chunk.get(c.chunk_id.as_str()).map(|d| d.to_string())
                } else {
                    None
                }
            })
            .collect();
        let res = self.v2_resolution_for(citations, &real_doc_ids, scope, ctx);
        res.governing_line
    }

    /// Re-resolve every corpus citation's treatment against the
    /// current edge set. Used for cache hits (edges may have landed
    /// after the answer was cached, e.g. via backfill) — verified
    /// annotations are never rewritten.
    fn refresh_treatments(&self, answer: &mut QueryAnswer, scope: Option<&DocScope>, ctx: &CitatorContext) {
        if answer.verified {
            return;
        }
        let effective: Option<DocScope> = match &answer.scope_note {
            Some(note) if note.contains("answered from the full vault") => None,
            _ => scope.cloned(),
        };
        let edges = self.treatments.read().expect("lock poisoned").all();
        for c in answer.citations.iter_mut() {
            if c.doc_id.starts_with("doc:") {
                c.treatment = Some(crate::citator::resolve_treatment(
                    c.doc_title.as_deref(),
                    &edges,
                    effective.as_ref(),
                ));
            } else {
                c.treatment = Some(crate::citator::CitationTreatment::unknown());
            }
        }
        // V2 status refresh only (cache path): statuses may change, but
        // citation order never moves — the cached text's [doc:N] markers
        // already fixed the order.
        if crate::citator_v2::citator_v2_enabled() {
            let edges = self.treatments.read().expect("lock poisoned").all();
            let stored_v2 = self.v2_treatments.read().expect("lock poisoned").all();
            // Cached citations lost their real document ids when they
            // were built (positional "doc:N" only), so refresh falls
            // back to title inference; fresh queries use stored
            // metadata via prepare_context.
            let meta_empty: std::collections::HashMap<String, crate::pipeline::DocMeta> =
                std::collections::HashMap::new();
            let map_empty: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let res = crate::citator_v2::resolve_v2(&answer.citations, &edges, &stored_v2, effective.as_ref(), &meta_empty, ctx, &map_empty);
            let by_id: std::collections::HashMap<&str, crate::citator::CitationTreatment> = res
                .treatments
                .iter()
                .map(|(id, t)| (id.as_str(), t.clone()))
                .collect();
            for c in answer.citations.iter_mut() {
                if let Some(t) = by_id.get(c.doc_id.as_str()) {
                    c.treatment = Some(t.clone());
                }
            }
        }
    }

    /// Staleness note for one document, when it is known superseded.
    pub(crate) fn stale_of(&self, doc_id: &str) -> Option<String> {
        let f = self.doc_meta.read().expect("lock poisoned").clone();
        f(doc_id).and_then(|m| m.stale_note())
    }

    /// Legal-validity metadata for one document, when a source is wired.
    fn meta_of(&self, doc_id: &str) -> Option<crate::pipeline::DocMeta> {
        let f = self.doc_meta.read().expect("lock poisoned").clone();
        f(doc_id)
    }

    /// Documents in a candidate set whose titles match a statute the
    /// query names (the fusion statute term's target set).
    fn statute_docs_for(
        &self,
        question: &str,
        hits: &[ScoredChunk],
        extra: &[ChunkRecord],
    ) -> std::collections::HashSet<String> {
        let mentioned = statute_mentions(&question.to_lowercase());
        if mentioned.is_empty() {
            return std::collections::HashSet::new();
        }
        let mut out = std::collections::HashSet::new();
        for doc_id in hits
            .iter()
            .map(|h| &h.record.doc_id)
            .chain(extra.iter().map(|r| &r.doc_id))
        {
            if out.contains(doc_id) {
                continue;
            }
            if let Some(title) = (self.doc_titles)(doc_id) {
                let t = title.to_lowercase();
                if mentioned.iter().any(|m| title_matches_statute(&t, m)) {
                    out.insert(doc_id.clone());
                }
            }
        }
        out
    }

    /// doc_id → effective_from, for the documents in a candidate set
    /// (the recency term of the fusion suite reads dates per document).
    fn doc_dates_for(
        &self,
        hits: &[ScoredChunk],
        extra: &[ChunkRecord],
    ) -> std::collections::HashMap<String, Option<String>> {
        let mut out: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        for doc_id in hits
            .iter()
            .map(|h| &h.record.doc_id)
            .chain(extra.iter().map(|r| &r.doc_id))
        {
            if !out.contains_key(doc_id) {
                out.insert(
                    doc_id.clone(),
                    self.meta_of(doc_id).and_then(|m| m.effective_from),
                );
            }
        }
        out
    }

    /// Metadata-match boost (Haystack's MetaFieldRanker pattern, native):
    /// when the query names a statute or jurisdiction, chunks from the
    /// matching documents get a score multiplier, so the correct Act's
    /// section beats a same-numbered section of the wrong Act into the
    /// rerank pool. Gated by `LEGAL_RAG_META_BOOST` (set to `1` to
    /// enable; factors via `LEGAL_RAG_META_BOOST_STATUTE` /
    /// `LEGAL_RAG_META_BOOST_JURISDICTION`).
    fn apply_metadata_boost(&self, question: &str, mut hits: Vec<ScoredChunk>) -> Vec<ScoredChunk> {
        if std::env::var("LEGAL_RAG_META_BOOST").map(|v| v != "1").unwrap_or(true) {
            return hits;
        }
        let q = question.to_lowercase();
        let mentioned = statute_mentions(&q);
        let statute_factor = std::env::var("LEGAL_RAG_META_BOOST_STATUTE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(1.35)
            .clamp(1.0, 3.0);
        let jurisdiction_factor = std::env::var("LEGAL_RAG_META_BOOST_JURISDICTION")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(1.15)
            .clamp(1.0, 3.0);
        if mentioned.is_empty() && jurisdiction_factor <= 1.0 {
            return hits;
        }
        let mut boosted = 0usize;
        for h in hits.iter_mut() {
            let mut factor: f32 = 1.0;
            if !mentioned.is_empty() {
                if let Some(title) = (self.doc_titles)(&h.record.doc_id) {
                    let t = title.to_lowercase();
                    if mentioned.iter().any(|m| title_matches_statute(&t, m)) {
                        factor *= statute_factor;
                    }
                }
            }
            if let Some(meta) = self.meta_of(&h.record.doc_id) {
                if let Some(j) = &meta.jurisdiction {
                    if jurisdiction_mentioned(&q, j) {
                        factor *= jurisdiction_factor;
                    }
                }
            }
            if factor > 1.0 {
                h.score *= factor.min(1.6);
                boosted += 1;
            }
        }
        if boosted > 0 {
            hits.sort_by(|a, b| {
                b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
            });
            info!(boosted, "metadata boost applied");
        }
        hits
    }

    /// Run one query end to end — like `aquery`.
    pub async fn query(&self, question: &str, mode: QueryMode, top_k: usize, ctx: CitatorContext) -> Result<QueryAnswer> {
        self.query_inner(question, mode, top_k, &[], None, &[], false, None, &ctx, &AskOptions::default()).await
    }

    /// Query with per-request Ask options (Pieces 12–13): optional
    /// document scope, hard-boundary flag, brief-vault mode and
    /// refinement terms. This is the Ask route's plain (non-web,
    /// non-private) entry; the older scoped wrappers delegate here.
    pub async fn query_opts(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        scope_docs: Option<Vec<String>>,
        strict: bool,
        opts: &AskOptions,
        ctx: CitatorContext,
    ) -> Result<QueryAnswer> {
        let scope: Option<DocScope> = scope_docs.map(|docs| docs.into_iter().collect());
        self.query_inner(question, mode, top_k, &[], scope, &[], strict, None, &ctx, opts).await
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
        ctx: CitatorContext,
        opts: &AskOptions,
    ) -> Result<QueryAnswer> {
        let scope: Option<DocScope> = scope_docs.map(|docs| docs.into_iter().collect());
        self.query_inner(question, mode, top_k, &[], scope, &extra, strict, llm_override, &ctx, opts).await
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
        ctx: CitatorContext,
    ) -> Result<QueryAnswer> {
        let scope: DocScope = scope_docs.into_iter().collect();
        self.query_inner(question, mode, top_k, &[], Some(scope), &[], false, None, &ctx, &AskOptions::default()).await
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
        ctx: CitatorContext,
    ) -> Result<QueryAnswer> {
        let scope: DocScope = scope_docs.into_iter().collect();
        self.query_inner(question, mode, top_k, &[], Some(scope), &[], true, None, &ctx, &AskOptions::default()).await
    }

    /// Query with live web augmentation: local retrieval plus stealth-fetched
    /// web pages, cited as `[web:N]` alongside corpus `[doc:N]` citations.
    /// The web leg is failure-isolated: whatever happens to it, the
    /// corpus query still runs, and when the leg did not deliver the
    /// answer carries an explicit note saying so (scope_note).
    pub async fn query_with_web(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        web_results: usize,
        scope_docs: Option<Vec<String>>,
        strict: bool,
        ctx: CitatorContext,
        opts: &AskOptions,
    ) -> Result<QueryAnswer> {
        let gather = self.gather_web(question, web_results).await;
        let scope: Option<DocScope> = scope_docs.map(|docs| docs.into_iter().collect());
        let mut ans = self
            .query_inner(question, mode, top_k, &gather.pages, scope, &[], strict, None, &ctx, opts)
            .await?;
        if let Some(note) = gather.outcome.note() {
            ans.scope_note = Some(match ans.scope_note.take() {
                Some(existing) => format!("{existing} {note}"),
                None => note,
            });
        }
        Ok(ans)
    }

    /// Fetch + extract top web pages for a question. The leg is
    /// failure-isolated (it never returns Err) but never silent: the
    /// returned [`WebGather`] carries the outcome so callers can tell
    /// the user when an answer is corpus-only.
    ///
    /// THE 502, PINNED (2026-10-29): the web leg itself never produced
    /// the bare 502 users saw on web-mode Ask. This function always
    /// swallowed search errors and returned an empty vec, after which
    /// the corpus query ran and either succeeded (silently corpus-only,
    /// no note) or failed at a LATER stage — and any `Error::Upstream`
    /// from that stage (embeddings/LLM: the Cloudflare 429 neuron wall
    /// in prod, bridge failures on the test instance) is mapped to a
    /// bare HTTP 502 by the server's error type. Web mode made it the
    /// visible failure because the dead search leg first burned
    /// 60–150 s returning nothing (throttled engines, a Bing parser
    /// broken by a markup change, a Firefox fallback that got the same
    /// block page), so the user paid full latency for zero web value
    /// and then met the next flaky stage with no fallback and no
    /// explanation. The fixes are the three parts of this change: a
    /// search that actually returns (sidecar rebuild), strict time
    /// budgets here, and the outcome note surfaced to the user.
    ///
    /// Fetching runs in two waves: the top-3 candidates (primary
    /// sources — court PDFs are slow under load) get a protected wave
    /// with a full budget, then the rest of the window races to fill
    /// the remaining slots, keeping the first `max_results` successes
    /// in ranked order — one bot-guarded site can neither stall the
    /// leg nor sink it (no fallback candidates).
    pub(crate) async fn gather_web(&self, question: &str, max_results: usize) -> WebGather {
        // Search stage: hard 60s cap (the sidecar self-limits to ~40s;
        // this guards a hung sidecar process). The provider's own
        // 120s process timeout remains the outer bound for fetches.
        let results = match tokio::time::timeout(
            std::time::Duration::from_secs(60),
            self.web.search(question, max_results),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                warn!(stage = "search", error = %e, "web leg failed");
                return WebGather {
                    pages: Vec::new(),
                    outcome: WebOutcome::Failed(e.to_string()),
                };
            }
            Err(_) => {
                warn!(stage = "search", "web leg failed: search timed out after 60s");
                return WebGather {
                    pages: Vec::new(),
                    outcome: WebOutcome::Failed("search timed out".to_string()),
                };
            }
        };
        if results.is_empty() {
            info!(stage = "search", "web leg: search returned no results");
            return WebGather {
                pages: Vec::new(),
                outcome: WebOutcome::NoResults,
            };
        }
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
                tokio::time::Instant::now() + std::time::Duration::from_secs(40);
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
                    || elapsed > std::time::Duration::from_secs(15)
                {
                    set.abort_all();
                    break;
                }
            }
            if elapsed > std::time::Duration::from_secs(35) && !ok.is_empty() {
                set.abort_all();
                break;
            }
            if elapsed > std::time::Duration::from_secs(60) {
                // Absolute cap: even with zero successes the leg must
                // end — each fetch is bounded by the sidecar timeout,
                // but the leg's own budget is stricter.
                set.abort_all();
                break;
            }
        }
        ok.sort_by_key(|(i, _)| *i);
        let pages: Vec<FetchedPage> =
            ok.into_iter().map(|(_, p)| p).take(max_results).collect();
        if pages.is_empty() {
            warn!(
                stage = "fetch",
                candidates = window,
                "web leg failed: no candidate page could be fetched"
            );
            return WebGather {
                pages,
                outcome: WebOutcome::Failed(format!(
                    "none of the {window} candidate pages could be fetched"
                )),
            };
        }
        info!(stage = "fetch", pages = pages.len(), "web pages gathered");
        WebGather {
            pages,
            outcome: WebOutcome::Delivered,
        }
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
        let scored = match reranker.rerank_scored(question, &docs, top_k).await {
            Ok(o) if !o.is_empty() => o,
            _ => {
                warn!("rerank failed; keeping retrieval order");
                return Ok(hits.into_iter().take(top_k).collect());
            }
        };
        let order: Vec<usize> = scored.iter().map(|(i, _)| *i).collect();
        let mut ranked: Vec<ScoredChunk> = Vec::with_capacity(top_k);
        let mut ranked_scores: Vec<f32> = Vec::with_capacity(top_k);
        for &(i, s) in &scored {
            if let Some(h) = hits.get(i) {
                ranked.push(h.clone());
                ranked_scores.push(s);
            }
        }
        // Score-gap cutoff (Verba's Autocut pattern): when the reranker's
        // own scores fall off a cliff inside the kept set, padding the
        // context out to a fixed top_k only adds noise. Gated by
        // LEGAL_RAG_AUTOCUT=1; gap threshold LEGAL_RAG_AUTOCUT_GAP.
        let ranked = apply_score_gap_cutoff(ranked, &ranked_scores);
        let mut ranked = ranked;
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
        ctx: &CitatorContext,
        opts: &AskOptions,
    ) -> Result<(String, String, Vec<Citation>, Option<String>, Vec<String>, Option<String>)> {
        let top_k = top_k.clamp(1, 50);

        if mode == QueryMode::Bypass {
            return Ok((
                BYPASS_SYSTEM.to_string(),
                format!("Question: {question}"),
                Vec::new(),
                None,
                Vec::new(),
                None,
            ));
        }

        // HyDE / multi-query expansion (Tier-2): terse citation-style
        // questions embed poorly, so generate a hypothetical answer
        // passage + paraphrase variants and pool their searches.
        // Env gate LEGAL_RAG_HYDE=0 disables; failures degrade to the
        // bare question.
        let mut expansions: Vec<String> = if hyde_trigger(question) {
            self.expand_query(question).await
        } else {
            Vec::new()
        };
        // Refinement terms (Piece 13) join the SAME pool as one extra
        // query — the hybrid mix: the user sees the refined wording,
        // retrieval additionally searches for the terms it names.
        if !opts.refine_terms.is_empty() {
            let pooled = refine_terms_query(&opts.refine_terms);
            if !pooled.is_empty() && !expansions.contains(&pooled) {
                expansions.push(pooled);
            }
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
        let (mut hits, mut graph_context) = self
            .retrieve_expanded(question, mode, fetch_k, scope.as_ref(), &expansions)
            .await?;
        let mut scope_note = None;
        // Citator scope follows retrieval scope exactly: a fallback to
        // the full vault widens treatment evidence with it (the answer
        // itself is full-vault then); otherwise only in-scope treating
        // judgments count — never another brief's.
        let mut fell_back = false;
        if scope.is_some() && hits.is_empty() && graph_context.is_empty() {
            if let Some(label) = &opts.brief_vault {
                // Brief-vault mode (Piece 12): the brief is a hard
                // boundary BY THE USER'S EXPLICIT CHOICE — there is no
                // fallback to widen into. The answer proceeds with no
                // corpus sources and refuses, grounded, and the note
                // states the restriction. (Web pages, when the web leg
                // is on, are unaffected: they are not vault documents.)
                scope_note = Some(brief_vault_note_empty(label));
            } else if strict {
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
                fell_back = true;
                scope_note =
                    Some("No documents in this brief matched — answered from the full vault.".to_string());
            }
        }
        // Brief-vault mode declares itself on EVERY answer, not just
        // refusals: the user must always be able to tell the answer
        // came from the brief alone.
        if scope_note.is_none() {
            if let Some(label) = &opts.brief_vault {
                if scope.is_some() {
                    scope_note = Some(brief_vault_note_hit(label));
                }
            }
        }
        let hits = self.apply_metadata_boost(question, hits);
        let hits = self.maybe_rerank(question, hits, top_k).await?;
        // ColBERT late interaction (when configured): token-level
        // MaxSim blend over the reranker's verdict.
        let hits = self.apply_late_interaction(question, hits).await;
        let hits = apply_per_doc_cap(hits);
        // Parent-child: the reranker judged the precise child spans; the
        // LLM now gets each hit's parent — the whole section — as
        // context, and citations point at the parent.
        let hits = self.expand_parents(hits).await;

        let mut context_parts = Vec::new();
        if !graph_context.is_empty() {
            context_parts.push(format!("Knowledge graph context:\n{graph_context}"));
        }
        let chunks_text = litm_order(hits.len())
            .into_iter()
            .map(|i| {
                let h = &hits[i];
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

        // Positional "doc:N" ids discard the real document id; keep the
        // mapping so V2 resolution can look up stored metadata by real id.
        let real_doc_ids: Vec<String> =
            hits.iter().map(|h| h.record.doc_id.clone()).collect();
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
                treatment: None,
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
                treatment: None,
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
                treatment: None,
            });
        }
        // Citator resolution (see `crate::citator`): corpus citations
        // resolve against treatment edges visible in the effective
        // scope; web / private citations are always Unknown, and a
        // citation is never dropped for its status.
        {
            let effective_scope: Option<DocScope> = if fell_back { None } else { scope.clone() };
            let edges = self.treatments.read().expect("lock poisoned").all();
            for c in citations.iter_mut() {
                c.treatment = Some(if c.doc_id.starts_with("doc:") {
                    crate::citator::resolve_treatment(
                        c.doc_title.as_deref(),
                        &edges,
                        effective_scope.as_ref(),
                    )
                } else {
                    crate::citator::CitationTreatment::unknown()
                });
            }
        }
        // Citator V2 (kill switch LEGAL_RAG_CITATOR_V2=1, default OFF):
        // overrides corpus treatments with chain-aware resolution, ranks
        // citations live-first, and yields a governing-authority line.
        // Ranking happens pre-generation and keeps each citation's doc_id,
        // so [doc:N] markers keep resolving by id. Nothing is dropped.
        let mut governing_line: Option<String> = None;
        let mut system_hint: Option<String> = None;
        if crate::citator_v2::citator_v2_enabled() {
            let effective_scope: Option<DocScope> = if fell_back { None } else { scope.clone() };
            let edges = self.treatments.read().expect("lock poisoned").all();
            // Stored V2 edges (full-text ingest detection) outrank
            // snippet detection; the adapter dedups against them.
            let stored_v2 = self.v2_treatments.read().expect("lock poisoned").all();
            // Stored metadata, keyed by BOTH the positional citation id
            // (for cited judgments) and the real document id (for V1
            // edge sources, which carry real ids).
            let mut meta_map: std::collections::HashMap<String, crate::pipeline::DocMeta> =
                std::collections::HashMap::new();
            for c in citations.iter().filter(|c| c.doc_id.starts_with("doc:")) {
                if let Some(idx) = c
                    .doc_id
                    .strip_prefix("doc:")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    if let Some(real) = real_doc_ids.get(idx) {
                        if let Some(m) = self.meta_of(real) {
                            meta_map.insert(c.doc_id.clone(), m.clone());
                            meta_map.insert(real.clone(), m);
                        }
                    }
                }
            }
            let mut real_to_positional: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for (i, real) in real_doc_ids.iter().enumerate() {
                real_to_positional.insert(real.clone(), format!("doc:{i}"));
            }
            let res = crate::citator_v2::resolve_v2(&citations, &edges, &stored_v2, effective_scope.as_ref(), &meta_map, ctx, &real_to_positional);
            if !res.treatments.is_empty() {
                let by_id: std::collections::HashMap<&str, crate::citator::CitationTreatment> = res
                    .treatments
                    .iter()
                    .map(|(id, t)| (id.as_str(), t.clone()))
                    .collect();
                for c in citations.iter_mut() {
                    if let Some(t) = by_id.get(c.doc_id.as_str()) {
                        c.treatment = Some(t.clone());
                    }
                }
                let mut ranked: Vec<Citation> = Vec::with_capacity(citations.len());
                for id in &res.ranked_corpus_ids {
                    if let Some(pos) = citations.iter().position(|c| &c.doc_id == id) {
                        ranked.push(citations[pos].clone());
                    }
                }
                for c in citations.iter() {
                    if !c.doc_id.starts_with("doc:") {
                        ranked.push(c.clone());
                    }
                }
                // Corpus citations V2 did not rank (no title) keep their
                // place at the end of the corpus block.
                for c in citations.iter() {
                    if c.doc_id.starts_with("doc:") && !res.ranked_corpus_ids.contains(&c.doc_id) {
                        ranked.push(c.clone());
                    }
                }
                citations = ranked;
            }
            governing_line = res.governing_line;
            system_hint = res.system_hint;
        }
        let mut system = self.answer_system();
        if let Some(hint) = &system_hint {
            system.push_str("\n\n");
            system.push_str(hint);
        }
        if !web_pages.is_empty() {
            system.push_str(WEB_FORMAT_ADDENDUM);
        } else {
            system.push_str(CORPUS_FORMAT_ADDENDUM);
        }
        Ok((system, user_prompt, citations, scope_note, expansions, governing_line))
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
        ctx: &CitatorContext,
        opts: &AskOptions,
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
                if let Some(mut hit) = self.annotation_lookup(&qvec, mode, scope.as_ref(), &fp) {
                    info!("verified annotation served (zero LLM cost)");
                    // Brief-vault mode declares its scope on every
                    // answer — a verified annotation carries no note of
                    // its own, so stamp the mode's note on the way out.
                    // (Annotation scope matching is exact, so a served
                    // annotation is always within the brief's doc set.)
                    if let Some(label) = &opts.brief_vault {
                        if hit.scope_note.is_none() && scope.is_some() {
                            hit.scope_note = Some(brief_vault_note_hit(label));
                        }
                    }
                    return Ok(hit);
                }
                if self.answer_cache_enabled {
                    let key = self.answer_scope_key(mode, top_k, scope.as_ref(), &fp, ctx, opts.brief_vault_on());
                    if let Some(mut hit) = self.answer_cache.lookup(&qvec, &key) {
                        info!("semantic answer cache hit (zero LLM cost)");
                        // Edges may have landed since this answer was
                        // cached (ingest / backfill): refresh statuses.
                        self.refresh_treatments(&mut hit, scope.as_ref(), ctx);
                        return Ok(hit);
                    }
                    cache_slot = Some((qvec, key));
                }
            }
        }
        let (system, user_prompt, citations, scope_note, expansions, governing_line) =
            self.prepare_context(question, mode, top_k, web_pages, scope, extra, strict, ctx, opts).await?;
        // Only this final call ever sees confidential (extra) source
        // text, so only it honors the private-LLM override.
        let gen_llm = llm_override.as_ref().unwrap_or(&self.llm);
        let answer = gen_llm.complete(&system, &user_prompt).await?;
        // A provider failure page is not an answer (Piece 14): error
        // out BEFORE the audit, the treatment note, and the answer
        // cache, so a transient generation failure is never polished
        // into — or stored as — a grounded-looking response.
        if is_failure_output(&answer) {
            return Err(legal_rag_common::Error::Upstream(
                "llm provider returned a failure message instead of an answer".into(),
            ));
        }
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
        // Treatment flag, never a silent drop: an overruled or
        // distinguished cited authority gets a line in the answer.
        let answer = crate::citator::append_treatment_note(answer, &citations);
        // V2 governing authority, when determined, rides along as a line.
        let answer = match &governing_line {
            Some(line) => format!("{answer}\n\n{line}"),
            None => answer,
        };
        info!(mode = ?mode, citations = citations.len(), web_pages = web_pages.len(), "answered query");
        let out = QueryAnswer {
            answer,
            citations,
            mode,
            scope_note,
            verified: false,
            citation_audit: Some(audit),
            expansions,
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
        self.query_stream_scoped(question, mode, top_k, web, web_results, None, &[], false, None, CitatorContext::default(), &AskOptions::default(), tx)
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
        ctx: CitatorContext,
        opts: &AskOptions,
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
            let gather = self.gather_web(question, web_results).await;
            if let Some(note) = gather.outcome.note() {
                let _ = tx.send(QueryEvent::Status { message: note });
            }
            gather.pages
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
                    // Brief-vault parity: the scope note rides a Status
                    // event (the stream's scope mechanism), as on the
                    // freshly-generated path below.
                    if let Some(label) = &opts.brief_vault {
                        if scope.is_some() {
                            let _ = tx.send(QueryEvent::Status {
                                message: brief_vault_note_hit(label),
                            });
                        }
                    }
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
                    let _ = tx.send(QueryEvent::Done { answer: hit.answer, audit: None, expansions: Vec::new(), sub_agents: Vec::new() });
                    return Ok(());
                }
                if self.answer_cache_enabled {
                    let key = self.answer_scope_key(mode, top_k, scope.as_ref(), &fp, &ctx, opts.brief_vault_on());
                    if let Some(mut hit) = self.answer_cache.lookup(&qvec, &key) {
                        let _ = tx.send(QueryEvent::Status {
                            message: "Matched an earlier answer — serving it instantly.".to_string(),
                        });
                        // Brief-vault parity: replay the stored scope
                        // note as a Status so the turn declares its
                        // scope exactly like a fresh answer does.
                        if opts.brief_vault_on() {
                            if let Some(note) = &hit.scope_note {
                                let _ = tx.send(QueryEvent::Status { message: note.clone() });
                            }
                        }
                        // Same refresh as the blocking path: statuses
                        // track the current edge set, not cache time.
                        self.refresh_treatments(&mut hit, scope.as_ref(), &ctx);
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
                            expansions: hit.expansions.clone(),
                            sub_agents: Vec::new(),
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
        let prep = self.prepare_context(question, mode, top_k, &pages, scope, extra, strict, &ctx, opts);
        tokio::pin!(prep);
        let mut beat = tokio::time::interval(std::time::Duration::from_millis(1600));
        beat.tick().await; // consume the immediate first tick
        let heartbeats = [
            "Ranking the most relevant passages…",
            "Checking the knowledge graph…",
            "Weighing the evidence…",
        ];
        let mut beat_no = 0usize;
        let (system, user_prompt, citations, scope_note, expansions, governing_line) = loop {
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
        if !expansions.is_empty() {
            let _ = tx.send(QueryEvent::Status {
                message: format!(
                    "Expanded the question into {} retrieval variants (HyDE / multi-query)",
                    expansions.len()
                ),
            });
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
        // Same failure-page guard as the blocking path (Piece 14):
        // no Done, no answer-cache store — the stream reports an
        // error and a retry regenerates.
        if is_failure_output(&answer) {
            return Err(legal_rag_common::Error::Upstream(
                "llm provider returned a failure message instead of an answer".into(),
            ));
        }
        // Same integrity audit as the blocking path. Deltas already
        // streamed raw; the Done event carries the audited answer,
        // which is what the client persists and re-renders.
        let audit_sources: Vec<(String, String)> = citations
            .iter()
            .map(|c| (c.doc_id.clone(), c.snippet.clone()))
            .collect();
        let (answer, audit) = crate::audit::audit_answer(&answer, &audit_sources);
        // Same treatment-note rule as the blocking path; the Done
        // event carries the noted answer, which is what the client
        // persists and re-renders.
        let answer = crate::citator::append_treatment_note(answer, &citations);
        let answer = match &governing_line {
            Some(line) => format!("{answer}\n\n{line}"),
            None => answer,
        };
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
                        expansions: expansions.clone(),
                    },
                );
            }
        }
        let _ = tx.send(QueryEvent::Done { answer, audit: Some(audit), expansions, sub_agents: Vec::new() });
        Ok(())
    }

    /// Retrieve chunks + graph context for a mode. When `scope` is set,
    /// chunk hits and graph context are limited to the scoped documents.
    /// Retrieval with the retrieval-tier results cache in front
    /// (Spice's check-before-planning pattern): fresh entries replay
    /// directly; stale entries replay while one background task
    /// revalidates; misses run the full pipeline and populate the
    /// cache. Disabled with `LEGAL_RAG_RETRIEVAL_CACHE=0`.
    pub(crate) async fn retrieve(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        scope: Option<&DocScope>,
    ) -> Result<(Vec<ScoredChunk>, String)> {
        use crate::retrieval_cache::Lookup;
        if std::env::var("LEGAL_RAG_RETRIEVAL_CACHE")
            .map(|v| v == "0")
            .unwrap_or(false)
        {
            return self.retrieve_uncached(question, mode, top_k, scope, &[]).await;
        }
        let fp = self.corpus_fingerprint().await;
        let scope_docs: Option<Vec<String>> =
            scope.map(|s| s.iter().cloned().collect());
        let key = crate::retrieval_cache::retrieval_key(
            question,
            &format!("{mode:?}"),
            top_k,
            scope_docs.as_deref(),
            &fp,
        );
        match self.retrieval_cache.lookup(&key) {
            (Lookup::Fresh, Some(value)) => return Ok(value),
            (Lookup::Stale, Some(value)) => {
                self.spawn_revalidation(key, question.to_string(), mode, top_k, scope.cloned());
                return Ok(value);
            }
            _ => {}
        }
        let value = self.retrieve_uncached(question, mode, top_k, scope, &[]).await?;
        self.retrieval_cache.store(key, value.clone());
        Ok(value)
    }

    /// Generate HyDE + multi-query expansions for a triggered
    /// question: one hypothetical answer passage (embedded in place of
    /// the terse question, it lands near real answer text) plus up to
    /// three paraphrase variants. Empty on any failure.
    async fn expand_query(&self, question: &str) -> Vec<String> {
        #[derive(serde::Deserialize)]
        struct Expansion {
            #[serde(default)]
            hyde: String,
            #[serde(default)]
            variants: Vec<String>,
        }
        let raw = match self
            .llm
            .complete(
                "You expand terse legal questions for document retrieval. \
                 Reply with ONLY JSON: {\"hyde\": \"<a 2-3 sentence passage that would directly \
                 answer the question, written as formal legal text citing the relevant Act and \
                 section>\", \"variants\": [\"<two or three paraphrases of the question: one \
                 formal, one plain-language, one in Hinglish if the question is Indian law>\"]}",
                question,
            )
            .await
        {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let Some(parsed) = crate::graph::parse_llm_json::<Expansion>(&raw) else {
            return Vec::new();
        };
        let mut out: Vec<String> = Vec::new();
        if !parsed.hyde.trim().is_empty() {
            out.push(parsed.hyde.trim().to_string());
        }
        for v in parsed.variants {
            let v = v.trim().to_string();
            if !v.is_empty() && v != question && out.len() < 4 {
                out.push(v);
            }
        }
        out
    }

    /// Retrieval with HyDE / multi-query expansion queries pooled into
    /// the vector leg. Bypasses the retrieval cache: expansions are
    /// generated per call, so the pooled result is not the canonical
    /// result for the bare question.
    pub(crate) async fn retrieve_expanded(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        scope: Option<&DocScope>,
        extra_queries: &[String],
    ) -> Result<(Vec<ScoredChunk>, String)> {
        if extra_queries.is_empty() {
            return self.retrieve(question, mode, top_k, scope).await;
        }
        self.retrieve_uncached(question, mode, top_k, scope, extra_queries).await
    }

    /// Background refresh for a stale cache entry. No-ops when the
    /// service was never bound to its owning Arc (tests, tools).
    fn spawn_revalidation(
        &self,
        key: String,
        question: String,
        mode: QueryMode,
        top_k: usize,
        scope: Option<DocScope>,
    ) {
        let weak = self.weak_self.read().expect("lock poisoned").clone();
        let Some(weak) = weak else { return };
        tokio::spawn(async move {
            let Some(svc) = weak.upgrade() else { return };
            svc.retrieval_cache.note_revalidation();
            if let Ok(value) =
                svc.retrieve_uncached(&question, mode, top_k, scope.as_ref(), &[]).await
            {
                svc.retrieval_cache.store(key, value);
            }
        });
    }

    async fn retrieve_uncached(
        &self,
        question: &str,
        mode: QueryMode,
        top_k: usize,
        scope: Option<&DocScope>,
        extra_queries: &[String],
    ) -> Result<(Vec<ScoredChunk>, String)> {
        let mut graph_context = String::new();

        // The three legs are independent — run them concurrently so the
        // local leg's entity-extraction LLM call overlaps the embedding +
        // vector search instead of queueing behind them.
        let vec_leg = async {
            if matches!(mode, QueryMode::Naive | QueryMode::Mix) {
                // HyDE / multi-query: the bare question plus any
                // expansion queries each get a search pool; pools merge
                // by rank interleave (scores across query variants are
                // commensurable here — same embedder, same store — but
                // interleave keeps any single variant from dominating).
                let mut queries: Vec<String> = vec![question.to_string()];
                queries.extend(extra_queries.iter().cloned());
                let qvecs = self.embeddings.embed(&queries).await?;
                // Search wide, then keep searchable units only: context-
                // only parents (pc=parent) keep embeddings for storage
                // parity but are not match units — their children are.
                let wide = (top_k * 4).clamp(top_k, 200);
                let mut pools: Vec<Vec<ScoredChunk>> = Vec::new();
                for qvec in &qvecs {
                    let raw = self.store.search(qvec, wide).await?;
                    pools.push(
                        raw.into_iter().filter(|h| is_searchable(&h.record)).collect(),
                    );
                }
                let mut vec_hits: Vec<ScoredChunk> = merge_legs(pools);
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
        let vec_hits = vec_res?;
        let (local_hits, local_ctx) = local_res?;
        if !local_ctx.is_empty() {
            graph_context.push_str(&local_ctx);
        }
        let (global_hits, global_ctx) = global_res?;
        if !graph_context.is_empty() && !global_ctx.is_empty() {
            graph_context.push_str("\n\n");
        }
        graph_context.push_str(&global_ctx);

        // Merge the legs by RANK INTERLEAVE, never by raw score: the
        // legs' scores are incommensurable (vector cosines ~0.4–0.6,
        // graph legs carry a flat 0.85), so the old score sort let
        // graph-neighborhood chunks flood the merged list and truncate
        // the vector leg's best hits out of the rerank pool — measured
        // 2026-10-23: the Section 86 question's vector rank-1 answer
        // never reached the context in Mix mode and the model refused.
        let merged = merge_legs(vec![vec_hits, local_hits, global_hits]);
        // Auxiliary match units (augmentation questions/keywords,
        // hyperedges) resolve to their real chunks here; hyperedge
        // hits also pull in their sibling members as extra context.
        let (mut hits, siblings) = self.canonicalize_hits(merged).await;
        if let Some(scope) = scope {
            hits.retain(|h| scope.contains(&h.record.doc_id));
        }
        hits.truncate(top_k);
        let mut present: std::collections::HashSet<String> =
            hits.iter().map(|h| h.record.id.clone()).collect();
        for sib in siblings {
            if scope.map(|s| s.contains(&sib.record.doc_id)).unwrap_or(true)
                && present.insert(sib.record.id.clone())
            {
                hits.push(sib);
            }
        }
        Ok((hits, graph_context))
    }

    /// Map auxiliary-unit hits (pc=aug → source chunk, pc=hyper →
    /// first member) to their canonical chunks, deduplicating by
    /// canonical id (first/best-ranked occurrence wins). Returns the
    /// canonical hits plus hyperedge sibling chunks for context
    /// expansion (max 2 per matched hyperedge).
    async fn canonicalize_hits(
        &self,
        hits: Vec<ScoredChunk>,
    ) -> (Vec<ScoredChunk>, Vec<ScoredChunk>) {
        let mut fetch: Vec<String> = Vec::new();
        for h in &hits {
            if let Some(t) = canonical_target(&h.record) {
                fetch.push(t);
            }
            if h.record.metadata.get("pc").map(|s| s.as_str()) == Some("hyper") {
                if let Some(members) = h.record.metadata.get("members") {
                    fetch.extend(members.split(',').map(|s| s.to_string()));
                }
            }
        }
        if fetch.is_empty() {
            return (hits, Vec::new());
        }
        let fetched = self.store.get_by_ids(&fetch).await.unwrap_or_default();
        let by_id: std::collections::HashMap<&str, &ChunkRecord> =
            fetched.iter().map(|r| (r.id.as_str(), r)).collect();
        let mut out: Vec<ScoredChunk> = Vec::with_capacity(hits.len());
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut siblings: Vec<ScoredChunk> = Vec::new();
        for h in hits {
            let is_hyper = h.record.metadata.get("pc").map(|s| s.as_str()) == Some("hyper");
            let members: Vec<String> = if is_hyper {
                h.record
                    .metadata
                    .get("members")
                    .map(|m| m.split(',').map(|s| s.to_string()).collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let canonical = match canonical_target(&h.record) {
                Some(t) => match by_id.get(t.as_str()) {
                    Some(rec) => (*rec).clone(),
                    None => continue,
                },
                None => h.record.clone(),
            };
            if seen.insert(canonical.id.clone()) {
                out.push(ScoredChunk { record: canonical, score: h.score });
            }
            if is_hyper {
                let mut added = 0;
                for mid in members {
                    if added >= 2 {
                        break;
                    }
                    if let Some(rec) = by_id.get(mid.as_str()) {
                        if !seen.contains(rec.id.as_str()) {
                            siblings.push(ScoredChunk {
                                record: (*rec).clone(),
                                score: h.score * 0.99,
                            });
                            added += 1;
                        }
                    }
                }
            }
        }
        (out, siblings)
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
        let cfg = FusionConfig::from_env();
        let doc_dates = self.doc_dates_for(&vec_hits, &extra);
        let statute_docs = self.statute_docs_for(question, &vec_hits, &extra);
        Ok(fuse_rankings(question, vec_hits, &kw, extra, top_k, &cfg, &doc_dates, &statute_docs))
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

/// Score-gap cutoff ("Autocut", Verba's pattern): given hits in rerank
/// order with their reranker scores, truncate at the largest relative
/// score cliff when it is steep enough. Scores are min-max normalized
/// over the kept set; a normalized gap of `LEGAL_RAG_AUTOCUT_GAP`
/// (default 0.45) or more, with at least 4 candidates, cuts the tail.
/// Disabled unless `LEGAL_RAG_AUTOCUT=1`. Backfill items (unscored) are
/// appended by the caller after this runs, so a cut is never undone.
fn apply_score_gap_cutoff(hits: Vec<ScoredChunk>, scores: &[f32]) -> Vec<ScoredChunk> {
    if std::env::var("LEGAL_RAG_AUTOCUT").map(|v| v != "1").unwrap_or(true) {
        return hits;
    }
    let n = hits.len().min(scores.len());
    if n < 4 || scores.iter().take(n).any(|s| !s.is_finite()) {
        return hits;
    }
    let gap_min = std::env::var("LEGAL_RAG_AUTOCUT_GAP")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(0.45)
        .clamp(0.05, 0.95);
    let hi = scores[..n].iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let lo = scores[..n].iter().copied().fold(f32::INFINITY, f32::min);
    if hi - lo < 1e-6 {
        return hits;
    }
    let norm = |s: f32| (s - lo) / (hi - lo);
    let mut best: Option<(usize, f32)> = None;
    for i in 1..n {
        let gap = norm(scores[i - 1]) - norm(scores[i]);
        if gap >= gap_min && best.is_none_or(|(_, g)| gap > g) {
            best = Some((i, gap));
        }
    }
    match best {
        Some((cut, gap)) if cut >= 1 && cut < hits.len() => {
            info!(cut, gap, "score-gap cutoff applied");
            hits.into_iter().take(cut).collect()
        }
        _ => hits,
    }
}

/// Presentation order for the answer context ("lost in the middle",
/// Haystack's pattern): LLMs attend best to the start and end of the
/// context, so emit ranked hits as best, 3rd, 5th, …, 6th, 4th, 2nd —
/// the strongest sources sit at both edges. Returns indices into the
/// ranked list; citation numbering keeps rank order, only the prompt
/// emission order changes. Disabled with `LEGAL_RAG_LITM=0`.
fn litm_order(n: usize) -> Vec<usize> {
    if n < 4 || std::env::var("LEGAL_RAG_LITM").map(|v| v == "0").unwrap_or(false) {
        return (0..n).collect();
    }
    let mut out: Vec<usize> = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        out.push(i);
        i += 2;
    }
    let mut j = if n % 2 == 0 { n - 1 } else { n - 2 };
    loop {
        out.push(j);
        if j < 2 {
            break;
        }
        j -= 2;
    }
    out
}

/// HyDE / multi-query trigger: short questions (≤8 words) embed
/// poorly, and citation-style questions (a statute + a section
/// reference) are the tersest of all. Env gate LEGAL_RAG_HYDE=0
/// disables expansion entirely.
pub fn hyde_trigger(question: &str) -> bool {
    if std::env::var("LEGAL_RAG_HYDE").map(|v| v == "0").unwrap_or(false) {
        return false;
    }
    let words = question.split_whitespace().count();
    if words <= 8 {
        return true;
    }
    let lower = question.to_lowercase();
    !statute_mentions(&lower).is_empty()
        && !crate::pipeline::find_section_refs(&lower).is_empty()
}

/// Keyword engine selection: the in-house BM25 (default, zero deps,
/// exact tokenization incl. bare numbers like "86") or Tantivy
/// (LEGAL_RAG_BM25_ENGINE=tantivy) for corpora where the in-memory
/// native index becomes the bottleneck. Both expose the same
/// build/search contract; selection happens per index build.
pub enum Bm25Engine {
    Native(Bm25Index),
    Tantivy(crate::tantivy_bm25::TantivyBm25Index),
}

impl Bm25Engine {
    pub fn build(records: &[ChunkRecord]) -> Self {
        match std::env::var("LEGAL_RAG_BM25_ENGINE").as_deref() {
            Ok("tantivy") => {
                Self::Tantivy(crate::tantivy_bm25::TantivyBm25Index::build(records))
            }
            _ => Self::Native(Bm25Index::build(records)),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Native(_) => "native",
            Self::Tantivy(_) => crate::tantivy_bm25::engine_name(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Native(i) => i.is_empty(),
            Self::Tantivy(i) => i.is_empty(),
        }
    }

    pub fn search(&self, query: &str, top_k: usize) -> Vec<(String, f32)> {
        match self {
            Self::Native(i) => i.search(query, top_k),
            Self::Tantivy(i) => i.search(query, top_k),
        }
    }
}

/// The canonical chunk an auxiliary match unit votes for:
/// augmentation units (pc=aug) → their source chunk; hyperedge units
/// (pc=hyper) → their first member chunk. Real chunks return None.
pub fn canonical_target(rec: &ChunkRecord) -> Option<String> {
    match rec.metadata.get("pc").map(|s| s.as_str()) {
        Some("aug") => rec.metadata.get("source_id").cloned(),
        Some("hyper") => rec
            .metadata
            .get("members")
            .and_then(|m| m.split(',').next())
            .map(|s| s.to_string()),
        _ => None,
    }
}

/// Blend ColBERT MaxSim scores into reranked hits: both score sets
/// are min-max normalized to [0,1] per query, combined as
/// (1-w)·rerank + w·maxsim, and re-sorted. Hits missing a MaxSim
/// score keep their normalized rerank score only.
pub fn blend_maxsim(
    hits: Vec<ScoredChunk>,
    maxsim: &[(String, f32)],
    weight: f32,
) -> Vec<ScoredChunk> {
    if hits.is_empty() || maxsim.is_empty() {
        return hits;
    }
    let norm = |vals: &[f32]| -> Vec<f32> {
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for v in vals {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
        if !(hi > lo) {
            return vec![0.5; vals.len()];
        }
        vals.iter().map(|v| (v - lo) / (hi - lo)).collect()
    };
    let rer_norm = norm(&hits.iter().map(|h| h.score).collect::<Vec<_>>());
    let ms_map: std::collections::HashMap<&str, f32> =
        maxsim.iter().map(|(id, s)| (id.as_str(), *s)).collect();
    let ms_vals: Vec<f32> = hits
        .iter()
        .map(|h| ms_map.get(h.record.id.as_str()).copied().unwrap_or(f32::NAN))
        .collect();
    let present: Vec<f32> = ms_vals.iter().copied().filter(|v| !v.is_nan()).collect();
    let ms_norm_all = norm(&present);
    let mut ms_iter = ms_norm_all.iter();
    let ms_norm: Vec<f32> = ms_vals
        .iter()
        .map(|v| {
            if v.is_nan() {
                0.0
            } else {
                *ms_iter.next().unwrap_or(&0.0)
            }
        })
        .collect();
    let mut out: Vec<ScoredChunk> = hits
        .into_iter()
        .enumerate()
        .map(|(i, mut h)| {
            h.score = (1.0 - weight) * rer_norm[i] + weight * ms_norm[i];
            h
        })
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Merge retrieval legs by rank interleave: round-robin across the
/// legs (in the given priority order), each leg contributing its hits
/// in its own ranked order, deduplicated by chunk id (first occurrence
/// wins). Legs score on incommensurable scales (cosines vs the graph
/// legs' flat 0.85), so a raw-score merge is a category error — this is
/// the cross-leg analogue of rank fusion.
fn merge_legs(legs: Vec<Vec<ScoredChunk>>) -> Vec<ScoredChunk> {
    let total: usize = legs.iter().map(|l| l.len()).sum();
    let mut out: Vec<ScoredChunk> = Vec::with_capacity(total);
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut idx: Vec<usize> = vec![0; legs.len()];
    loop {
        let mut progressed = false;
        for (leg_i, leg) in legs.iter().enumerate() {
            while idx[leg_i] < leg.len() {
                let h = &leg[idx[leg_i]];
                idx[leg_i] += 1;
                progressed = true;
                if seen.insert(h.record.id.clone()) {
                    out.push(h.clone());
                    break;
                }
            }
        }
        if !progressed {
            break;
        }
    }
    out
}

/// Fusion strategy for merging the vector and BM25 rankings.
///
/// `Rrf` is the original behaviour (equal weights, k=60). `WeightedRrf`
/// and `Dbsf` are the Spice.ai / Haystack patterns rewritten natively
/// (Spice ships a tunable RRF UDTF with per-leg rank weights; Haystack's
/// DocumentJoiner ships distribution-based fusion): rank-only fusion
/// discards score magnitude, which is one way a strong exact-match BM25
/// hit from the correct Act gets diluted by mid-pack vector hits from a
/// wrong one (measured 2026-10-23: CPC s.11 vs Contract Act s.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FusionMode {
    Rrf,
    WeightedRrf,
    Dbsf,
}

/// Fusion knobs, read from the environment per call (the codebase's
/// existing pattern for retrieval tuning), so the test instance can A/B
/// configurations across restarts without a rebuild.
#[derive(Debug, Clone, Copy)]
pub struct FusionConfig {
    pub mode: FusionMode,
    /// RRF smoothing constant (Spice exposes the same knob: k≈20 makes
    /// rank differences bite harder than the classic 60).
    pub k: f32,
    pub w_vector: f32,
    pub w_bm25: f32,
    /// Extra BM25 weight multiplier when the query carries a legal
    /// identifier (section/article number, statute name) — exact-term
    /// evidence should dominate paraphrase evidence there.
    pub identifier_bm25_boost: f32,
    /// Recency term weight (Spice's RRF recency boosting): adds
    /// `w * decay` per document, decay from the document's effective
    /// date with the configured half-life. 0 disables.
    pub recency_weight: f32,
    pub recency_half_life_days: f32,
    /// Statute-match factor applied INSIDE fusion to chunks whose
    /// document title matches a statute the query names. Placement is
    /// the lesson of the 2026-10-23 A/B: the one discriminating rule
    /// (Section 86, IT Act) died at the fusion cut, below the reach of
    /// any post-fusion boost — the signal has to act before the cut.
    pub statute_boost: f32,
}

impl FusionConfig {
    pub fn from_env() -> Self {
        let envf = |key: &str, default: f32| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(default)
        };
        let mode = match std::env::var("LEGAL_RAG_FUSION")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "wrrf" | "weighted" | "weighted_rrf" => FusionMode::WeightedRrf,
            "dbsf" => FusionMode::Dbsf,
            _ => FusionMode::Rrf,
        };
        Self {
            mode,
            k: envf("LEGAL_RAG_FUSION_K", 60.0).clamp(1.0, 1000.0),
            w_vector: envf("LEGAL_RAG_FUSION_W_VECTOR", 1.0).max(0.0),
            w_bm25: envf("LEGAL_RAG_FUSION_W_BM25", 1.0).max(0.0),
            identifier_bm25_boost: envf("LEGAL_RAG_FUSION_IDENTIFIER_BOOST", 1.5).max(0.0),
            recency_weight: envf("LEGAL_RAG_FUSION_RECENCY_WEIGHT", 0.0).max(0.0),
            recency_half_life_days: envf("LEGAL_RAG_FUSION_RECENCY_HALF_LIFE_DAYS", 3650.0)
                .max(1.0),
            statute_boost: envf("LEGAL_RAG_FUSION_STATUTE_BOOST", 1.35).clamp(1.0, 3.0),
        }
    }

    /// Effective BM25 weight for this query (identifier-aware).
    fn bm25_weight(&self, question: &str) -> f32 {
        match self.mode {
            FusionMode::Rrf => 1.0,
            _ if query_has_identifier(question) => self.w_bm25 * self.identifier_bm25_boost,
            _ => self.w_bm25,
        }
    }

    fn vector_weight(&self) -> f32 {
        match self.mode {
            FusionMode::Rrf => 1.0,
            _ => self.w_vector,
        }
    }
}

/// Does the query carry a legal identifier — a section/article/rule
/// reference or a statute name — where exact-term matching should be
/// trusted over semantic similarity?
pub fn query_has_identifier(question: &str) -> bool {
    let q = question.to_lowercase();
    let tokens: Vec<&str> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    const MARKERS: &[&str] = &[
        "section", "sec", "article", "rule", "schedule", "clause", "ipc", "cpc", "crpc",
        "bns", "bnss", "bsa",
    ];
    if tokens.iter().any(|t| MARKERS.contains(t)) {
        return true;
    }
    !statute_mentions(&q).is_empty()
}

/// Canonical Indian statutes with the aliases users actually type.
/// The canonical string is matched against document titles; aliases are
/// matched against the query (short aliases need a whole-token match).
pub(crate) const STATUTES: &[(&str, &[&str])] = &[
    ("indian penal code", &["ipc", "indian penal code", "penal code"]),
    ("code of civil procedure", &["cpc", "civil procedure code"]),
    ("code of criminal procedure", &["crpc", "criminal procedure code"]),
    ("indian contract act", &["contract act"]),
    ("indian evidence act", &["evidence act"]),
    ("constitution of india", &["constitution", "constitution of india"]),
    ("central excise act", &["excise act", "central excise"]),
    ("information technology act", &["it act", "information technology act"]),
    ("companies act", &["companies act"]),
    ("income tax act", &["income tax act", "income-tax act"]),
    ("negotiable instruments act", &["negotiable instruments", "ni act"]),
    ("specific relief act", &["specific relief"]),
    ("limitation act", &["limitation act"]),
    ("arbitration and conciliation act", &["arbitration act", "arbitration and conciliation"]),
    ("consumer protection act", &["consumer protection"]),
    ("transfer of property act", &["transfer of property"]),
    ("motor vehicles act", &["motor vehicles"]),
    ("hindu marriage act", &["hindu marriage"]),
    ("registration act", &["registration act"]),
    ("indian stamp act", &["stamp act"]),
];

/// Canonical statute names mentioned in a (lowercased) text.
pub fn statute_mentions(text_lower: &str) -> Vec<&'static str> {
    let tokens: std::collections::HashSet<&str> = text_lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let mut out = Vec::new();
    for (canonical, aliases) in STATUTES {
        let hit = aliases.iter().any(|a| {
            if a.len() <= 4 {
                tokens.contains(a)
            } else {
                text_lower.contains(a)
            }
        });
        if hit {
            out.push(*canonical);
        }
    }
    out
}

/// Does a document title name this statute? Titles use both full names
/// ("Indian Contract Act — Sections 73–74") and abbreviations
/// ("Section 86 IT Act") — matching only the canonical name missed the
/// abbreviated titles entirely (measured 2026-10-23 on the Section 86
/// rule), so aliases count too.
pub fn title_matches_statute(title_lower: &str, canonical: &str) -> bool {
    if title_lower.contains(canonical) {
        return true;
    }
    let Some((_, aliases)) = STATUTES.iter().find(|(c, _)| *c == canonical) else {
        return false;
    };
    let tokens: std::collections::HashSet<&str> = title_lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    aliases.iter().any(|a| {
        if a.len() <= 4 {
            tokens.contains(a)
        } else {
            title_lower.contains(a)
        }
    })
}

/// Does the (lowercased) query mention this jurisdiction? Codes map to
/// the names users type ("IN" → india/indian); other codes match their
/// own lowercase form as a token.
fn jurisdiction_mentioned(query_lower: &str, code: &str) -> bool {
    let names: &[&str] = match code.to_uppercase().as_str() {
        "IN" => &["india", "indian"],
        "IN-DL" => &["delhi"],
        "IN-MH" => &["maharashtra", "mumbai", "bombay"],
        "IN-KA" => &["karnataka", "bangalore", "bengaluru"],
        "IN-UP" => &["uttar pradesh"],
        "IN-WB" => &["west bengal", "kolkata", "calcutta"],
        "US" => &["united states", "america", "american"],
        "UK" | "GB" => &["united kingdom", "britain", "england"],
        "EU" => &["european union", "europe"],
        _ => &[],
    };
    if names.iter().any(|n| query_lower.contains(n)) {
        return true;
    }
    let c = code.to_lowercase();
    query_lower
        .split(|ch: char| !ch.is_alphanumeric() && ch != '-')
        .any(|t| t == c)
}

/// Recency decay for a document effective date (`YYYY-MM-DD`), Spice's
/// exponential-decay pattern: 1.0 today, 0.5 one half-life ago.
fn recency_decay(effective_from: Option<&str>, half_life_days: f32) -> f32 {
    let Some(day) = effective_from else { return 0.0 };
    let Ok(date) = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") else {
        return 0.0;
    };
    let age = (chrono::Utc::now().date_naive() - date).num_days().max(0) as f32;
    0.5f32.powf(age / half_life_days)
}

/// Fuse a vector ranking with a BM25 ranking under `cfg`. The fused
/// ORDER decides the candidate pool; vector hits keep their cosine
/// scores, and keyword-only hits (records supplied via `extra`) inherit
/// the cosine of the vector hit at their fused rank (× 0.98), so
/// downstream margin gates and citation scores stay on the cosine scale.
fn fuse_rankings(
    question: &str,
    vec_hits: Vec<ScoredChunk>,
    kw_ranked: &[(String, f32)],
    extra: Vec<ChunkRecord>,
    top_k: usize,
    cfg: &FusionConfig,
    doc_dates: &std::collections::HashMap<String, Option<String>>,
    statute_docs: &std::collections::HashSet<String>,
) -> Vec<ScoredChunk> {
    let k = if cfg.mode == FusionMode::Rrf { 60.0 } else { cfg.k };
    let w_vec = cfg.vector_weight();
    let w_kw = cfg.bm25_weight(question);
    let mut fused_score: std::collections::HashMap<String, f32> =
        std::collections::HashMap::new();
    match cfg.mode {
        FusionMode::Rrf | FusionMode::WeightedRrf => {
            for (rank, h) in vec_hits.iter().enumerate() {
                *fused_score.entry(h.record.id.clone()).or_default() +=
                    w_vec / (k + rank as f32 + 1.0);
            }
            for (rank, (id, _)) in kw_ranked.iter().enumerate() {
                *fused_score.entry(id.clone()).or_default() +=
                    w_kw / (k + rank as f32 + 1.0);
            }
        }
        FusionMode::Dbsf => {
            // Distribution-based fusion (Haystack): normalize each leg's
            // scores by that leg's own distribution, then weighted-sum.
            // A chunk missing from a leg scores at that leg's floor.
            let norm = |scores: &[f32]| -> (f32, f32) {
                if scores.is_empty() {
                    return (0.0, 1.0);
                }
                let mean = scores.iter().sum::<f32>() / scores.len() as f32;
                let var = scores.iter().map(|s| (s - mean) * (s - mean)).sum::<f32>()
                    / scores.len() as f32;
                (mean, var.sqrt().max(1e-6))
            };
            let dbsf = |score: f32, mean: f32, std: f32| (score - mean) / (3.0 * std);
            let vec_scores: Vec<f32> = vec_hits.iter().map(|h| h.score).collect();
            let (v_mean, v_std) = norm(&vec_scores);
            let v_floor = vec_scores.iter().copied().fold(f32::INFINITY, f32::min);
            let kw_scores: Vec<f32> = kw_ranked.iter().map(|(_, s)| *s).collect();
            let (k_mean, k_std) = norm(&kw_scores);
            let k_floor = kw_scores.iter().copied().fold(f32::INFINITY, f32::min);
            let kw_by_id: std::collections::HashMap<&str, f32> =
                kw_ranked.iter().map(|(id, s)| (id.as_str(), *s)).collect();
            let mut ids: std::collections::HashSet<String> =
                vec_hits.iter().map(|h| h.record.id.clone()).collect();
            ids.extend(kw_ranked.iter().map(|(id, _)| id.clone()));
            for id in ids {
                let v = vec_hits
                    .iter()
                    .find(|h| h.record.id == id)
                    .map(|h| dbsf(h.score, v_mean, v_std))
                    .unwrap_or_else(|| dbsf(v_floor, v_mean, v_std));
                let kw = kw_by_id
                    .get(id.as_str())
                    .map(|s| dbsf(*s, k_mean, k_std))
                    .unwrap_or_else(|| dbsf(k_floor, k_mean, k_std));
                fused_score.insert(id, w_vec * v + w_kw * kw);
            }
        }
    }
    // Chunk id → document id, for the per-document terms below.
    let doc_of: std::collections::HashMap<&str, &str> = vec_hits
        .iter()
        .map(|h| (h.record.id.as_str(), h.record.doc_id.as_str()))
        .chain(extra.iter().map(|r| (r.id.as_str(), r.doc_id.as_str())))
        .collect();
    // Statute-match term: chunks of a document whose title matches a
    // statute named in the query get their fused score lifted BEFORE
    // the top_k cut — the measured failure mode is the correct Act's
    // chunks dying at exactly that cut.
    if cfg.statute_boost > 1.0 && !statute_docs.is_empty() {
        for (id, score) in fused_score.iter_mut() {
            let is_statute_doc = doc_of
                .get(id.as_str())
                .map(|doc| statute_docs.contains(*doc))
                .unwrap_or(false);
            if is_statute_doc {
                match cfg.mode {
                    // DBSF scores are signed normalized units; a
                    // multiplicative lift would push negatives down.
                    FusionMode::Dbsf => *score += cfg.statute_boost - 1.0,
                    _ => *score *= cfg.statute_boost,
                }
            }
        }
    }
    // Recency term (Spice pattern): scale-free additive vote, strongest
    // for a document effective today, halving every half-life. Dates are
    // per document; chunk ids resolve through their records.
    if cfg.recency_weight > 0.0 {
        let scale = match cfg.mode {
            FusionMode::Dbsf => 0.25,
            _ => 1.0 / (k + 1.0),
        };
        for (id, score) in fused_score.iter_mut() {
            let decay = doc_of
                .get(id.as_str())
                .and_then(|doc| doc_dates.get(*doc))
                .map(|d| recency_decay(d.as_deref(), cfg.recency_half_life_days))
                .unwrap_or(0.0);
            *score += cfg.recency_weight * decay * scale;
        }
    }
    let vec_cosines: Vec<f32> = vec_hits.iter().map(|h| h.score).collect();
    let mut by_id: std::collections::HashMap<String, ScoredChunk> =
        vec_hits.into_iter().map(|h| (h.record.id.clone(), h)).collect();
    let mut ids: Vec<&String> = fused_score.keys().collect();
    ids.sort_by(|a, b| {
        fused_score[*b]
            .partial_cmp(&fused_score[*a])
            .unwrap_or(std::cmp::Ordering::Equal)
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

/// Reciprocal-rank fusion (k=60) of a vector ranking with a BM25 ranking
/// — the original fusion, kept as the `FusionMode::Rrf` default.
fn fuse_rrf(
    vec_hits: Vec<ScoredChunk>,
    kw_ranked: &[(String, f32)],
    extra: Vec<ChunkRecord>,
    top_k: usize,
) -> Vec<ScoredChunk> {
    fuse_rankings(
        "",
        vec_hits,
        kw_ranked,
        extra,
        top_k,
        &FusionConfig {
            mode: FusionMode::Rrf,
            k: 60.0,
            w_vector: 1.0,
            w_bm25: 1.0,
            identifier_bm25_boost: 1.0,
            recency_weight: 0.0,
            recency_half_life_days: 3650.0,
            statute_boost: 1.0,
        },
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
    )
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

    #[test]
    fn hyde_trigger_short_and_citation_style() {
        assert!(hyde_trigger("Section 86 IT Act"));
        assert!(hyde_trigger("what is theft"));
        assert!(hyde_trigger(
            "Explain in detail the scope of Section 86 of the Information Technology Act and how it interacts with other laws currently in force in India today"
        ));
        assert!(!hyde_trigger(
            "Explain in detail the historical development of preventive detention jurisprudence in India across the decades since independence and its many phases"
        ));
    }

    #[test]
    fn blend_maxsim_promotes_exact_term_hit() {
        let mk = |id: &str, score: f32| ScoredChunk {
            record: ChunkRecord {
                id: id.to_string(),
                doc_id: "d".to_string(),
                ordinal: 0,
                text: "t".to_string(),
                embedding: vec![0.0; 2],
                metadata: Default::default(),
            },
            score,
        };
        // Reranker prefers "a"; MaxSim strongly prefers "b" (exact
        // section-number evidence) — at w=0.5, b must win.
        let hits = vec![mk("a", 0.9), mk("b", 0.8), mk("c", 0.1)];
        let ms = vec![
            ("a".to_string(), 4.0),
            ("b".to_string(), 14.0),
            ("c".to_string(), 3.0),
        ];
        let out = blend_maxsim(hits, &ms, 0.5);
        assert_eq!(out[0].record.id, "b");
        assert_eq!(out.len(), 3);
        // w=0 → pure reranker order.
        let hits = vec![mk("a", 0.9), mk("b", 0.8)];
        let out = blend_maxsim(hits, &ms, 0.0);
        assert_eq!(out[0].record.id, "a");
    }

    #[test]
    fn canonical_target_maps_aux_units() {
        let mk = |pc: &str, extra: (&str, &str)| {
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("pc".to_string(), pc.to_string());
            metadata.insert(extra.0.to_string(), extra.1.to_string());
            ChunkRecord {
                id: "x".to_string(),
                doc_id: "d".to_string(),
                ordinal: 0,
                text: "t".to_string(),
                embedding: vec![0.0; 2],
                metadata,
            }
        };
        assert_eq!(
            canonical_target(&mk("aug", ("source_id", "d#0"))),
            Some("d#0".to_string())
        );
        assert_eq!(
            canonical_target(&mk("hyper", ("members", "d#1,d#2"))),
            Some("d#1".to_string())
        );
        assert_eq!(canonical_target(&mk("child", ("parent_id", "d#0"))), None);
    }

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

    /// An LLM whose ANSWER call first returns a provider failure page
    /// — as an `Ok` completion, exactly how a web-backed bridge
    /// delivers it — and then recovers. Every non-answer call gets
    /// "[]", which the expansion/extraction callers degrade on.
    struct FlakyAnswerLlm {
        answer_calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmClient for FlakyAnswerLlm {
        async fn complete(&self, system: &str, _user: &str) -> Result<String> {
            if system.contains("using ONLY the provided source excerpts") {
                let n = self
                    .answer_calls
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n == 0 {
                    return Ok(
                        "Sorry, something went wrong. Please try your request again."
                            .to_string(),
                    );
                }
                return Ok("Recovered grounded answer [doc:0].".to_string());
            }
            Ok("[]".to_string())
        }
    }

    /// `seeded()` with the flaky answer LLM wired into the service.
    async fn flaky_seeded() -> QueryService {
        let rag = Arc::new(RagAnything::new(
            RagAnythingConfig::default(),
            Arc::new(MockEmbeddings::new(16)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(MockLlm::new(r#"["Acme"]"#)),
            500,
            50,
        ));
        rag.process_document_complete(
            "nda.txt",
            b"Acme Corp agrees to confidentiality. Acme shall not disclose.",
            None,
            None,
        )
        .await
        .unwrap();
        let rag2 = rag.clone();
        QueryService::new(
            rag2.backends().0,
            rag2.backends().1,
            rag2.backends().2,
            Arc::new(FlakyAnswerLlm {
                answer_calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            Arc::new(move |id: &str| rag2.doc_title(id)),
            Arc::new(crate::web::NoopWebSearch),
            None,
        )
    }

    #[tokio::test]
    async fn failure_output_is_an_error_and_never_cached() {
        // Piece 14 regression: a provider failure page must surface
        // as an error — never as an answer, and never into the
        // semantic answer cache, where it would be re-served forever.
        let svc = flaky_seeded().await;
        let first = svc
            .query("What did Acme agree to?", QueryMode::Mix, 3, CitatorContext::default())
            .await;
        assert!(
            first.is_err(),
            "failure page must not be served as an answer: {:?}",
            first.map(|a| a.answer)
        );
        // The identical retry regenerates (nothing was cached) and
        // succeeds now that the provider has recovered.
        let second = svc
            .query("What did Acme agree to?", QueryMode::Mix, 3, CitatorContext::default())
            .await
            .unwrap();
        assert!(
            second.answer.contains("Recovered grounded answer"),
            "{}",
            second.answer
        );
        // And a successful answer still serves normally afterwards.
        let third = svc
            .query("What did Acme agree to?", QueryMode::Mix, 3, CitatorContext::default())
            .await
            .unwrap();
        assert!(
            third.answer.contains("Recovered grounded answer"),
            "{}",
            third.answer
        );
    }

    #[tokio::test]
    async fn failure_output_stream_is_an_error_and_never_cached() {
        // Piece 14, streaming path: the failed stream errors (no
        // Done, no cache store); the identical retry streams to Done.
        let svc = flaky_seeded().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<QueryEvent>();
        let first = svc
            .query_stream("What did Acme agree to?", QueryMode::Mix, 3, false, 3, tx)
            .await;
        assert!(first.is_err(), "streamed failure page must error, not Done");
        while rx.try_recv().is_ok() {}
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel::<QueryEvent>();
        svc.query_stream("What did Acme agree to?", QueryMode::Mix, 3, false, 3, tx2)
            .await
            .unwrap();
        let mut done_answer = None;
        while let Ok(ev) = rx2.try_recv() {
            if let QueryEvent::Done { answer, .. } = ev {
                done_answer = Some(answer);
            }
        }
        let done = done_answer.expect("second stream completes with Done");
        assert!(done.contains("Recovered grounded answer"), "{done}");
    }

    /// Piece 12 fixture: two documents that read as two briefs —
    /// "nda.txt" in brief A, "rent.txt" outside it. Questions are
    /// worded so each retrieves only its own document under the
    /// mock embedder.
    async fn seeded_two_briefs() -> (QueryService, String, String) {
        let rag = Arc::new(RagAnything::new(
            RagAnythingConfig::default(),
            Arc::new(MockEmbeddings::new(16)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(MockLlm::new(r#"["Acme"]"#)),
            500,
            50,
        ));
        rag.process_document_complete(
            "nda.txt",
            b"Acme Corp agrees to confidentiality. Acme shall not disclose trade secrets.",
            None,
            None,
        )
        .await
        .unwrap();
        rag.process_document_complete(
            "rent.txt",
            b"Beta Properties served a notice to quit on the tenant for arrears of rent. The tenant must vacate the premises.",
            None,
            None,
        )
        .await
        .unwrap();
        let docs = rag.list_documents();
        let id_of = |name: &str| {
            docs.iter()
                .find(|d| rag.doc_title(&d.id).as_deref() == Some(name))
                .unwrap_or_else(|| panic!("doc titled {name} seeded"))
                .id
                .clone()
        };
        let (a_id, b_id) = (id_of("nda.txt"), id_of("rent.txt"));
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
        (svc, a_id, b_id)
    }

    fn brief_vault_opts() -> AskOptions {
        AskOptions {
            brief_vault: Some("Brief A".to_string()),
            refine_terms: Vec::new(),
        }
    }

    #[tokio::test]
    async fn brief_vault_refuses_cross_brief_question() {
        // Piece 12: with Brief vault ON, a question whose evidence
        // lives in another brief can never cite that brief. Against
        // brief A's document the unrelated question's citations stay
        // inside A (retrieval is nearest-neighbour, so the guarantee
        // is containment, stamped with the mode's note); against an
        // EMPTY brief — zero in-scope evidence — the mode refuses
        // outright instead of widening, while the same question with
        // the mode OFF falls back to the full vault as today.
        let (svc, a_id, _b_id) = seeded_two_briefs().await;
        let q = "What notice did Beta Properties serve for rent arrears?";
        let on = svc
            .query_opts(q, QueryMode::Mix, 3, Some(vec![a_id.clone()]), true, &brief_vault_opts(), CitatorContext::default())
            .await
            .unwrap();
        assert!(
            on.citations.iter().all(|c| c.doc_title.as_deref() == Some("nda.txt")),
            "brief-vault must cite nothing outside the brief: {:?}",
            on.citations.iter().map(|c| &c.doc_title).collect::<Vec<_>>()
        );
        assert_eq!(
            on.scope_note.as_deref(),
            Some(brief_vault_note_hit("Brief A").as_str())
        );
        // Zero in-scope evidence: grounded refusal, no fallback.
        let refused = svc
            .query_opts(q, QueryMode::Mix, 3, Some(Vec::new()), true, &brief_vault_opts(), CitatorContext::default())
            .await
            .unwrap();
        assert!(
            refused.citations.is_empty(),
            "an empty brief answers from nothing: {:?}",
            refused.citations.iter().map(|c| &c.doc_title).collect::<Vec<_>>()
        );
        assert_eq!(
            refused.scope_note.as_deref(),
            Some(brief_vault_note_empty("Brief A").as_str())
        );
        // Control — mode OFF (today's soft scoping): widened + noted.
        let off = svc
            .query_scoped(q, QueryMode::Mix, 3, Vec::new(), CitatorContext::default())
            .await
            .unwrap();
        assert!(!off.citations.is_empty(), "soft mode falls back to the full vault");
        assert!(
            off.citations.iter().any(|c| c.doc_title.as_deref() == Some("rent.txt")),
            "fallback reaches the other brief's document: {:?}",
            off.citations.iter().map(|c| &c.doc_title).collect::<Vec<_>>()
        );
        assert!(
            off.scope_note
                .as_deref()
                .is_some_and(|n| n.contains("answered from the full vault")),
            "note: {:?}",
            off.scope_note
        );
    }

    #[tokio::test]
    async fn brief_vault_in_brief_answer_is_brief_only_and_noted() {
        let (svc, a_id, _b_id) = seeded_two_briefs().await;
        let ans = svc
            .query_opts(
                "What did Acme agree to?",
                QueryMode::Mix,
                3,
                Some(vec![a_id]),
                true,
                &brief_vault_opts(),
                CitatorContext::default(),
            )
            .await
            .unwrap();
        assert!(!ans.citations.is_empty(), "in-brief evidence answers");
        assert!(
            ans.citations.iter().all(|c| c.doc_title.as_deref() == Some("nda.txt")),
            "every citation is the brief's document: {:?}",
            ans.citations.iter().map(|c| &c.doc_title).collect::<Vec<_>>()
        );
        assert_eq!(
            ans.scope_note.as_deref(),
            Some(brief_vault_note_hit("Brief A").as_str())
        );
    }

    #[tokio::test]
    async fn brief_vault_cache_never_cross_serves() {
        // Piece 12: the semantic cache is partitioned by mode. Over
        // an empty brief, run the cross-brief question with the mode
        // OFF first — the full-vault fallback answer is cached under
        // the soft key — then ON: the cached fallback must NOT be
        // served; the hard mode refuses instead. And the reverse
        // order on a fresh service: ON first (refusal), OFF still
        // falls back.
        let q = "What notice did Beta Properties serve for rent arrears?";
        let (svc, _a_id, _b_id) = seeded_two_briefs().await;
        let off = svc
            .query_scoped(q, QueryMode::Mix, 3, Vec::new(), CitatorContext::default())
            .await
            .unwrap();
        assert!(!off.citations.is_empty(), "soft ask populates the cache");
        let on = svc
            .query_opts(q, QueryMode::Mix, 3, Some(Vec::new()), true, &brief_vault_opts(), CitatorContext::default())
            .await
            .unwrap();
        assert!(
            on.citations.is_empty(),
            "the cached full-vault answer must never serve a brief-vault ask"
        );
        assert_eq!(
            on.scope_note.as_deref(),
            Some(brief_vault_note_empty("Brief A").as_str())
        );
        // The soft answer is still intact for soft asks.
        let off2 = svc
            .query_scoped(q, QueryMode::Mix, 3, Vec::new(), CitatorContext::default())
            .await
            .unwrap();
        assert!(!off2.citations.is_empty());

        let (svc2, _a2, _b2) = seeded_two_briefs().await;
        let on_first = svc2
            .query_opts(q, QueryMode::Mix, 3, Some(Vec::new()), true, &brief_vault_opts(), CitatorContext::default())
            .await
            .unwrap();
        assert!(on_first.citations.is_empty());
        let off_after = svc2
            .query_scoped(q, QueryMode::Mix, 3, Vec::new(), CitatorContext::default())
            .await
            .unwrap();
        assert!(
            !off_after.citations.is_empty(),
            "a brief-vault refusal must not poison the soft mode"
        );
    }

    #[tokio::test]
    async fn brief_vault_with_web_keeps_corpus_brief_only() {
        // Piece 12 (d): Web stays an independent toggle — the web
        // leg's outcome note composes with the brief-vault note, and
        // corpus citations remain brief-only.
        let (svc, a_id, _b_id) = seeded_two_briefs().await;
        let ans = svc
            .query_with_web(
                "What did Acme agree to?",
                QueryMode::Mix,
                3,
                2,
                Some(vec![a_id.clone()]),
                true,
                CitatorContext::default(),
                &brief_vault_opts(),
            )
            .await
            .unwrap();
        assert!(!ans.citations.is_empty());
        assert!(
            ans.citations.iter().all(|c| c.doc_title.as_deref() == Some("nda.txt")),
            "corpus citations stay inside the brief"
        );
        let note = ans.scope_note.clone().unwrap_or_default();
        assert!(note.contains("Brief vault"), "note: {note}");
        assert!(
            note.contains("Live web search"),
            "the web outcome note composes after the brief note: {note}"
        );
        // Cross-brief with web on: corpus citations stay inside the
        // brief (any web citations are the web leg's own, allowed).
        let cross = svc
            .query_with_web(
                "What notice did Beta Properties serve for rent arrears?",
                QueryMode::Mix,
                3,
                2,
                Some(vec![a_id]),
                true,
                CitatorContext::default(),
                &brief_vault_opts(),
            )
            .await
            .unwrap();
        assert!(
            cross
                .citations
                .iter()
                .filter(|c| c.doc_id.starts_with("doc:"))
                .all(|c| c.doc_title.as_deref() == Some("nda.txt")),
            "no corpus citations from outside the brief: {:?}",
            cross.citations.iter().map(|c| &c.doc_title).collect::<Vec<_>>()
        );
        assert!(
            cross.scope_note.as_deref().is_some_and(|n| n.contains("Brief vault")),
            "note: {:?}",
            cross.scope_note
        );
    }

    #[tokio::test]
    async fn brief_vault_stream_carries_scope_note_status() {
        // Piece 12 (e): streaming parity — the scope note rides the
        // stream's Status mechanism, and the stream still completes.
        let (svc, _a_id, _b_id) = seeded_two_briefs().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<QueryEvent>();
        svc.query_stream_scoped(
            "What notice did Beta Properties serve for rent arrears?",
            QueryMode::Mix,
            3,
            false,
            3,
            Some(Vec::new()),
            &[],
            true,
            None,
            CitatorContext::default(),
            &brief_vault_opts(),
            tx,
        )
        .await
        .unwrap();
        let mut saw_note = false;
        let mut saw_done = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                QueryEvent::Status { message } => {
                    if message == brief_vault_note_empty("Brief A") {
                        saw_note = true;
                    }
                }
                QueryEvent::Done { .. } => saw_done = true,
                _ => {}
            }
        }
        assert!(saw_note, "the brief-vault scope note rides a Status event");
        assert!(saw_done, "the stream completes with Done");
    }

    #[tokio::test]
    async fn answer_scope_key_separates_citator_context() {
        // FIX 3: a cached answer bakes in its request's governing line,
        // so state/applicant must partition the cache — same context
        // replays, different context never does.
        let (svc, _) = seeded().await;
        let base = svc.answer_scope_key(QueryMode::Mix, 3, None, "fp", &CitatorContext::default(), false);
        let same = svc.answer_scope_key(QueryMode::Mix, 3, None, "fp", &CitatorContext::default(), false);
        assert_eq!(base, same);
        let mh = svc.answer_scope_key(
            QueryMode::Mix,
            3,
            None,
            "fp",
            &CitatorContext { state: Some("MH".to_string()), applicant: None },
            false,
        );
        assert_ne!(base, mh);
        let app = svc.answer_scope_key(
            QueryMode::Mix,
            3,
            None,
            "fp",
            &CitatorContext {
                state: None,
                applicant: Some("Nexa Traders Pvt Ltd".to_string()),
            },
            false,
        );
        assert_ne!(base, app);
        assert_ne!(mh, app);
        // Piece 12: brief-vault mode partitions the cache from the soft
        // mode over the identical scope — the two must never cross-serve.
        let hard = svc.answer_scope_key(QueryMode::Mix, 3, None, "fp", &CitatorContext::default(), true);
        assert_ne!(base, hard);
    }

    /// Phase 2A helper: district + reversing-HC docs indexed through
    /// the pipeline, so stored V2 edges + registry metadata exist, and
    /// the QueryService bound to those stores (as main.rs binds them).
    async fn v2_fixture() -> (QueryService, String, String) {
        let rag = Arc::new(RagAnything::new(
            RagAnythingConfig::default(),
            Arc::new(MockEmbeddings::new(16)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(MockLlm::new("[]")),
            500,
            50,
        ));
        let dist = rag
            .process_document_complete(
                "district.md",
                b"IN THE COURT OF THE DISTRICT JUDGE, LUCKNOW\nCivil Suit No. 12 of 2020\nDecided on 10 May 2021.\nThe plaintiff Mehta sued Desai for recovery of Rs. 5,00,000. The court decreed the suit.",
                Some("Mehta v. Desai — District Court (2021)".to_string()),
                None,
            )
            .await
            .unwrap();
        let hc = rag
            .process_document_complete(
                "hc.md",
                b"IN THE HIGH COURT OF JUDICATURE AT ALLAHABAD\nFirst Appeal No. 45 of 2021\nDecided on 20 June 2023.\nIn Mehta v. Desai, the appeal is allowed and the impugned judgment is set aside.",
                Some("Mehta v. Desai — Allahabad High Court (2023)".to_string()),
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
        svc.set_treatment_store(rag.treatments());
        svc.set_v2_edge_store(rag.v2_treatments());
        let rag3 = rag.clone();
        svc.set_doc_meta_source(Arc::new(move |id: &str| rag3.document_meta(id)));
        (svc, dist.doc_id, hc.doc_id)
    }

    fn v2cit(id: &str, title: &str) -> Citation {
        Citation {
            doc_title: Some(title.to_string()),
            stale: None,
            sentence: None,
            snippet: "neutral text".to_string(),
            doc_id: id.to_string(),
            chunk_id: format!("chunk-{id}"),
            ordinal: 0,
            score: 1.0,
            source_url: None,
            source_date: None,
            treatment: None,
        }
    }

    #[tokio::test]
    async fn v2_resolution_for_applies_stored_real_id_edges() {
        // The research/orchestrator helper over REAL stored edges
        // (ingest ids) + positional citations: the stored HC reversal
        // must land on the cited district doc, with positional
        // evidence attribution, and UP context must govern the HC.
        let (svc, dist, hc) = v2_fixture().await;
        let citations = vec![
            v2cit("doc:0", "Mehta v. Desai — District Court (2021)"),
            v2cit("doc:1", "Mehta v. Desai — Allahabad High Court (2023)"),
        ];
        let real = vec![Some(dist), Some(hc)];
        let res = svc.v2_resolution_for(
            &citations,
            &real,
            None,
            &CitatorContext { state: Some("UP".to_string()), applicant: None },
        );
        let by_id: std::collections::HashMap<&str, &crate::citator::CitationTreatment> = res
            .treatments
            .iter()
            .map(|(id, t)| (id.as_str(), t))
            .collect();
        assert_eq!(by_id["doc:0"].status, crate::citator::TreatmentStatus::Reversed);
        assert_eq!(by_id["doc:0"].evidence_doc_id.as_deref(), Some("doc:1"));
        assert_eq!(res.ranked_corpus_ids.first().unwrap(), "doc:1");
        assert_eq!(res.ranked_corpus_ids.last().unwrap(), "doc:0");
        let line = res.governing_line.expect("UP governing line");
        assert!(line.contains("Allahabad"), "{line}");
        let res_none = svc.v2_resolution_for(&citations, &real, None, &CitatorContext::default());
        assert!(res_none.governing_line.is_none(), "no context: state law must not govern");
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
            let ans = svc.query("What did Acme agree to?", mode, 3, CitatorContext::default()).await.unwrap();
            assert!(!ans.answer.is_empty(), "mode {mode:?} gave empty answer");
            assert_eq!(ans.mode, mode);
        }
    }

    #[tokio::test]
    async fn citator_status_attaches_and_flags_overruled() {
        let (svc, _) = seeded().await;
        let store = std::sync::Arc::new(crate::citator::TreatmentStore::new());
        store.add(vec![crate::citator::TreatmentEdge {
            target: "Nda".into(),
            kind: crate::citator::TreatmentKind::Overruled,
            source_doc_id: "later-doc".into(),
            source_chunk_id: "later-chunk".into(),
            source_title: Some("Later Judgment".into()),
            passage: "We overrule Nda.".into(),
        }]);
        svc.set_treatment_store(store);
        let ans = svc.query("What did Acme agree to?", QueryMode::Mix, 3, CitatorContext::default()).await.unwrap();
        assert!(!ans.citations.is_empty());
        let t = ans.citations[0].treatment.as_ref().expect("treatment attached");
        assert_eq!(t.status, crate::citator::TreatmentStatus::Overruled);
        assert_eq!(t.evidence_passage.as_deref(), Some("We overrule Nda."));
        // The citation stays AND the answer flags it — never a silent drop.
        assert!(ans.answer.contains("Treatment note:"));
        assert!(ans.answer.contains("was overruled in Later Judgment"));
    }

    #[tokio::test]
    async fn citator_respects_brief_scope() {
        // Edge sourced in ANOTHER brief must not leak into a
        // brief-scoped answer, even though the cited doc is in scope.
        let (svc, rag) = seeded().await;
        let cited = rag.list_documents()[0].id.clone();
        let store = std::sync::Arc::new(crate::citator::TreatmentStore::new());
        store.add(vec![crate::citator::TreatmentEdge {
            target: "Nda".into(),
            kind: crate::citator::TreatmentKind::Overruled,
            source_doc_id: "other-brief-doc".into(),
            source_chunk_id: "x".into(),
            source_title: Some("Other Brief Judgment".into()),
            passage: "We overrule Nda.".into(),
        }]);
        svc.set_treatment_store(store);
        let ans = svc
            .query_scoped("What did Acme agree to?", QueryMode::Mix, 3, vec![cited.clone()], CitatorContext::default())
            .await
            .unwrap();
        assert!(!ans.citations.is_empty(), "in-scope hits expected");
        assert!(ans.scope_note.is_none(), "no fallback: cited doc is in scope");
        let t = ans.citations[0].treatment.as_ref().expect("treatment attached");
        assert_eq!(t.status, crate::citator::TreatmentStatus::Unknown);
        assert!(!ans.answer.contains("Treatment note:"));
        // Same edge sourced INSIDE the scope is visible.
        let (svc2, rag2) = seeded().await;
        let cited2 = rag2.list_documents()[0].id.clone();
        let store2 = std::sync::Arc::new(crate::citator::TreatmentStore::new());
        store2.add(vec![crate::citator::TreatmentEdge {
            target: "Nda".into(),
            kind: crate::citator::TreatmentKind::Overruled,
            source_doc_id: cited2.clone(),
            source_chunk_id: "y".into(),
            source_title: Some("In-Scope Judgment".into()),
            passage: "We overrule Nda.".into(),
        }]);
        svc2.set_treatment_store(store2);
        let ans2 = svc2
            .query_scoped("What did Acme agree to?", QueryMode::Mix, 3, vec![cited2], CitatorContext::default())
            .await
            .unwrap();
        assert!(!ans2.citations.is_empty());
        assert_eq!(
            ans2.citations[0].treatment.as_ref().unwrap().status,
            crate::citator::TreatmentStatus::Overruled
        );
    }

    #[tokio::test]
    async fn bypass_has_no_citations() {
        let (svc, _) = seeded().await;
        let ans = svc.query("What is a contract?", QueryMode::Bypass, 3, CitatorContext::default()).await.unwrap();
        assert!(ans.citations.is_empty());
        assert!(!ans.answer.is_empty());
    }

    #[tokio::test]
    async fn verdict_first_format_for_corpus_and_web() {
        let (svc, _) = seeded().await;
        let (system, _, _, _, _, _) = svc
            .prepare_context("What did Acme agree to?", QueryMode::Mix, 3, &[], None, &[], false, &CitatorContext::default(), &AskOptions::default())
            .await
            .unwrap();
        assert!(system.contains("## Verdict"), "corpus answers must carry the verdict-first format");
        assert!(system.contains("[doc:N]"), "corpus format must cite corpus sources");
        assert!(!system.contains("[web:N]"), "corpus format must never invite web citations");
        let page = crate::web::FetchedPage {
            url: "https://example.com".into(),
            title: "Example".into(),
            text: "Acme agrees to confidentiality.".into(),
            date: None,
        };
        let (web_system, _, _, _, _, _) = svc
            .prepare_context("What did Acme agree to?", QueryMode::Mix, 3, &[page], None, &[], false, &CitatorContext::default(), &AskOptions::default())
            .await
            .unwrap();
        assert!(web_system.contains("## Verdict"), "web answers keep the verdict-first format");
        assert!(web_system.contains("WEB-AUGMENTED ANSWER FORMAT"));
        let (bypass_system, _, _, _, _, _) = svc
            .prepare_context("What is a contract?", QueryMode::Bypass, 3, &[], None, &[], false, &CitatorContext::default(), &AskOptions::default())
            .await
            .unwrap();
        assert!(!bypass_system.contains("## Verdict"), "bypass answers get no format contract");
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
        let a = svc.query("What did Acme agree to?", QueryMode::Naive, 3, CitatorContext::default()).await.unwrap();
        // Different wording, same embedding → semantic hit, no new LLM call.
        let b = svc.query("Tell me Acme's commitments", QueryMode::Naive, 3, CitatorContext::default()).await.unwrap();
        // Call accounting: the 5-word question triggers HyDE expansion
        // (call 1, whose non-JSON mock reply yields no expansions), so
        // generation is call 2. The paraphrase is served from cache
        // BEFORE any expansion, costing no calls — the behavior this
        // test exists to pin.
        assert_eq!(a.answer, "Acme agrees to confidentiality (call 2) [doc:0]");
        assert_eq!(b.answer, a.answer, "paraphrase must be served from the answer cache");
        assert_eq!(svc.answer_cache_stats().hits, 1);
        // A different brief scope is a different cache key → fresh call.
        let doc_id = svc.store.all_chunks().await.unwrap()[0].doc_id.clone();
        let c = svc
            .query_scoped("What did Acme agree to?", QueryMode::Naive, 3, vec![doc_id], CitatorContext::default())
            .await
            .unwrap();
        assert_eq!(c.answer, "Acme agrees to confidentiality (call 4) [doc:0]");
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
        let a = svc.query("What did Acme agree to?", QueryMode::Naive, 3, CitatorContext::default()).await.unwrap();
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
        let b = svc.query("What did Acme agree to?", QueryMode::Naive, 3, CitatorContext::default()).await.unwrap();
        assert!(!b.verified);
        // Generation is call 2: the short question first spends call 1
        // on the HyDE expansion attempt (mock reply is not JSON, so no
        // expansions result). The verified path above spent no calls.
        assert_eq!(b.answer, "Acme agrees to confidentiality (call 2) [doc:0]", "stale annotation must not serve");
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

    fn fusion_cfg(mode: FusionMode) -> FusionConfig {
        FusionConfig {
            mode,
            k: 20.0,
            w_vector: 1.0,
            w_bm25: 1.0,
            identifier_bm25_boost: 1.5,
            recency_weight: 0.0,
            recency_half_life_days: 3650.0,
            statute_boost: 1.0,
        }
    }

    #[test]
    fn weighted_rrf_identifier_query_trusts_exact_match() {
        let rec = |id: &str| ChunkRecord {
            id: id.into(),
            doc_id: "d".into(),
            ordinal: 0,
            text: id.into(),
            embedding: vec![],
            metadata: Default::default(),
        };
        let scored = |id: &str, s: f32| ScoredChunk { record: rec(id), score: s };
        let vec_hits = vec![scored("v1", 0.9), scored("v2", 0.8)];
        let kw = vec![("k1".to_string(), 9.0)];
        // Identifier query ("Section 11 … Contract Act"): BM25 weight is
        // boosted ×1.5, so the keyword-only exact match leads the pool.
        let fused = fuse_rankings(
            "What does Section 11 of the Contract Act say?",
            vec_hits.clone(),
            &kw,
            vec![rec("k1")],
            3,
            &fusion_cfg(FusionMode::WeightedRrf),
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(fused[0].record.id, "k1");
        // Same query with the BM25 leg downweighted: the vector leg's
        // rank 1 leads instead — the weights, not luck, decide.
        let fused = fuse_rankings(
            "What does Section 11 of the Contract Act say?",
            vec_hits,
            &kw,
            vec![rec("k1")],
            3,
            &FusionConfig { w_bm25: 0.4, ..fusion_cfg(FusionMode::WeightedRrf) },
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(fused[0].record.id, "v1");
    }

    #[test]
    fn dbsf_keeps_leg_dominant_keyword_hit() {
        let rec = |id: &str| ChunkRecord {
            id: id.into(),
            doc_id: "d".into(),
            ordinal: 0,
            text: id.into(),
            embedding: vec![],
            metadata: Default::default(),
        };
        let scored = |id: &str, s: f32| ScoredChunk { record: rec(id), score: s };
        // Flat vector leg; BM25 has one dominant exact hit (k1).
        let vec_hits = vec![scored("v1", 0.81), scored("v2", 0.80), scored("v3", 0.79)];
        let kw = vec![("k1".to_string(), 25.0), ("k2".to_string(), 2.0), ("k3".to_string(), 1.0)];
        let extra = vec![rec("k1"), rec("k2"), rec("k3")];
        let fused = fuse_rankings(
            "q",
            vec_hits,
            &kw,
            extra,
            3,
            &fusion_cfg(FusionMode::Dbsf),
            &Default::default(),
            &Default::default(),
        );
        let ids: Vec<&str> = fused.iter().map(|h| h.record.id.as_str()).collect();
        assert_eq!(ids[0], "v1");
        assert_eq!(ids[1], "k1", "dominant BM25 hit must survive distribution fusion");
    }

    #[test]
    fn recency_term_prefers_recently_effective_doc() {
        let rec = |id: &str, doc: &str| ChunkRecord {
            id: id.into(),
            doc_id: doc.into(),
            ordinal: 0,
            text: id.into(),
            embedding: vec![],
            metadata: Default::default(),
        };
        let old = ScoredChunk { record: rec("a1", "doc-old"), score: 0.9 };
        let new = ScoredChunk { record: rec("b1", "doc-new"), score: 0.88 };
        let today = chrono::Utc::now().date_naive().format("%Y-%m-%d").to_string();
        let mut dates = std::collections::HashMap::new();
        dates.insert("doc-old".to_string(), Some("2010-01-01".to_string()));
        dates.insert("doc-new".to_string(), Some(today));
        let cfg = FusionConfig { recency_weight: 2.0, ..fusion_cfg(FusionMode::WeightedRrf) };
        let fused = fuse_rankings("q", vec![old.clone(), new.clone()], &[], vec![], 2, &cfg, &dates, &Default::default());
        assert_eq!(fused[0].record.id, "b1", "recency term flips the order");
        let cfg_off = FusionConfig { recency_weight: 0.0, ..cfg };
        let fused = fuse_rankings("q", vec![old, new], &[], vec![], 2, &cfg_off, &dates, &Default::default());
        assert_eq!(fused[0].record.id, "a1", "without the term, rank order stands");
    }

    #[test]
    fn statute_term_lifts_named_act_inside_fusion() {
        // The measured A/B failure: two Acts share a section number; the
        // vector leg ranks the named Act second. The in-fusion statute
        // term must promote it BEFORE the top_k cut.
        let rec = |id: &str, doc: &str| ChunkRecord {
            id: id.into(),
            doc_id: doc.into(),
            ordinal: 0,
            text: id.into(),
            embedding: vec![],
            metadata: Default::default(),
        };
        let excise = ScoredChunk { record: rec("e1", "doc-excise"), score: 0.90 };
        let it_act = ScoredChunk { record: rec("i1", "doc-it"), score: 0.88 };
        let statute_docs: std::collections::HashSet<String> =
            ["doc-it".to_string()].into_iter().collect();
        let cfg = FusionConfig { statute_boost: 1.35, ..fusion_cfg(FusionMode::WeightedRrf) };
        let fused = fuse_rankings(
            "What does Section 86 of the IT Act deal with?",
            vec![excise.clone(), it_act.clone()],
            &[],
            vec![],
            2,
            &cfg,
            &Default::default(),
            &statute_docs,
        );
        assert_eq!(fused[0].record.id, "i1", "named Act wins inside fusion");
        let cfg_off = FusionConfig { statute_boost: 1.0, ..cfg };
        let fused = fuse_rankings(
            "What does Section 86 of the IT Act deal with?",
            vec![excise, it_act],
            &[],
            vec![],
            2,
            &cfg_off,
            &Default::default(),
            &statute_docs,
        );
        assert_eq!(fused[0].record.id, "e1", "without the term, vector order stands");
    }

    #[test]
    fn merge_legs_interleaves_by_rank_not_score() {
        let rec = |id: &str| ChunkRecord {
            id: id.into(),
            doc_id: "d".into(),
            ordinal: 0,
            text: id.into(),
            embedding: vec![],
            metadata: Default::default(),
        };
        let scored = |id: &str, s: f32| ScoredChunk { record: rec(id), score: s };
        // Vector leg: calibrated cosines. Graph leg: flat 0.85 scores
        // that would swamp a raw-score merge (the measured Mix bug).
        let vec_leg = vec![scored("v1", 0.45), scored("v2", 0.44), scored("v3", 0.43)];
        let graph_leg = vec![scored("g1", 0.85), scored("g2", 0.85), scored("v2", 0.85)];
        let merged = merge_legs(vec![vec_leg, graph_leg]);
        let ids: Vec<&str> = merged.iter().map(|h| h.record.id.as_str()).collect();
        assert_eq!(ids, vec!["v1", "g1", "v2", "g2", "v3"], "round-robin, deduped");
        // Single leg passes through in its own order.
        let merged = merge_legs(vec![vec![scored("a", 0.1), scored("b", 0.9)]]);
        assert_eq!(merged[0].record.id, "a");
        assert_eq!(merged[1].record.id, "b");
    }

    #[test]
    fn identifier_and_statute_detection() {
        assert!(query_has_identifier("What does Section 11 of the CPC provide?"));
        assert!(query_has_identifier("Explain Article 21"));
        assert!(!query_has_identifier("What makes an agreement enforceable?"));
        assert!(statute_mentions("section 378 of the ipc").contains(&"indian penal code"));
        assert!(statute_mentions("under the contract act, 1872").contains(&"indian contract act"));
        assert!(jurisdiction_mentioned("what does delhi high court say", "IN-DL"));
        assert!(jurisdiction_mentioned("indian law on bail", "IN"));
        assert!(!jurisdiction_mentioned("french law on bail", "IN"));
        // Abbreviated titles must match their statute (the Section 86
        // doc is literally titled "Section 86 IT Act").
        assert!(title_matches_statute("section 86 it act", "information technology act"));
        assert!(title_matches_statute(
            "indian contract act — sections 73-74",
            "indian contract act"
        ));
        assert!(!title_matches_statute(
            "indian contract act — sections 73-74",
            "information technology act"
        ));
        assert!(title_matches_statute("ipc — section 378 (theft)", "indian penal code"));
    }

    #[test]
    fn litm_order_puts_strongest_at_both_edges() {
        assert_eq!(litm_order(5), vec![0, 2, 4, 3, 1]);
        assert_eq!(litm_order(4), vec![0, 2, 3, 1]);
        assert_eq!(litm_order(3), vec![0, 1, 2]);
        assert_eq!(litm_order(6), vec![0, 2, 4, 5, 3, 1]);
    }

    #[test]
    fn score_gap_cutoff_trims_the_cliff() {
        std::env::set_var("LEGAL_RAG_AUTOCUT", "1");
        let rec = |id: &str| ChunkRecord {
            id: id.into(),
            doc_id: "d".into(),
            ordinal: 0,
            text: id.into(),
            embedding: vec![],
            metadata: Default::default(),
        };
        let hits: Vec<ScoredChunk> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|id| ScoredChunk { record: rec(id), score: 0.5 })
            .collect();
        let scores = vec![0.95, 0.90, 0.88, 0.20, 0.15];
        let kept = apply_score_gap_cutoff(hits.clone(), &scores);
        assert_eq!(kept.len(), 3, "tail below the cliff is cut");
        // No cliff → nothing cut.
        let flat = vec![0.90, 0.88, 0.86, 0.84, 0.82];
        let kept = apply_score_gap_cutoff(hits, &flat);
        assert_eq!(kept.len(), 5);
        std::env::remove_var("LEGAL_RAG_AUTOCUT");
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

    // --- Web leg failure isolation (Piece 1b, 2026-10-29) -------------

    struct FailingWeb;
    #[async_trait::async_trait]
    impl WebSearch for FailingWeb {
        fn name(&self) -> &str {
            "failing-test"
        }
        async fn search(
            &self,
            _q: &str,
            _m: usize,
        ) -> legal_rag_common::Result<Vec<crate::web::WebResult>> {
            Err(legal_rag_common::Error::Upstream("engine exploded".into()))
        }
        async fn fetch(&self, _u: &str) -> legal_rag_common::Result<FetchedPage> {
            Err(legal_rag_common::Error::Upstream("fetch exploded".into()))
        }
    }

    struct EmptyWeb;
    #[async_trait::async_trait]
    impl WebSearch for EmptyWeb {
        fn name(&self) -> &str {
            "empty-test"
        }
        async fn search(
            &self,
            _q: &str,
            _m: usize,
        ) -> legal_rag_common::Result<Vec<crate::web::WebResult>> {
            Ok(Vec::new())
        }
        async fn fetch(&self, _u: &str) -> legal_rag_common::Result<FetchedPage> {
            Err(legal_rag_common::Error::Upstream("no fetch expected".into()))
        }
    }

    struct OnePageWeb;
    #[async_trait::async_trait]
    impl WebSearch for OnePageWeb {
        fn name(&self) -> &str {
            "one-page-test"
        }
        async fn search(
            &self,
            _q: &str,
            _m: usize,
        ) -> legal_rag_common::Result<Vec<crate::web::WebResult>> {
            Ok(vec![crate::web::WebResult {
                url: "https://indiankanoon.org/doc/1/".into(),
                title: "Kanoon result".into(),
                snippet: String::new(),
            }])
        }
        async fn fetch(&self, url: &str) -> legal_rag_common::Result<FetchedPage> {
            Ok(FetchedPage {
                url: url.to_string(),
                title: "Kanoon result".into(),
                text: "Agricultural income is exempt under section 10(1).".into(),
                date: None,
            })
        }
    }

    struct UnfetchableWeb;
    #[async_trait::async_trait]
    impl WebSearch for UnfetchableWeb {
        fn name(&self) -> &str {
            "unfetchable-test"
        }
        async fn search(
            &self,
            _q: &str,
            _m: usize,
        ) -> legal_rag_common::Result<Vec<crate::web::WebResult>> {
            Ok(vec![crate::web::WebResult {
                url: "https://example.test/dead".into(),
                title: "Dead page".into(),
                snippet: String::new(),
            }])
        }
        async fn fetch(&self, _u: &str) -> legal_rag_common::Result<FetchedPage> {
            Err(legal_rag_common::Error::Upstream("connection reset".into()))
        }
    }

    async fn svc_with_web(web: Arc<dyn WebSearch>) -> QueryService {
        let rag = Arc::new(RagAnything::new(
            RagAnythingConfig::default(),
            Arc::new(MockEmbeddings::new(16)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(MockLlm::new(r#"["Acme"]"#)),
            500,
            50,
        ));
        rag.process_document_complete(
            "nda.txt",
            b"Acme Corp agrees to confidentiality. Acme shall not disclose.",
            None,
            None,
        )
        .await
        .unwrap();
        let rag2 = rag.clone();
        QueryService::new(
            rag2.backends().0,
            rag2.backends().1,
            rag2.backends().2,
            Arc::new(MockLlm::new("The answer is grounded [doc:0].")),
            Arc::new(move |id: &str| rag2.doc_title(id)),
            web,
            None,
        )
    }

    #[tokio::test]
    async fn gather_web_outcomes_distinguish_failure_modes() {
        let svc = svc_with_web(Arc::new(FailingWeb)).await;
        let g = svc.gather_web("anything", 3).await;
        assert!(g.pages.is_empty());
        assert!(
            matches!(g.outcome, WebOutcome::Failed(ref r) if r.contains("engine exploded")),
            "got: {:?}",
            g.outcome
        );

        let svc = svc_with_web(Arc::new(EmptyWeb)).await;
        let g = svc.gather_web("anything", 3).await;
        assert!(g.pages.is_empty());
        assert_eq!(g.outcome, WebOutcome::NoResults);

        let svc = svc_with_web(Arc::new(UnfetchableWeb)).await;
        let g = svc.gather_web("anything", 3).await;
        assert!(g.pages.is_empty());
        assert!(
            matches!(g.outcome, WebOutcome::Failed(ref r) if r.contains("candidate pages could be fetched")),
            "got: {:?}",
            g.outcome
        );

        let svc = svc_with_web(Arc::new(OnePageWeb)).await;
        let g = svc.gather_web("agricultural income", 3).await;
        assert_eq!(g.pages.len(), 1);
        assert_eq!(g.outcome, WebOutcome::Delivered);
    }

    #[tokio::test]
    async fn query_with_web_failure_answers_corpus_only_with_note() {
        let svc = svc_with_web(Arc::new(FailingWeb)).await;
        let ans = svc
            .query_with_web(
                "What does Acme agree to?",
                QueryMode::Mix,
                5,
                3,
                None,
                false,
                CitatorContext::default(),
                &AskOptions::default(),
            )
            .await
            .expect("web failure must never fail the ask");
        assert!(ans.answer.contains("grounded"), "got: {}", ans.answer);
        let note = ans.scope_note.expect("a web-unavailable note");
        assert!(
            note.contains("Live web search was unavailable"),
            "got: {note}"
        );
        assert!(
            !ans.citations.iter().any(|c| c.doc_id.starts_with("web:")),
            "no web citations on a failed leg"
        );
    }

    #[tokio::test]
    async fn query_with_web_delivered_blends_without_note() {
        let svc = svc_with_web(Arc::new(OnePageWeb)).await;
        let ans = svc
            .query_with_web(
                "What does Acme agree to?",
                QueryMode::Mix,
                5,
                3,
                None,
                false,
                CitatorContext::default(),
                &AskOptions::default(),
            )
            .await
            .unwrap();
        assert!(
            ans.citations.iter().any(|c| c.doc_id == "web:0"),
            "web citation blended: {:?}",
            ans.citations.iter().map(|c| &c.doc_id).collect::<Vec<_>>()
        );
        let note = ans.scope_note.unwrap_or_default();
        assert!(
            !note.contains("web search was unavailable")
                && !note.contains("no usable sources"),
            "no failure note on a delivered leg, got: {note}"
        );
    }
}
