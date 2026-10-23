//! Document parsers — mirrors RAG-Anything's parser registry.
//!
//! Each parser turns raw bytes into typed [`ContentBlock`]s
//! (RAG-Anything's `content_list`). `get_parser("auto")` picks by extension,
//! like `raganything.parser.get_parser`.

use std::sync::Arc;

use legal_rag_common::{Error, Result};
use tracing::info;

use crate::config::RagAnythingConfig;
use crate::content::ContentBlock;
use crate::ocr::{ocr_pdf_page, CommandOcr};

/// Parses raw bytes into content blocks.
pub trait DocumentParser: Send + Sync {
    fn name(&self) -> &'static str;
    fn supported_extensions(&self) -> &'static [&'static str];
    fn parse(&self, file_name: &str, bytes: &[u8]) -> Result<Vec<ContentBlock>>;
}

/// Plain text / Markdown parser.
///
/// Headings (`#`, `##`, …) become leveled text blocks, mirroring MinerU's
/// `text_level`; everything else becomes body blocks.
pub struct TextParser;

impl DocumentParser for TextParser {
    fn name(&self) -> &'static str {
        "text"
    }

    fn supported_extensions(&self) -> &'static [&'static str] {
        &[".txt", ".md", ".markdown", ".rst"]
    }

    fn parse(&self, file_name: &str, bytes: &[u8]) -> Result<Vec<ContentBlock>> {
        let text = String::from_utf8_lossy(bytes);
        if text.trim().is_empty() {
            return Err(Error::BadRequest(format!("{file_name} has no extractable text")));
        }
        let mut blocks = Vec::new();
        let mut para: Vec<&str> = Vec::new();
        let flush = |para: &mut Vec<&str>, blocks: &mut Vec<ContentBlock>| {
            if para.is_empty() {
                return;
            }
            blocks.push(ContentBlock::Text {
                text: para.join("\n"),
                level: 0,
                page_idx: None,
            });
            para.clear();
        };
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                flush(&mut para, &mut blocks);
                continue;
            }
            // Markdown heading: 1-6 #'s followed by a space.
            let level = trimmed.chars().take_while(|&c| c == '#').count();
            let rest = trimmed.get(level..).unwrap_or("").trim();
            if (1..=6).contains(&level)
                && !rest.is_empty()
                && trimmed[level..].starts_with([' ', '\t'])
            {
                flush(&mut para, &mut blocks);
                blocks.push(ContentBlock::Text {
                    text: rest.to_string(),
                    level: level as u32,
                    page_idx: None,
                });
                continue;
            }
            para.push(line);
        }
        flush(&mut para, &mut blocks);
        // Detect simple markdown tables and re-tag them as table blocks.
        let blocks = retag_tables(blocks);
        Ok(blocks)
    }
}

/// Re-tags text blocks that look like markdown tables.
fn retag_tables(blocks: Vec<ContentBlock>) -> Vec<ContentBlock> {
    blocks
        .into_iter()
        .map(|b| match b {
            ContentBlock::Text { text, level: 0, page_idx }
                if is_table_text(&text) =>
            {
                ContentBlock::Table { body: text, caption: None, page_idx }
            }
            other => other,
        })
        .collect()
}

fn is_table_text(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 2 {
        return false;
    }
    lines.iter().all(|l| l.contains('|'))
        && lines.iter().filter(|l| l.trim().starts_with('|')).count() >= 2
}

/// PDF parser: text-layer extraction with per-page OCR fallback.
///
/// Pages whose text layer is sparse (below `ocr_min_text_chars`) are rendered
/// with `pdftoppm` and OCR'd with Surya, so scanned/image PDFs index just
/// like born-digital ones.
pub struct PdfParser {
    ocr_enabled: bool,
    ocr_min_text_chars: usize,
    ocr_dpi: u32,
    pdftoppm_cmd: String,
    ocr_primary_cmd: String,
}

impl PdfParser {
    pub fn new(config: &RagAnythingConfig) -> Self {
        Self {
            ocr_enabled: config.ocr_enabled,
            ocr_min_text_chars: config.ocr_min_text_chars,
            ocr_dpi: config.ocr_dpi,
            pdftoppm_cmd: config.pdftoppm_cmd.clone(),
            ocr_primary_cmd: config.ocr_primary_cmd.clone(),
        }
    }

