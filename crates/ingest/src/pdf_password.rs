//! Password-protected PDF inputs (feature: accept protected files at upload).
//!
//! Decryption happens in memory, before extraction, so a decrypted PDF flows
//! through the unchanged pipeline — including the render → Surya OCR path
//! for scanned pages. Decrypted bytes are never written to disk by this
//! module (callers keep them in memory only), the password is never stored
//! and never logged, and callers zeroize it once decryption returns.
//!
//! Mechanism, settled by measurement on real fixtures (2026-10-26), not by
//! argument — see the tests in this module:
//!
//! * `lopdf` 0.34 (pure Rust) decrypts RC4 standard-security-handler files
//!   and round-trips them to text identical to the unprotected original.
//!   It does NOT cover AES: AES-128 fails with "encryption scheme not
//!   supported" and AES-256 with "unsupported key length".
//! * `lopdf` 0.46 does decrypt AES in memory, but it loads only the
//!   `/Encrypt` object (1 object, 0 pages) from the xref-stream files a
//!   mainstream producer (pypdf) emits, so its re-saved output is an
//!   empty document — unusable as pipeline input. It was rejected.
//! * So: try `lopdf` first; anything it cannot handle falls through to the
//!   `pypdf` helper (`tools/pdf_decrypt_sidecar.py`, same sidecar pattern
//!   as the Surya OCR script; its `pypdf`/`cryptography` deps are vendored
//!   under `tools/pdf-decrypt-pylibs/` because a system pip install does
//!   not survive VM replacement). The helper receives the password on stdin
//!   (first line) followed by the raw PDF bytes — never argv, never logs —
//!   writes decrypted PDF bytes on stdout, and exits 42 for a wrong
//!   password (42, not 2: the interpreter itself exits 2 when the script
//!   file is missing, and that must never masquerade as a wrong password).
//!   Its stderr is deliberately discarded, never logged.
//!
//! Error contract (callers and the HTTP layer rely on these exact strings):
//!
//! * wrong password → `wrong file password` (never the generic extraction
//!   failure, so clients can prompt again instead of reporting corruption).
//! * encrypted file, no password supplied → the pipeline entry points
//!   return `file is password-protected; supply file_password to upload it`
//!   before any extraction is attempted.
//! * unprotected file + password supplied → password ignored, bytes returned
//!   unchanged, no error.

use legal_rag_common::{Error, Result};
use std::io::Write;
use std::process::{Command, Stdio};

/// Distinct error for a wrong file password (see module docs).
pub const WRONG_PASSWORD_MSG: &str = "wrong file password";

/// Error when an encrypted PDF arrives without a `file_password`.
pub const PROTECTED_NO_PASSWORD_MSG: &str =
    "file is password-protected; supply file_password to upload it";

/// Error when an encrypted PDF cannot be decrypted for a reason other than
/// a wrong password (e.g. the `pypdf` helper is unavailable).
pub const DECRYPT_FAILED_MSG: &str =
    "encrypted PDF could not be decrypted (unsupported scheme or decryption helper unavailable)";

/// True when `bytes` look like an encrypted PDF, per `lopdf`'s own
/// `/Encrypt` detection. Deliberately NOT a raw byte scan for `/Encrypt`:
/// xref-stream files (the modern default) carry the entry compressed, so a
/// scan would miss exactly the files that matter, while a literal in a
/// comment or content stream would false-positive. A file `lopdf` cannot
/// parse at all reports false here and then fails downstream with a
/// per-file error (S3) or the generic extraction failure — never silently.
pub fn is_encrypted_pdf(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"%PDF-") {
        return false;
    }
    lopdf::Document::load_mem(bytes)
        .map(|doc| doc.is_encrypted())
        .unwrap_or(false)
}

/// Decrypt `bytes` with `password` when the PDF is encrypted; return the
/// input unchanged when it is not (so a password supplied for an
/// unprotected file is silently ignored, per spec).
///
/// The caller owns zeroizing `password` after this returns.
pub fn decrypt_pdf(bytes: &[u8], password: &str) -> Result<Vec<u8>> {
    if !is_encrypted_pdf(bytes) {
        return Ok(bytes.to_vec());
    }
    match try_lopdf(bytes, password) {
        Ok(out) => Ok(out),
        Err(LopdfOutcome::WrongPassword) => {
            Err(Error::BadRequest(WRONG_PASSWORD_MSG.to_string()))
        }
        Err(LopdfOutcome::Unsupported) => decrypt_via_pypdf(bytes, password),
    }
}

/// Outcome of the `lopdf` attempt: success, a definitively wrong password
/// (RC4 key validation is authoritative), or "cannot handle this file"
/// (AES schemes, unparseable layouts) → fall through to `pypdf`.
enum LopdfOutcome {
    WrongPassword,
    Unsupported,
}

fn try_lopdf(bytes: &[u8], password: &str) -> std::result::Result<Vec<u8>, LopdfOutcome> {
    let mut doc = lopdf::Document::load_mem(bytes).map_err(|_| LopdfOutcome::Unsupported)?;
    if !doc.is_encrypted() {
        return Ok(bytes.to_vec());
    }
    doc.decrypt(password.as_bytes()).map_err(|e| match e {
        lopdf::Error::Decryption(lopdf::encryption::DecryptionError::IncorrectPassword) => {
            LopdfOutcome::WrongPassword
        }
        _ => LopdfOutcome::Unsupported,
    })?;
    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|_| LopdfOutcome::Unsupported)?;
    // Validate structurally, not by text: a scanned PDF has no text layer
    // yet must still round-trip to the OCR path. A re-save that lost its
    // pages (the lopdf 0.46 failure mode) must fall through, not ship.
    let check = lopdf::Document::load_mem(&out).map_err(|_| LopdfOutcome::Unsupported)?;
    if check.is_encrypted() || check.get_pages().is_empty() {
        return Err(LopdfOutcome::Unsupported);
    }
    Ok(out)
}

