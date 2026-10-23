//! Context extraction for modal processors.
//!
//! Mirrors `raganything.modalprocessors.ContextExtractor`: given the full
//! content list and a multimodal item's position, pull surrounding text,
//! section headers, and captions so the captioning model sees the item
//! in context.

use crate::config::RagAnythingConfig;
use crate::content::ContentBlock;

/// Extracts surrounding context for a multimodal content block.
pub struct ContextExtractor {
    window: usize,
    max_chars: usize,
    include_headers: bool,
    include_captions: bool,
}

impl ContextExtractor {
    pub fn new(config: &RagAnythingConfig) -> Self {
        Self {
            window: config.context_window.max(1),
            // Rough chars-per-token estimate for the token budget.
            max_chars: config.max_context_tokens * 4,
            include_headers: config.include_headers,
            include_captions: config.include_captions,
        }
    }

    /// Build context for the block at `index`:
    /// section path (headers) + neighboring text + captions.
    pub fn extract_context(&self, blocks: &[ContentBlock], index: usize) -> String {
        let mut parts = Vec::new();

        if self.include_headers {
            let path = section_path(blocks, index);
            if !path.is_empty() {
                parts.push(format!("Section: {path}"));
            }
        }

        let start = index.saturating_sub(self.window);
        let end = (index + self.window + 1).min(blocks.len());
        let mut neighbor = Vec::new();
        for (i, block) in blocks.iter().enumerate().take(end).skip(start) {
            if i == index {
                continue;
            }
            match block {
                ContentBlock::Text { text, .. } => neighbor.push(text.clone()),
                ContentBlock::Image { caption: Some(c), .. }
                | ContentBlock::Table { caption: Some(c), .. }
                    if self.include_captions =>
                {
                    neighbor.push(format!("(caption: {c})"))
                }
                _ => {}
            }
        }
        if !neighbor.is_empty() {
            parts.push(format!("Surrounding text: {}", neighbor.join(" ")));
        }

        let mut context = parts.join("\n");
        if context.len() > self.max_chars {
            context.truncate(self.max_chars);
        }
        context
    }
}

/// Reconstruct the heading chain above `index`, like
/// `extract_section_path_from_content_list` ("A > B > C").
fn section_path(blocks: &[ContentBlock], index: usize) -> String {
    let mut chain: Vec<(u32, String)> = Vec::new();
    for block in blocks.iter().take(index) {
        if let ContentBlock::Text { text, level, .. } = block {
            if *level == 0 {
                continue;
            }
            while chain.last().is_some_and(|(l, _)| *l >= *level) {
                chain.pop();
            }
            chain.push((*level, text.trim().to_string()));
        }
    }
    chain.into_iter().map(|(_, t)| t).collect::<Vec<_>>().join(" > ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RagAnythingConfig {
        RagAnythingConfig::default()
    }

    #[test]
    fn extracts_section_path_and_neighbors() {
        let blocks = vec![
            ContentBlock::Text { text: "Agreement".into(), level: 1, page_idx: None },
            ContentBlock::Text { text: "Preamble words".into(), level: 0, page_idx: None },
            ContentBlock::Table { body: "a|b".into(), caption: None, page_idx: None },
            ContentBlock::Text { text: "After words".into(), level: 0, page_idx: None },
        ];
        let ctx = ContextExtractor::new(&cfg()).extract_context(&blocks, 2);
        assert!(ctx.contains("Section: Agreement"));
        assert!(ctx.contains("Preamble words"));
        assert!(ctx.contains("After words"));
    }
}