    /// Text-layer only (no OCR) — for tests and constrained environments.
    pub fn without_ocr() -> Self {
        Self {
            ocr_enabled: false,
            ocr_min_text_chars: 50,
            ocr_dpi: 300,
            pdftoppm_cmd: "pdftoppm".into(),
            ocr_primary_cmd: String::new(),
        }
    }
}

impl DocumentParser for PdfParser {
    fn name(&self) -> &'static str {
        "pdf"
    }

    fn supported_extensions(&self) -> &'static [&'static str] {
        &[".pdf"]
    }

    fn parse(&self, file_name: &str, bytes: &[u8]) -> Result<Vec<ContentBlock>> {
        let text = pdf_extract::extract_text_from_mem(bytes)
            .map_err(|e| Error::BadRequest(format!("pdf extraction failed for {file_name}: {e}")))?;
        let raw_pages: Vec<&str> = text.split('\u{c}').collect();
        let ocr = CommandOcr::from_command("surya", &self.ocr_primary_cmd);

        let mut blocks = Vec::new();
        let mut ocr_pages = 0usize;
        for (idx, page) in raw_pages.iter().enumerate() {
            let layer_text = page.trim();
            let mut page_text = layer_text.to_string();
            // Sparse text layer → OCR fallback (scanned/image page).
            if self.ocr_enabled && layer_text.chars().count() < self.ocr_min_text_chars {
                if let Some(engine) = &ocr {
                    if let Some(ocr_text) = ocr_pdf_page(
                        engine,
                        &self.pdftoppm_cmd,
                        bytes,
                        idx as u32 + 1,
                        self.ocr_dpi,
                    ) {
                        // Prefer whichever yielded more text.
                        if ocr_text.chars().count() > layer_text.chars().count() {
                            page_text = ocr_text;
                            ocr_pages += 1;
                        }
                    }
                }
            }
            if page_text.trim().is_empty() {
                continue;
            }
            blocks.push(ContentBlock::Text {
                text: page_text,
                level: 0,
                page_idx: Some(idx as u32),
            });
        }
        if blocks.is_empty() {
            return Err(Error::BadRequest(format!("{file_name} has no extractable text")));
        }
        if ocr_pages > 0 {
            info!(file = %file_name, ocr_pages, "pdf parser used OCR fallback");
        }
        Ok(blocks)
    }
}

/// Pick a parser by name (`"auto"` = by file extension).
pub fn get_parser(
    name: &str,
    file_name: &str,
    config: &RagAnythingConfig,
) -> Result<Arc<dyn DocumentParser>> {
    let parsers: Vec<Arc<dyn DocumentParser>> =
        vec![Arc::new(PdfParser::new(config)), Arc::new(TextParser)];
    if name != "auto" {
        return parsers
            .into_iter()
            .find(|p| p.name() == name)
            .ok_or_else(|| Error::Config(format!("unknown parser: {name}")));
    }
    let lower = file_name.to_lowercase();
    parsers
        .into_iter()
        .find(|p| p.supported_extensions().iter().any(|e| lower.ends_with(e)))
        .ok_or_else(|| Error::BadRequest(format!("unsupported file type: {file_name}")))
}

/// File extensions accepted for batch ingestion.
pub fn supported_extensions() -> Vec<String> {
    [".pdf", ".txt", ".md", ".markdown", ".rst"]
        .into_iter()
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_parser_extracts_headings() {
        let blocks = TextParser
            .parse("doc.md", b"# Agreement\n\nBody text here.\n")
            .unwrap();
        assert!(matches!(&blocks[0], ContentBlock::Text { level: 1, .. }));
        assert!(matches!(&blocks[1], ContentBlock::Text { level: 0, .. }));
    }

    #[test]
    fn text_parser_retags_tables() {
        let blocks = TextParser
            .parse("t.md", b"| a | b |\n|---|---|\n| 1 | 2 |\n")
            .unwrap();
        assert!(matches!(&blocks[0], ContentBlock::Table { .. }));
    }

    #[test]
    fn auto_parser_picks_by_extension() {
        let config = RagAnythingConfig::default();
        assert_eq!(get_parser("auto", "x.pdf", &config).unwrap().name(), "pdf");
        assert_eq!(get_parser("auto", "x.txt", &config).unwrap().name(), "text");
        assert!(get_parser("auto", "x.xyz", &config).is_err());
    }
}
