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
//! - Forum / state / date come from stored document metadata first
//!   (`DocMeta`: ingest inference from title + judgment header, or an
//!   explicit upload override). Title inference is only the last-resort
//!   fallback for documents whose metadata is still empty, so a title
//!   with no forum marker no longer forces `Unknown` forum when the
//!   header names the court.
//! - Treatment + reversal detection runs over full parent-chunk texts
//!   at ingest, and the resulting V2 edges are stored (`V2EdgeStore`,
//!   persisted as `citator_v2.json`). Ask resolution uses those stored
//!   edges; snippet detection remains only as a fallback for documents
//!   ingested before this change (no stored edges yet).
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
use crate::query::{CitatorContext, Citation, DocScope};
use citator_v2::{
    detect_reversals, detect_treatments, resolve_ask, AskInput, Domain, EdgeTarget, Forum,
    Judgment, SimpleDate, TreatmentEdge as V2Edge, TreatmentKind as V2Kind,
    TreatmentStatus as V2Status,
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

// ---------------------------------------------------------------------------
// Stored forum / state / date metadata (FIX 1)
// ---------------------------------------------------------------------------
//
// The engine compares state strings exactly, so every layer — ingest
// inference, upload override, request context — normalizes to the same
// two-letter code vocabulary. Forums serialize as their variant name
// ("HighCourt", "Aar", …); dates serialize as YYYY-MM-DD.

/// State name (or High Court seat name) → code. The seat names mirror
/// `infer_state`'s table so inference and request normalization agree;
/// the joint Punjab and Haryana High Court maps to PB by that same
/// convention.
const STATE_NAMES: &[(&str, &str)] = &[
    ("andhra pradesh", "AP"),
    ("arunachal pradesh", "AR"),
    ("assam", "AS"),
    ("bihar", "BR"),
    ("chhattisgarh", "CG"),
    ("goa", "GA"),
    ("gujarat", "GJ"),
    ("haryana", "HR"),
    ("himachal pradesh", "HP"),
    ("jammu and kashmir", "JK"),
    ("jharkhand", "JH"),
    ("karnataka", "KA"),
    ("kerala", "KL"),
    ("madhya pradesh", "MP"),
    ("maharashtra", "MH"),
    ("manipur", "MN"),
    ("meghalaya", "ML"),
    ("odisha", "OD"),
    ("orissa", "OD"),
    ("punjab", "PB"),
    ("punjab and haryana", "PB"),
    ("rajasthan", "RJ"),
    ("sikkim", "SK"),
    ("tamil nadu", "TN"),
    ("telangana", "TS"),
    ("tripura", "TR"),
    ("uttar pradesh", "UP"),
    ("uttarakhand", "UK"),
    ("west bengal", "WB"),
    ("delhi", "DL"),
    ("national capital territory of delhi", "DL"),
    ("allahabad", "UP"),
    ("bombay", "MH"),
    ("calcutta", "WB"),
    ("madras", "TN"),
    ("gauhati", "AS"),
];

/// Normalize a user-supplied state (request context or upload override)
/// to a code. Accepts codes ("MH"), full names ("Maharashtra"), and
/// dotted codes ("M.H."). Unknown input yields None: for request
/// context that means "no state context" (never a crash); upload
/// overrides reject unknown input separately, at the API layer.
pub fn normalize_request_state(s: &str) -> Option<String> {
    let squashed: String = s
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| *c != '.')
        .collect();
    let norm = squashed.split_whitespace().collect::<Vec<_>>().join(" ");
    if norm.is_empty() || norm == "in" || norm == "india" {
        return None;
    }
    if norm.len() == 2 && norm.chars().all(|c| c.is_ascii_alphabetic()) {
        let code = norm.to_uppercase();
        if STATE_NAMES.iter().any(|(_, c)| *c == code) {
            return Some(code);
        }
        return None;
    }
    STATE_NAMES
        .iter()
        .find(|(name, _)| *name == norm)
        .map(|(_, code)| code.to_string())
}

/// Parse a forum from an explicit override value. Accepts variant names
/// ("HighCourt"), display names ("High Court"), and human forms
/// ("Supreme Court of India", "High Court of Bombay") via the title
/// inference fallback. Unknown input yields None.
pub fn parse_forum(s: &str) -> Option<Forum> {
    let squashed: String = s
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let direct = match squashed.as_str() {
        "supremecourt" | "supremecourtofindia" => Some(Forum::SupremeCourt),
        "highcourt" => Some(Forum::HighCourt),
        "itat" | "incometaxappellatetribunal" => Some(Forum::Itat),
        "aar" | "authorityforadvanceruling" => Some(Forum::Aar),
        "aaar" | "appellateauthorityforadvanceruling" => Some(Forum::Aaar),
        "nclt" | "nationalcompanylawtribunal" => Some(Forum::Nclt),
        "nclat" | "nationalcompanylawappellatetribunal" => Some(Forum::Nclat),
        "cci" | "competitioncommissionofindia" => Some(Forum::Cci),
        "sat" | "securitiesappellatetribunal" => Some(Forum::Sat),
        "ncdrc" | "nationalconsumerdisputesredressalcommission" => Some(Forum::Ncdrc),
        "statecommission" | "stateconsumerdisputesredressalcommission" => {
            Some(Forum::StateCommission)
        }
        "districtcommission" | "districtconsumerdisputesredressalcommission" => {
            Some(Forum::DistrictCommission)
        }
        "cat" | "centraladministrativetribunal" => Some(Forum::Cat),
        "ngt" | "nationalgreentribunal" => Some(Forum::Ngt),
        "districtcourt" | "districtjudge" => Some(Forum::DistrictCourt),
        "sessionscourt" | "courtofsession" => Some(Forum::SessionsCourt),
        "magistrate" | "judicialmagistrate" | "metropolitanmagistrate" => Some(Forum::Magistrate),
        "foreigncourt" => Some(Forum::ForeignCourt),
        "internationalcourt" | "internationalcourtofjustice" | "icj" => {
            Some(Forum::InternationalCourt)
        }
        "arbitraltribunal" => Some(Forum::ArbitralTribunal),
        _ => None,
    };
    if direct.is_some() {
        return direct;
    }
    match infer_forum(s) {
        Forum::Unknown => None,
        f => Some(f),
    }
}