/// Resolve the helper command. `LEGAL_RAG_PDF_DECRYPT_CMD` wins when set
/// (mirroring the Surya OCR command convention). Otherwise walk up from
/// the working directory for `tools/pdf_decrypt_sidecar.py` — the server
/// runs from the repo root, but library tests run from the crate dir, so
/// a bare relative path would silently resolve to nothing in tests.
fn helper_argv() -> Vec<String> {
    if let Ok(cmdline) = std::env::var("LEGAL_RAG_PDF_DECRYPT_CMD") {
        return cmdline.split_whitespace().map(|s| s.to_string()).collect();
    }
    let mut dir = std::env::current_dir().ok();
    while let Some(d) = dir {
        let candidate = d.join("tools").join("pdf_decrypt_sidecar.py");
        if candidate.is_file() {
            return vec![
                "python3".to_string(),
                candidate.to_string_lossy().into_owned(),
            ];
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    vec![
        "python3".to_string(),
        "tools/pdf_decrypt_sidecar.py".to_string(),
    ]
}

/// `pypdf` helper fallback. Password on stdin, never argv; stderr is
/// discarded, never logged, so it cannot leak the password or file bytes.
fn decrypt_via_pypdf(bytes: &[u8], password: &str) -> Result<Vec<u8>> {
    let argv = helper_argv();
    if argv.is_empty() {
        return Err(Error::BadRequest(DECRYPT_FAILED_MSG.to_string()));
    }
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Error::BadRequest(DECRYPT_FAILED_MSG.to_string()))?;
    // Protocol: password line, then raw PDF bytes. Passwords containing a
    // newline cannot be expressed in this framing; that is documented on
    // the helper and is the only accepted limitation.
    let mut input = password.as_bytes().to_vec();
    input.push(b'\n');
    input.extend_from_slice(bytes);
    child
        .stdin
        .as_mut()
        .expect("stdin piped")
        .write_all(&input)
        .map_err(|_| Error::BadRequest(DECRYPT_FAILED_MSG.to_string()))?;
    drop(child.stdin.take());
    let out = child
        .wait_with_output()
        .map_err(|_| Error::BadRequest(DECRYPT_FAILED_MSG.to_string()))?;
    match out.status.code() {
        Some(0) => Ok(out.stdout),
        Some(42) => Err(Error::BadRequest(WRONG_PASSWORD_MSG.to_string())),
        _ => Err(Error::BadRequest(DECRYPT_FAILED_MSG.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::extract_text;

    /// Test-only password for the committed fixtures — obviously not a
    /// real credential. All fixtures share it.
    const FIXTURE_PASSWORD: &str = "test-fixture-pw";

    fn fixture(name: &str) -> Vec<u8> {
        // Fixtures live in this crate's test data dir, anchored at the
        // crate root so the test works regardless of the runner's cwd.
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"))
    }

    fn norm(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn detects_encrypted_pdfs() {
        assert!(!is_encrypted_pdf(&fixture("plain.pdf")));
        assert!(is_encrypted_pdf(&fixture("enc-rc4.pdf")));
        assert!(is_encrypted_pdf(&fixture("enc-aes128.pdf")));
        assert!(is_encrypted_pdf(&fixture("enc-aes256.pdf")));
        assert!(!is_encrypted_pdf(b"not a pdf at all"));
        assert!(!is_encrypted_pdf(b"%PDF-1.4\n/Encrypt in a comment"));
    }

    #[test]
    fn round_trips_all_schemes_to_plain_text() {
        let plain = norm(&extract_text("plain.pdf", &fixture("plain.pdf")).unwrap());
        assert!(plain.contains("Section 86 of the Information Technology Act"));
        for name in ["enc-rc4.pdf", "enc-aes128.pdf", "enc-aes256.pdf"] {
            let decrypted = decrypt_pdf(&fixture(name), FIXTURE_PASSWORD).unwrap();
            let text = norm(&extract_text(name, &decrypted).unwrap());
            assert_eq!(text, plain, "{name} must round-trip to the plain text");
        }
    }

    #[test]
    fn wrong_password_is_distinct_for_every_scheme() {
        for name in ["enc-rc4.pdf", "enc-aes128.pdf", "enc-aes256.pdf"] {
            let err = decrypt_pdf(&fixture(name), "definitely-wrong").unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains(WRONG_PASSWORD_MSG),
                "{name}: expected distinct wrong-password error, got {msg}"
            );
            assert!(
                !msg.contains("pdf extraction failed"),
                "{name}: must not collapse into the generic extraction failure"
            );
        }
    }

    #[test]
    fn unprotected_file_with_password_is_ignored() {
        let plain = fixture("plain.pdf");
        let out = decrypt_pdf(&plain, FIXTURE_PASSWORD).unwrap();
        assert_eq!(out, plain);
    }
}
