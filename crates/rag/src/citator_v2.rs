//! Citator V2 adapter — wires the standalone `citator-v2` engine into Ask.
//!
//! Kill switch: `LEGAL_RAG_CITATOR_V2=1` enables V2 resolution. Default OFF:
//! when the switch is off, the V1 path in `query.rs` runs exactly as before
//! and this module is never called.
//!
//! What V2 adds over V1 when on:
//! - Reversed statuses from appellate-reversal language detected in the
//!   cited snippets ("appeal allowed", "impugned judgment set aside"),
//!   linked by party match / appeal number / CNR / impugned date.
//! - Appellate chains and a governing authority for the conclusion
//!   (latest in chain, else highest binding forum), surfaced as a line in
//!   the answer and a hint in the system prompt.
//! - Rank-down: live law first, dead law last (corpus citations only,
//!   applied pre-generation in `prepare_context`, so `[doc:N]` markers
//!   keep resolving by id).
//!
//! Honest limits of this adapter layer (the engine itself has none of
//! these — they come from what Ask hands it):
//! - Forum / state / date are inferred from citation titles only
//!   (titles like "X — Allahabad High Court (2023)"); documents whose
//!   titles carry no forum marker get `Unknown` forum and cannot govern
//!   or extend a chain, though their statuses still resolve from edges.
//!   Full forum metadata at ingest would remove this limit.
//! - Reversal detection runs on cited snippets (first 1500 chars of the
//!   retrieved chunk), not full judgment texts; full-text detection stays
//!   an ingest concern.
//! - No per-request state or applicant context exists in the query API,
//!   so governing selection runs with state None (only national authority
//!   governs outright) and applicant None (AAR/AAAR never govern).
//! - V1 edges are converted to V2 edges, but their sources are real
//!   document ids while citations carry positional `doc:N` ids, so source
//!   judgments are synthesized from edge source titles for chain walking.
//!   When a converted edge's target name matches several cited judgments,
//!   it attaches to the highest-forum match below the source forum
//!   (nearest appellate step) — deterministic, best-effort, and always
//!   shown with its passage for audit.

use std::collections::HashMap;

use crate::citator::{
    CitationTreatment, TreatmentEdge as V1Edge, TreatmentKind as V1Kind,
    TreatmentStatus as V1Status,
};
use crate::query::{Citation, DocScope};
use citator_v2::{
    detect_reversals, resolve_ask, AskInput, Domain, EdgeTarget, Forum, Judgment, SimpleDate,
    TreatmentEdge as V2Edge, TreatmentKind as V2Kind, TreatmentStatus as V2Status,
};

/// Kill switch: `LEGAL_RAG_CITATOR_V2=1` (or `true`) enables V2 resolution.
/// Anything else, including unset, means V1 behavior.
pub fn citator_v2_enabled() -> bool {
    std::env::var("LEGAL_RAG_CITATOR_V2")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Word-bounded match: is `word` a standalone word in `lower`?
fn has_word(lower: &str, word: &str) -> bool {
    lower
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w == word)
}

/// Infer a forum from a document title. Foreign markers are checked first
/// ("UK Supreme Court" must not become the Indian Supreme Court).
pub fn infer_forum(title: &str) -> Forum {
    let lower = title.to_lowercase();
    if lower.contains("international court of justice")
        || lower.contains("icj")
        || has_word(&lower, "icj")
    {
        return Forum::InternationalCourt;
    }
    if lower.contains("supreme court of the united states")
        || lower.contains("u.s. supreme court")
        || lower.contains("uk supreme court")
        || lower.contains("supreme court of the united kingdom")
        || lower.contains("high court of england")
        || lower.contains("court of appeal of england")
        || (lower.contains("singapore")
            && (lower.contains("court of appeal") || lower.contains("high court")))
    {
        return Forum::ForeignCourt;
    }
    if lower.contains("siac") || lower.contains("icc arbitration") || lower.contains("arbitral tribunal") {
        return Forum::ArbitralTribunal;
    }
    if lower.contains("supreme court") {
        return Forum::SupremeCourt;
    }
    if lower.contains("high court") {
        return Forum::HighCourt;
    }
    if has_word(&lower, "itat") || lower.contains("income tax appellate") || lower.contains("income-tax appellate") {
        return Forum::Itat;
    }
    if has_word(&lower, "aaar") {
        return Forum::Aaar;
    }
    if has_word(&lower, "aar") {
        return Forum::Aar;
    }
    if has_word(&lower, "nclat") {
        return Forum::Nclat;
    }
    if has_word(&lower, "nclt") {
        return Forum::Nclt;
    }
    if has_word(&lower, "cci") || lower.contains("competition commission") {
        return Forum::Cci;
    }
    if has_word(&lower, "sat") || lower.contains("securities appellate") {
        return Forum::Sat;
    }
    if has_word(&lower, "ncdrc") || lower.contains("national consumer") {
        return Forum::Ncdrc;
    }
    if lower.contains("state commission") {
        return Forum::StateCommission;
    }
    if lower.contains("district commission") {
        return Forum::DistrictCommission;
    }
    if has_word(&lower, "ngt") || lower.contains("green tribunal") {
        return Forum::Ngt;
    }
    if has_word(&lower, "cat") || lower.contains("central administrative") {
        return Forum::Cat;
    }
    if lower.contains("sessions") {
        return Forum::SessionsCourt;
    }
    if lower.contains("magistrate") {
        return Forum::Magistrate;
    }
    if lower.contains("district court") || lower.contains("district judge") {
        return Forum::DistrictCourt;
    }
    Forum::Unknown
}

