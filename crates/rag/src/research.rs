//! Research mode — the epistemic loop.
//!
//! One-shot RAG answers "what does the corpus say". Research answers "what
//! do we actually know, and on what grounds". The loop, adapted from the
//! methodology of clearai-dsh (which itself is a DSH-platform plugin and
//! cannot be embedded here, so the loop is implemented natively):
//!
//! 1. **Frame** — the question is decomposed into 2–4 candidate
//!    sub-questions (hypotheses), each with a testable prediction.
//! 2. **Evidence** — every candidate runs through retrieval (corpus, plus
//!    web when enabled). Each claim keeps its citations.
//! 3. **Adjudicate** — evidence is weighed per candidate: supported,
//!    refuted, or inconclusive. Where sources disagree a **conflict** is
//!    recorded — a conflict is a finding, not noise, and refuted
//!    hypotheses are kept, not deleted.
//! 4. **Report** — conclusion / basis / conflicts / open points /
//!    decisions, all cited. Anything without evidence lands in open
//!    points, never in the conclusion. Verdicts are enforced against
//!    the delivered evidence in code (a hypothesis with no delivered
//!    evidence is inconclusive, never supported), and every leg of
//!    the loop is failure-isolated: a dead leg degrades the report,
//!    never the request.

use serde::{Deserialize, Serialize};
use tracing::warn;

use legal_rag_common::Result;