/// Serialize a forum for storage in `DocMeta.forum`.
pub fn forum_to_stored(f: Forum) -> &'static str {
    match f {
        Forum::DistrictCourt => "DistrictCourt",
        Forum::Magistrate => "Magistrate",
        Forum::SessionsCourt => "SessionsCourt",
        Forum::HighCourt => "HighCourt",
        Forum::SupremeCourt => "SupremeCourt",
        Forum::Itat => "Itat",
        Forum::Aar => "Aar",
        Forum::Aaar => "Aaar",
        Forum::Nclt => "Nclt",
        Forum::Nclat => "Nclat",
        Forum::Cci => "Cci",
        Forum::Sat => "Sat",
        Forum::DistrictCommission => "DistrictCommission",
        Forum::StateCommission => "StateCommission",
        Forum::Ncdrc => "Ncdrc",
        Forum::Cat => "Cat",
        Forum::Ngt => "Ngt",
        Forum::ForeignCourt => "ForeignCourt",
        Forum::InternationalCourt => "InternationalCourt",
        Forum::ArbitralTribunal => "ArbitralTribunal",
        Forum::Unknown => "Unknown",
    }
}

/// Parse a stored forum back. "Unknown" and unrecognized values yield
/// None so the caller falls back to title inference.
pub fn forum_from_stored(s: &str) -> Option<Forum> {
    match s {
        "DistrictCourt" => Some(Forum::DistrictCourt),
        "Magistrate" => Some(Forum::Magistrate),
        "SessionsCourt" => Some(Forum::SessionsCourt),
        "HighCourt" => Some(Forum::HighCourt),
        "SupremeCourt" => Some(Forum::SupremeCourt),
        "Itat" => Some(Forum::Itat),
        "Aar" => Some(Forum::Aar),
        "Aaar" => Some(Forum::Aaar),
        "Nclt" => Some(Forum::Nclt),
        "Nclat" => Some(Forum::Nclat),
        "Cci" => Some(Forum::Cci),
        "Sat" => Some(Forum::Sat),
        "DistrictCommission" => Some(Forum::DistrictCommission),
        "StateCommission" => Some(Forum::StateCommission),
        "Ncdrc" => Some(Forum::Ncdrc),
        "Cat" => Some(Forum::Cat),
        "Ngt" => Some(Forum::Ngt),
        "ForeignCourt" => Some(Forum::ForeignCourt),
        "InternationalCourt" => Some(Forum::InternationalCourt),
        "ArbitralTribunal" => Some(Forum::ArbitralTribunal),
        _ => None,
    }
}

/// Strict YYYY-MM-DD date (the `decided_on` storage shape).
pub fn parse_decided_on(s: &str) -> Option<SimpleDate> {
    let b = s.as_bytes();
    if s.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: u32 = s[0..4].parse().ok()?;
    let m: u32 = s[5..7].parse().ok()?;
    let d: u32 = s[8..10].parse().ok()?;
    valid_date_parts(y, m, d)
}

fn valid_date_parts(y: u32, m: u32, d: u32) -> Option<SimpleDate> {
    if !(1900..=2100).contains(&y) || !(1..=12).contains(&m) || d < 1 {
        return None;
    }
    let max = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => 29,
    };
    if d > max {
        return None;
    }
    Some(SimpleDate::new(y, m, d))
}

/// Format a date for `decided_on` storage.
pub fn format_decided_on(d: SimpleDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)
}

