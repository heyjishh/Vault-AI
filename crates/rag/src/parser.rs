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
use crate::ocr::{ocr_pdf_page, CommandOcr, OcrEngine};

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
        &[".txt", ".md", ".markdown", ".rst", ".csv", ".tsv"]
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

/// Image parser: photos and scans go through the same Surya OCR stage
/// the PDF parser uses for rendered pages. There is no text layer to
/// fall back on, so an image the OCR stage cannot read (or that yields
/// no text) surfaces the standard "has no extractable text" error.
///
/// Multi-frame formats (GIF, TIFF) are read as their first frame/page —
/// that is what the OCR sidecar's image loader hands to Surya.
pub struct ImageParser {
    ocr_enabled: bool,
    ocr_primary_cmd: String,
}

impl ImageParser {
    pub fn new(config: &RagAnythingConfig) -> Self {
        Self {
            ocr_enabled: config.ocr_enabled,
            ocr_primary_cmd: config.ocr_primary_cmd.clone(),
        }
    }
}

impl DocumentParser for ImageParser {
    fn name(&self) -> &'static str {
        "image"
    }

    fn supported_extensions(&self) -> &'static [&'static str] {
        &[".png", ".jpg", ".jpeg", ".webp", ".bmp", ".tif", ".tiff", ".gif"]
    }

    fn parse(&self, file_name: &str, bytes: &[u8]) -> Result<Vec<ContentBlock>> {
        let no_text = || Error::BadRequest(format!("{file_name} has no extractable text"));
        if !self.ocr_enabled {
            return Err(no_text());
        }
        let engine = CommandOcr::from_command("surya", &self.ocr_primary_cmd).ok_or_else(no_text)?;
        // The sidecar feeds the bytes to Surya's image loader (PIL),
        // which sniffs the real format from content, so JPEG/WebP/BMP/
        // TIFF/GIF bytes are accepted exactly like PNG.
        match engine.ocr_png(bytes) {
            Ok(text) if !text.trim().is_empty() => Ok(vec![ContentBlock::Text {
                text,
                level: 0,
                page_idx: Some(0),
            }]),
            Ok(_) => Err(no_text()),
            Err(e) => {
                tracing::warn!(file = %file_name, error = %e, "image OCR failed");
                Err(no_text())
            }
        }
    }
}

/// Word/OpenDocument parser for the zip+XML formats: .docx/.docm read
/// `word/document.xml` (plus footnotes/endnotes when present), .odt
/// reads `content.xml`. Tags collapse to plain text — paragraph and
/// row ends become newlines, cell ends become " | " separators — and
/// the result flows through the text parser's block splitting.
/// Embedded images inside Office files are NOT OCR'd in this pass;
/// only the document's own text layer is extracted.
pub struct OfficeDocParser;

impl DocumentParser for OfficeDocParser {
    fn name(&self) -> &'static str {
        "office"
    }

    fn supported_extensions(&self) -> &'static [&'static str] {
        &[".docx", ".docm", ".odt"]
    }

    fn parse(&self, file_name: &str, bytes: &[u8]) -> Result<Vec<ContentBlock>> {
        use std::io::Read as _;
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).map_err(|e| {
            Error::BadRequest(format!("office extraction failed for {file_name}: {e}"))
        })?;
        let lower = file_name.to_lowercase();
        let mut xml_parts: Vec<String> = Vec::new();
        if lower.ends_with(".odt") {
            let mut f = zip.by_name("content.xml").map_err(|e| {
                Error::BadRequest(format!("office extraction failed for {file_name}: {e}"))
            })?;
            let mut s = String::new();
            f.read_to_string(&mut s).map_err(|e| {
                Error::BadRequest(format!("office extraction failed for {file_name}: {e}"))
            })?;
            xml_parts.push(s);
        } else {
            for part in ["word/document.xml", "word/footnotes.xml", "word/endnotes.xml"] {
                if let Ok(mut f) = zip.by_name(part) {
                    let mut s = String::new();
                    if f.read_to_string(&mut s).is_ok() {
                        xml_parts.push(s);
                    }
                }
            }
            if xml_parts.is_empty() {
                return Err(Error::BadRequest(format!(
                    "office extraction failed for {file_name}: word/document.xml missing"
                )));
            }
        }
        let mut text = String::new();
        for part in &xml_parts {
            text.push_str(&xml_to_text(part));
            text.push('\n');
        }
        TextParser.parse(file_name, text.as_bytes())
    }
}

