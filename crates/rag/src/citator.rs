//! Citator — "is this case still good law?"
//!
//! Every cited authority in an answer carries a treatment status with
//! provenance: Overruled / Followed / Distinguished / Applied / Good law
//! / Unknown. The governing rule is absolute: **a status is never
//! asserted without a cited passage**. A treatment edge exists only
//! when a judgment in the corpus says so in explicit words, and the
//! status shown to the user always carries that passage with it.
//! With no evidence, the status is Unknown — never a guess, and never
//! a silent claim that the authority is still good law.
//!
//! Detection is deterministic pattern matching over judgment text —
//! no LLM call, no extra embedding, so it costs nothing at ingest
//! beyond a text scan and nothing at all at query time (edges are
//! resolved from an in-memory store). That determinism is the point:
//! treatment language in judgments is formulaic ("X must be and is
//! accordingly overruled", "M.P. Sharma (supra) ... stand overruled",
//! "we follow Smith v. Jones"), and a pattern that fires only on
//! those formulas is auditable in a way an LLM judgment is not.
//!
//! Detection rules (per sentence):
//! - **v-pattern**: `A v. B` case names near a treatment verb (within
//!   12 words — a farther verb belongs to another clause, and
//!   attributing it would invent an edge).
//! - **(supra) references**: Indian judgments cite short forms as
//!   `ADM Jabalpur (supra)` / `M.P. Sharma (supra)`; the sentence's
//!   treatment verb governs every such reference in the sentence.
//! - **Verb-following phrase**: `overrule/follow/... the decision in
//!   X` and bare short forms (`which followed ADM Jabalpur`).
//! - **Sentence-initial subject**: `ADM Jabalpur must be and is
//!   accordingly overruled` — the operative formula has no `v.` and
//!   no `(supra)` at all.
//! - **Negation guard**: `not / never / decline / refuse / fail to`
//!   before the verb, and noun-ish `the overruling`, never create an
//!   edge. Evaluative language with no treatment verb (`are seriously
//!   flawed`) likewise creates nothing — that sentence is evidence of
//!   criticism, not of a treatment, and is reported as Unknown.
//!
//! Resolution rules (at Ask time):
//! - Only edges whose **source document is visible in the current
//!   scope** count (brief docs when brief-scoped, full vault
//!   otherwise; when a brief-scoped query falls back to the full vault
//!   the effective scope is the full vault, matching the answer).
//! - The cited authority matches an edge target by normalized
//!   case-name tokens (aliases expanded: `ADM` = Additional District
//!   Magistrate; initials merged: `M P` = `M.P.`). All target tokens
//!   must appear in the cited title — a partial match is Unknown.
//! - Precedence when several edges match: Overruled > Distinguished >
//!   Good law (affirmed) > Followed > Applied. The winning edge's
//!   passage is the provenance shown.
//! - Web and private citations are always Unknown: web authorities
//!   are outside the corpus scope, and private documents never enter
//!   the treatment store.
//! - A cited authority is **never dropped** because of its status.
//!   Overruled / Distinguished citations additionally get a
//!   deterministic treatment-note line appended to the answer.

use std::collections::HashSet;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::query::{Citation, DocScope};

/// A treatment verb found in judgment text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreatmentKind {
    Overruled,
    Followed,
    Distinguished,
    Applied,
    Affirmed,
}

/// The status shown on a citation. `Affirmed` edges surface as
/// `GoodLaw`: an explicit affirmance is the only passage-backed route
/// to a good-law claim in V1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreatmentStatus {
    Overruled,
    Followed,
    Distinguished,
    Applied,
    GoodLaw,
    Unknown,
}

impl From<TreatmentKind> for TreatmentStatus {
    fn from(kind: TreatmentKind) -> Self {
        match kind {
            TreatmentKind::Overruled => TreatmentStatus::Overruled,
            TreatmentKind::Followed => TreatmentStatus::Followed,
            TreatmentKind::Distinguished => TreatmentStatus::Distinguished,
            TreatmentKind::Applied => TreatmentStatus::Applied,
            TreatmentKind::Affirmed => TreatmentStatus::GoodLaw,
        }
    }
}

impl TreatmentStatus {
    /// Rank for precedence: lower wins when several edges match.
    fn rank(self) -> usize {
        match self {
            TreatmentStatus::Overruled => 0,
            TreatmentStatus::Distinguished => 1,
            TreatmentStatus::GoodLaw => 2,
            TreatmentStatus::Followed => 3,
            TreatmentStatus::Applied => 4,
            TreatmentStatus::Unknown => 5,
        }
    }

