//! Citation-integrity audit: a mechanical check that every citation
//! marker in a generated answer is actually supported by the source it
//! points at. For each `[doc:N]` / `[web:N]` / `[private:N]` marker
//! (including grouped forms like `[web:1, web:2]`), the key terms of
//! its containing sentence (content words plus every number — dates
//! and amounts are the classic hallucination carriers) are checked
//! against the cited source's excerpt — the same excerpt the model was
//! shown. A marker whose sentence shares less than half its key terms
//! with the source is stripped; a citation to a source that was never
//! provided is stripped outright. Counts are returned as a receipt the
//! UI shows ("7 citations checked, 7 verified").
//!
//! Deliberately conservative: unverifiable sentences (no key terms)
//! are left untouched, and the answer text itself is never rewritten —
//! only unsupported markers are removed.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct CitationAudit {
    /// Markers that could be checked (sentence had key terms, or the
    /// referenced source does not exist at all).
    pub checked: usize,
    /// Markers whose sentence is supported by the cited source.
    pub verified: usize,
    /// Markers removed for lack of support.
    pub stripped: usize,
}

const STOP: &[&str] = &[
    "the", "a", "an", "of", "to", "in", "on", "for", "and", "or", "is",
    "are", "was", "were", "be", "been", "by", "at", "as", "it", "its",
    "this", "that", "with", "from", "under", "per", "shall", "have",
    "has", "had", "not", "but", "any", "all", "such", "which", "who",
    "when", "where", "what", "how", "than", "then", "there", "into",
    "over", "after", "before", "between", "through", "during", "about",
    "against", "within", "without", "upon", "also", "may", "can",
    "will", "would", "should", "could", "one", "two", "held", "case",
    "court", "act", "section", "however", "thus", "hence", "therefore",
];

fn key_terms(sentence: &str) -> Vec<String> {
    let mut terms: Vec<String> = crate::bm25::tokenize(sentence)
        .into_iter()
        .filter(|t| {
            t.chars().any(|c| c.is_ascii_digit())
                || (t.len() >= 4 && !STOP.contains(&t.as_str()))
        })
        .collect();
    terms.sort();
    terms.dedup();
    terms
}