const MONTH_WORDS: &[(&str, u32)] = &[
    ("january", 1),
    ("february", 2),
    ("march", 3),
    ("april", 4),
    ("may", 5),
    ("june", 6),
    ("july", 7),
    ("august", 8),
    ("september", 9),
    ("october", 10),
    ("november", 11),
    ("december", 12),
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("sept", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
];

/// Find a date inside free text: "24 August 2017", "August 24, 2017",
/// "24.08.2017", "24/08/2017", "2017-08-24". Returns the first
/// plausible date, skipping dates that describe an impugned (lower)
/// judgment rather than this one.
fn parse_date_in_text(text: &str) -> Option<SimpleDate> {
    let lower = text.to_lowercase();
    // Numeric forms first (unambiguous): scan for d[d] sep d[d] sep yyyy
    // and yyyy-mm-dd.
    let chars: Vec<char> = lower.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let num1: String = chars[i..j].iter().collect();
            if j < chars.len() && matches!(chars[j], '.' | '/' | '-') {
                let sep = chars[j];
                let mut k = j + 1;
                while k < chars.len() && chars[k].is_ascii_digit() {
                    k += 1;
                }
                let num2: String = chars[j + 1..k].iter().collect();
                if k < chars.len() && chars[k] == sep {
                    let mut l = k + 1;
                    while l < chars.len() && chars[l].is_ascii_digit() {
                        l += 1;
                    }
                    let num3: String = chars[k + 1..l].iter().collect();
                    if num1.len() == 4 {
                        // YYYY-MM-DD (only with '-').
                        if sep == '-' {
                            if let (Ok(y), Ok(m), Ok(d)) =
                                (num1.parse(), num2.parse(), num3.parse())
                            {
                                if let Some(date) = valid_date_parts(y, m, d) {
                                    return Some(date);
                                }
                            }
                        }
                    } else if num3.len() == 4 {
                        if let (Ok(d), Ok(m), Ok(y)) =
                            (num1.parse(), num2.parse(), num3.parse())
                        {
                            if let Some(date) = valid_date_parts(y, m, d) {
                                return Some(date);
                            }
                        }
                    }
                    i = l;
                    continue;
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    // Month-word forms: "24 August 2017" or "August 24, 2017".
    let tokens: Vec<String> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();
    for (idx, tok) in tokens.iter().enumerate() {
        let Some((_, month)) = MONTH_WORDS.iter().find(|(w, _)| *w == tok.as_str()) else {
            continue;
        };
        let day_before = idx
            .checked_sub(1)
            .and_then(|p| tokens[p].parse::<u32>().ok());
        let day_after = tokens
            .get(idx + 1)
            .and_then(|t| t.parse::<u32>().ok());
        let year_next = tokens.get(idx + 1).and_then(|t| {
            if t.len() == 4 {
                t.parse::<u32>().ok()
            } else {
                None
            }
        });
        let year_after = tokens.get(idx + 2).and_then(|t| {
            if t.len() == 4 {
                t.parse::<u32>().ok()
            } else {
                None
            }
        });
        // "24 August 2017": day before, year right after the month.
        if let (Some(d), Some(y)) = (day_before, year_next) {
            if let Some(date) = valid_date_parts(y, *month, d) {
                return Some(date);
            }
        }
        // "August 24, 2017": day after, year two after the month.
        if let (Some(d), Some(y)) = (day_after, year_after) {
            if let Some(date) = valid_date_parts(y, *month, d) {
                return Some(date);
            }
        }
        // "August 2017": no day — not a decision date, skip.
    }
    None
}

/// Scan a judgment header for a decision date. Prefers a date near
/// "decided on" / "pronounced on" / "delivered on" wording (this
/// judgment's date); falls back to the first plausible date in the
/// header that is not an impugned judgment's date.
fn scan_header_date(header: &str) -> Option<SimpleDate> {
    let lower = header.to_lowercase();
    for kw in ["decided on", "pronounced on", "delivered on", "date of judgment", "date of decision"] {
        if let Some(pos) = lower.find(kw) {
            let end = (pos + 80).min(lower.len());
            if let Some(d) = parse_date_in_text(&lower[pos..end]) {
                return Some(d);
            }
        }
    }
    // Fallback: first plausible date, skipping impugned references.
    let mut search_from = 0usize;
    while let Some(d) = parse_date_in_text(&lower[search_from..]) {
        // Re-find the date's position to check its context. The numeric
        // and month forms both contain the year; locate it.
        let year_str = d.year.to_string();
        if let Some(rel) = lower[search_from..].find(&year_str) {
            let abs = search_from + rel;
            let ctx_start = abs.saturating_sub(40);
            if lower[ctx_start..abs].contains("impugned") {
                search_from = abs + year_str.len();
                continue;
            }
        }
        return Some(d);
    }
    None
}

/// Court-name needles for header scanning, most specific first.
/// Position in the header decides (the deciding court's name sits at
/// the very top); order breaks ties between needles at one position.
const HEADER_FORUMS: &[(&str, Forum)] = &[
    ("appellate authority for advance ruling", Forum::Aaar),
    ("authority for advance ruling", Forum::Aar),
    ("national company law appellate tribunal", Forum::Nclat),
    ("national company law tribunal", Forum::Nclt),
    ("income tax appellate tribunal", Forum::Itat),
    ("national consumer disputes redressal commission", Forum::Ncdrc),
    ("state consumer disputes redressal commission", Forum::StateCommission),
    ("district consumer disputes redressal commission", Forum::DistrictCommission),
    ("competition commission of india", Forum::Cci),
    ("securities appellate tribunal", Forum::Sat),
    ("central administrative tribunal", Forum::Cat),
    ("national green tribunal", Forum::Ngt),
    ("supreme court of india", Forum::SupremeCourt),
    ("supreme court", Forum::SupremeCourt),
    ("high court", Forum::HighCourt),
    ("court of session", Forum::SessionsCourt),
    ("sessions court", Forum::SessionsCourt),
    ("district court", Forum::DistrictCourt),
    ("district judge", Forum::DistrictCourt),
    ("judicial magistrate", Forum::Magistrate),
    ("metropolitan magistrate", Forum::Magistrate),
];

/// Earliest court-name needle in a header, or None.
fn scan_header_forum(header_lower: &str) -> Option<Forum> {
    let mut best: Option<(usize, usize, Forum)> = None;
    for (order, (needle, forum)) in HEADER_FORUMS.iter().enumerate() {
        if let Some(pos) = header_lower.find(needle) {
            let better = match best {
                None => true,
                Some((bp, bo, _)) => pos < bp || (pos == bp && order < bo),
            };
            if better {
                best = Some((pos, order, *forum));
            }
        }
    }
    best.map(|(_, _, f)| f)
}

/// Deterministically inferred forum / state / decision date for one
/// document, from its title first and its judgment header second.
/// Title wins for forum (a citation title is explicit); for dates the
/// header's exact date wins over the title's bare year.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferredMeta {
    pub forum: Forum,
    pub state: Option<String>,
    pub date: Option<SimpleDate>,
}

pub fn infer_doc_meta(title: &str, header_text: &str) -> InferredMeta {
    let head: String = header_text.chars().take(4000).collect();
    let head_lower = head.to_lowercase();
    let forum = match infer_forum(title) {
        Forum::Unknown => scan_header_forum(&head_lower).unwrap_or(Forum::Unknown),
        f => f,
    };
    let state = infer_state(title).or_else(|| infer_state(&head));
    let date = scan_header_date(&head).or_else(|| title_year(title));
    InferredMeta { forum, state, date }
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

// ---------------------------------------------------------------------------
// Stored V2 edges (FIX 2)
// ---------------------------------------------------------------------------
//
// Full-text V2 detection at ingest writes edges here; Ask resolution
// reads them. This store is deliberately SEPARATE from V1's
// `TreatmentStore`: V1's store, detection, and resolution are never
// touched by V2 writes, so V1 behavior with the switch off is exactly
// as before (the V1 path never reads this store).

/// Treatment kind for a stored V2 edge. Mirrors the engine's
/// `TreatmentKind` (including `Reversed`, which V1's enum lacks).
/// Serialized snake_case for `citator_v2.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredV2Kind {
    Overruled,
    Reversed,
    Followed,
    Distinguished,
    Applied,
    Affirmed,
}

fn stored_kind_to_engine(kind: StoredV2Kind) -> V2Kind {
    match kind {
        StoredV2Kind::Overruled => V2Kind::Overruled,
        StoredV2Kind::Reversed => V2Kind::Reversed,
        StoredV2Kind::Followed => V2Kind::Followed,
        StoredV2Kind::Distinguished => V2Kind::Distinguished,
        StoredV2Kind::Applied => V2Kind::Applied,
        StoredV2Kind::Affirmed => V2Kind::Affirmed,
    }
}

fn engine_kind_to_stored(kind: V2Kind) -> StoredV2Kind {
    match kind {
        V2Kind::Overruled => StoredV2Kind::Overruled,
        V2Kind::Reversed => StoredV2Kind::Reversed,
        V2Kind::Followed => StoredV2Kind::Followed,
        V2Kind::Distinguished => StoredV2Kind::Distinguished,
        V2Kind::Applied => StoredV2Kind::Applied,
        V2Kind::Affirmed => StoredV2Kind::Affirmed,
    }
}

/// One V2 treatment relationship, as detected at ingest over a full
/// parent-chunk text. `target_doc_id` is set when the engine linked
/// the edge to a specific judgment (appeal number / CNR / party
/// match); otherwise the target is a case name resolved at Ask time
/// by the nearest-below rule. The passage is the provenance — a status
/// is never asserted without one.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredV2Edge {
    pub target_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_doc_id: Option<String>,
    pub kind: StoredV2Kind,
    pub source_doc_id: String,
    pub source_chunk_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_title: Option<String>,
    pub passage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linkage: Option<String>,
}

/// In-memory V2 edge store, shared between the ingest pipeline
/// (writer) and the query service (reader) behind one Arc. Mirrors
/// V1's `TreatmentStore` semantics: deduped insert, chunk-scoped
/// removal on document deletion, snapshot/restore for persistence.
#[derive(Debug, Default)]
pub struct V2EdgeStore {
    edges: std::sync::RwLock<Vec<StoredV2Edge>>,
}

impl V2EdgeStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn edge_key(e: &StoredV2Edge) -> String {
        format!(
            "{}|{}|{:?}|{}",
            e.source_chunk_id,
            e.target_name.to_lowercase(),
            e.kind,
            e.target_doc_id.as_deref().unwrap_or("")
        )
    }

    /// Insert edges, skipping exact duplicates. Returns new count.
    pub fn add(&self, edges: Vec<StoredV2Edge>) -> usize {
        let mut guard = self.edges.write().expect("lock poisoned");
        let mut known: std::collections::HashSet<String> =
            guard.iter().map(Self::edge_key).collect();
        let mut added = 0usize;
        for e in edges {
            if known.insert(Self::edge_key(&e)) {
                guard.push(e);
                added += 1;
            }
        }
        added
    }

    pub fn all(&self) -> Vec<StoredV2Edge> {
        self.edges.read().expect("lock poisoned").clone()
    }

    pub fn count(&self) -> usize {
        self.edges.read().expect("lock poisoned").len()
    }

    pub fn snapshot(&self) -> Vec<StoredV2Edge> {
        self.all()
    }

    pub fn restore(&self, edges: Vec<StoredV2Edge>) {
        *self.edges.write().expect("lock poisoned") = edges;
    }

    /// Drop edges sourced from deleted chunks (document removal).
    /// Returns removed count.
    pub fn remove_chunks(&self, chunk_ids: &[String]) -> usize {
        let removed: std::collections::HashSet<&str> =
            chunk_ids.iter().map(|s| s.as_str()).collect();
        let mut guard = self.edges.write().expect("lock poisoned");
        let before = guard.len();
        guard.retain(|e| !removed.contains(e.source_chunk_id.as_str()));
        before - guard.len()
    }
}

