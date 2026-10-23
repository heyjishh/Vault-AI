//! Typed content blocks — mirrors RAG-Anything's `content_list`.
//!
//! The parser produces a list of blocks (`type` = text/image/table/equation).
//! [`separate_content`] splits them exactly like
//! `raganything.utils.separate_content_with_page_map`: plain text is merged
//! (with per-block page provenance), everything else becomes multimodal items.

use serde::{Deserialize, Serialize};

/// One parsed content block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
        /// Heading level (0 = body text), like MinerU's `text_level`.
        #[serde(default)]
        level: u32,
        #[serde(default)]
        page_idx: Option<u32>,
    },
    Image {
        /// Caption or alt text when the parser provides one.
        #[serde(default)]
        caption: Option<String>,
        #[serde(default)]
        page_idx: Option<u32>,
        /// Where the image bytes/reference came from.
        #[serde(default)]
        source: String,
    },
    Table {
        /// Table body as text (markdown/CSV-ish).
        body: String,
        #[serde(default)]
        caption: Option<String>,
        #[serde(default)]
        page_idx: Option<u32>,
    },
    Equation {
        /// LaTeX source.
        latex: String,
        #[serde(default)]
        page_idx: Option<u32>,
    },
}

impl ContentBlock {
    pub fn kind(&self) -> &'static str {
        match self {
            ContentBlock::Text { .. } => "text",
            ContentBlock::Image { .. } => "image",
            ContentBlock::Table { .. } => "table",
            ContentBlock::Equation { .. } => "equation",
        }
    }

    pub fn page_idx(&self) -> Option<u32> {
        match self {
            ContentBlock::Text { page_idx, .. }
            | ContentBlock::Image { page_idx, .. }
            | ContentBlock::Table { page_idx, .. }
            | ContentBlock::Equation { page_idx, .. } => *page_idx,
        }
    }
}

/// A non-text block routed to a modal processor, with its position.
#[derive(Debug, Clone)]
pub struct ModalItem {
    pub index: usize,
    pub block: ContentBlock,
}

/// Character interval of one text block inside the merged text, with page info.
#[derive(Debug, Clone)]
pub struct PageInterval {
    pub start: usize,
    pub end: usize,
    pub page_idx: Option<u32>,
}

/// Split blocks into merged text + multimodal items + page provenance.
///
/// Mirrors `separate_content_with_page_map`.
pub fn separate_content(blocks: &[ContentBlock]) -> (String, Vec<ModalItem>, Vec<PageInterval>) {
    let mut text_parts: Vec<&str> = Vec::new();
    let mut modal_items = Vec::new();
    let mut intervals = Vec::new();
    let mut text_len = 0usize;

    for (index, block) in blocks.iter().enumerate() {
        match block {
            ContentBlock::Text { text, page_idx, .. } => {
                if text.trim().is_empty() {
                    continue;
                }
                let start = text_len + if text_parts.is_empty() { 0 } else { 2 };
                text_parts.push(text.as_str());
                let end = start + text.len();
                text_len = end;
                intervals.push(PageInterval { start, end, page_idx: *page_idx });
            }
            other => {
                let _ = other;
                modal_items.push(ModalItem { index, block: block.clone() });
            }
        }
    }

    (text_parts.join("\n\n"), modal_items, intervals)
}

/// Content statistics, like RAG-Anything's `display_content_stats`.
pub fn content_stats(blocks: &[ContentBlock]) -> String {
    let mut counts = std::collections::HashMap::new();
    for b in blocks {
        *counts.entry(b.kind()).or_insert(0usize) += 1;
    }
    let mut parts: Vec<String> =
        counts.into_iter().map(|(k, v)| format!("{k}: {v}")).collect();
    parts.sort();
    format!("blocks={} [{}]", blocks.len(), parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separates_text_and_modal_with_pages() {
        let blocks = vec![
            ContentBlock::Text { text: "Intro".into(), level: 1, page_idx: Some(0) },
            ContentBlock::Image { caption: None, page_idx: Some(0), source: "fig1.png".into() },
            ContentBlock::Text { text: "Body".into(), level: 0, page_idx: Some(1) },
            ContentBlock::Table { body: "a|b".into(), caption: None, page_idx: Some(1) },
        ];
        let (text, modal, intervals) = separate_content(&blocks);
        assert_eq!(text, "Intro\n\nBody");
        assert_eq!(modal.len(), 2);
        assert_eq!(modal[0].block.kind(), "image");
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[1].page_idx, Some(1));
    }
}