/// Strip an OOXML/ODF XML document down to its text: character data is
/// kept (entities unescaped), block-level closes become newlines and
/// cell closes become " | ". Field-instruction text (`w:instrText`,
/// TOC/hyperlink field codes) is skipped — it is markup, not content.
fn xml_to_text(xml: &str) -> String {
    let bytes = xml.as_bytes();
    let mut out = String::with_capacity(xml.len() / 2);
    let mut i = 0usize;
    let mut skip_depth = 0usize; // inside w:instrText
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let Some(rel_end) = xml[i..].find('>') else { break };
            let tag = &xml[i + 1..i + rel_end];
            i += rel_end + 1;
            let t = tag.trim();
            if t.starts_with('?') || t.starts_with('!') {
                continue;
            }
            let closing = t.starts_with('/');
            let self_closing = t.ends_with('/');
            let name = t
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("");
            let local = name.rsplit(':').next().unwrap_or(name);
            if local == "instrText" {
                if closing {
                    skip_depth = skip_depth.saturating_sub(1);
                } else if !self_closing {
                    skip_depth += 1;
                }
                continue;
            }
            if skip_depth > 0 {
                continue;
            }
            if closing {
                match local {
                    "p" | "h" | "tr" | "table-row" => out.push('\n'),
                    "tc" | "table-cell" => out.push_str(" | "),
                    _ => {}
                }
            } else {
                match local {
                    "tab" => out.push('\t'),
                    "br" | "cr" => out.push('\n'),
                    _ => {}
                }
            }
            continue;
        }
        // Character data up to the next tag.
        let next = xml[i..].find('<').map(|j| i + j).unwrap_or(xml.len());
        if skip_depth == 0 {
            out.push_str(&xml_unescape(&xml[i..next]));
        }
        i = next;
    }
    out
}

/// Unescape XML entities in one pass (`&amp;`, `&lt;`, numeric refs…).
fn xml_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let Some(semi) = rest[pos..].find(';') else {
            out.push_str(&rest[pos..]);
            return out;
        };
        let ent = &rest[pos + 1..pos + semi];
        let decoded: Option<String> = match ent {
            "amp" => Some("&".into()),
            "lt" => Some("<".into()),
            "gt" => Some(">".into()),
            "quot" => Some("\"".into()),
            "apos" => Some("'".into()),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32).map(|c| c.to_string())
            }
            _ if ent.starts_with('#') => {
                ent[1..].parse::<u32>().ok().and_then(char::from_u32).map(|c| c.to_string())
            }
            _ => None,
        };
        match decoded {
            Some(d) => out.push_str(&d),
            None => {
                out.push('&');
                out.push_str(ent);
                out.push(';');
            }
        }
        rest = &rest[pos + semi + 1..];
    }
    out.push_str(rest);
    out
}

/// RTF parser: a best-effort control-word stripper. Paragraph marks
/// become newlines, `\'xx` hex escapes and `\uN` unicode escapes are
/// decoded (CP1252 for hex bytes), and destination groups that hold no
/// body text (font/color/style tables, info, pictures, objects) are
/// skipped wholesale.
pub struct RtfParser;

impl DocumentParser for RtfParser {
    fn name(&self) -> &'static str {
        "rtf"
    }

    fn supported_extensions(&self) -> &'static [&'static str] {
        &[".rtf"]
    }

    fn parse(&self, file_name: &str, bytes: &[u8]) -> Result<Vec<ContentBlock>> {
        let text = rtf_to_text(bytes);
        TextParser.parse(file_name, text.as_bytes())
    }
}