/// Infer a state code from a High Court name in the title. Best-effort;
/// unknown courts yield None (lineage then falls back to unknown-side rules).
pub fn infer_state(title: &str) -> Option<String> {
    let lower = title.to_lowercase();
    let table: &[(&str, &str)] = &[
        ("allahabad", "UP"),
        ("bombay", "MH"),
        ("calcutta", "WB"),
        ("delhi", "DL"),
        ("madras", "TN"),
        ("karnataka", "KA"),
        ("kerala", "KL"),
        ("gujarat", "GJ"),
        ("rajasthan", "RJ"),
        ("punjab and haryana", "PB"),
        ("andhra", "AP"),
        ("telangana", "TS"),
        ("orissa", "OD"),
        ("odisha", "OD"),
        ("madhya pradesh", "MP"),
        ("chhattisgarh", "CG"),
        ("jharkhand", "JH"),
        ("bihar", "BR"),
        ("assam", "AS"),
        ("gauhati", "AS"),
        ("himachal", "HP"),
        ("uttarakhand", "UK"),
        ("jammu", "JK"),
        ("goa", "GA"),
        ("manipur", "MN"),
        ("meghalaya", "ML"),
        ("tripura", "TR"),
        ("sikkim", "SK"),
    ];
    for (needle, code) in table {
        if lower.contains(needle) {
            return Some(code.to_string());
        }
    }
    None
}

/// Match name for a title: cut any " — Forum (year)" suffix and any
/// trailing "(YYYY)". Used for name matching only; display keeps the full
/// title.
pub fn match_name(title: &str) -> String {
    let mut s = title.trim();
    for sep in [" — ", " - ", " – "] {
        if let Some(idx) = s.find(sep) {
            s = s[..idx].trim();
            break;
        }
    }
    if s.ends_with(')') {
        if let Some(open) = s.rfind('(') {
            let year = &s[open + 1..s.len() - 1];
            if year.len() == 4 && year.chars().all(|c| c.is_ascii_digit()) {
                s = s[..open].trim();
            }
        }
    }
    s.to_string()
}

/// Year in a title ("(2023)") as a 1 January date, for ordering only.
pub fn title_year(title: &str) -> Option<SimpleDate> {
    let bytes = title.as_bytes();
    let mut i = 0;
    while i + 5 < bytes.len() {
        if bytes[i] == b'('
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 5] == b')'
            && title[i + 1..i + 5].chars().all(|c| c.is_ascii_digit())
        {
            if let Ok(y) = title[i + 1..i + 5].parse::<u32>() {
                return Some(SimpleDate::new(y, 1, 1));
            }
        }
        i += 1;
    }
    None
}

const ALL_DOMAINS: &[Domain] = &[
    Domain::GeneralCivil,
    Domain::Criminal,
    Domain::Tax,
    Domain::Company,
    Domain::Competition,
    Domain::Securities,
    Domain::Consumer,
    Domain::Service,
    Domain::Environment,
];

/// Best-fit domain: the domain whose ladders contain the most cited
/// forums. Ties break toward GeneralCivil (first in the table).
pub fn best_fit_domain(forums: &[Forum]) -> Domain {
    let mut best = Domain::GeneralCivil;
    let mut best_score = 0usize;
    for domain in ALL_DOMAINS {
        let score = forums
            .iter()
            .filter(|f| citator_v2::ladders::rank_in_domain(**f, *domain).is_some())
            .count();
        if score > best_score {
            best_score = score;
            best = *domain;
        }
    }
    best
}