/// Run V2 detection (treatments + appellate reversals) over one full
/// parent-chunk text. `candidates` are the judgments the reversal
/// linkage may attach to (every other document in the corpus).
/// Same-caption edges are KEPT here on purpose: a judgment affirming
/// a lower judgment with the same caption is real, and Ask-time
/// resolution attaches it by document id (never to the source
/// itself). Filtering by current corpus would break retroactively
/// when a lower judgment is uploaded later, so storage keeps
/// everything and Ask time decides.
pub fn detect_v2_edges_for_chunk(
    text: &str,
    source: &Judgment,
    candidates: &[Judgment],
    source_chunk_id: &str,
    source_title: Option<&str>,
) -> Vec<StoredV2Edge> {
    let mut out: Vec<StoredV2Edge> = Vec::new();
    let mut push = |e: V2Edge| {
        let (target_name, target_doc_id) = match &e.target {
            EdgeTarget::JudgmentId(id) => (
                candidates
                    .iter()
                    .find(|j| &j.id == id)
                    .map(|j| j.case_name.clone())
                    .unwrap_or_default(),
                Some(id.clone()),
            ),
            EdgeTarget::CaseName(name) => (name.clone(), None),
        };
        out.push(StoredV2Edge {
            target_name,
            target_doc_id,
            kind: engine_kind_to_stored(e.kind),
            source_doc_id: e.source_doc_id.clone(),
            source_chunk_id: source_chunk_id.to_string(),
            source_title: source_title.map(|s| s.to_string()),
            passage: e.passage.clone(),
            linkage: e.linkage.clone(),
        });
    };
    for e in detect_treatments(text, source) {
        push(e);
    }
    for e in detect_reversals(text, source, candidates) {
        push(e);
    }
    out
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

/// Resolve a case-name target to the nearest judgment BELOW the source
/// forum (the decision under review). With several below-source matches
/// the highest forum wins; a tie at the highest rank keeps the CaseName
/// and the engine's strict rule drops it (never a guess between
/// same-rank candidates). The source document is excluded by id, not by
/// name — which is what makes same-caption appellate affirmances
/// attach: an SC judgment titled "Kumar v. State" affirming
/// "Kumar v. State" resolves to the HC step, not to the district step
/// and not to itself (V1's name-based self-guard cannot make that
/// distinction, so it drops the ingest edge entirely).
fn resolve_below(
    target: &str,
    source_forum: Forum,
    domain: Domain,
    judgments: &[Judgment],
    source_id: &str,
) -> EdgeTarget {
    let source_rank = citator_v2::ladders::rank_in_domain(source_forum, domain);
    let mut matches: Vec<&Judgment> = judgments
        .iter()
        .filter(|j| j.id != source_id)
        .filter(|j| citator_v2::normalize::same_matter(&j.case_name, target))
        .filter(|j| match (source_rank, citator_v2::ladders::rank_in_domain(j.forum, domain)) {
            (Some(sr), Some(cr)) => cr < sr,
            _ => true,
        })
        .collect();
    matches.sort_by(|a, b| {
        let ra = citator_v2::ladders::rank_in_domain(a.forum, domain).unwrap_or(0);
        let rb = citator_v2::ladders::rank_in_domain(b.forum, domain).unwrap_or(0);
        rb.cmp(&ra).then_with(|| a.id.cmp(&b.id))
    });
    if matches.len() >= 2 {
        let top = citator_v2::ladders::rank_in_domain(matches[0].forum, domain).unwrap_or(0);
        let second = citator_v2::ladders::rank_in_domain(matches[1].forum, domain).unwrap_or(0);
        if top == second {
            return EdgeTarget::CaseName(target.to_string());
        }
    }
    match matches.first() {
        Some(m) => EdgeTarget::JudgmentId(m.id.clone()),
        None => EdgeTarget::CaseName(target.to_string()),
    }
}

/// Build a V2 judgment for one document: stored metadata wins, title
/// inference is the last-resort fallback. `meta` maps doc id → DocMeta;
/// a missing entry means "no metadata" (legacy document), not an error.
pub(crate) fn judgment_from_meta(
    doc_id: &str,
    title: &str,
    meta: &HashMap<String, crate::pipeline::DocMeta>,
) -> Judgment {
    let m = meta.get(doc_id);
    let forum = m
        .and_then(|m| m.forum.as_deref())
        .and_then(forum_from_stored)
        .unwrap_or_else(|| infer_forum(title));
    let mut j = Judgment::new(doc_id, &match_name(title), forum);
    j.state = m
        .and_then(|m| m.state.clone())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| infer_state(title));
    j.date = m
        .and_then(|m| m.decided_on.as_deref())
        .and_then(parse_decided_on)
        .or_else(|| title_year(title));
    j.applicant = m
        .and_then(|m| m.applicant.clone())
        .filter(|s| !s.trim().is_empty());
    j.statute_jurisdiction = m
        .and_then(|m| m.jurisdiction.clone())
        .filter(|s| !s.trim().is_empty());
    j
}