    /// Past-participle wording for the treatment-note line.
    fn participle(self) -> &'static str {
        match self {
            TreatmentStatus::Overruled => "overruled",
            TreatmentStatus::Distinguished => "distinguished",
            _ => "treated",
        }
    }
}

/// One treatment relationship: a later judgment (`source_*`) treating
/// an earlier cited case (`target`) in a stated way, with the exact
/// sentence as provenance. `target` is the case name as written
/// (e.g. `ADM Jabalpur`, `Union of India v Bhanudas Krishna Gawde`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreatmentEdge {
    pub target: String,
    pub kind: TreatmentKind,
    pub source_doc_id: String,
    pub source_chunk_id: String,
    pub source_title: Option<String>,
    /// The sentence the treatment was read from — the provenance.
    pub passage: String,
}

/// The status attached to one citation, with its evidence (if any).
/// `Unknown` carries no evidence fields by construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationTreatment {
    pub status: TreatmentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_source_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_passage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_doc_id: Option<String>,
}

impl CitationTreatment {
    pub fn unknown() -> Self {
        Self {
            status: TreatmentStatus::Unknown,
            evidence_source_title: None,
            evidence_passage: None,
            evidence_doc_id: None,
        }
    }

    fn from_edge(edge: &TreatmentEdge) -> Self {
        Self {
            status: edge.kind.into(),
            evidence_source_title: edge.source_title.clone(),
            evidence_passage: Some(edge.passage.clone()),
            evidence_doc_id: Some(edge.source_doc_id.clone()),
        }
    }
}

/// In-memory treatment store, shared between the ingest pipeline
/// (writer) and the query service (reader) behind one Arc.
#[derive(Debug, Default)]
pub struct TreatmentStore {
    edges: RwLock<Vec<TreatmentEdge>>,
}

impl TreatmentStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn edge_key(e: &TreatmentEdge) -> String {
        format!(
            "{}|{:?}|{}",
            normalize_case_name(&e.target),
            e.kind,
            e.source_chunk_id
        )
    }

    /// Insert edges, skipping exact duplicates. Returns new count.
    pub fn add(&self, edges: Vec<TreatmentEdge>) -> usize {
        let mut guard = self.edges.write().expect("lock poisoned");
        let mut known: HashSet<String> = guard.iter().map(Self::edge_key).collect();
        let mut added = 0usize;
        for e in edges {
            if known.insert(Self::edge_key(&e)) {
                guard.push(e);
                added += 1;
            }
        }
        added
    }

    pub fn all(&self) -> Vec<TreatmentEdge> {
        self.edges.read().expect("lock poisoned").clone()
    }

    pub fn count(&self) -> usize {
        self.edges.read().expect("lock poisoned").len()
    }

    pub fn snapshot(&self) -> Vec<TreatmentEdge> {
        self.all()
    }

    pub fn restore(&self, edges: Vec<TreatmentEdge>) {
        *self.edges.write().expect("lock poisoned") = edges;
    }

    /// Drop edges sourced from deleted chunks (document removal).
    /// Returns removed count.
    pub fn remove_chunks(&self, chunk_ids: &[String]) -> usize {
        let removed: HashSet<&str> = chunk_ids.iter().map(|s| s.as_str()).collect();
        let mut guard = self.edges.write().expect("lock poisoned");
        let before = guard.len();
        guard.retain(|e| !removed.contains(e.source_chunk_id.as_str()));
        before - guard.len()
    }
}

/// Resolve one cited authority's status from treatment edges visible
/// in `scope` (`None` = full vault). `title` is the cited document's
/// title; `None` (or no matching edge) resolves to Unknown.
pub fn resolve_treatment(
    title: Option<&str>,
    edges: &[TreatmentEdge],
    scope: Option<&DocScope>,
) -> CitationTreatment {
    let Some(title) = title.filter(|t| !t.trim().is_empty()) else {
        return CitationTreatment::unknown();
    };
    let mut best: Option<&TreatmentEdge> = None;
    for edge in edges {
        if let Some(scope) = scope {
            if !scope.contains(edge.source_doc_id.as_str()) {
                continue;
            }
        }
        if !names_match(&edge.target, title) {
            continue;
        }
        let better = match best {
            None => true,
            Some(b) => TreatmentStatus::from(edge.kind).rank() < TreatmentStatus::from(b.kind).rank(),
        };
        if better {
            best = Some(edge);
        }
    }
    match best {
        Some(edge) => CitationTreatment::from_edge(edge),
        None => CitationTreatment::unknown(),
    }
}

