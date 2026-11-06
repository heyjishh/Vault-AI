//! Document editing primitives (Piece 85).
//!
//! The server-side edit flow (propose → approve → write back as a new
//! version) needs three things the codebase did not have; this module
//! is exactly those, nothing more:
//!
//! - [`apply_edit`] — one LLM edit pass over a document's text: the
//!   complete edited document back, fences stripped, provider failure
//!   pages rejected via the shared `is_failure_output` detector.
//! - [`compute_diff`] — a paragraph-level diff (added / removed
//!   passages) a human can actually approve from. Paragraphs are the
//!   unit because document edits are passage edits; a character diff
//!   of a contract is unreviewable.
//! - [`text_to_rtf`] — a minimal RTF writer so an edited RTF document
//!   can be regenerated in its own format server-side (the response
//!   export machinery is browser-side TypeScript and cannot be called
//!   from here). Formats with no server-side writer (PDF, DOCX) fall
//!   back to a stored text version, and the receipt says so — see the
//!   server's edit store.

use legal_rag_common::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::llm::{is_failure_output, LlmClient};

const EDIT_SYSTEM: &str = "You are a precise legal-document editor. Apply the EDIT INSTRUCTION to the \
DOCUMENT and return the COMPLETE edited document text — every passage, in order — and nothing else: \
no commentary, no summary of changes, no code fences. Apply the instruction exactly and completely; \
preserve everything the instruction does not touch, including headings, numbering, defined terms, \
names, dates and amounts. If the instruction asks for a passage to be added, write it in the \
document's own register and place it where it belongs.";

/// Run one edit pass: (document title, original text, instruction) →
/// the full edited text. An empty or provider-failure completion is
/// an error, never an "edited" document.
pub async fn apply_edit(
    llm: &dyn LlmClient,
    title: &str,
    original_text: &str,
    instruction: &str,
) -> Result<String> {
    let user = format!(
        "DOCUMENT TITLE: {title}\n\nEDIT INSTRUCTION: {instruction}\n\nDOCUMENT:\n{original_text}"
    );
    let raw = llm.complete(EDIT_SYSTEM, &user).await?;
    let text = strip_fences(&raw);
    if text.trim().is_empty() {
        return Err(Error::Upstream("edit pass returned no text".to_string()));
    }
    if is_failure_output(text.trim()) {
        return Err(Error::Upstream(
            "edit pass returned a provider failure page".to_string(),
        ));
    }
    Ok(text.trim().to_string())
}

/// Remove a wrapping markdown code fence, if the model added one.
fn strip_fences(raw: &str) -> String {
    let t = raw.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // Drop the fence line (and its optional language tag), then a
        // trailing fence if present.
        let body = match rest.find('\n') {
            Some(i) => &rest[i + 1..],
            None => rest,
        };
        let body = body.trim_end();
        let body = body.strip_suffix("```").unwrap_or(body);
        return body.trim().to_string();
    }
    t.to_string()
}

// ---------------------------------------------------------------------------
// Paragraph diff
// ---------------------------------------------------------------------------

/// Added and removed passages between two versions of a document,
/// in document order.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PassageDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

impl PassageDiff {
    pub fn added_count(&self) -> usize {
        self.added.len()
    }
    pub fn removed_count(&self) -> usize {
        self.removed.len()
    }
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
    /// One-line summary for receipts and list views.
    pub fn summary_line(&self) -> String {
        format!(
            "{} passage(s) added, {} removed",
            self.added.len(),
            self.removed.len()
        )
    }
}

