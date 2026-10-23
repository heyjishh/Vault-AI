//! OCR fallback for scanned/image PDFs.
//!
//! The PDF parser extracts the text layer first (fast path). Pages with
//! little or no extractable text are rendered with `pdftoppm` and OCR'd by
//! Surya (via `tools/ocr/surya_sidecar.py`), so scanned documents index
//! just like born-digital ones. Surya is the only OCR engine: a Docling
//! fallback was wired briefly on 2026-10-20 and removed the same day at
//! the product owner's decision — the benchmarks showed no workload where
//! a second engine rescued quality, only added weight.
//!
//! Why no Tesseract tier (benchmark, 2026-10-20, ~/workspace/ocr-bench):
//! Surya 0.22 tied Tesseract 5 on English prose (0.00% CER both) and beat it
//! everywhere else that matters for Indian legal PDFs — Hindi 0.00% vs
//! 0.86% CER, ruled tables 0.80% vs 49.9% CER, real 1947 Gazette page 6.66%
//! vs 7.8% CER. A fast tier that only ties the strong tier buys nothing.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use legal_rag_common::Result;
use tracing::{debug, warn};

/// OCR backend: image bytes (PNG) in, text out.
pub trait OcrEngine: Send + Sync {
    /// Rendered page image (PNG bytes) → recognized text.
    fn ocr_png(&self, png_bytes: &[u8]) -> Result<String>;
    /// Whether the backend is usable in this environment.
    fn available(&self) -> bool;
}

/// A pluggable OCR engine: any external command that reads a PNG on stdin
/// and writes recognized text on stdout (the sidecar contract used by
/// `tools/ocr/surya_sidecar.py`).
pub struct CommandOcr {
    name: String,
    argv: Vec<String>,
}

impl CommandOcr {
    /// Parse a command line like "python3 /opt/ocr/surya_sidecar.py".
    pub fn from_command(name: impl Into<String>, cmdline: &str) -> Option<Self> {
        let argv: Vec<String> = cmdline.split_whitespace().map(|s| s.to_string()).collect();
        if argv.is_empty() {
            None
        } else {
            Some(Self { name: name.into(), argv })
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl OcrEngine for CommandOcr {
    fn available(&self) -> bool {
        // The interpreter/binary existing is the cheap check; a missing
        // script or model surfaces as an ocr_png error, which the caller
        // treats as "no OCR text for this page".
        Command::new(&self.argv[0])
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    fn ocr_png(&self, png_bytes: &[u8]) -> Result<String> {
        let mut child = Command::new(&self.argv[0])
            .args(&self.argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                legal_rag_common::Error::BadRequest(format!(
                    "{} ocr spawn failed: {e}",
                    self.name
                ))
            })?;
        child
            .stdin
            .as_mut()
            .expect("stdin piped")
            .write_all(png_bytes)
            .map_err(|e| {
                legal_rag_common::Error::BadRequest(format!(
                    "{} ocr stdin failed: {e}",
                    self.name
                ))
            })?;
        let out = child.wait_with_output().map_err(|e| {
            legal_rag_common::Error::BadRequest(format!("{} ocr wait failed: {e}", self.name))
        })?;
        if !out.status.success() {
            return Err(legal_rag_common::Error::BadRequest(format!(
                "{} ocr failed",
                self.name
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

/// Render one PDF page (1-based) to PNG bytes via `pdftoppm`.
pub fn render_page_png(
    pdftoppm_cmd: &str,
    pdf_bytes: &[u8],
    page_1based: u32,
    dpi: u32,
) -> Result<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("legal-rag-ocr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| {
        legal_rag_common::Error::BadRequest(format!("tmpdir create failed: {e}"))
    })?;
    let pdf_path = dir.join("page.pdf");
    std::fs::write(&pdf_path, pdf_bytes).map_err(|e| {
        legal_rag_common::Error::BadRequest(format!("tmp pdf write failed: {e}"))
    })?;
    let prefix: PathBuf = dir.join("pg");
    let status = Command::new(pdftoppm_cmd)
        .args([
            "-png",
            "-r",
            &dpi.to_string(),
            "-f",
            &page_1based.to_string(),
            "-l",
            &page_1based.to_string(),
            "-singlefile",
            pdf_path.to_str().expect("tmp path"),
            prefix.to_str().expect("tmp path"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| {
            legal_rag_common::Error::BadRequest(format!("pdftoppm spawn failed: {e}"))
        })?;
    if !status.success() {
        return Err(legal_rag_common::Error::BadRequest(
            "pdftoppm render failed".to_string(),
        ));
    }
    let png_path = dir.join("pg.png");
    let bytes = std::fs::read(&png_path).map_err(|e| {
        legal_rag_common::Error::BadRequest(format!("png read failed: {e}"))
    })?;
    let _ = std::fs::remove_dir_all(&dir);
    debug!(page = page_1based, bytes = bytes.len(), "rendered pdf page");
    Ok(bytes)
}

/// OCR one PDF page: render then recognize. Returns `None` when OCR is
/// unavailable or yields nothing (caller keeps whatever text layer exists).
pub fn ocr_pdf_page(
    ocr: &dyn OcrEngine,
    pdftoppm_cmd: &str,
    pdf_bytes: &[u8],
    page_1based: u32,
    dpi: u32,
) -> Option<String> {
    if !ocr.available() {
        warn!("no OCR engine available; skipping OCR for page {page_1based}");
        return None;
    }
    match render_page_png(pdftoppm_cmd, pdf_bytes, page_1based, dpi) {
        Ok(png) => match ocr.ocr_png(&png) {
            Ok(text) if !text.trim().is_empty() => {
                debug!(page = page_1based, chars = text.len(), "ocr recovered text");
                Some(text)
            }
            Ok(_) => None,
            Err(e) => {
                warn!(page = page_1based, error = %e, "ocr failed");
                None
            }
        },
        Err(e) => {
            warn!(page = page_1based, error = %e, "page render failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_command_is_no_engine() {
        assert!(CommandOcr::from_command("surya", "").is_none());
        assert!(CommandOcr::from_command("surya", "   ").is_none());
    }

    #[test]
    fn command_engine_round_trips_stdin_to_stdout() {
        // `cat` echoes stdin back as the "recognized text" — the sidecar
        // contract (PNG bytes in, text out) without a real OCR model.
        let engine = CommandOcr::from_command("echo", "cat").unwrap();
        assert!(engine.available());
        let text = engine.ocr_png(b"recognized text").unwrap();
        assert_eq!(text, "recognized text");
    }

    #[test]
    fn command_engine_failure_is_an_error() {
        let engine = CommandOcr::from_command("bad", "false").unwrap();
        assert!(engine.ocr_png(b"x").is_err());
    }
}