fn map_kind(kind: V1Kind) -> V2Kind {
    match kind {
        V1Kind::Overruled => V2Kind::Overruled,
        V1Kind::Followed => V2Kind::Followed,
        V1Kind::Distinguished => V2Kind::Distinguished,
        V1Kind::Applied => V2Kind::Applied,
        V1Kind::Affirmed => V2Kind::Affirmed,
    }
}

fn map_status(status: V2Status) -> V1Status {
    match status {
        V2Status::Overruled => V1Status::Overruled,
        V2Status::Reversed => V1Status::Reversed,
        V2Status::Distinguished => V1Status::Distinguished,
        V2Status::GoodLaw => V1Status::GoodLaw,
        V2Status::Followed => V1Status::Followed,
        V2Status::Applied => V1Status::Applied,
        V2Status::Persuasive => V1Status::Persuasive,
        V2Status::Unknown => V1Status::Unknown,
    }
}

/// Full V2 resolution for one Ask's citations.
pub struct V2Resolution {
    /// (citation doc_id, treatment) for every corpus citation resolved.
    pub treatments: Vec<(String, CitationTreatment)>,
    /// Corpus citation doc_ids in V2 ranked order (live first, dead last).
    pub ranked_corpus_ids: Vec<String>,
    /// Governing-authority line for the answer text, if determined.
    pub governing_line: Option<String>,
    /// System-prompt hint steering the verdict, if governing exists.
    pub system_hint: Option<String>,
}