/// RTF destination groups whose contents are not document body text.
const RTF_SKIP_DESTS: &[&str] = &[
    "fonttbl", "colortbl", "stylesheet", "info", "pict", "object", "header", "footer",
    "listtable", "listoverridetable", "rsidtbl", "generator", "themedata", "datastore",
    "colorschememapping", "latentstyles", "datafield", "objclass", "shppict", "nonshppict",
    "filetbl", "revtbl", "operator", "author", "manager", "company", "title", "subject",
    "keywords", "comment", "doccomm", "creatim", "revtim", "printim", "buptim", "version",
    "vern", "edmins", "nofpages", "nofwords", "nofchars", "id", "wgrffmtfilter",
];

fn rtf_to_text(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() / 2);
    let mut pending: Vec<u8> = Vec::new(); // CP1252 bytes from \'xx escapes
    let flush = |pending: &mut Vec<u8>, out: &mut String| {
        if !pending.is_empty() {
            out.push_str(&cp1252_decode(pending));
            pending.clear();
        }
    };
    // Group stack: (skip, decided) — `decided` flips once the group's
    // first token has been seen (the destination word decides skipping).
    let mut stack: Vec<(bool, bool)> = Vec::new();
    let skipped = |stack: &Vec<(bool, bool)>| stack.iter().any(|(s, _)| *s);
    let mut i = 0usize;
    let b = bytes;
    while i < b.len() {
        match b[i] {
            b'{' => {
                stack.push((false, false));
                i += 1;
            }
            b'}' => {
                stack.pop();
                i += 1;
            }
            b'\\' => {
                i += 1;
                if i >= b.len() {
                    break;
                }
                // A group's first control token decides whether the
                // whole group is a skipped destination.
                let first_in_group = matches!(stack.last(), Some((_, false)));
                if first_in_group {
                    if let Some(top) = stack.last_mut() {
                        top.1 = true;
                    }
                }
                match b[i] {
                    b'\'' => {
                        // \'xx hex byte (CP1252).
                        if i + 2 < b.len() {
                            let hex = &b[i + 1..i + 3];
                            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(hex).unwrap_or(""), 16) {
                                if !skipped(&stack) {
                                    pending.push(v);
                                }
                            }
                            i += 3;
                        } else {
                            i = b.len();
                        }
                    }
                    b'*' => {
                        if first_in_group {
                            if let Some(top) = stack.last_mut() {
                                top.0 = true;
                            }
                        }
                        i += 1;
                    }
                    b'{' | b'}' | b'\\' => {
                        if !skipped(&stack) {
                            flush(&mut pending, &mut out);
                            out.push(b[i] as char);
                        }
                        i += 1;
                    }
                    b'~' => {
                        if !skipped(&stack) {
                            flush(&mut pending, &mut out);
                            out.push(' ');
                        }
                        i += 1;
                    }
                    b'-' | b'_' => {
                        i += 1; // soft hyphen / non-breaking hyphen approximations dropped
                    }
                    c if c.is_ascii_alphabetic() => {
                        // Control word: letters, optional signed number,
                        // optional single-space delimiter.
                        let start = i;
                        while i < b.len() && b[i].is_ascii_alphabetic() {
                            i += 1;
                        }
                        let word = std::str::from_utf8(&b[start..i]).unwrap_or("");
                        let mut num: Option<i64> = None;
                        if i < b.len() && (b[i] == b'-' || b[i].is_ascii_digit()) {
                            let nstart = i;
                            if b[i] == b'-' {
                                i += 1;
                            }
                            while i < b.len() && b[i].is_ascii_digit() {
                                i += 1;
                            }
                            num = std::str::from_utf8(&b[nstart..i])
                                .ok()
                                .and_then(|s| s.parse::<i64>().ok());
                        }
                        if i < b.len() && b[i] == b' ' {
                            i += 1;
                        }
                        if first_in_group && RTF_SKIP_DESTS.contains(&word) {
                            if let Some(top) = stack.last_mut() {
                                top.0 = true;
                            }
                        }
                        if !skipped(&stack) {
                            match word {
                                "par" | "line" => {
                                    flush(&mut pending, &mut out);
                                    out.push('\n');
                                }
                                "tab" | "cell" => {
                                    flush(&mut pending, &mut out);
                                    out.push(if word == "tab" { '\t' } else { '|' });
                                }
                                "row" => {
                                    flush(&mut pending, &mut out);
                                    out.push('\n');
                                }
                                "u" => {
                                    if let Some(n) = num {
                                        flush(&mut pending, &mut out);
                                        let cp = if n < 0 { n + 65536 } else { n };
                                        if let Some(ch) = u32::try_from(cp).ok().and_then(char::from_u32) {
                                            out.push(ch);
                                        }
                                        // Skip the one-char substitution
                                        // that follows a \uN escape (a
                                        // literal byte or a \'xx escape).
                                        if i + 3 < b.len() && b[i] == b'\\' && b[i + 1] == b'\'' {
                                            i += 4;
                                        } else if i < b.len() && b[i] != b'\\' && b[i] != b'{' && b[i] != b'}' {
                                            i += 1;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {
                        i += 1; // other control symbols (\:, \/, …) carry no text
                    }
                }
            }
            b'\r' | b'\n' => {
                i += 1; // raw line breaks are insignificant in RTF
            }
            c => {
                if let Some(top) = stack.last_mut() {
                    top.1 = true;
                }
                if !skipped(&stack) {
                    pending.push(c);
                }
                i += 1;
            }
        }
    }
    flush(&mut pending, &mut out);
    out
}

/// Decode CP1252 bytes (RTF `\'xx` escapes, legacy .doc text).
fn cp1252_decode(bytes: &[u8]) -> String {
    const HIGH: &[char] = &[
        '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}',
        '\u{017D}', '\u{008F}', '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
    ];
    bytes
        .iter()
        .map(|&c| match c {
            0x80..=0x9F => HIGH[(c - 0x80) as usize],
            _ => c as char,
        })
        .collect()
}

/// Legacy Word (.doc, OLE/CFB) parser — best effort. The compound file
/// is opened with the `cfb` crate and the WordDocument stream is
/// decoded: the FIB header points at the main text range; when that
/// range decodes cleanly it wins, otherwise the whole stream is
/// scanned for printable text runs (UTF-16LE and CP1252 passes, the
/// wordier result is kept).
pub struct LegacyDocParser;

impl DocumentParser for LegacyDocParser {
    fn name(&self) -> &'static str {
        "doc"
    }

    fn supported_extensions(&self) -> &'static [&'static str] {
        &[".doc"]
    }

    fn parse(&self, file_name: &str, bytes: &[u8]) -> Result<Vec<ContentBlock>> {
        use std::io::Read as _;
        let mut cf = cfb::CompoundFile::open(std::io::Cursor::new(bytes.to_vec())).map_err(|e| {
            Error::BadRequest(format!("legacy .doc extraction failed for {file_name}: {e}"))
        })?;
        let mut stream = Vec::new();
        {
            let mut s = cf.open_stream("/WordDocument").map_err(|e| {
                Error::BadRequest(format!("legacy .doc extraction failed for {file_name}: {e}"))
            })?;
            s.read_to_end(&mut stream).map_err(|e| {
                Error::BadRequest(format!("legacy .doc extraction failed for {file_name}: {e}"))
            })?;
        }
        let text = doc_stream_text(&stream);
        TextParser.parse(file_name, text.as_bytes())
    }
}

/// Extract the document text from a WordDocument stream. The FIB
/// (file information block) carries fcMin/fcMax at 0x18/0x1C bounding
/// the main text; a sane range whose decode looks like prose is used
/// directly. Anything else falls back to a full-stream run scan.
fn doc_stream_text(stream: &[u8]) -> String {
    if stream.len() >= 0x20 {
        let fc_min = u32::from_le_bytes(stream[0x18..0x1C].try_into().unwrap()) as usize;
        let fc_max = u32::from_le_bytes(stream[0x1C..0x20].try_into().unwrap()) as usize;
        if fc_min < fc_max && fc_max <= stream.len() && fc_max - fc_min >= 8 {
            let slice = &stream[fc_min..fc_max];
            let utf16 = utf16le_runs(slice);
            let single = cp1252_runs(slice);
            let best = if wordiness(&utf16) >= wordiness(&single) { utf16 } else { single };
            if best.chars().count() >= 20 {
                return best;
            }
        }
    }
    let utf16 = utf16le_runs(stream);
    let single = cp1252_runs(stream);
    if wordiness(&utf16) >= wordiness(&single) {
        utf16
    } else {
        single
    }
}

/// Score how prose-like a candidate extraction is (letters + spaces
/// vs. everything else), used to pick between decodings.
fn wordiness(s: &str) -> usize {
    s.chars().filter(|c| c.is_alphabetic() || *c == ' ').count()
}

/// Longest printable runs in a UTF-16LE decoding of `bytes`, joined by
/// newlines. Runs shorter than 4 chars are treated as binary noise.
fn utf16le_runs(bytes: &[u8]) -> String {
    let mut runs: Vec<String> = Vec::new();
    let mut cur = String::new();
    for pair in bytes.chunks_exact(2) {
        let v = u16::from_le_bytes([pair[0], pair[1]]);
        let ch = char::from_u32(v as u32).unwrap_or('\u{fffd}');
        if is_text_char(ch) {
            cur.push(ch);
        } else {
            if cur.chars().count() >= 4 {
                runs.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if cur.chars().count() >= 4 {
        runs.push(cur);
    }
    runs.join("\n")
}

/// Longest printable runs in a CP1252 decoding of `bytes`.
fn cp1252_runs(bytes: &[u8]) -> String {
    let mut runs: Vec<String> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    for &b in bytes {
        let ch = cp1252_decode(&[b]).chars().next().unwrap_or('\u{fffd}');
        if is_text_char(ch) {
            cur.push(b);
        } else {
            if cur.len() >= 4 {
                runs.push(cp1252_decode(&cur));
            }
            cur.clear();
        }
    }
    if cur.len() >= 4 {
        runs.push(cp1252_decode(&cur));
    }
    runs.join("\n")
}

fn is_text_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(c, ' ' | '.' | ',' | ';' | ':' | '!' | '?' | '-' | '\'' | '"' | '(' | ')' | '/' | '&' | '%' | '—' | '–' | '।')
}

/// Spreadsheet parser (calamine): .xls, .xlsx, .xlsm, .xlsb and .ods.
/// Each sheet becomes a level-1 heading block plus one text block of
/// its rows ("cell | cell | …"); empty rows are skipped.
pub struct SpreadsheetParser;

impl DocumentParser for SpreadsheetParser {
    fn name(&self) -> &'static str {
        "spreadsheet"
    }

    fn supported_extensions(&self) -> &'static [&'static str] {
        &[".xls", ".xlsx", ".xlsm", ".xlsb", ".ods"]
    }

    fn parse(&self, file_name: &str, bytes: &[u8]) -> Result<Vec<ContentBlock>> {
        use calamine::Reader as _;
        // calamine 0.37 has no format-sniffing reader over a generic
        // stream, so dispatch on the extension to its concrete readers.
        let lower = file_name.to_lowercase();
        let cursor = || std::io::Cursor::new(bytes.to_vec());
        let fail = |e: &dyn std::fmt::Debug| {
            Error::BadRequest(format!("spreadsheet extraction failed for {file_name}: {e:?}"))
        };
        let blocks = if lower.ends_with(".xls") {
            workbook_blocks(calamine::Xls::new(cursor()).map_err(|e| fail(&e))?)
        } else if lower.ends_with(".xlsb") {
            workbook_blocks(calamine::Xlsb::new(cursor()).map_err(|e| fail(&e))?)
        } else if lower.ends_with(".ods") {
            workbook_blocks(calamine::Ods::new(cursor()).map_err(|e| fail(&e))?)
        } else {
            workbook_blocks(calamine::Xlsx::new(cursor()).map_err(|e| fail(&e))?)
        };
        if blocks.is_empty() {
            return Err(Error::BadRequest(format!("{file_name} has no extractable text")));
        }
        Ok(blocks)
    }
}

/// Shared sheet walker for every calamine reader: one heading block
/// per sheet plus one block of "cell | cell" rows (empty rows skipped).
fn workbook_blocks<RS, R>(mut wb: R) -> Vec<ContentBlock>
where
    RS: std::io::Read + std::io::Seek,
    R: calamine::Reader<RS>,
{
    let mut blocks = Vec::new();
    for (sheet, range) in wb.worksheets() {
        let mut lines: Vec<String> = Vec::new();
        for row in range.rows() {
            let cells: Vec<String> = row.iter().map(cell_text).collect();
            if cells.iter().all(|c| c.is_empty()) {
                continue;
            }
            lines.push(cells.join(" | "));
        }
        if lines.is_empty() {
            continue;
        }
        blocks.push(ContentBlock::Text {
            text: sheet,
            level: 1,
            page_idx: None,
        });
        blocks.push(ContentBlock::Text {
            text: lines.join("\n"),
            level: 0,
            page_idx: None,
        });
    }
    blocks
}

fn cell_text(c: &calamine::Data) -> String {
    match c {
        calamine::Data::Empty => String::new(),
        calamine::Data::String(s) => s.clone(),
        calamine::Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        calamine::Data::Int(i) => format!("{i}"),
        calamine::Data::Bool(b) => format!("{b}"),
        calamine::Data::DateTime(dt) => format!("{dt}"),
        calamine::Data::DateTimeIso(s) | calamine::Data::DurationIso(s) => s.clone(),
        calamine::Data::Error(e) => format!("{e:?}"),
    }
}

/// Pick a parser by name (`"auto"` = by file extension).
pub fn get_parser(
    name: &str,
    file_name: &str,
    config: &RagAnythingConfig,
) -> Result<Arc<dyn DocumentParser>> {
    let parsers: Vec<Arc<dyn DocumentParser>> = vec![
        Arc::new(PdfParser::new(config)),
        Arc::new(TextParser),
        Arc::new(ImageParser::new(config)),
        Arc::new(OfficeDocParser),
        Arc::new(RtfParser),
        Arc::new(LegacyDocParser),
        Arc::new(SpreadsheetParser),
    ];
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
    [
        ".pdf", ".txt", ".md", ".markdown", ".rst", ".csv", ".tsv", ".doc", ".docx", ".docm",
        ".odt", ".rtf", ".xls", ".xlsx", ".xlsm", ".xlsb", ".ods", ".png", ".jpg", ".jpeg",
        ".webp", ".bmp", ".tif", ".tiff", ".gif",
    ]
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

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"))
    }

    fn blocks_text(blocks: &[ContentBlock]) -> String {
        blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.clone()),
                ContentBlock::Table { body, .. } => Some(body.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn auto_parser_covers_the_full_format_matrix() {
        let config = RagAnythingConfig::default();
        for (file, want) in [
            ("a.png", "image"), ("a.jpg", "image"), ("a.jpeg", "image"),
            ("a.webp", "image"), ("a.bmp", "image"), ("a.tif", "image"),
            ("a.tiff", "image"), ("a.gif", "image"),
            ("a.docx", "office"), ("a.docm", "office"), ("a.odt", "office"),
            ("a.rtf", "rtf"),
            ("a.doc", "doc"),
            ("a.xls", "spreadsheet"), ("a.xlsx", "spreadsheet"),
            ("a.xlsm", "spreadsheet"), ("a.xlsb", "spreadsheet"),
            ("a.ods", "spreadsheet"),
            ("a.csv", "text"), ("a.tsv", "text"),
        ] {
            assert_eq!(
                get_parser("auto", file, &config).unwrap().name(),
                want,
                "wrong parser for {file}"
            );
        }
        assert!(get_parser("auto", "a.heic", &config).is_err());
        let exts = supported_extensions();
        for e in [".docx", ".doc", ".odt", ".rtf", ".xlsx", ".xlsb", ".ods", ".csv", ".png", ".jpeg", ".webp", ".tiff", ".gif"] {
            assert!(exts.iter().any(|x| x == e), "supported_extensions missing {e}");
        }
    }

    #[test]
    fn office_formats_extract_their_text() {
        let config = RagAnythingConfig::default();
        for name in ["fixture.docx", "fixture.docm", "fixture.odt", "fixture.rtf", "fixture.doc"] {
            let parser = get_parser("auto", name, &config).unwrap();
            let text = blocks_text(&parser.parse(name, &fixture(name)).unwrap());
            assert!(text.contains("ZEBRAFIXTUREDOC"), "{name} lost its text: {text:.120}");
            assert!(text.contains("310"), "{name} lost the royalty figure");
        }
        // Table cells survive as separated text in the zip+XML formats.
        let parser = get_parser("auto", "fixture.docx", &config).unwrap();
        let text = blocks_text(&parser.parse("fixture.docx", &fixture("fixture.docx")).unwrap());
        assert!(text.contains("Kankar Mines"), "docx table text missing: {text:.200}");
    }

    #[test]
    fn spreadsheet_formats_extract_cells() {
        let config = RagAnythingConfig::default();
        for name in ["fixture.xlsx", "fixture.xls", "fixture.ods"] {
            let parser = get_parser("auto", name, &config).unwrap();
            let text = blocks_text(&parser.parse(name, &fixture(name)).unwrap());
            assert!(text.contains("ZEBRAFIXTURESHEET"), "{name} lost its cells: {text:.120}");
            assert!(text.contains("310"), "{name} lost the amount cell");
            assert!(text.contains("Ledger"), "{name} lost the sheet name");
        }
    }

    #[test]
    fn xml_and_rtf_strippers_handle_their_markup() {
        let xml = "<w:p><w:r><w:t>Fish &amp; Chips</w:t></w:r></w:p><w:p><w:r><w:t>Second</w:t></w:r></w:p>";
        let text = xml_to_text(xml);
        assert!(text.contains("Fish & Chips"), "entity/para handling: {text}");
        assert!(text.contains("Second"));
        let rtf = r"{\rtf1\ansi{\fonttbl{\f0 Arial;}}\f0\fs24 Hello \b World\b0\par Second line \'93quoted\'94}";
        let text = rtf_to_text(rtf.as_bytes());
        assert!(text.contains("Hello"), "rtf body: {text}");
        assert!(text.contains("World"), "rtf body: {text}");
        assert!(text.contains("Second line"), "rtf \\par: {text}");
        assert!(text.contains('\u{201c}'), "rtf hex escape: {text}");
        assert!(!text.contains("Arial"), "font table must be skipped: {text}");
    }

    #[test]
    fn doc_stream_text_uses_the_fib_range() {
        // Synthetic WordDocument stream: garbage, then the main text
        // (UTF-16LE) inside the fcMin..fcMax window the FIB advertises.
        let mut stream = vec![0u8; 0x40];
        let text = "The quick brown fox agreement text lives here.";
        let mut enc = Vec::new();
        for u in text.encode_utf16() {
            enc.extend_from_slice(&u.to_le_bytes());
        }
        let fc_min = stream.len() as u32;
        stream.extend_from_slice(&enc);
        let fc_max = stream.len() as u32;
        stream[0x18..0x1C].copy_from_slice(&fc_min.to_le_bytes());
        stream[0x1C..0x20].copy_from_slice(&fc_max.to_le_bytes());
        let out = doc_stream_text(&stream);
        assert!(out.contains("quick brown fox"), "fib extraction: {out}");
    }

    #[test]
    fn image_parser_without_ocr_reports_no_extractable_text() {
        let mut config = RagAnythingConfig::default();
        config.ocr_enabled = false;
        let parser = get_parser("auto", "photo.jpg", &config).unwrap();
        let err = parser.parse("photo.jpg", b"\xff\xd8\xff").unwrap_err();
        assert!(err.to_string().contains("has no extractable text"), "{err}");
    }
}
