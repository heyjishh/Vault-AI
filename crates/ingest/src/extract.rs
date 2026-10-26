//! Text extraction from raw bytes.

use legal_rag_common::{Error, Result};

/// Extract plain text based on file extension.
pub fn extract_text(file_name: &str, bytes: &[u8]) -> Result<String> {
    extract_text_with_password(file_name, bytes, None)
}

/// Like [`extract_text`], but a password-protected PDF is decrypted in
/// memory first (see [`crate::pdf_password`]). `password` is ignored for
/// unprotected files and for non-PDF inputs. The caller zeroizes it.
pub fn extract_text_with_password(
    file_name: &str,
    bytes: &[u8],
    password: Option<&str>,
) -> Result<String> {
    let lower = file_name.to_lowercase();
    if lower.ends_with(".pdf") {
        if crate::pdf_password::is_encrypted_pdf(bytes) {
            let pw = password.ok_or_else(|| {
                Error::BadRequest(crate::pdf_password::PROTECTED_NO_PASSWORD_MSG.to_string())
            })?;
            let decrypted = crate::pdf_password::decrypt_pdf(bytes, pw)?;
            return extract_pdf(&decrypted);
        }
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

    #[test]
    fn encrypted_pdf_without_password_reports_protected_not_generic() {
        let enc =
            std::fs::read(format!("{}/tests/fixtures/enc-aes256.pdf", env!("CARGO_MANIFEST_DIR")))
                .unwrap();
        let err = extract_text("enc-aes256.pdf", &enc).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("password-protected"), "got {msg}");
        assert!(!msg.contains("pdf extraction failed"), "got {msg}");
    }
}
