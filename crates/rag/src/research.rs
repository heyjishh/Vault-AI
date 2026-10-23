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
//! 4. **Report** — conclusion / basis / conflicts / open points, all cited.
//!    Anything without evidence lands in open points, never in the
//!    conclusion.

use serde::{Deserialize, Serialize};

use legal_rag_common::Result;

use crate::graph::{parse_llm_json, repair_escapes};
use crate::query::{Citation, QueryMode, QueryService};
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
    pub citations: Vec<Citation>,
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
both positions quoted. List open points: questions the evidence could not resolve. Respond with ONLY a \
JSON object: {\"conclusion\":\"...\",\"assessments\":[{\"index\":0,\"status\":\"supported|refuted|inconclusive\",\
\"key_claims\":[{\"claim\":\"...\",\"source\":\"[doc:0]\"}]}],\"conflicts\":[{\"topic\":\"...\",\"position_a\":\"...\",\
\"position_b\":\"...\"}],\"open_points\":[\"...\"]}. Never assert as fact anything without cited evidence.";

impl QueryService {
    /// Run the epistemic loop over a research question.
    pub async fn research(&self, question: &str, use_web: bool) -> Result<ResearchReport> {
        // 1. Frame.
        let frame_raw = self
            .llm()
            .complete(
                &self.research_frame_system(FRAME_SYSTEM),
                &format!("Research question: {question}"),
            )
            .await?;
        let frame: FrameOutput = parse_llm_json(&frame_raw).unwrap_or(FrameOutput {
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
        let mut evidence_blocks: Vec<String> = Vec::new();
        // Per-hypothesis doc/web id offsets so citations stay unique.
        let mut doc_offset = 0usize;
        let mut web_offset = 0usize;
        let mut hyp_evidence: Vec<Vec<(String, String, String)>> = Vec::new(); // (text, cite_id, title)

        for (hi, h) in hyps.iter().enumerate() {
            let (hits, graph_ctx) = self.retrieve(&h.question, QueryMode::Mix, 5, None).await?;
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
                });
            }
            doc_offset += hits.len();

            if use_web {
                let pages: Vec<FetchedPage> = self.gather_web(&h.question, 2).await;
                for (i, page) in pages.iter().enumerate() {
                    let id = web_offset + i;
                    let text: String = page.text.chars().take(1200).collect();
                    parts.push(format!("[web:{id}] {} ({})\n{text}", page.title, page.url));
                    ev.push((
                        text.chars().take(200).collect(),
                        format!("[web:{id}]"),
                        page.title.clone(),
                    ));
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
                    });
                }
                web_offset += pages.len();
            }

            hyp_evidence.push(ev);
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

        // 3. Adjudicate.
        let adj_raw = self
            .llm()
            .complete(
                &self.research_adjudicate_system(ADJUDICATE_SYSTEM),
                &format!(
                    "Research question: {question}\n\n{}",
                    evidence_blocks.join("\n\n---\n\n")
                ),
            )
            .await?;
        // Tolerate fence-wrapped or slightly dirty JSON.
        let adj: Adjudication = parse_llm_json(&adj_raw)
            .or_else(|| parse_llm_json(&repair_escapes(&adj_raw)))
            .unwrap_or(Adjudication {
                conclusion: "Evidence was insufficient to adjudicate.".into(),
                assessments: vec![],
                conflicts: vec![],
                open_points: vec!["Adjudication output could not be parsed.".into()],
            });

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
            open_points: adj.open_points,
            citations: all_citations,
        })
    }
}