/// True when a CaseName target's only matching judgment is the source
/// itself — a self-treatment, which must never resolve (a judgment
/// describing its own holding is not a later treatment of it). Genuine
/// above-source targets (a lower judgment following a higher one) are
/// untouched: their match set is not the source alone.
fn is_self_only_target(
    name: &str,
    source_id: &str,
    judgments: &[Judgment],
) -> bool {
    let matches: Vec<&Judgment> = judgments
        .iter()
        .filter(|j| citator_v2::normalize::same_matter(name, &j.case_name))
        .collect();
    matches.len() == 1 && matches[0].id == source_id
}

pub fn resolve_v2(
    citations: &[Citation],
    v1_edges: &[V1Edge],
    stored_v2: &[StoredV2Edge],
    scope: Option<&DocScope>,
    meta: &HashMap<String, crate::pipeline::DocMeta>,
    ctx: &CitatorContext,
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
        let j = judgment_from_meta(&c.doc_id, title, meta);
        display.insert(c.doc_id.clone(), title.to_string());
        judgments.push(j);
    }
    if judgments.is_empty() {
        return empty();
    }

    // Synthesized judgments for edge sources (chain walking needs them).
    // Only scope-visible edges are converted, mirroring the V1 rule.
    let visible_edges: Vec<&V1Edge> = v1_edges
        .iter()
        .filter(|e| match scope {
            Some(s) => s.contains(e.source_doc_id.as_str()),
            None => true,
        })
        .collect();
    let visible_stored: Vec<&StoredV2Edge> = stored_v2
        .iter()
        .filter(|e| match scope {
            Some(s) => s.contains(e.source_doc_id.as_str()),
            None => true,
        })
        .collect();
    for e in &visible_edges {
        let source_title = e.source_title.clone().unwrap_or_default();
        if !judgments.iter().any(|j| j.id == e.source_doc_id) {
            let sj = judgment_from_meta(&e.source_doc_id, &source_title, meta);
            judgments.push(sj);
            if !source_title.is_empty() {
                display.insert(e.source_doc_id.clone(), source_title);
            }
        }
    }
    for e in &visible_stored {
        let source_title = e.source_title.clone().unwrap_or_default();
        if !judgments.iter().any(|j| j.id == e.source_doc_id) {
            let sj = judgment_from_meta(&e.source_doc_id, &source_title, meta);
            judgments.push(sj);
            if !source_title.is_empty() {
                display.insert(e.source_doc_id.clone(), source_title);
            }
        }
    }

    let domain = best_fit_domain(&judgments.iter().map(|j| j.forum).collect::<Vec<_>>());

    // Forum of an edge's source, from its (metadata-aware) judgment
    // when synthesized, else title inference.
    let source_forum_of = |source_doc_id: &str, source_title: &str| -> Forum {
        judgments
            .iter()
            .find(|j| j.id == source_doc_id)
            .map(|j| j.forum)
            .unwrap_or_else(|| infer_forum(source_title))
    };

    // Convert V1 edges, resolving name targets against the judgment set
    // with the nearest-below rule (see `resolve_below`).
    let mut edges: Vec<V2Edge> = Vec::new();
    for e in &visible_edges {
        let source_title = e.source_title.clone().unwrap_or_default();
        let source_forum = source_forum_of(&e.source_doc_id, &source_title);
        let target = resolve_below(&e.target, source_forum, domain, &judgments, &e.source_doc_id);
        edges.push(V2Edge {
            target,
            kind: map_kind(e.kind),
            source_doc_id: e.source_doc_id.clone(),
            source_title: e.source_title.clone().unwrap_or_default(),
            passage: e.passage.clone(),
            linkage: Some("v1-edge".to_string()),
        });
    }

    // Convert stored V2 edges (full-text ingest detection). A stored
    // edge duplicating a V1 edge (same source, kind, target) is
    // skipped — the V1 passage wins, as with snippets below.
    for e in &visible_stored {
        let source_title = e.source_title.clone().unwrap_or_default();
        let source_forum = source_forum_of(&e.source_doc_id, &source_title);
        let target = match &e.target_doc_id {
            Some(id) => EdgeTarget::JudgmentId(id.clone()),
            None => resolve_below(&e.target_name, source_forum, domain, &judgments, &e.source_doc_id),
        };
        let kind = stored_kind_to_engine(e.kind);
        let dup = edges.iter().any(|x| {
            x.source_doc_id == e.source_doc_id && x.kind == kind && x.target == target
        });
        if !dup {
            edges.push(V2Edge {
                target,
                kind,
                source_doc_id: e.source_doc_id.clone(),
                source_title: source_title.clone(),
                passage: e.passage.clone(),
                linkage: e.linkage.clone(),
            });
        }
    }

    // Snippet detection as a FALLBACK for documents ingested before
    // stored V2 edges existed: candidates restricted to judgments
    // below the source forum (an appellate judgment reverses or treats
    // a lower one, never a higher one). Unknown source rank keeps all
    // candidates except the source itself. Stored edges (above) already
    // cover same-caption affirmances — V1's name-based self-guard
    // dropped an SC judgment affirming "Kumar v. State" when the SC
    // judgment is itself titled "Kumar v. State", but the stored V2
    // path resolves by document id, so the affirmance lands on the HC
    // step below, never on the source itself — and the self-only guard
    // below drops it entirely when no lower judgment shares the caption.
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
        let mut snippet_edges = detect_reversals(&c.snippet, src, &candidates);
        snippet_edges.extend(detect_treatments(&c.snippet, src));
        for mut e in snippet_edges {
            if let EdgeTarget::CaseName(name) = &e.target {
                e.target = resolve_below(name, src.forum, domain, &cited_judgments, &src.id);
            }
            // Don't duplicate an edge the V1 conversion already
            // produced (same source, kind, and target); the V1
            // passage (from ingest) wins over the snippet passage.
            let dup = edges.iter().any(|x| {
                x.source_doc_id == e.source_doc_id && x.kind == e.kind && x.target == e.target
            });
            if !dup {
                edges.push(e);
            }
        }
    }

    // Self-only guard: drop converted edges whose CaseName target
    // matches nothing but the source judgment itself (a judgment
    // describing its own holding). Stored V2 edges keep same-caption
    // affirmances by design, so this is the check that stops one from
    // self-affirming when no lower judgment shares the caption.
    edges.retain(|e| match &e.target {
        EdgeTarget::CaseName(name) => !is_self_only_target(name, &e.source_doc_id, &judgments),
        EdgeTarget::JudgmentId(_) => true,
    });

    let cited_ids: Vec<String> = cited_judgments.iter().map(|j| j.id.clone()).collect();
    // Per-request context (already normalized at the API boundary):
    // the request state lets that state's High Court govern within it,
    // and the applicant gates the AAR/AAAR applicant-only rule.
    let input = AskInput {
        state: ctx.state.clone(),
        domain,
        applicant: ctx.applicant.clone(),
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
        let res = resolve_v2(&citations, &v1_edges, &[], None, &HashMap::new(), &CitatorContext::default());
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
    fn snippet_affirmance_resolves_to_nearest_below() {
        // Same-caption appellate case with NO V1 edge (V1's name-based
        // self-guard drops an SC judgment affirming "Kumar v. State"
        // when the SC judgment is itself titled "Kumar v. State").
        // The snippet path resolves by document id, so the affirmance
        // lands on the HC step below, never on the source itself.
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
                "We affirm Kumar v. State. The Supreme Court held that the High Court correctly applied the standard of proof.",
            ),
        ];
        let res = resolve_v2(&citations, &[], &[], None, &HashMap::new(), &CitatorContext::default());
        let by_id: HashMap<&str, &CitationTreatment> = res
            .treatments
            .iter()
            .map(|(id, t)| (id.as_str(), t))
            .collect();
        assert_eq!(by_id["doc:0"].status, V1Status::Reversed, "district reversed by HC snippet");
        assert_eq!(by_id["doc:1"].status, V1Status::GoodLaw, "HC affirmed by SC snippet");
        let line = res.governing_line.expect("governing line");
        assert!(line.contains("Supreme Court"), "{line}");
        assert_eq!(res.ranked_corpus_ids.last().unwrap(), "doc:0");
    }

    #[test]
    fn v2_disabled_by_default_off_switch() {
        std::env::remove_var("LEGAL_RAG_CITATOR_V2");
        assert!(!citator_v2_enabled());
    }

    // -- FIX 1: stored metadata -------------------------------------------

    #[test]
    fn parse_forum_accepts_variant_display_and_human_forms() {
        assert_eq!(parse_forum("HighCourt"), Some(Forum::HighCourt));
        assert_eq!(parse_forum("High Court"), Some(Forum::HighCourt));
        assert_eq!(parse_forum("High Court of Bombay"), Some(Forum::HighCourt));
        assert_eq!(parse_forum("Supreme Court of India"), Some(Forum::SupremeCourt));
        assert_eq!(parse_forum("UK Supreme Court"), Some(Forum::ForeignCourt));
        assert_eq!(parse_forum("AAR"), Some(Forum::Aar));
        assert_eq!(parse_forum("Appellate Authority for Advance Ruling"), Some(Forum::Aaar));
        assert_eq!(parse_forum("ITAT"), Some(Forum::Itat));
        assert_eq!(parse_forum("NCDRC"), Some(Forum::Ncdrc));
        assert_eq!(parse_forum("Sessions Court"), Some(Forum::SessionsCourt));
        assert_eq!(parse_forum("Section 86 IT Act"), None);
        assert_eq!(parse_forum("Some Random Tribunal"), None);
    }

    #[test]
    fn normalize_request_state_codes_names_unknown() {
        assert_eq!(normalize_request_state("MH").as_deref(), Some("MH"));
        assert_eq!(normalize_request_state("Maharashtra").as_deref(), Some("MH"));
        assert_eq!(normalize_request_state("M.H.").as_deref(), Some("MH"));
        assert_eq!(normalize_request_state("  up ").as_deref(), Some("UP"));
        assert_eq!(normalize_request_state("Uttar Pradesh").as_deref(), Some("UP"));
        assert_eq!(normalize_request_state("Punjab and Haryana").as_deref(), Some("PB"));
        assert_eq!(normalize_request_state("Tamil Nadu").as_deref(), Some("TN"));
        assert_eq!(normalize_request_state("XX"), None);
        assert_eq!(normalize_request_state("IN"), None);
        assert_eq!(normalize_request_state("India"), None);
        assert_eq!(normalize_request_state(""), None);
    }

    #[test]
    fn forum_stored_round_trip() {
        for f in [
            Forum::DistrictCourt,
            Forum::Magistrate,
            Forum::SessionsCourt,
            Forum::HighCourt,
            Forum::SupremeCourt,
            Forum::Itat,
            Forum::Aar,
            Forum::Aaar,
            Forum::Nclt,
            Forum::Nclat,
            Forum::Cci,
            Forum::Sat,
            Forum::DistrictCommission,
            Forum::StateCommission,
            Forum::Ncdrc,
            Forum::Cat,
            Forum::Ngt,
            Forum::ForeignCourt,
            Forum::InternationalCourt,
            Forum::ArbitralTribunal,
        ] {
            assert_eq!(forum_from_stored(forum_to_stored(f)), Some(f), "{f:?}");
        }
        assert_eq!(forum_from_stored("Unknown"), None);
        assert_eq!(forum_from_stored("Nope"), None);
    }

    #[test]
    fn infer_doc_meta_title_and_header() {
        // Title-only: forum + state + year.
        let m = infer_doc_meta("X v. Y — Allahabad High Court (2023)", "");
        assert_eq!(m.forum, Forum::HighCourt);
        assert_eq!(m.state.as_deref(), Some("UP"));
        assert_eq!(m.date, Some(SimpleDate::new(2023, 1, 1)));
        // Header-only: court at the top + "Decided on" exact date.
        let m = infer_doc_meta(
            "Kumar v. State",
            "IN THE SUPREME COURT OF INDIA\nCivil Appeal No. 12 of 2024\nDecided on 24 August 2017\nThe appeal is allowed.",
        );
        assert_eq!(m.forum, Forum::SupremeCourt);
        assert_eq!(m.date, Some(SimpleDate::new(2017, 8, 24)));
        // Header HC seat name yields forum + state.
        let m = infer_doc_meta(
            "Mehta v. Desai",
            "IN THE HIGH COURT OF JUDICATURE AT BOMBAY\nWrit Petition No. 5 of 2022\nDecided on 15 March 2023",
        );
        assert_eq!(m.forum, Forum::HighCourt);
        assert_eq!(m.state.as_deref(), Some("MH"));
        assert_eq!(m.date, Some(SimpleDate::new(2023, 3, 15)));
        // Title forum wins when both name a forum; header exact date
        // wins over the title's bare year.
        let m = infer_doc_meta(
            "Mehta v. Desai — Supreme Court (2025)",
            "IN THE HIGH COURT OF JUDICATURE AT BOMBAY\nDecided on 15 March 2023",
        );
        assert_eq!(m.forum, Forum::SupremeCourt);
        assert_eq!(m.date, Some(SimpleDate::new(2023, 3, 15)));
        // Numeric date forms.
        let m = infer_doc_meta("X", "Decided on 12.01.2024");
        assert_eq!(m.date, Some(SimpleDate::new(2024, 1, 12)));
        let m = infer_doc_meta("X", "Decided on 2024-01-12");
        assert_eq!(m.date, Some(SimpleDate::new(2024, 1, 12)));
    }

    #[test]
    fn header_date_skips_impugned_dates() {
        let m = infer_doc_meta(
            "Kumar v. State",
            "IN THE SUPREME COURT OF INDIA\nAppeal against the impugned judgment dated 12 January 2020.\nDecided on 15 March 2023.",
        );
        assert_eq!(m.date, Some(SimpleDate::new(2023, 3, 15)));
    }

    #[test]
    fn judgment_from_meta_prefers_stored_over_title() {
        let mut meta = HashMap::new();
        meta.insert(
            "doc:9".to_string(),
            crate::pipeline::DocMeta {
                forum: Some("HighCourt".to_string()),
                state: Some("MH".to_string()),
                decided_on: Some("2023-05-01".to_string()),
                applicant: Some("Acme Ltd".to_string()),
                jurisdiction: Some("IN".to_string()),
                ..Default::default()
            },
        );
        // Title claims SC 2025; stored metadata says HC/MH/2023-05-01.
        let j = judgment_from_meta("doc:9", "Mehta v. Desai — Supreme Court (2025)", &meta);
        assert_eq!(j.forum, Forum::HighCourt);
        assert_eq!(j.state.as_deref(), Some("MH"));
        assert_eq!(j.date, Some(SimpleDate::new(2023, 5, 1)));
        assert_eq!(j.applicant.as_deref(), Some("Acme Ltd"));
        assert_eq!(j.statute_jurisdiction.as_deref(), Some("IN"));
        // No metadata entry: title inference, as before.
        let j = judgment_from_meta("doc:8", "X — Allahabad High Court (2023)", &meta);
        assert_eq!(j.forum, Forum::HighCourt);
        assert_eq!(j.state.as_deref(), Some("UP"));
        assert_eq!(j.date, Some(SimpleDate::new(2023, 1, 1)));
        assert_eq!(j.applicant, None);
    }

    // -- FIX 2: stored V2 edges -------------------------------------------

    fn stored(
        target_name: &str,
        target_doc_id: Option<&str>,
        kind: StoredV2Kind,
        source_doc_id: &str,
        source_title: &str,
    ) -> StoredV2Edge {
        StoredV2Edge {
            target_name: target_name.to_string(),
            target_doc_id: target_doc_id.map(|s| s.to_string()),
            kind,
            source_doc_id: source_doc_id.to_string(),
            source_chunk_id: format!("chunk-{source_doc_id}"),
            source_title: Some(source_title.to_string()),
            passage: "passage".to_string(),
            linkage: None,
        }
    }

    #[test]
    fn stored_v2_edge_store_dedupes_and_removes() {
        let store = V2EdgeStore::new();
        let e = stored("Mehta v. Desai", None, StoredV2Kind::Reversed, "real-hc", "HC");
        assert_eq!(store.add(vec![e.clone(), e.clone()]), 1);
        assert_eq!(store.count(), 1);
        assert_eq!(store.remove_chunks(&["chunk-real-hc".to_string()]), 1);
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn full_text_detection_finds_late_reversal() {
        // Reversal language past the first 1500 chars: a snippet-only
        // scan misses it, full-text ingest detection must not.
        let filler = "The court heard arguments on the merits and considered the record in detail. ";
        let mut text = filler.repeat(40);
        text.push_str("In Mehta v. Desai, the appeal is allowed and the impugned judgment is set aside.");
        assert!(text.len() > 3000);
        let snippet: String = text.chars().take(1500).collect();
        assert!(!snippet.contains("appeal is allowed"));
        let source = Judgment::new("real-hc", "Kumar v. State", Forum::HighCourt);
        let candidates = vec![Judgment::new("real-dist", "Mehta v. Desai", Forum::DistrictCourt)];
        let edges = detect_v2_edges_for_chunk(&text, &source, &candidates, "c-hc", Some("HC"));
        assert!(
            edges.iter().any(|e| e.kind == StoredV2Kind::Reversed
                && e.target_name.contains("Mehta")),
            "got {edges:?}"
        );
        // The same detection over the snippet alone finds nothing.
        let snippet_edges = detect_reversals(&snippet, &source, &candidates);
        assert!(snippet_edges.is_empty(), "got {snippet_edges:?}");
    }

    #[test]
    fn stored_edge_resolution_matches_engine() {
        // No V1 edges, neutral snippets: statuses come from stored V2
        // edges alone (full-text ingest), including governing + rank-down.
        let citations = vec![
            cit("doc:0", "Mehta v. Desai — District Court (2021)", "The accused was convicted."),
            cit("doc:1", "Mehta v. Desai — Allahabad High Court (2023)", "The court heard the appeal."),
            cit("doc:2", "Mehta v. Desai — Supreme Court (2025)", "The court heard the matter."),
        ];
        let stored = vec![
            stored("Mehta v. Desai", Some("doc:0"), StoredV2Kind::Reversed, "real-hc", "Mehta v. Desai — Allahabad High Court (2023)"),
            stored("Mehta v. Desai", Some("doc:1"), StoredV2Kind::Affirmed, "real-sc", "Mehta v. Desai — Supreme Court (2025)"),
        ];
        let res = resolve_v2(&citations, &[], &stored, None, &HashMap::new(), &CitatorContext::default());
        let by_id: HashMap<&str, &CitationTreatment> = res
            .treatments
            .iter()
            .map(|(id, t)| (id.as_str(), t))
            .collect();
        assert_eq!(by_id["doc:0"].status, V1Status::Reversed);
        assert_eq!(by_id["doc:1"].status, V1Status::GoodLaw);
        let line = res.governing_line.expect("governing line");
        assert!(line.contains("Supreme Court"), "{line}");
        assert_eq!(res.ranked_corpus_ids.last().unwrap(), "doc:0");
    }

    #[test]
    fn same_caption_affirmance_stored_not_self() {
        // SC "Kumar v. State" affirming HC "Kumar v. State": the stored
        // CaseName edge must land on the HC step below, never on the
        // SC source itself.
        let citations = vec![
            cit("doc:0", "State v. Kumar — District Court (2021)", "The accused was convicted."),
            cit("doc:1", "Kumar v. State — Allahabad High Court (2023)", "The court heard the appeal."),
            cit("doc:2", "Kumar v. State — Supreme Court (2025)", "We affirm Kumar v. State."),
        ];
        let stored = vec![stored(
            "Kumar v. State",
            None,
            StoredV2Kind::Affirmed,
            "doc:2",
            "Kumar v. State — Supreme Court (2025)",
        )];
        let res = resolve_v2(&citations, &[], &stored, None, &HashMap::new(), &CitatorContext::default());
        let by_id: HashMap<&str, &CitationTreatment> = res
            .treatments
            .iter()
            .map(|(id, t)| (id.as_str(), t))
            .collect();
        assert_eq!(by_id["doc:1"].status, V1Status::GoodLaw, "HC affirmed, not Unknown");
        assert_eq!(by_id["doc:2"].status, V1Status::Unknown, "SC must not self-affirm");
    }

    #[test]
    fn self_only_affirmance_dropped() {
        // Only the SC is cited: its own "We affirm Kumar v. State"
        // edge names nothing but itself, so it must be dropped rather
        // than self-affirming.
        let citations = vec![cit(
            "doc:2",
            "Kumar v. State — Supreme Court (2025)",
            "We affirm Kumar v. State.",
        )];
        let stored = vec![stored(
            "Kumar v. State",
            None,
            StoredV2Kind::Affirmed,
            "doc:2",
            "Kumar v. State — Supreme Court (2025)",
        )];
        let res = resolve_v2(&citations, &[], &stored, None, &HashMap::new(), &CitatorContext::default());
        let by_id: HashMap<&str, &CitationTreatment> = res
            .treatments
            .iter()
            .map(|(id, t)| (id.as_str(), t))
            .collect();
        assert_eq!(by_id["doc:2"].status, V1Status::Unknown);
    }

    // -- FIX 3: per-request state/applicant context ------------------------

    fn ctx(state: Option<&str>, applicant: Option<&str>) -> CitatorContext {
        CitatorContext {
            state: state.map(|s| s.to_string()),
            applicant: applicant.map(|s| s.to_string()),
        }
    }

    fn state_chain_citations() -> Vec<Citation> {
        vec![
            cit("doc:0", "Sharma v. State — District Court (2021)", "The accused was convicted."),
            cit("doc:1", "Sharma v. State — High Court of Bombay (2023)", "The court heard the appeal."),
            cit("doc:2", "Sharma v. State — High Court of Delhi (2023)", "The court heard the appeal."),
        ]
    }

    #[test]
    fn state_scoped_governing_same_state_hc_governs() {
        // Bombay HC reverses the district step; with state MH the
        // same-state HC governs, and with no state context no
        // state-track authority binds at all (backward compatible).
        let citations = state_chain_citations();
        let stored = vec![stored(
            "Sharma v. State",
            Some("doc:0"),
            StoredV2Kind::Reversed,
            "real-bom",
            "Sharma v. State — High Court of Bombay (2023)",
        )];
        let res = resolve_v2(&citations, &[], &stored, None, &HashMap::new(), &ctx(Some("MH"), None));
        let line = res.governing_line.expect("MH governing line");
        assert!(line.contains("Bombay"), "{line}");
        let res_none = resolve_v2(&citations, &[], &stored, None, &HashMap::new(), &CitatorContext::default());
        assert!(res_none.governing_line.is_none(), "no state context binds no state authority");
    }

    #[test]
    fn national_governs_when_state_matches_nothing() {
        // Same chain plus the Supreme Court: with an unmatched state
        // (KA) both HCs are inapplicable and national authority
        // governs outright; with MH the SC still wins on forum rank
        // (hierarchy beats state, per the V2 conclusion policy).
        let mut citations = state_chain_citations();
        citations.push(cit(
            "doc:3",
            "Sharma v. State — Supreme Court (2025)",
            "The court heard the matter.",
        ));
        let stored = vec![stored(
            "Sharma v. State",
            Some("doc:0"),
            StoredV2Kind::Reversed,
            "real-bom",
            "Sharma v. State — High Court of Bombay (2023)",
        )];
        for state in ["KA", "MH"] {
            let res = resolve_v2(&citations, &[], &stored, None, &HashMap::new(), &ctx(Some(state), None));
            let line = res.governing_line.expect("governing line");
            assert!(line.contains("Supreme Court"), "state {state}: {line}");
        }
    }

    #[test]
    fn applicant_scoped_aar_rule() {
        // An AAR ruling governs only for its own applicant: match →
        // governs, mismatch or absent applicant → nothing governs.
        let citations = vec![cit(
            "doc:0",
            "In re: Nexa Traders Pvt Ltd — Authority for Advance Ruling (2023)",
            "The authority ruled on the classification.",
        )];
        let mut meta = HashMap::new();
        meta.insert(
            "doc:0".to_string(),
            crate::pipeline::DocMeta {
                forum: Some("Aar".to_string()),
                applicant: Some("Nexa Traders Pvt Ltd".to_string()),
                ..Default::default()
            },
        );
        let res = resolve_v2(
            &citations,
            &[],
            &[],
            None,
            &meta,
            &ctx(None, Some("Nexa Traders Pvt Ltd")),
        );
        let line = res.governing_line.expect("applicant-match governing line");
        assert!(line.contains("Advance Ruling"), "{line}");
        let res_other = resolve_v2(&citations, &[], &[], None, &meta, &ctx(None, Some("Other Corp")));
        assert!(res_other.governing_line.is_none(), "other applicant: AAR must not govern");
        let res_none = resolve_v2(&citations, &[], &[], None, &meta, &CitatorContext::default());
        assert!(res_none.governing_line.is_none(), "absent applicant: AAR must not govern");
    }

    #[test]
    fn stored_meta_lets_bare_title_join_chain() {
        // The HC citation carries NO forum marker in its title; only
        // stored metadata names it HighCourt/MH. It must still reverse
        // the district judgment and sit in the SC-governed chain.
        let citations = vec![
            cit("doc:0", "Mehta v. Desai — District Court (2021)", "The accused was convicted."),
            cit(
                "doc:1",
                "Mehta v. Desai",
                "In Mehta v. Desai, the appeal is allowed and the impugned judgment is set aside.",
            ),
            cit(
                "doc:2",
                "Mehta v. Desai — Supreme Court (2025)",
                "We affirm Mehta v. Desai.",
            ),
        ];
        let mut meta = HashMap::new();
        meta.insert(
            "doc:1".to_string(),
            crate::pipeline::DocMeta {
                forum: Some("HighCourt".to_string()),
                state: Some("MH".to_string()),
                decided_on: Some("2023-05-01".to_string()),
                ..Default::default()
            },
        );
        let res = resolve_v2(&citations, &[], &[], None, &meta, &CitatorContext::default());
        let by_id: HashMap<&str, &CitationTreatment> = res
            .treatments
            .iter()
            .map(|(id, t)| (id.as_str(), t))
            .collect();
        assert_eq!(by_id["doc:0"].status, V1Status::Reversed, "district reversed by metadata-HC");
        assert_eq!(by_id["doc:1"].status, V1Status::GoodLaw, "HC affirmed by SC");
        let line = res.governing_line.expect("governing line");
        assert!(line.contains("Supreme Court"), "{line}");
        assert_eq!(res.ranked_corpus_ids.last().unwrap(), "doc:0");
    }
}
