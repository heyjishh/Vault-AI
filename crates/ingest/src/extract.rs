//! Text extraction from raw bytes.

use legal_rag_common::{Error, Result};

/// Extract plain text based on file extension.
pub fn extract_text(file_name: &str, bytes: &[u8]) -> Result<String> {
    let lower = file_name.to_lowercase();
    if lower.ends_with(".pdf") {
        extract_pdf(bytes)
    } else if lower.ends_with(".txt")
        || lower.ends_with(".md")
        || lower.ends_with(".markdown")
        || lower.ends_with(".rst")
    {
        Ok(String::from_utf8_lossy(bytes).into_owned())
    } else {
        // Best effort for unknown types: treat as text when plausible.
        let text = String::from_utf8_lossy(bytes);
        if text.trim().is_empty() {
            return Err(Error::BadRequest(format!("unsupported or empty file: {file_name}")));
        }
        Ok(text.into_owned())
    }
}

fn extract_pdf(bytes: &[u8]) -> Result<String> {
    pdf_extract::extract_text_from_mem(bytes)
        .map_err(|e| Error::BadRequest(format!("pdf extraction failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_text() {
        let text = extract_text("contract.txt", b"Section 1. Parties agree.").unwrap();
        assert!(text.contains("Parties agree"));
    }

    #[test]
    fn rejects_empty_unknown() {
        assert!(extract_text("blob.bin", b"").is_err());
    }
}