use crate::graph::{parse_llm_json, repair_escapes};
use crate::query::{CitatorContext, Citation, QueryMode, QueryService, WebOutcome};
use crate::web::FetchedPage;

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceNote {
    pub claim: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hypothesis {
    pub question: String,
    pub prediction: String,
    pub status: String,
    pub evidence: Vec<EvidenceNote>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Conflict {
    pub topic: String,
    pub position_a: String,
    pub position_b: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResearchReport {
    pub question: String,
    pub conclusion: String,
    pub hypotheses: Vec<Hypothesis>,
    pub conflicts: Vec<Conflict>,
    pub open_points: Vec<String>,
    /// The fourth part of the clearai-dsh answer shape (conclusion /
    /// basis / open points / **for you to decide**): questions only
    /// the user can settle. Every recorded conflict lands here —
    /// keeping or retracting a conflicted reading is a human call.
    #[serde(default)]
    pub decisions: Vec<String>,
    /// Set when web evidence was requested but the web leg did not
    /// deliver: the report is corpus-only, and says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_note: Option<String>,
    pub citations: Vec<Citation>,
    /// V2 governing authority for the cited evidence, when the switch
    /// is on and one governs (None otherwise — never invented).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governing_line: Option<String>,
}

#[derive(Deserialize)]
struct FramedHypothesis {
    #[serde(default)]
    question: String,
    #[serde(default)]
    prediction: String,
}

#[derive(Deserialize)]
struct FrameOutput {
    #[serde(default)]
    hypotheses: Vec<FramedHypothesis>,
}

#[derive(Deserialize)]
struct KeyClaim {
    #[serde(default)]
    claim: String,
    #[serde(default)]
    source: String,
}

#[derive(Deserialize)]
struct Assessment {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    status: String,
    #[serde(default)]
    key_claims: Vec<KeyClaim>,
}

#[derive(Deserialize)]
struct Adjudication {
    #[serde(default)]
    conclusion: String,
    #[serde(default)]
    assessments: Vec<Assessment>,
    #[serde(default)]
    conflicts: Vec<ConflictDe>,
    #[serde(default)]
    open_points: Vec<String>,
    #[serde(default)]
    decisions: Vec<String>,
}

#[derive(Deserialize)]
struct ConflictDe {
    #[serde(default)]
    topic: String,
    #[serde(default)]
    position_a: String,
    #[serde(default)]
    position_b: String,
}

pub const FRAME_SYSTEM: &str = "You are a legal research planner. Break the user's research question into \
2 to 4 candidate sub-questions (hypotheses) that can each be answered from legal sources. For each, \
write a one-sentence prediction of what the evidence will show. Respond with ONLY a JSON object: \
{\"hypotheses\":[{\"question\":\"...\",\"prediction\":\"...\"}]}.";

pub const ADJUDICATE_SYSTEM: &str = "You are a rigorous legal research adjudicator. For each hypothesis, weigh \
the cited evidence and mark it supported, refuted, or inconclusive. Extract the key claims with their \
citation ids (like [doc:0] or [web:1]). Flag conflicts: topics where sources genuinely disagree, with \
both positions quoted. List open points: questions the evidence could not resolve. List decisions: \
questions only the user can settle, including which side of a conflict to rely on. Respond with ONLY a \
JSON object: {\"conclusion\":\"...\",\"assessments\":[{\"index\":0,\"status\":\"supported|refuted|inconclusive\",\
\"key_claims\":[{\"claim\":\"...\",\"source\":\"[doc:0]\"}]}],\"conflicts\":[{\"topic\":\"...\",\"position_a\":\"...\",\
\"position_b\":\"...\"}],\"open_points\":[\"...\"],\"decisions\":[\"...\"]}. Never assert as fact anything without cited evidence.";

impl QueryService {
    /// Run the epistemic loop over a research question. `ctx` carries
    /// the request's state/applicant for V2 governing selection (the
    /// Ask route's normalization applies at the API boundary).
    pub async fn research(
        &self,
        question: &str,
        use_web: bool,
        ctx: CitatorContext,
    ) -> Result<ResearchReport> {
        // Degradations are findings, not failures: every leg that could
        // not run is recorded here and lands in the report's open
        // points. The request only errors if a stage *outside* the
        // loop (a programming error) fails — a dead LLM, a dead
        // retrieval leg, or a dead web leg degrades the report, never
        // surfaces as a bare 502. (The pre-fix behavior — `?` on every
        // leg — is exactly how a single bridge failure killed whole
        // research requests, e.g. the BNS research 502 of 2026-10-25.)
        let mut degraded: Vec<String> = Vec::new();

        // 1. Frame. Predictions are written per hypothesis here,
        // BEFORE any evidence is gathered (the epistemic contract).
        let frame_raw = match self
            .llm()
            .complete(
                &self.research_frame_system(FRAME_SYSTEM),
                &format!("Research question: {question}"),
            )
            .await
        {
            Ok(r) => Some(r),
            Err(e) => {
                warn!(stage = "frame", error = %e, "research framing failed; degrading");
                degraded.push(format!(
                    "Framing failed ({e}); the question was researched as a single hypothesis."
                ));
                None
            }
        };
        let frame: FrameOutput = frame_raw
            .as_deref()
            .and_then(parse_llm_json)
            .unwrap_or(FrameOutput {
                hypotheses: vec![FramedHypothesis {
                    question: question.to_string(),
                    prediction: String::new(),
                }],
            });
        let mut hyps: Vec<FramedHypothesis> = frame.hypotheses.into_iter().take(4).collect();
        if hyps.is_empty() {
            hyps.push(FramedHypothesis {
                question: question.to_string(),
                prediction: String::new(),
            });
        }

        // 2. Evidence — each candidate through retrieval (+ web).
        let mut all_citations: Vec<Citation> = Vec::new();
        // Real doc ids aligned with all_citations by index (None for
        // web citations) — the citator needs them for stored metadata.
        let mut real_doc_ids: Vec<Option<String>> = Vec::new();
        let mut evidence_blocks: Vec<String> = Vec::new();
        // Per-hypothesis doc/web id offsets so citations stay unique.
        let mut doc_offset = 0usize;
        let mut web_offset = 0usize;
        let mut hyp_evidence: Vec<Vec<(String, String, String)>> = Vec::new(); // (text, cite_id, title)
        // Delivered-evidence count per hypothesis (corpus hits + web
        // pages actually handed to the adjudicator). Verdicts are
        // enforced against this below: no delivered evidence means
        // inconclusive, whatever the adjudicator claims.
        let mut hyp_evidence_counts: Vec<usize> = Vec::new();
        // First non-delivered web outcome across hypotheses (the leg
        // is shared, so one failure explains them all).
        let mut web_failure: Option<WebOutcome> = None;

        for (hi, h) in hyps.iter().enumerate() {
            let (hits, graph_ctx) = match self
                .retrieve(&h.question, QueryMode::Mix, 5, None)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    warn!(stage = "retrieve", hypothesis = hi, error = %e, "research retrieval failed");
                    degraded.push(format!(
                        "Retrieval failed for hypothesis {} (\"{}\"): {e}",
                        hi + 1,
                        h.question
                    ));
                    (Vec::new(), String::new())
                }
            };
            let mut parts: Vec<String> = Vec::new();
            let mut ev: Vec<(String, String, String)> = Vec::new();
            if !graph_ctx.is_empty() {
                parts.push(format!("Graph context:\n{graph_ctx}"));
            }
            for (i, hit) in hits.iter().enumerate() {
                let id = doc_offset + i;
                let text: String = hit.record.text.chars().take(1200).collect();
                parts.push(format!("[doc:{id}]:\n{text}"));
                ev.push((text.chars().take(200).collect(), format!("[doc:{id}]"), String::new()));
                real_doc_ids.push(Some(hit.record.doc_id.clone()));
                all_citations.push(Citation {
                    doc_title: (self.doc_titles())(&hit.record.doc_id),
                    stale: self.stale_of(&hit.record.doc_id),
                    sentence: crate::query::best_sentence(&hit.record.text, &h.question),
                    snippet: hit.record.text.chars().take(240).collect(),
                    doc_id: format!("doc:{id}"),
                    chunk_id: hit.record.id.clone(),
                    ordinal: hit.record.ordinal,
                    score: hit.score,
                    source_url: None,
                    source_date: None,
                    treatment: None,
                });
            }
            doc_offset += hits.len();

            let mut web_pages_len = 0usize;
            if use_web {
                let gather = self.gather_web(&h.question, 2).await;
                if gather.outcome != WebOutcome::Delivered && web_failure.is_none() {
                    web_failure = Some(gather.outcome.clone());
                }
                let pages: Vec<FetchedPage> = gather.pages;
                web_pages_len = pages.len();
                for (i, page) in pages.iter().enumerate() {
                    let id = web_offset + i;
                    let text: String = page.text.chars().take(1200).collect();
                    parts.push(format!("[web:{id}] {} ({})\n{text}", page.title, page.url));
                    ev.push((
                        text.chars().take(200).collect(),
                        format!("[web:{id}]"),
                        page.title.clone(),
                    ));
                    real_doc_ids.push(None);
                    all_citations.push(Citation {
                        doc_title: Some(page.title.clone()),
                        stale: None,
                        sentence: crate::query::best_sentence(&text, &h.question),
                        snippet: text.chars().take(240).collect(),
                        doc_id: format!("web:{id}"),
                        chunk_id: page.url.clone(),
                        ordinal: id,
                        score: 0.7,
                        source_url: Some(page.url.clone()),
                        source_date: page.date.clone(),
                        treatment: None,
                    });
                }
                web_offset += pages.len();
            }

            hyp_evidence.push(ev);
            hyp_evidence_counts.push(hits.len() + web_pages_len);
            evidence_blocks.push(format!(
                "Hypothesis {}: {}\nPrediction: {}\nEvidence:\n{}",
                hi,
                h.question,
                h.prediction,
                if parts.is_empty() {
                    "(no evidence retrieved)".to_string()
                } else {
                    parts.join("\n\n")
                }
            ));
        }

        // The web leg's report-level note (first failure explains the
        // shared leg for every hypothesis).
        let web_note: Option<String> = web_failure.map(|o| match o {
            WebOutcome::Failed(r) => format!(
                "Live web search was unavailable ({r}); this report is based only on the vault."
            ),
            WebOutcome::NoResults => {
                "Live web search found no usable sources; this report is based only on the vault."
                    .to_string()
            }
            WebOutcome::Delivered => {
                unreachable!("only non-delivered outcomes are recorded")
            }
        });

        // 3. Adjudicate. A failed adjudication call degrades to the
        // deterministic fallback (everything inconclusive, the gathered
        // evidence still cited) — never a request-level error.
        let adj_raw = match self
            .llm()
            .complete(
                &self.research_adjudicate_system(ADJUDICATE_SYSTEM),
                &format!(
                    "Research question: {question}\n\n{}",
                    evidence_blocks.join("\n\n---\n\n")
                ),
            )
            .await
        {
            Ok(r) => Some(r),
            Err(e) => {
                warn!(stage = "adjudicate", error = %e, "research adjudication failed; degrading");
                degraded.push(format!(
                    "Adjudication failed ({e}); every hypothesis is reported as inconclusive."
                ));
                None
            }
        };
        // Tolerate fence-wrapped or slightly dirty JSON.
        let mut adj: Adjudication = adj_raw
            .as_deref()
            .and_then(|raw| {
                parse_llm_json(raw).or_else(|| parse_llm_json(&repair_escapes(raw)))
            })
            .unwrap_or(Adjudication {
                conclusion: if adj_raw.is_none() {
                    "Adjudication could not be completed, so no conclusion could be drawn \
                     from the gathered evidence."
                        .into()
                } else {
                    "Evidence was insufficient to adjudicate.".into()
                },
                assessments: vec![],
                conflicts: vec![],
                open_points: vec!["Adjudication output could not be parsed.".into()],
                decisions: vec![],
            });

        // 3b. Enforce the epistemic contract in code, not on the
        // model's say-so (upstream clearai-dsh: completion is computed
        // from delivered evidence — the model's tools hold no field in
        // which it can declare the work done). Three hard rules:
        // a claim may only cite delivered evidence; a hypothesis with
        // no delivered evidence is inconclusive, never supported or
        // refuted; and with no delivered evidence at all there is no
        // conclusion, whatever the adjudicator wrote. Refuted
        // hypotheses are kept — a refutation is a result.
        let delivered_ids: std::collections::HashSet<&str> =
            all_citations.iter().map(|c| c.doc_id.as_str()).collect();
        let total_delivered: usize = hyp_evidence_counts.iter().sum();
        let mut enforced: Vec<Assessment> = Vec::new();
        for mut a in adj.assessments {
            if a.index >= hyps.len() {
                continue; // assessment for a hypothesis that does not exist
            }
            let verdict = a.status.trim().to_lowercase();
            let is_verdict = verdict == "supported" || verdict == "refuted";
            let before = a.key_claims.len();
            a.key_claims.retain(|k| {
                let id = k
                    .source
                    .trim()
                    .trim_matches(|c| c == '[' || c == ']' || c == ' ');
                delivered_ids.contains(id)
            });
            let dropped = before - a.key_claims.len();
            if dropped > 0 {
                adj.open_points.push(format!(
                    "Hypothesis {}: the adjudicator cited {dropped} source(s) outside the \
                     delivered evidence; those claims were disregarded.",
                    a.index + 1
                ));
            }
            if is_verdict && hyp_evidence_counts[a.index] == 0 {
                adj.open_points.push(format!(
                    "Hypothesis {} (\"{}\") had no delivered evidence and is reported as \
                     inconclusive.",
                    a.index + 1,
                    hyps[a.index].question
                ));
                a.status = "inconclusive".to_string();
            } else if is_verdict && before > 0 && a.key_claims.is_empty() {
                adj.open_points.push(format!(
                    "Hypothesis {} was marked {verdict} on citations outside the delivered \
                     evidence; reported as inconclusive.",
                    a.index + 1
                ));
                a.status = "inconclusive".to_string();
            }
            // A forced inconclusive whose claims all fell away drops
            // its assessment, so the report falls back to the raw
            // delivered excerpts — what was actually seen, unweighed.
            if a.status == "inconclusive" && a.key_claims.is_empty() && before > 0 {
                continue;
            }
            enforced.push(a);
        }
        adj.assessments = enforced;
        if total_delivered == 0 {
            adj.conclusion =
                "The available evidence was insufficient to reach a conclusion.".to_string();
            adj.open_points.push(
                "No evidence could be retrieved from the vault or the web for any hypothesis."
                    .to_string(),
            );
        }
        // A framing whose predictions cannot tell the hypotheses apart
        // (identical, or all missing) is itself an open point: the
        // evidence gathered under it may not settle which is right.
        if hyps.len() >= 2 {
            let preds: Vec<&str> =
                hyps.iter().map(|h| h.prediction.trim()).collect();
            if preds.iter().all(|p| p.is_empty())
                || preds.iter().all(|p| *p == preds[0])
            {
                adj.open_points.push(
                    "The hypotheses' predictions do not discriminate between them \
                     (identical or missing), so the evidence may not settle which is right."
                        .to_string(),
                );
            }
        }

        // Citator V2 over the cited evidence (switch on only; when off
        // the report is exactly as before — treatments None, original
        // order, no governing line). Citation doc_ids are unchanged by
        // rank-down, so the adjudicator's [doc:N] markers keep
        // resolving; web citations keep treatment None and hold their
        // relative order after the ranked corpus evidence. Note the
        // adjudicator above weighed unannotated evidence — treatments
        // surface on the report, they do not rewrite the adjudication.
        let mut all_citations = all_citations;
        let mut governing_line: Option<String> = None;
        if crate::citator_v2::citator_v2_enabled() {
            let res = self.v2_resolution_for(&all_citations, &real_doc_ids, None, &ctx);
            let by_id: std::collections::HashMap<&str, &crate::citator::CitationTreatment> = res
                .treatments
                .iter()
                .map(|(id, t)| (id.as_str(), t))
                .collect();
            for c in all_citations.iter_mut() {
                if let Some(t) = by_id.get(c.doc_id.as_str()) {
                    c.treatment = Some((*t).clone());
                }
            }
            let rank: std::collections::HashMap<&str, usize> = res
                .ranked_corpus_ids
                .iter()
                .enumerate()
                .map(|(i, id)| (id.as_str(), i))
                .collect();
            all_citations.sort_by(|a, b| {
                let ga = if a.doc_id.starts_with("doc:") { 0 } else { 1 };
                let gb = if b.doc_id.starts_with("doc:") { 0 } else { 1 };
                ga.cmp(&gb).then_with(|| {
                    let ra = rank.get(a.doc_id.as_str()).copied().unwrap_or(usize::MAX);
                    let rb = rank.get(b.doc_id.as_str()).copied().unwrap_or(usize::MAX);
                    ra.cmp(&rb)
                })
            });
            governing_line = res.governing_line;
        }

        // 4. Report.
        let hypotheses = hyps
            .into_iter()
            .enumerate()
            .map(|(i, h)| {
                let a = adj.assessments.iter().find(|a| a.index == i);
                Hypothesis {
                    question: h.question,
                    prediction: h.prediction,
                    status: a
                        .map(|a| a.status.clone())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "inconclusive".into()),
                    evidence: a
                        .map(|a| {
                            a.key_claims
                                .iter()
                                .map(|k| EvidenceNote {
                                    claim: k.claim.clone(),
                                    source: k.source.clone(),
                                })
                                .collect()
                        })
                        .unwrap_or_else(|| {
                            hyp_evidence[i]
                                .iter()
                                .map(|(t, c, _)| EvidenceNote {
                                    claim: t.clone(),
                                    source: c.clone(),
                                })
                                .collect()
                        }),
                }
            })
            .collect();

        // Decisions: the adjudicator's, plus one per recorded
        // conflict — keeping or retracting a conflicted reading is
        // the user's call, never the system's.
        let mut decisions = adj.decisions.clone();
        for c in &adj.conflicts {
            let d = format!(
                "Decide which position to rely on — {}: {} vs {}",
                c.topic, c.position_a, c.position_b
            );
            if !decisions.contains(&d) {
                decisions.push(d);
            }
        }
        // Open points: the adjudicator's and the enforcement notes
        // (both already in adj.open_points), then leg degradations
        // and the web note — deduped, order preserved. Nothing that
        // went wrong or stayed unexplained is dropped on the floor.
        let mut open_points = adj.open_points;
        for extra in degraded.iter().chain(web_note.iter()) {
            if !open_points.contains(extra) {
                open_points.push(extra.clone());
            }
        }

        Ok(ResearchReport {
            question: question.to_string(),
            conclusion: adj.conclusion,
            hypotheses,
            conflicts: adj
                .conflicts
                .into_iter()
                .map(|c| Conflict {
                    topic: c.topic,
                    position_a: c.position_a,
                    position_b: c.position_b,
                })
                .collect(),
            open_points,
            decisions,
            web_note,
            citations: all_citations,
            governing_line,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RagAnythingConfig;
    use crate::embeddings::MockEmbeddings;
    use crate::graph::InMemoryGraphStore;
    use crate::llm::{LlmClient, MockLlm};
    use crate::pipeline::RagAnything;
    use crate::vector_store::InMemoryVectorStore;
    use std::sync::Arc;

    /// LLM scripted by system-prompt routing: the frame call gets
    /// `frame`, the adjudication call gets `adjudicate`, every other
    /// call (retrieval keyword extraction etc.) gets "[]".
    struct ScriptedLlm {
        frame: std::result::Result<String, String>,
        adjudicate: std::result::Result<String, String>,
    }

    #[async_trait::async_trait]
    impl LlmClient for ScriptedLlm {
        async fn complete(&self, system: &str, _user: &str) -> Result<String> {
            let pick = if system.contains("research planner") {
                &self.frame
            } else if system.contains("research adjudicator") {
                &self.adjudicate
            } else {
                return Ok("[]".to_string());
            };
            pick.clone()
                .map_err(legal_rag_common::Error::Upstream)
        }
    }

    struct FailingWeb;
    #[async_trait::async_trait]
    impl crate::web::WebSearch for FailingWeb {
        fn name(&self) -> &str {
            "failing-test"
        }
        async fn search(
            &self,
            _q: &str,
            _m: usize,
        ) -> Result<Vec<crate::web::WebResult>> {
            Err(legal_rag_common::Error::Upstream("engine exploded".into()))
        }
        async fn fetch(&self, _u: &str) -> Result<FetchedPage> {
            Err(legal_rag_common::Error::Upstream("fetch exploded".into()))
        }
    }

    async fn research_svc(
        llm: Arc<dyn LlmClient>,
        web: Arc<dyn crate::web::WebSearch>,
        seed_doc: bool,
    ) -> QueryService {
        let rag = Arc::new(RagAnything::new(
            RagAnythingConfig::default(),
            Arc::new(MockEmbeddings::new(16)),
            Arc::new(InMemoryVectorStore::new()),
            Arc::new(InMemoryGraphStore::new()),
            Arc::new(MockLlm::new("[]")),
            500,
            50,
        ));
        if seed_doc {
            rag.process_document_complete(
                "nda.txt",
                b"Acme Corp agrees to confidentiality. Acme shall not disclose trade secrets.",
                None,
                None,
            )
            .await
            .unwrap();
        }
        let rag2 = rag.clone();
        QueryService::new(
            rag2.backends().0,
            rag2.backends().1,
            rag2.backends().2,
            llm,
            Arc::new(move |id: &str| rag2.doc_title(id)),
            web,
            None,
        )
    }

    const FRAME_ONE: &str = r#"{"hypotheses":[{"question":"Does Acme Corp agree to confidentiality?","prediction":"The evidence will show Acme agreed to confidentiality."}]}"#;
    const ADJ_EMPTY: &str =
        r#"{"conclusion":"Nothing could be concluded.","assessments":[],"conflicts":[],"open_points":[]}"#;

    #[tokio::test]
    async fn research_frame_failure_degrades_to_single_hypothesis() {
        let svc = research_svc(
            Arc::new(ScriptedLlm {
                frame: Err("llm down".into()),
                adjudicate: Ok(ADJ_EMPTY.into()),
            }),
            Arc::new(crate::web::NoopWebSearch),
            false,
        )
        .await;
        let report = svc
            .research("Is agricultural income taxable?", false, CitatorContext::default())
            .await
            .expect("frame failure must degrade, not error");
        assert_eq!(report.hypotheses.len(), 1);
        assert_eq!(report.hypotheses[0].question, "Is agricultural income taxable?");
        assert!(
            report.open_points.iter().any(|p| p.contains("Framing failed")),
            "open points: {:?}",
            report.open_points
        );
    }

    #[tokio::test]
    async fn research_adjudicate_failure_degrades_to_inconclusive() {
        let svc = research_svc(
            Arc::new(ScriptedLlm {
                frame: Ok(FRAME_ONE.into()),
                adjudicate: Err("bridge dead".into()),
            }),
            Arc::new(crate::web::NoopWebSearch),
            true,
        )
        .await;
        let report = svc
            .research("Does Acme Corp agree to confidentiality?", false, CitatorContext::default())
            .await
            .expect("adjudication failure must degrade, not error");
        assert!(
            report.hypotheses.iter().all(|h| h.status == "inconclusive"),
            "statuses: {:?}",
            report.hypotheses.iter().map(|h| &h.status).collect::<Vec<_>>()
        );
        assert!(
            report.conclusion.contains("no conclusion could be drawn"),
            "conclusion: {}",
            report.conclusion
        );
        assert!(
            report.open_points.iter().any(|p| p.contains("Adjudication failed")),
            "open points: {:?}",
            report.open_points
        );
    }

    #[tokio::test]
    async fn research_supported_without_evidence_is_forced_inconclusive() {
        let svc = research_svc(
            Arc::new(ScriptedLlm {
                frame: Ok(FRAME_ONE.into()),
                adjudicate: Ok(
                    r#"{"conclusion":"It is exempt.","assessments":[{"index":0,"status":"supported","key_claims":[{"claim":"Section 10(1) exempts it","source":"[doc:0]"}]}],"conflicts":[],"open_points":[]}"#
                        .into(),
                ),
            }),
            Arc::new(crate::web::NoopWebSearch),
            false, // empty vault: nothing can be delivered
        )
        .await;
        let report = svc
            .research("Does Acme Corp agree to confidentiality?", false, CitatorContext::default())
            .await
            .unwrap();
        assert_eq!(report.hypotheses[0].status, "inconclusive");
        assert_eq!(
            report.conclusion,
            "The available evidence was insufficient to reach a conclusion."
        );
        assert!(
            report
                .open_points
                .iter()
                .any(|p| p.contains("no delivered evidence")),
            "open points: {:?}",
            report.open_points
        );
    }

    #[tokio::test]
    async fn research_refuted_is_kept_and_conflicts_become_decisions() {
        let svc = research_svc(
            Arc::new(ScriptedLlm {
                frame: Ok(FRAME_ONE.into()),
                adjudicate: Ok(
                    r#"{"conclusion":"Refuted on the documents.","assessments":[{"index":0,"status":"refuted","key_claims":[{"claim":"The NDA imposes a confidentiality duty","source":"[doc:0]"}]}],"conflicts":[{"topic":"Confidentiality duty","position_a":"A duty exists","position_b":"No duty exists"}],"open_points":[],"decisions":[]}"#
                        .into(),
                ),
            }),
            Arc::new(crate::web::NoopWebSearch),
            true,
        )
        .await;
        let report = svc
            .research("Does Acme Corp agree to confidentiality?", false, CitatorContext::default())
            .await
            .unwrap();
        assert_eq!(report.hypotheses.len(), 1);
        assert_eq!(
            report.hypotheses[0].status, "refuted",
            "a refuted hypothesis is a result and is kept"
        );
        assert!(
            !report.hypotheses[0].evidence.is_empty(),
            "delivered claims survive enforcement"
        );
        assert!(
            report
                .decisions
                .iter()
                .any(|d| d.contains("Decide which position to rely on")),
            "decisions: {:?}",
            report.decisions
        );
    }

    #[tokio::test]
    async fn research_web_failure_is_noted_and_never_fatal() {
        let svc = research_svc(
            Arc::new(ScriptedLlm {
                frame: Ok(FRAME_ONE.into()),
                adjudicate: Ok(ADJ_EMPTY.into()),
            }),
            Arc::new(FailingWeb),
            false,
        )
        .await;
        let report = svc
            .research("Does Acme Corp agree to confidentiality?", true, CitatorContext::default())
            .await
            .expect("web failure must degrade, not error");
        let note = report.web_note.expect("web note on the report");
        assert!(note.contains("unavailable"), "note: {note}");
        assert!(
            report.open_points.iter().any(|p| p.contains("unavailable")),
            "open points carry the note too: {:?}",
            report.open_points
        );
    }

    #[tokio::test]
    async fn research_identical_predictions_are_flagged() {
        let svc = research_svc(
            Arc::new(ScriptedLlm {
                frame: Ok(
                    r#"{"hypotheses":[{"question":"Is it exempt?","prediction":"Same."},{"question":"Is it taxable?","prediction":"Same."}]}"#
                        .into(),
                ),
                adjudicate: Ok(ADJ_EMPTY.into()),
            }),
            Arc::new(crate::web::NoopWebSearch),
            false,
        )
        .await;
        let report = svc
            .research("Is agricultural income taxable?", false, CitatorContext::default())
            .await
            .unwrap();
        assert!(
            report
                .open_points
                .iter()
                .any(|p| p.contains("do not discriminate")),
            "open points: {:?}",
            report.open_points
        );
    }
}
