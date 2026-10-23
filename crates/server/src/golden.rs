//! Golden ruleset: curated evaluation questions with expected answer
//! terms and expected sources. Real user queries are captured (activity)
//! and promoted into rules; running the set against the live RAG shows
//! whether retrieval quality is improving or regressing — the feedback
//! loop that keeps enhancing the corpus and prompts. Persisted to
//! `golden.json` in the data directory.

use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleResult {
    pub passed: bool,
    pub missing_terms: Vec<String>,
    pub missing_sources: Vec<String>,
    pub answer_excerpt: String,
    pub citations: usize,
    pub ran_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenRule {
    pub id: String,
    pub question: String,
    pub expected_terms: Vec<String>,
    pub expected_sources: Vec<String>,
    pub notes: String,
    /// manual | captured
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub last_result: Option<RuleResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub ran_at: DateTime<Utc>,
    pub total: usize,
    pub passed: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct GoldenData {
    rules: Vec<GoldenRule>,
    last_run: Option<RunSummary>,
}

pub struct GoldenStore {
    data: RwLock<GoldenData>,
    path: PathBuf,
}

impl GoldenStore {
    pub fn load(path: PathBuf) -> Self {
        let data: GoldenData = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { data: RwLock::new(data), path }
    }

    fn save(&self) {
        if let Ok(data) = self.data.read() {
            if let Ok(bytes) = serde_json::to_vec_pretty(&*data) {
                let tmp = self.path.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(tmp, &self.path);
                }
            }
        }
    }

    pub fn list(&self) -> Vec<GoldenRule> {
        self.data.read().unwrap().rules.clone()
    }

    pub fn last_run(&self) -> Option<RunSummary> {
        self.data.read().unwrap().last_run.clone()
    }

    pub fn add(&self, rule: GoldenRule) -> GoldenRule {
        {
            let mut data = self.data.write().unwrap();
            data.rules.push(rule.clone());
        }
        self.save();
        rule
    }

    pub fn delete(&self, id: &str) -> bool {
        let removed = {
            let mut data = self.data.write().unwrap();
            let before = data.rules.len();
            data.rules.retain(|r| r.id != id);
            data.rules.len() != before
        };
        if removed {
            self.save();
        }
        removed
    }

    pub fn set_result(&self, id: &str, result: RuleResult) {
        {
            let mut data = self.data.write().unwrap();
            if let Some(rule) = data.rules.iter_mut().find(|r| r.id == id) {
                rule.last_result = Some(result);
            }
        }
        self.save();
    }

    pub fn set_run_summary(&self, summary: RunSummary) {
        {
            let mut data = self.data.write().unwrap();
            data.last_run = Some(summary);
        }
        self.save();
    }
}

/// Evaluate one rule against a produced answer + its citation titles.
/// Pass = every expected term appears in the answer and every expected
/// source matches some citation (title or doc id, substring, any case).
/// Normalize for term matching: lowercase, punctuation becomes a space,
/// whitespace collapsed. Handles Devanagari (letters are alphanumeric;
/// the danda becomes a separator).
fn normalize(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "that", "this", "those", "these", "of", "to", "in", "on", "for", "is",
    "are", "was", "were", "be", "been", "it", "its", "by", "at", "as",
];

/// An expected term matches when its normalized form appears in the
/// answer, OR when every content token of the term appears somewhere in
/// the answer (order-insensitive). This scores the substance of the
/// answer rather than its punctuation and clause order — an exact
/// substring check failed correct answers for writing "Fair, just, and
/// reasonable" against the term "just, fair and reasonable", or for
/// quoting a definition in a different clause order (measured on the
/// 2026-10-23 expanded ruleset). Morphology is NOT bridged: "arose"
/// still does not match "arising".
fn term_matches(term: &str, answer_norm: &str, answer_tokens: &std::collections::HashSet<&str>) -> bool {
    let term_norm = normalize(term);
    if term_norm.is_empty() {
        return true;
    }
    if answer_norm.contains(&term_norm) {
        return true;
    }
    let content: Vec<&str> = term_norm
        .split(' ')
        .filter(|t| !STOPWORDS.contains(t))
        .collect();
    !content.is_empty() && content.iter().all(|t| answer_tokens.contains(t))
}

/// Per-stage retrieval score for one rule: did the expected source
/// appear in this stage's ranking, and at what rank? This is the
/// Haystack evaluation discipline — score every stage separately
/// (vector, BM25, fused, reranked) so a golden failure is located to
/// the stage that lost the document, not just observed end-to-end.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageScore {
    pub stage: String,
    pub hit: bool,
    pub reciprocal_rank: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageAggregate {
    pub stage: String,
    pub rules: usize,
    pub hits: usize,
    pub recall: f32,
    pub mrr: f32,
}

/// Score one hit-test report against a rule's expected sources, using
/// the same matching semantics as the answer-level evaluator (title
/// contains the expected string, any case). Only the first `limit`
/// hits of each stage count — production reranks a wide pool down to
/// the answer's top_k, so stage quality is judged at that cut. Rules
/// without expected sources score a hit at every stage.
pub fn stage_scores(
    report: &legal_rag_rag::HitTestReport,
    expected_sources: &[String],
    limit: usize,
) -> Vec<StageScore> {
    let needles: Vec<String> = expected_sources.iter().map(|s| s.to_lowercase()).collect();
    report
        .stages
        .iter()
        .map(|stage| {
            if needles.is_empty() {
                return StageScore { stage: stage.name.clone(), hit: true, reciprocal_rank: 1.0 };
            }
            let mut rr = 0.0f32;
            for (i, hit) in stage.hits.iter().take(limit.max(1)).enumerate() {
                let title = hit.doc_title.clone().unwrap_or_default().to_lowercase();
                if needles.iter().any(|n| title.contains(n.as_str())) {
                    rr = 1.0 / (i as f32 + 1.0);
                    break;
                }
            }
            StageScore { stage: stage.name.clone(), hit: rr > 0.0, reciprocal_rank: rr }
        })
        .collect()
}

/// Aggregate per-rule stage scores into recall / MRR per stage name,
/// preserving first-seen stage order.
pub fn aggregate_stages(all: &[Vec<StageScore>]) -> Vec<StageAggregate> {
    let mut order: Vec<String> = Vec::new();
    let mut acc: std::collections::HashMap<String, (usize, usize, f32)> =
        std::collections::HashMap::new();
    for scores in all {
        for s in scores {
            if !acc.contains_key(&s.stage) {
                order.push(s.stage.clone());
            }
            let e = acc.entry(s.stage.clone()).or_default();
            e.0 += 1;
            if s.hit {
                e.1 += 1;
            }
            e.2 += s.reciprocal_rank;
        }
    }
    order
        .into_iter()
        .map(|stage| {
            let (rules, hits, rr_sum) = acc[&stage];
            StageAggregate {
                stage,
                rules,
                hits,
                recall: hits as f32 / rules.max(1) as f32,
                mrr: rr_sum / rules.max(1) as f32,
            }
        })
        .collect()
}

pub fn evaluate(
    rule: &GoldenRule,
    answer: &str,
    citation_labels: &[String],
    citation_count: usize,
) -> RuleResult {
    let answer_norm = normalize(answer);
    let answer_tokens: std::collections::HashSet<&str> = answer_norm.split(' ').collect();
    let missing_terms: Vec<String> = rule
        .expected_terms
        .iter()
        .filter(|t| !term_matches(t, &answer_norm, &answer_tokens))
        .cloned()
        .collect();
    let labels_lc: Vec<String> = citation_labels.iter().map(|l| l.to_lowercase()).collect();
    let missing_sources: Vec<String> = rule
        .expected_sources
        .iter()
        .filter(|s| {
            let needle = s.to_lowercase();
            !labels_lc.iter().any(|l| l.contains(&needle))
        })
        .cloned()
        .collect();
    RuleResult {
        passed: missing_terms.is_empty() && missing_sources.is_empty(),
        missing_terms,
        missing_sources,
        answer_excerpt: crate::activity::excerpt(answer, 400),
        citations: citation_count,
        ran_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(terms: Vec<&str>, sources: Vec<&str>) -> GoldenRule {
        GoldenRule {
            id: "r1".into(),
            question: "q".into(),
            expected_terms: terms.into_iter().map(|s| s.to_string()).collect(),
            expected_sources: sources.into_iter().map(|s| s.to_string()).collect(),
            notes: String::new(),
            source: "manual".into(),
            created_at: Utc::now(),
            last_result: None,
        }
    }

    #[test]
    fn evaluate_pass_and_fail() {
        let r = rule(vec!["section 86", "CERT-In"], vec!["information technology act"]);
        let ok = evaluate(
            &r,
            "Under Section 86, CERT-In may issue directions…",
            &["Information Technology Act, 2000".into()],
            2,
        );
        assert!(ok.passed);
        let bad = evaluate(&r, "Nothing relevant.", &["Some other doc".into()], 1);
        assert!(!bad.passed);
        assert_eq!(bad.missing_terms.len(), 2);
        assert_eq!(bad.missing_sources.len(), 1);
    }

    #[test]
    fn stage_scores_locate_the_losing_stage() {
        use legal_rag_rag::{HitStage, HitTestReport, HitView};
        let view = |title: &str| HitView {
            chunk_id: "c".into(),
            doc_id: "d".into(),
            doc_title: Some(title.into()),
            score: 0.5,
            snippet: String::new(),
            is_child: false,
        };
        let report = HitTestReport {
            question: "q".into(),
            stages: vec![
                HitStage {
                    name: "vector".into(),
                    hits: vec![view("Indian Penal Code"), view("Indian Contract Act")],
                },
                HitStage {
                    name: "fused".into(),
                    hits: vec![view("Indian Contract Act"), view("Code of Civil Procedure")],
                },
            ],
        };
        let scores = stage_scores(&report, &["contract act".to_string()], 10);
        assert_eq!(scores[0].stage, "vector");
        assert!(scores[0].hit);
        assert_eq!(scores[0].reciprocal_rank, 0.5, "contract act is rank 2 in vector");
        assert_eq!(scores[1].reciprocal_rank, 1.0, "fusion promoted it to rank 1");
        let agg = aggregate_stages(&[scores]);
        assert_eq!(agg.len(), 2);
        assert_eq!(agg[0].recall, 1.0);
        assert_eq!(agg[1].mrr, 1.0);
        // A stage missing the source entirely scores zero.
        let miss = stage_scores(&report, &["motor vehicles act".to_string()], 10);
        assert!(!miss[0].hit && miss[0].reciprocal_rank == 0.0);
    }

    #[test]
    fn evaluate_tolerates_punctuation_and_order() {
        // The measured 2026-10-23 brittle cases: punctuation/order only.
        let r = rule(vec!["just, fair and reasonable"], vec![]);
        let ok = evaluate(&r, "The procedure must be Fair, just, and reasonable.", &[], 0);
        assert!(ok.passed, "missing: {:?}", ok.missing_terms);
        let r2 = rule(vec!["must prove that those facts exist"], vec![]);
        let ok2 = evaluate(
            &r2,
            "A person who asserts that those facts exist must prove the assertion.",
            &[],
            0,
        );
        assert!(ok2.passed, "missing: {:?}", ok2.missing_terms);
        // Substance still required: token genuinely absent fails.
        let r3 = rule(vec!["reasonable compensation not exceeding"], vec![]);
        let bad = evaluate(&r3, "The court awarded full compensation.", &[], 0);
        assert!(!bad.passed);
        assert_eq!(bad.missing_terms.len(), 1);
    }
}