pub fn resolve_v2(
    citations: &[Citation],
    v1_edges: &[V1Edge],
    scope: Option<&DocScope>,
) -> V2Resolution {
    let empty = || V2Resolution {
        treatments: Vec::new(),
        ranked_corpus_ids: Vec::new(),
        governing_line: None,
        system_hint: None,
    };
    // Corpus citations only; web / private stay V1-Unknown.
    let corpus: Vec<&Citation> = citations
        .iter()
        .filter(|c| c.doc_id.starts_with("doc:"))
        .collect();
    if corpus.is_empty() {
        return empty();
    }

    // Judgments for cited docs + display titles.
    let mut judgments: Vec<Judgment> = Vec::new();
    let mut display: HashMap<String, String> = HashMap::new();
    for c in &corpus {
        let Some(title) = c.doc_title.as_deref().filter(|t| !t.trim().is_empty()) else {
            continue;
        };
        let mut j = Judgment::new(&c.doc_id, &match_name(title), infer_forum(title));
        j.state = infer_state(title);
        j.date = title_year(title);
        display.insert(c.doc_id.clone(), title.to_string());
        judgments.push(j);
    }
    if judgments.is_empty() {
        return empty();
    }

    // Synthesized judgments for V1 edge sources (chain walking needs them).
    // Only scope-visible edges are converted, mirroring the V1 rule.
    let visible_edges: Vec<&V1Edge> = v1_edges
        .iter()
        .filter(|e| match scope {
            Some(s) => s.contains(e.source_doc_id.as_str()),
            None => true,
        })
        .collect();
    for e in &visible_edges {
        let source_title = e.source_title.clone().unwrap_or_default();
        if !judgments.iter().any(|j| j.id == e.source_doc_id) {
            let mut sj = Judgment::new(&e.source_doc_id, &match_name(&source_title), infer_forum(&source_title));
            sj.state = infer_state(&source_title);
            sj.date = title_year(&source_title);
            judgments.push(sj);
            if !source_title.is_empty() {
                display.insert(e.source_doc_id.clone(), source_title);
            }
        }
    }

    let domain = best_fit_domain(&judgments.iter().map(|j| j.forum).collect::<Vec<_>>());

    // Convert V1 edges, resolving name targets against the judgment set.
    // A treatment flows downward, so candidates are name matches BELOW the
    // source forum; with several, the highest (nearest appellate step)
    // wins. No below-source match keeps the CaseName target and lets the
    // engine's strict rule decide (ambiguous names resolve to nothing).
    let mut edges: Vec<V2Edge> = Vec::new();
    for e in &visible_edges {
        let source_title = e.source_title.clone().unwrap_or_default();
        let source_forum = infer_forum(&source_title);
        let source_rank = citator_v2::ladders::rank_in_domain(source_forum, domain);
        let mut matches: Vec<&Judgment> = judgments
            .iter()
            .filter(|j| j.id != e.source_doc_id)
            .filter(|j| citator_v2::normalize::same_matter(&j.case_name, &e.target))
            .collect();
        matches.retain(|j| match (source_rank, citator_v2::ladders::rank_in_domain(j.forum, domain)) {
            (Some(sr), Some(cr)) => cr < sr,
            _ => true,
        });
        matches.sort_by(|a, b| {
            let ra = citator_v2::ladders::rank_in_domain(a.forum, domain).unwrap_or(0);
            let rb = citator_v2::ladders::rank_in_domain(b.forum, domain).unwrap_or(0);
            rb.cmp(&ra).then_with(|| a.id.cmp(&b.id))
        });
        let target = match matches.first() {
            Some(m) => EdgeTarget::JudgmentId(m.id.clone()),
            None => EdgeTarget::CaseName(e.target.clone()),
        };
        edges.push(V2Edge {
            target,
            kind: map_kind(e.kind),
            source_doc_id: e.source_doc_id.clone(),
            source_title: e.source_title.clone().unwrap_or_default(),
            passage: e.passage.clone(),
            linkage: Some("v1-edge".to_string()),
        });
    }

    // V2 reversal detection over cited snippets, candidates restricted to
    // judgments below the source forum (an appellate judgment reverses a
    // lower one, never a higher one). Unknown source rank keeps all
    // candidates except the source itself.
    let cited_judgments: Vec<Judgment> = judgments
        .iter()
        .filter(|j| j.id.starts_with("doc:"))
        .cloned()
        .collect();
    for c in &corpus {
        let Some(src) = cited_judgments.iter().find(|j| j.id == c.doc_id) else {
            continue;
        };
        let src_rank = citator_v2::ladders::rank_in_domain(src.forum, domain);
        let candidates: Vec<Judgment> = cited_judgments
            .iter()
            .filter(|j| j.id != src.id)
            .filter(|j| match (src_rank, citator_v2::ladders::rank_in_domain(j.forum, domain)) {
                (Some(sr), Some(cr)) => cr < sr,
                _ => true,
            })
            .cloned()
            .collect();
        edges.extend(detect_reversals(&c.snippet, src, &candidates));
    }

    let cited_ids: Vec<String> = cited_judgments.iter().map(|j| j.id.clone()).collect();
    let input = AskInput {
        state: None,
        domain,
        applicant: None,
        good_law_only: false,
    };
    let out = resolve_ask(&judgments, &edges, &cited_ids, &input);

    let mut treatments = Vec::new();
    for cited in &out.citations {
        let t = match &cited.evidence {
            Some(ev) => CitationTreatment {
                status: map_status(cited.status),
                evidence_source_title: Some(ev.source_title.clone()),
                evidence_passage: Some(ev.passage.clone()),
                evidence_doc_id: Some(ev.source_doc_id.clone()),
            },
            None => CitationTreatment {
                status: map_status(cited.status),
                evidence_source_title: None,
                evidence_passage: None,
                evidence_doc_id: None,
            },
        };
        // Unknown carries no evidence by construction; V2 agrees.
        let t = if cited.status == V2Status::Unknown {
            CitationTreatment::unknown()
        } else {
            t
        };
        treatments.push((cited.judgment.id.clone(), t));
    }
    let ranked_corpus_ids: Vec<String> = out.citations.iter().map(|c| c.judgment.id.clone()).collect();

    let (governing_line, system_hint) = match &out.governing {
        Some(gov) => {
            let title = display
                .get(&gov.judgment.id)
                .cloned()
                .unwrap_or_else(|| gov.judgment.case_name.clone());
            let chain = gov
                .chain
                .iter()
                .map(|s| {
                    display
                        .get(&s.judgment_id)
                        .cloned()
                        .unwrap_or_else(|| s.title.clone())
                })
                .collect::<Vec<_>>()
                .join(" \u{2192} ");
            let basis = match gov.basis {
                citator_v2::GoverningBasis::LatestInChain => "latest in chain",
                citator_v2::GoverningBasis::HighestForum => "highest binding forum",
            };
            let line = if chain.is_empty() {
                format!(
                    "Governing authority (V2): {title} [{}] \u{2014} {basis}.",
                    gov.judgment.forum.display()
                )
            } else {
                format!(
                    "Governing authority (V2): {title} [{}] \u{2014} {basis}. Chain: {chain}.",
                    gov.judgment.forum.display()
                )
            };
            let hint = format!(
                "CITATOR (V2): The conclusion must follow the governing authority: {title} ({}). Do not present an overruled or reversed authority as current law.",
                gov.judgment.forum.display()
            );
            (Some(line), Some(hint))
        }
        None => (None, None),
    };

    V2Resolution {
        treatments,
        ranked_corpus_ids,
        governing_line,
        system_hint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Citation;

    fn cit(doc_id: &str, title: &str, snippet: &str) -> Citation {
        Citation {
            doc_id: doc_id.to_string(),
            doc_title: Some(title.to_string()),
            chunk_id: format!("chunk-{doc_id}"),
            ordinal: 0,
            score: 1.0,
            snippet: snippet.to_string(),
            sentence: None,
            stale: None,
            source_url: None,
            source_date: None,
            treatment: None,
        }
    }

    #[test]
    fn forum_inference_checks_foreign_first() {
        assert_eq!(infer_forum("Roe v. Wade — UK Supreme Court (1973)"), Forum::ForeignCourt);
        assert_eq!(infer_forum("X v. Y — Supreme Court (2024)"), Forum::SupremeCourt);
        assert_eq!(infer_forum("X v. Y — Allahabad High Court (2023)"), Forum::HighCourt);
        assert_eq!(infer_forum("In re Acme — AAR (2022)"), Forum::Aar);
        assert_eq!(infer_forum("In re Acme — AAAR (2023)"), Forum::Aaar);
        assert_eq!(infer_forum("Sharma v. Builder — NCDRC (2023)"), Forum::Ncdrc);
        assert_eq!(infer_forum("Section 86 IT Act"), Forum::Unknown);
    }

    #[test]
    fn state_inference_from_hc_name() {
        assert_eq!(infer_state("X — Allahabad High Court (2023)").as_deref(), Some("UP"));
        assert_eq!(infer_state("X — Bombay High Court (2023)").as_deref(), Some("MH"));
        assert_eq!(infer_state("X — Supreme Court (2024)"), None);
    }

    #[test]
    fn match_name_strips_suffix_and_year() {
        assert_eq!(match_name("Sharma v. State — District Court (2021)"), "Sharma v. State");
        assert_eq!(match_name("Puttaswamy v. Union of India (2017)"), "Puttaswamy v. Union of India");
        assert_eq!(match_name("Section 86 IT Act"), "Section 86 IT Act");
    }

    #[test]
    fn v2_end_to_end_reversal_governing_and_rank_down() {
        // District reversed by HC (snippet detection), SC affirms via a V1
        // edge converted with the nearest-below tie-break.
        let citations = vec![
            cit("doc:0", "State v. Kumar — District Court (2021)", "The accused was convicted."),
            cit(
                "doc:1",
                "Kumar v. State — Allahabad High Court (2023)",
                "In State v. Kumar, the appeal is allowed and the impugned judgment is set aside.",
            ),
            cit(
                "doc:2",
                "Kumar v. State — Supreme Court (2025)",
                "We affirm Kumar v. State.",
            ),
        ];
        let v1_edges = vec![V1Edge {
            target: "Kumar v. State".to_string(),
            kind: V1Kind::Affirmed,
            source_doc_id: "doc-real-sc".to_string(),
            source_chunk_id: "chunk-sc".to_string(),
            source_title: Some("Kumar v. State — Supreme Court (2025)".to_string()),
            passage: "We affirm Kumar v. State.".to_string(),
        }];
        let res = resolve_v2(&citations, &v1_edges, None);
        let by_id: HashMap<&str, &CitationTreatment> = res
            .treatments
            .iter()
            .map(|(id, t)| (id.as_str(), t))
            .collect();
        assert_eq!(by_id["doc:0"].status, V1Status::Reversed, "district reversed by HC snippet");
        // Governing: the SC ends the chain (via the converted affirm edge).
        let line = res.governing_line.expect("governing line");
        assert!(line.contains("Supreme Court"), "{line}");
        // Rank-down: live SC/HC before dead district.
        assert_eq!(res.ranked_corpus_ids.last().unwrap(), "doc:0");
    }

    #[test]
    fn v2_disabled_by_default_off_switch() {
        std::env::remove_var("LEGAL_RAG_CITATOR_V2");
        assert!(!citator_v2_enabled());
    }
}