/// Deterministic treatment-note lines for an answer's citations —
/// one line per Overruled / Distinguished authority, never dropping
/// the citation itself. Returns `None` when nothing qualifies.
pub fn treatment_note(citations: &[Citation]) -> Option<String> {
    let mut lines = Vec::new();
    for c in citations {
        let Some(t) = &c.treatment else { continue };
        if !matches!(t.status, TreatmentStatus::Overruled | TreatmentStatus::Distinguished) {
            continue;
        }
        let title = c.doc_title.clone().unwrap_or_else(|| c.doc_id.clone());
        let source = t
            .evidence_source_title
            .clone()
            .unwrap_or_else(|| "a later judgment".to_string());
        let passage = t.evidence_passage.clone().unwrap_or_default();
        if passage.is_empty() {
            lines.push(format!(
                "Treatment note: {title} was {} in {source}.",
                t.status.participle()
            ));
        } else {
            lines.push(format!(
                "Treatment note: {title} was {} in {source}: \u{201c}{passage}\u{201d}.",
                t.status.participle()
            ));
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Append the treatment note to an answer (after the citation audit,
/// like the audit itself, so the note's quoted passage is never
/// mistaken for a citation marker).
pub fn append_treatment_note(answer: String, citations: &[Citation]) -> String {
    match treatment_note(citations) {
        Some(note) => format!("{answer}\n\n{note}"),
        None => answer,
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Kill switch: `LEGAL_RAG_CITATOR=0` disables ingest-time detection
/// (resolution of already-stored edges is unaffected).
pub fn citator_enabled() -> bool {
    std::env::var("LEGAL_RAG_CITATOR").map(|v| v != "0").unwrap_or(true)
}

const VERB_FORMS: &[(&str, TreatmentKind)] = &[
    ("overrule", TreatmentKind::Overruled),
    ("overrules", TreatmentKind::Overruled),
    ("overruled", TreatmentKind::Overruled),
    ("overruling", TreatmentKind::Overruled),
    ("follow", TreatmentKind::Followed),
    ("follows", TreatmentKind::Followed),
    ("followed", TreatmentKind::Followed),
    ("following", TreatmentKind::Followed),
    ("distinguish", TreatmentKind::Distinguished),
    ("distinguishes", TreatmentKind::Distinguished),
    ("distinguished", TreatmentKind::Distinguished),
    ("distinguishing", TreatmentKind::Distinguished),
    ("apply", TreatmentKind::Applied),
    ("applies", TreatmentKind::Applied),
    ("applied", TreatmentKind::Applied),
    ("applying", TreatmentKind::Applied),
    ("affirm", TreatmentKind::Affirmed),
    ("affirms", TreatmentKind::Affirmed),
    ("affirmed", TreatmentKind::Affirmed),
    ("affirming", TreatmentKind::Affirmed),
];

fn verb_kind(word: &str) -> Option<TreatmentKind> {
    let w = clean_word(word);
    VERB_FORMS.iter().find(|(form, _)| *form == w).map(|(_, k)| *k)
}

/// Words stripped from a token before comparison.
fn clean_word(raw: &str) -> String {
    raw.trim_matches(|c: char| {
        matches!(c, ',' | ';' | ':' | '.' | '!' | '?' | '(' | ')' | '[' | ']' | '"' | '\'' | '\u{201c}' | '\u{201d}')
    })
    .to_lowercase()
}

/// Raw token as written (edge punctuation trimmed, internal dots kept
/// for initials like `M.P.`).
fn raw_word(raw: &str) -> String {
    raw.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | '(' | ')' | '[' | ']' | '"' | '\'' | '\u{201c}' | '\u{201d}'))
        .to_string()
}

fn is_capitalized(word: &str) -> bool {
    let w = raw_word(word);
    if w.is_empty() {
        return false;
    }
    // Initials (`M.P.`, `U.P.`) and all-caps short forms (`ADM`).
    if w.chars().all(|c| c.is_ascii_uppercase() || c == '.') {
        return w.chars().any(|c| c.is_ascii_uppercase());
    }
    w.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

fn is_connector(word: &str) -> bool {
    matches!(clean_word(word).as_str(), "of" | "and" | "&" | "the")
}

/// Filler words skipped between a treatment verb and its target
/// (`overrule the decision in X`).
fn is_filler(word: &str) -> bool {
    matches!(
        clean_word(word).as_str(),
        "the" | "a" | "an" | "decision" | "judgment" | "judgement" | "majority" | "view" | "ratio" | "in" | "of"
    )
}

/// Strip a sentence-final dot from a phrase-final word — `Jones.`
/// at a sentence end is `Jones`, while initials (`M.P.`) keep theirs.
fn trim_sentence_dot(part: &str) -> String {
    if part.ends_with('.') && part[..part.len() - 1].chars().any(|c| c.is_ascii_lowercase()) {
        part[..part.len() - 1].to_string()
    } else {
        part.to_string()
    }
}

/// Collect a capitalized phrase ending at `end` (exclusive) walking
/// left: `State of Kerala`, `M.P. Sharma`, `ADM Jabalpur`.
fn phrase_ending_at(words: &[String], end: usize) -> Option<(usize, String)> {
    if end == 0 {
        return None;
    }
    let mut start = end;
    let mut parts: Vec<String> = Vec::new();
    while start > 0 {
        let w = &words[start - 1];
        if is_capitalized(w) {
            parts.push(raw_word(w));
            start -= 1;
        } else if is_connector(w) && start >= 2 && is_capitalized(&words[start - 2]) {
            parts.push(raw_word(w));
            start -= 1;
        } else {
            break;
        }
    }
    parts.reverse();
    // Drop a leading connector (`the majority in X` walked past `in`).
    while parts.first().is_some_and(|p| is_connector(p)) {
        parts.remove(0);
    }
    if parts.is_empty() || parts.len() > 5 {
        return None;
    }
    if let Some(last) = parts.last_mut() {
        *last = trim_sentence_dot(last);
    }
    Some((start, parts.join(" ")))
}

/// Collect a capitalized phrase starting at `start` walking right.
fn phrase_starting_at(words: &[String], start: usize) -> Option<(usize, String)> {
    let mut end = start;
    let mut parts: Vec<String> = Vec::new();
    while end < words.len() {
        let w = &words[end];
        if is_capitalized(w) {
            parts.push(raw_word(w));
            end += 1;
        } else if is_connector(w) && end + 1 < words.len() && is_capitalized(&words[end + 1]) {
            parts.push(raw_word(w));
            end += 1;
        } else {
            break;
        }
    }
    while parts.last().is_some_and(|p| is_connector(p)) {
        parts.pop();
    }
    if parts.is_empty() || parts.len() > 5 {
        return None;
    }
    if let Some(last) = parts.last_mut() {
        *last = trim_sentence_dot(last);
    }
    Some((end, parts.join(" ")))
}

/// True when a verb occurrence already governs a bare-phrase object
/// after it (`followed ADM Jabalpur`) — such a verb belongs to that
/// object, not to a `v.`-target standing before it (`Gawde, which
/// followed ADM Jabalpur` must not make Gawde "followed"). A verb
/// whose following phrase runs into `v.` governs that v-target
/// instead, so it is NOT object-consuming.
fn verb_has_bare_object(words: &[String], vi: usize) -> bool {
    let mut j = vi + 1;
    while j < words.len() && is_filler(&words[j]) {
        j += 1;
    }
    if j >= words.len() {
        return false;
    }
    match phrase_starting_at(words, j) {
        Some((end, phrase)) if phrase.split_whitespace().count() >= 2 => {
            !(end < words.len() && matches!(clean_word(&words[end]).as_str(), "v" | "vs"))
        }
        _ => false,
    }
}

/// Sentence splitter that does NOT break after initials (`M.P.`),
/// the `v.` in case names, or `No.` — the shared splitter would cut
/// `M.P. Sharma (supra)` and `Smith v. Jones` in half.
fn split_treatment_sentences(text: &str) -> Vec<String> {
    let flowing = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = flowing.chars().peekable();
    // Track the token currently being built to test initials / v. / No.
    let mut token = String::new();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            token.clear();
            cur.push(c);
            continue;
        }
        token.push(c);
        cur.push(c);
        if matches!(c, '.' | '!' | '?' | '\u{964}') {
            let next_is_space_or_end = chars.peek().is_none_or(|n| n.is_whitespace());
            if !next_is_space_or_end {
                continue;
            }
            let is_initial = {
                let t = token.trim_end_matches('.');
                !t.is_empty()
                    && t.chars().all(|ch| ch.is_ascii_uppercase() || ch == '.')
            };
            let is_v_or_no = matches!(
                token.trim_end_matches(|ch| ch == '.' || ch == ',').to_lowercase().as_str(),
                "v" | "vs" | "no"
            );
            if c == '.' && (is_initial || is_v_or_no) {
                continue;
            }
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// A verb occurrence is negated when a negation/decline word sits
/// within 4 words before it, or the verb is noun-ish (`the overruling`).
fn verb_negated(words: &[String], idx: usize) -> bool {
    if idx > 0 && clean_word(&words[idx - 1]) == "the" {
        return true;
    }
    let from = idx.saturating_sub(4);
    words[from..idx].iter().any(|w| {
        matches!(
            clean_word(w).as_str(),
            "not" | "never" | "n't" | "decline" | "declines" | "declined" | "refuse" | "refuses"
                | "refused" | "reject" | "rejects" | "rejected" | "fail" | "failed" | "failure"
        )
    })
}

/// Detect treatment edges in one chunk's text.
pub fn detect_treatments(
    text: &str,
    source_doc_id: &str,
    source_chunk_id: &str,
    source_title: Option<&str>,
) -> Vec<TreatmentEdge> {
    let mut edges: Vec<TreatmentEdge> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut push = |target: String, kind: TreatmentKind, passage: String| {
        let key = format!("{}|{kind:?}", normalize_case_name(&target));
        if seen.insert(key) {
            edges.push(TreatmentEdge {
                target,
                kind,
                source_doc_id: source_doc_id.to_string(),
                source_chunk_id: source_chunk_id.to_string(),
                source_title: source_title.map(|s| s.to_string()),
                passage,
            });
        }
    };
    for sentence in split_treatment_sentences(text) {
        let sentence = sentence.trim().to_string();
        if sentence.chars().count() < 15 {
            continue;
        }
        let words: Vec<String> = sentence.split_whitespace().map(|w| w.to_string()).collect();
        // Verb occurrences (kind, word index), negation-filtered.
        let verbs: Vec<(TreatmentKind, usize)> = words
            .iter()
            .enumerate()
            .filter_map(|(i, w)| verb_kind(w).map(|k| (k, i)))
            .filter(|(_, i)| !verb_negated(&words, *i))
            .collect();
        if verbs.is_empty() {
            continue;
        }
        let passage: String = sentence.chars().take(800).collect();
        // --- v-pattern targets: nearest verb within 12 words. ---
        for (i, w) in words.iter().enumerate() {
            let cleaned = clean_word(w);
            if cleaned != "v" && cleaned != "vs" {
                continue;
            }
            let Some((lstart, left)) = phrase_ending_at(&words, i) else { continue };
            let Some((_, right)) = phrase_starting_at(&words, i + 1) else { continue };
            let _ = lstart;
            let target = format!("{left} v. {right}");
            let target_start = i.saturating_sub(left.split_whitespace().count());
            let target_end = i + 1 + right.split_whitespace().count();
            let nearest = verbs
                .iter()
                .filter(|(_, vi)| {
                    // A verb after the target that already governs a
                    // bare object belongs to that object, not here.
                    if *vi >= target_end && verb_has_bare_object(&words, *vi) {
                        return false;
                    }
                    let d = if *vi < target_start {
                        target_start - vi
                    } else if *vi >= target_end {
                        vi - target_end
                    } else {
                        0
                    };
                    d <= 12
                })
                .min_by_key(|(_, vi)| {
                    if *vi < target_start {
                        target_start - vi
                    } else if *vi >= target_end {
                        vi - target_end
                    } else {
                        0
                    }
                });
            if let Some((kind, _)) = nearest {
                push(target, *kind, passage.clone());
            }
        }
        // --- (supra) targets: the sentence's verb governs all of them,
        // unless several verb kinds compete (then nearest wins). ---
        let supra_targets: Vec<String> = words
            .iter()
            .enumerate()
            .filter(|(_, w)| {
                let t = w.trim_end_matches(',');
                t == "(supra)" || t == "(Supra)"
            })
            .filter_map(|(i, _)| phrase_ending_at(&words, i).map(|(_, p)| p))
            .collect();
        if !supra_targets.is_empty() {
            // Nearest verb to the first supra reference; ties across
            // kinds resolve to the sentence's first verb.
            let first_supra = words
                .iter()
                .position(|w| {
                    let t = w.trim_end_matches(',');
                    t == "(supra)" || t == "(Supra)"
                })
                .unwrap_or(0);
            let kind = verbs
                .iter()
                .min_by_key(|(_, vi)| vi.abs_diff(first_supra))
                .map(|(k, _)| *k)
                .unwrap_or(verbs[0].0);
            for t in supra_targets {
                push(t, kind, passage.clone());
            }
        }
        // --- Verb-following bare phrase (`followed ADM Jabalpur`). ---
        for (kind, vi) in &verbs {
            let mut j = vi + 1;
            while j < words.len() && is_filler(&words[j]) {
                j += 1;
            }
            if j >= words.len() {
                continue;
            }
            // A v-pattern target starting here owns this verb; the
            // bare prefix (`Union of India` before `v.`) is not a case.
            if let Some((end, phrase)) = phrase_starting_at(&words, j) {
                if phrase.split_whitespace().count() >= 2 {
                    let next_is_v = end < words.len()
                        && matches!(clean_word(&words[end]).as_str(), "v" | "vs");
                    if !next_is_v {
                        push(phrase, *kind, passage.clone());
                    }
                }
            }
        }
        // --- Sentence-initial subject + treatment verb. ---
        // Guarded twice: a leading phrase immediately followed by
        // `v.` belongs to the v-pattern target (e.g. `In Maneka
        // Gandhi v Union of India, ... was overruled` must NOT yield
        // an edge for Maneka Gandhi), and a leading preposition or
        // pronoun (`In ...`, `This Court ...`) is not a case name.
        if let Some((end, leading)) = phrase_starting_at(&words, 0) {
            let first = leading.split_whitespace().next().unwrap_or("");
            let next_is_v =
                end < words.len() && matches!(clean_word(&words[end]).as_str(), "v" | "vs");
            let first_is_filler = matches!(
                clean_word(first).as_str(),
                "in" | "the" | "a" | "an" | "this" | "that" | "it" | "we" | "our" | "for" | "on"
                    | "at" | "by" | "with" | "from"
            );
            if leading.split_whitespace().count() >= 2 && !next_is_v && !first_is_filler {
                let kind = verbs[0].0;
                push(leading, kind, passage.clone());
            }
        }
    }
    edges
}

// ---------------------------------------------------------------------------
// Name matching
// ---------------------------------------------------------------------------

/// Normalize a case name for comparison: lowercase, punctuation to
/// spaces, alias expansion, initials merged (`M P` -> `MP`).
pub fn normalize_case_name(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut spaced = String::with_capacity(lower.len());
    for c in lower.chars() {
        if c.is_alphanumeric() || c.is_whitespace() {
            spaced.push(c);
        } else {
            spaced.push(' ');
        }
    }
    let mut tokens: Vec<String> = spaced
        .split_whitespace()
        .map(|t| t.to_string())
        .collect();
    // Alias expansion (whole-word).
    tokens = tokens
        .into_iter()
        .flat_map(|t| {
            if t == "adm" {
                vec!["additional".to_string(), "district".to_string(), "magistrate".to_string()]
            } else {
                vec![t]
            }
        })
        .collect();
    // Merge runs of single letters into one initial token.
    let mut merged: Vec<String> = Vec::new();
    let mut run = String::new();
    for t in tokens {
        if t.chars().count() == 1 && t.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            run.push_str(&t);
        } else {
            if !run.is_empty() {
                merged.push(std::mem::take(&mut run));
            }
            merged.push(t);
        }
    }
    if !run.is_empty() {
        merged.push(run);
    }
    // Drop stopwords.
    merged
        .into_iter()
        .filter(|t| {
            !matches!(
                t.as_str(),
                "v" | "vs" | "the" | "a" | "an" | "and" | "of" | "in" | "for" | "on" | "at" | "to"
                    | "ors" | "anr" | "others" | "etc" | "supra"
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// True when every normalized target token appears in the normalized
/// cited title. Strict by design: a partial match is Unknown.
pub fn names_match(target: &str, title: &str) -> bool {
    let target_norm = normalize_case_name(target);
    let title_norm = normalize_case_name(title);
    let tt: Vec<&str> = target_norm.split(' ').collect();
    if tt.is_empty() || tt.iter().any(|t| t.is_empty()) {
        return false;
    }
    let ct: HashSet<&str> = title_norm.split(' ').collect();
    tt.iter().all(|t| ct.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(text: &str) -> Vec<TreatmentEdge> {
        detect_treatments(text, "src-doc", "src-chunk", Some("Later Judgment"))
    }

    #[test]
    fn v_pattern_overrule_follow_distinguish_apply_affirm() {
        let cases = [
            ("We overrule Smith v. Jones.", TreatmentKind::Overruled, "Smith v. Jones"),
            ("This Court follows Patel v. Shah.", TreatmentKind::Followed, "Patel v. Shah"),
            ("We distinguish Rao v. Mehta on its facts.", TreatmentKind::Distinguished, "Rao v. Mehta"),
            ("We apply Kumar v. Singh here.", TreatmentKind::Applied, "Kumar v. Singh"),
            ("We affirm Verma v. Gupta.", TreatmentKind::Affirmed, "Verma v. Gupta"),
        ];
        for (text, kind, target) in cases {
            let edges = detect(text);
            assert_eq!(edges.len(), 1, "for {text:?} got {edges:?}");
            assert_eq!(edges[0].kind, kind);
            assert_eq!(edges[0].target, target);
            assert!(edges[0].passage.contains(text.trim_end_matches('.')));
        }
    }

    #[test]
    fn supra_targets_share_sentence_verb() {
        let edges = detect(
            "M.P. Sharma (supra) and the majority in Kharak Singh (supra), to the extent \
             that they indicate to the contrary, stand overruled.",
        );
        let targets: Vec<&str> = edges.iter().map(|e| e.target.as_str()).collect();
        assert!(targets.contains(&"M.P. Sharma"), "got {targets:?}");
        assert!(targets.contains(&"Kharak Singh"), "got {targets:?}");
        assert!(edges.iter().all(|e| e.kind == TreatmentKind::Overruled));
    }

    #[test]
    fn sentence_initial_operative_formula() {
        let edges = detect("ADM Jabalpur must be and is accordingly overruled.");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target, "ADM Jabalpur");
        assert_eq!(edges[0].kind, TreatmentKind::Overruled);
    }

    #[test]
    fn verb_following_bare_short_form() {
        let edges = detect("We also overrule the decision in Union of India v Bhanudas Krishna Gawde, which followed ADM Jabalpur.");
        let by_target: Vec<(&str, TreatmentKind)> =
            edges.iter().map(|e| (e.target.as_str(), e.kind)).collect();
        assert!(
            by_target.contains(&("Union of India v. Bhanudas Krishna Gawde", TreatmentKind::Overruled)),
            "got {by_target:?}"
        );
        assert!(
            by_target.contains(&("ADM Jabalpur", TreatmentKind::Followed)),
            "got {by_target:?}"
        );
        // The bare prefix before `v.` must not become its own edge.
        assert!(!by_target.iter().any(|(t, _)| *t == "Union of India"));
    }

    #[test]
    fn negation_and_noun_forms_create_no_edge() {
        for text in [
            "We do not follow Smith v. Jones.",
            "Smith v. Jones was not overruled by this Court.",
            "The Court declined to overrule Patel v. Shah.",
            "The overruling decision in Puttaswamy changed the law.",
        ] {
            assert!(detect(text).is_empty(), "for {text:?} got {:?}", detect(text));
        }
    }

    #[test]
    fn evaluative_language_without_verb_creates_no_edge() {
        let edges = detect(
            "The judgments rendered by all the four judges constituting the majority in \
             Additional District Magistrate, Jabalpur are seriously flawed.",
        );
        assert!(edges.is_empty());
    }

    #[test]
    fn far_verb_does_not_attach_to_v_target() {
        // The overruling here belongs to the majority decision, not to
        // Maneka Gandhi — 25 words away, past the window.
        let edges = detect(
            "In Maneka Gandhi v Union of India, the minority judgment of Justice Subba Rao \
             in Kharak Singh was specifically approved of and the decision of the majority was overruled.",
        );
        assert!(
            !edges.iter().any(|e| e.target.contains("Maneka")),
            "got {edges:?}"
        );
    }

    #[test]
    fn names_match_with_adm_expansion_and_initials() {
        assert!(names_match(
            "ADM Jabalpur",
            "Additional District Magistrate, Jabalpur v. Shivakant Shukla"
        ));
        assert!(names_match("M.P. Sharma", "M. P. Sharma v. Satish Chandra"));
        assert!(names_match("M P Sharma", "M.P. Sharma v. Satish Chandra"));
        assert!(names_match("Kharak Singh", "Kharak Singh v. State of Uttar Pradesh"));
        assert!(!names_match("ADM Jabalpur", "Puttaswamy v. Union of India"));
        assert!(!names_match("Smith v. Jones", "Smith v. Patel"));
    }

    #[test]
    fn resolution_precedence_and_scope_boundary() {
        let mut scope: DocScope = HashSet::new();
        scope.insert("in-scope-doc".to_string());
        let edges = vec![
            TreatmentEdge {
                target: "Smith v. Jones".into(),
                kind: TreatmentKind::Followed,
                source_doc_id: "in-scope-doc".into(),
                source_chunk_id: "c1".into(),
                source_title: Some("Later One".into()),
                passage: "We follow Smith v. Jones.".into(),
            },
            TreatmentEdge {
                target: "Smith v. Jones".into(),
                kind: TreatmentKind::Overruled,
                source_doc_id: "other-brief-doc".into(),
                source_chunk_id: "c2".into(),
                source_title: Some("Other Brief Judgment".into()),
                passage: "We overrule Smith v. Jones.".into(),
            },
        ];
        // Out-of-scope edge must not leak into a brief-scoped answer.
        let scoped = resolve_treatment(Some("Smith v. Jones"), &edges, Some(&scope));
        assert_eq!(scoped.status, TreatmentStatus::Followed);
        assert_eq!(scoped.evidence_source_title.as_deref(), Some("Later One"));
        // Full vault: the overruling edge wins by precedence.
        let full = resolve_treatment(Some("Smith v. Jones"), &edges, None);
        assert_eq!(full.status, TreatmentStatus::Overruled);
        // No evidence at all: Unknown, never a guess.
        let none = resolve_treatment(Some("Nobody v. Noone"), &edges, None);
        assert_eq!(none.status, TreatmentStatus::Unknown);
        assert!(none.evidence_passage.is_none());
    }

    #[test]
    fn treatment_note_flags_only_adverse_statuses() {
        let mut overruled = Citation {
            doc_id: "doc:0".into(),
            doc_title: Some("Smith v. Jones".into()),
            chunk_id: "c0".into(),
            ordinal: 0,
            score: 0.9,
            snippet: "text".into(),
            sentence: None,
            stale: None,
            source_url: None,
            source_date: None,
            treatment: Some(CitationTreatment {
                status: TreatmentStatus::Overruled,
                evidence_source_title: Some("Later Judgment".into()),
                evidence_passage: Some("We overrule Smith v. Jones.".into()),
                evidence_doc_id: Some("d2".into()),
            }),
        };
        let note = treatment_note(std::slice::from_ref(&overruled)).unwrap();
        assert!(note.contains("was overruled in Later Judgment"));
        assert!(note.contains("We overrule Smith v. Jones."));
        overruled.treatment = Some(CitationTreatment {
            status: TreatmentStatus::Followed,
            evidence_source_title: Some("Later Judgment".into()),
            evidence_passage: Some("We follow Smith v. Jones.".into()),
            evidence_doc_id: Some("d2".into()),
        });
        assert!(treatment_note(std::slice::from_ref(&overruled)).is_none());
        overruled.treatment = Some(CitationTreatment::unknown());
        assert!(treatment_note(std::slice::from_ref(&overruled)).is_none());
    }

    #[test]
    fn store_add_dedupes_and_remove_chunks() {
        let store = TreatmentStore::new();
        let edge = TreatmentEdge {
            target: "Smith v. Jones".into(),
            kind: TreatmentKind::Overruled,
            source_doc_id: "d1".into(),
            source_chunk_id: "c1".into(),
            source_title: None,
            passage: "We overrule Smith v. Jones.".into(),
        };
        assert_eq!(store.add(vec![edge.clone(), edge.clone()]), 1);
        assert_eq!(store.count(), 1);
        assert_eq!(store.remove_chunks(&["c1".to_string()]), 1);
        assert_eq!(store.count(), 0);
    }

    // -- Real-pair eval (genuine public judgment text, not invented) ----
    //
    // Source: Justice K.S. Puttaswamy (Retd.) v. Union of India,
    // W.P.(C) No. 494 of 2012, Supreme Court of India, decided
    // 24 August 2017 — full judgment text as reproduced at
    // legalauthority.in/judgement/justice-k-s-puttaswamy-retd-vs-union-of-india-1041
    // (Chandrachud J., majority, para 121; Nariman J., concurring).
    #[test]
    fn real_pair_puttaswamy_overrules() {
        // Chandrachud J., para 121 (operative sentence).
        let edges = detect(
            "ADM Jabalpur must be and is accordingly overruled. We also overrule the decision \
             in Union of India v Bhanudas Krishna Gawde, which followed ADM Jabalpur.",
        );
        let kinds: Vec<(&str, TreatmentKind)> =
            edges.iter().map(|e| (e.target.as_str(), e.kind)).collect();
        assert!(kinds.contains(&("ADM Jabalpur", TreatmentKind::Overruled)), "got {kinds:?}");
        assert!(
            kinds.contains(&("Union of India v. Bhanudas Krishna Gawde", TreatmentKind::Overruled)),
            "got {kinds:?}"
        );
        // Resolution against the real cited titles.
        assert_eq!(
            resolve_treatment(
                Some("Additional District Magistrate, Jabalpur v. Shivakant Shukla"),
                &edges,
                None
            )
            .status,
            TreatmentStatus::Overruled
        );
        assert_eq!(
            resolve_treatment(
                Some("Union of India v. Bhanudas Krishna Gawde"),
                &edges,
                None
            )
            .status,
            TreatmentStatus::Overruled
        );
        // Nariman J., concurring: M.P. Sharma + Kharak Singh overruled.
        let edges2 = detect(
            "M.P. Sharma (supra) and the majority in Kharak Singh (supra), to the extent \
             that they indicate to the contrary, stand overruled.",
        );
        assert_eq!(
            resolve_treatment(Some("M. P. Sharma v. Satish Chandra"), &edges2, None).status,
            TreatmentStatus::Overruled
        );
        assert_eq!(
            resolve_treatment(Some("Kharak Singh v. State of Uttar Pradesh"), &edges2, None).status,
            TreatmentStatus::Overruled
        );
        // Documented non-detection (by design): the para-119
        // "seriously flawed" sentence states criticism, not a
        // treatment verb, so it yields no edge — Unknown, not a guess.
        let flawed = detect(
            "The judgments rendered by all the four judges constituting the majority in \
             Additional District Magistrate, Jabalpur are seriously flawed.",
        );
        assert!(flawed.is_empty());
    }
}