/// Split a document into its paragraphs (blank-line separated,
/// trimmed, empties dropped). CRLF is normalized first.
fn paragraphs(text: &str) -> Vec<String> {
    let normalized = text.replace("\r\n", "\n");
    normalized
        .split("\n\n")
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// DP table cap: beyond this many paragraph pairs the exact LCS is
/// not worth its memory; the multiset fallback takes over.
const LCS_CELL_CAP: usize = 4_000_000;

/// Compute the passage diff between two document versions.
pub fn compute_diff(old_text: &str, new_text: &str) -> PassageDiff {
    let old = paragraphs(old_text);
    let new = paragraphs(new_text);
    if old == new {
        return PassageDiff::default();
    }
    if old.len() * new.len() > LCS_CELL_CAP {
        return multiset_diff(&old, &new);
    }
    // LCS lengths, then backtrack to find the matched indices.
    let (n, m) = (old.len(), new.len());
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if old[i] == new[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut matched_old = vec![false; n];
    let mut matched_new = vec![false; m];
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old[i] == new[j] {
            matched_old[i] = true;
            matched_new[j] = true;
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    PassageDiff {
        added: new
            .iter()
            .zip(matched_new.iter())
            .filter(|(_, matched)| !**matched)
            .map(|(p, _)| p.clone())
            .collect(),
        removed: old
            .iter()
            .zip(matched_old.iter())
            .filter(|(_, matched)| !**matched)
            .map(|(p, _)| p.clone())
            .collect(),
    }
}

/// Order-insensitive fallback for very large documents: passages are
/// matched as a multiset; surplus occurrences are the diff.
fn multiset_diff(old: &[String], new: &[String]) -> PassageDiff {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for p in old {
        *counts.entry(p.as_str()).or_default() += 1;
    }
    for p in new {
        *counts.entry(p.as_str()).or_default() -= 1;
    }
    let mut removed = Vec::new();
    let mut seen: HashMap<&str, i64> = HashMap::new();
    for p in old {
        let surplus = counts.get(p.as_str()).copied().unwrap_or(0);
        let used = seen.entry(p.as_str()).or_default();
        if *used < surplus {
            removed.push(p.clone());
            *used += 1;
        }
    }
    let mut added = Vec::new();
    let mut seen: HashMap<&str, i64> = HashMap::new();
    for p in new {
        let deficit = -counts.get(p.as_str()).copied().unwrap_or(0);
        let used = seen.entry(p.as_str()).or_default();
        if *used < deficit {
            added.push(p.clone());
            *used += 1;
        }
    }
    PassageDiff { added, removed }
}

// ---------------------------------------------------------------------------
// Minimal RTF writer
// ---------------------------------------------------------------------------

/// Render plain text as a minimal, valid RTF document: every line a
/// paragraph, RTF metacharacters escaped, non-ASCII as `\uN` escapes.
/// Deliberately plain — it preserves the TEXT faithfully, not the
/// source document's styling (the receipt notes this where relevant).
pub fn text_to_rtf(text: &str) -> String {
    let mut out = String::from(
        "{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0\\fnil Times New Roman;}}\\f0\\fs24\n",
    );
    for line in text.replace("\r\n", "\n").split('\n') {
        for ch in line.chars() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '{' => out.push_str("\\{"),
                '}' => out.push_str("\\}"),
                c if (c as u32) < 128 => out.push(c),
                c => {
                    // RTF \uN takes a signed 16-bit value.
                    let n = (c as u32) as i32;
                    let n = if n > 32767 { n - 65536 } else { n };
                    out.push_str(&format!("\\u{n}?"));
                }
            }
        }
        out.push_str("\\par\n");
    }
    out.push('}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlm;

    const OLD: &str = "HEADING\n\nThe tenant shall pay rent monthly.\n\nThe deposit is one month of rent.\n\nEither party may terminate with notice.";
    const NEW: &str = "HEADING\n\nThe tenant shall pay rent monthly.\n\nThe deposit is two months of rent.\n\nEither party may terminate with notice.\n\nLate payment carries interest at 12% per annum.";

    #[test]
    fn diff_reports_added_and_removed_passages() {
        let d = compute_diff(OLD, NEW);
        assert_eq!(d.removed, vec!["The deposit is one month of rent.".to_string()]);
        assert_eq!(
            d.added,
            vec![
                "The deposit is two months of rent.".to_string(),
                "Late payment carries interest at 12% per annum.".to_string(),
            ]
        );
        assert_eq!(d.summary_line(), "2 passage(s) added, 1 removed");
    }

    #[test]
    fn diff_of_identical_text_is_empty() {
        assert!(compute_diff(OLD, OLD).is_empty());
        // Whitespace-only differences between paragraphs do not count.
        assert!(compute_diff(OLD, &OLD.replace("\n\n", "\n\n\n")).is_empty());
    }

    #[test]
    fn multiset_fallback_agrees_on_small_inputs() {
        let old = paragraphs(OLD);
        let new = paragraphs(NEW);
        let d = multiset_diff(&old, &new);
        assert_eq!(d.added_count(), 2);
        assert_eq!(d.removed_count(), 1);
    }

    #[test]
    fn rtf_escapes_metacharacters_and_unicode() {
        let rtf = text_to_rtf("Costs \\ {test} ₹500\n\nSecond para.");
        assert!(rtf.starts_with("{\\rtf1"));
        assert!(rtf.contains("\\\\"));
        assert!(rtf.contains("\\{test\\}"));
        // ₹ is U+20B9 = 8377 — under 32768, positive escape.
        assert!(rtf.contains("\\u8377?"));
        assert!(rtf.ends_with('}'));
        assert_eq!(rtf.matches("\\par").count(), 3);
    }

    #[tokio::test]
    async fn edit_pass_returns_the_model_text_and_strips_fences() {
        let llm = MockLlm::new("```\nThe deposit is two months of rent.\n```");
        let out = apply_edit(&llm, "Tenancy note", OLD, "double the deposit").await.unwrap();
        assert_eq!(out, "The deposit is two months of rent.");
    }

    #[tokio::test]
    async fn edit_pass_rejects_provider_failure_pages() {
        let llm = MockLlm::new("Sorry, something went wrong. Please try your request again.");
        let err = apply_edit(&llm, "T", OLD, "x").await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("failure"));
    }
}