fn is_marker_key(s: &str) -> bool {
    match s.split_once(':') {
        Some((kind, n)) => {
            matches!(kind, "doc" | "web" | "private")
                && !n.is_empty()
                && n.chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

/// Parse a bracket's contents into marker keys: `[web:1]`, or the
/// grouped `[web:1, web:2]` models like to emit. None if the bracket
/// is not (entirely) citation markers.
fn bracket_keys(inner: &str) -> Option<Vec<String>> {
    let keys: Vec<String> = inner.split(',').map(|p| p.trim().to_string()).collect();
    if !keys.is_empty() && keys.iter().all(|k| is_marker_key(k)) {
        Some(keys)
    } else {
        None
    }
}

/// Audit `answer` against `sources` (marker key -> source excerpt text).
/// Returns the cleaned answer and the audit receipt.
pub fn audit_answer(answer: &str, sources: &[(String, String)]) -> (String, CitationAudit) {
    let lookup: std::collections::HashMap<&str, &str> = sources
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut audit = CitationAudit::default();
    let mut out = String::with_capacity(answer.len());
    let bytes = answer.as_bytes();
    let mut i = 0;
    let mut copied_to = 0;

    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close) = answer[i + 1..].find(']') {
                let inner = &answer[i + 1..i + 1 + close];
                if let Some(keys) = bracket_keys(inner) {
                    let end = i + 1 + close + 1;
                    // Sentence span around the bracket.
                    let start = answer[..i]
                        .rfind(|c| c == '.' || c == '!' || c == '?' || c == '\n')
                        .map(|p| p + 1)
                        .unwrap_or(0);
                    let fwd = answer[end..]
                        .find(|c| c == '.' || c == '!' || c == '?' || c == '\n')
                        .map(|p| end + p)
                        .unwrap_or(answer.len());
                    let sentence: String =
                        answer[start..fwd].replace(&answer[i..end], " ");
                    let terms = key_terms(&sentence);

                    let mut survivors: Vec<&str> = Vec::new();
                    for key in &keys {
                        match lookup.get(key.as_str()) {
                            None => {
                                // Citation to a source that was never
                                // provided — hallucinated reference.
                                audit.checked += 1;
                                audit.stripped += 1;
                            }
                            Some(source) => {
                                if terms.is_empty() {
                                    survivors.push(key);
                                } else {
                                    audit.checked += 1;
                                    let hay = source.to_lowercase();
                                    let found = terms
                                        .iter()
                                        .filter(|t| hay.contains(t.as_str()))
                                        .count();
                                    if (found as f32) >= (terms.len() as f32) * 0.5 {
                                        audit.verified += 1;
                                        survivors.push(key);
                                    } else {
                                        audit.stripped += 1;
                                    }
                                }
                            }
                        }
                    }

                    out.push_str(&answer[copied_to..i]);
                    if !survivors.is_empty() {
                        out.push('[');
                        out.push_str(&survivors.join(", "));
                        out.push(']');
                    }
                    copied_to = end;
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }
    out.push_str(&answer[copied_to..]);

    if audit.stripped == 0 {
        return (out, audit);
    }
    // Tidy artefacts of removal: space before punctuation, doubles.
    let out = out
        .replace(" .", ".")
        .replace(" ,", ",")
        .replace(" ;", ";")
        .replace("  ", " ");
    (out, audit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn srcs() -> Vec<(String, String)> {
        vec![
            (
                "doc:0".into(),
                "Section 86: income-tax shall not be payable on interest due on any security of the Central Government issued income-tax free.".into(),
            ),
            (
                "web:0".into(),
                "The Supreme Court struck down the Electoral Bond Scheme on 15 February 2024 as unconstitutional.".into(),
            ),
        ]
    }

    #[test]
    fn supported_marker_is_kept() {
        let a = "The scheme was struck down by the Supreme Court on 15 February 2024 [web:0].";
        let (out, audit) = audit_answer(a, &srcs());
        assert_eq!(audit.verified, 1);
        assert_eq!(audit.stripped, 0);
        assert!(out.contains("[web:0]"));
    }

    #[test]
    fn unsupported_marker_is_stripped() {
        // The Kateel failure mode: an unrelated source cited for a claim.
        let a = "The annual licence fee for a ferry ghat is four hundred rupees [web:0].";
        let (out, audit) = audit_answer(a, &srcs());
        assert_eq!(audit.stripped, 1);
        assert!(!out.contains("[web:0]"));
        assert!(out.contains("four hundred rupees"));
    }

    #[test]
    fn grouped_markers_are_audited_individually() {
        let a = "Interest on Central Government securities is exempt from income-tax [doc:0, web:0].";
        let (out, audit) = audit_answer(a, &srcs());
        // doc:0 supports the sentence; web:0 (electoral bonds) does not.
        assert_eq!(audit.verified, 1);
        assert_eq!(audit.stripped, 1);
        assert!(out.contains("[doc:0]"));
        assert!(!out.contains("web:0"));
    }

    #[test]
    fn unknown_source_marker_is_stripped() {
        let a = "Interest on Central Government securities is exempt [doc:7].";
        let (out, audit) = audit_answer(a, &srcs());
        assert_eq!(audit.stripped, 1);
        assert!(!out.contains("[doc:7]"));
    }

    #[test]
    fn unverifiable_sentence_is_left_alone() {
        let a = "This is so [doc:0].";
        let (out, audit) = audit_answer(a, &srcs());
        assert_eq!(audit.checked, 0);
        assert!(out.contains("[doc:0]"));
    }
}
