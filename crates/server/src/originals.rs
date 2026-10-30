//! Retained original uploads (Piece 29).
//!
//! Until now the vault kept only a document's EXTRACTED TEXT: the
//! uploaded file itself was decoded, parsed and dropped, so a source
//! citation could never open the document the user actually uploaded.
//! At ingest we now also keep the raw bytes under
//! `<data_dir>/originals/`:
//!
//! - `<doc_id>.<ext>` — the original bytes (extension sanitized from
//!   the uploaded file name),
//! - `<doc_id>.name`  — a sidecar holding the original file name for
//!   the download's Content-Disposition.
//!
//! The directory follows the instance data dir
//! (`legal_rag_rag::persist::data_dir`, i.e. `LEGAL_RAG_DATA_DIR`), so
//! prod (`data/`) and test (`data-test/`) never share originals.
//!
//! The Confidential Vault is NEVER routed through here: sealing a
//! private document must not leave a plaintext copy on disk.
//!
//! Documents ingested before this piece have no retained file; the
//! endpoint 404s for them and the UI explains that the extracted text
//! is what exists.

use std::path::{Path, PathBuf};

/// Directory holding every retained original for this instance.
pub fn originals_dir() -> PathBuf {
    legal_rag_rag::persist::data_dir().join("originals")
}

/// Document ids are server-generated (`doc-<hex>`), but the id is
/// also a path component here, so it is treated as untrusted: only
/// [A-Za-z0-9_-] survives, anything else (notably '/' or '.') rejects
/// the operation outright rather than being rewritten into a
/// different document's slot.
fn safe_doc_key(doc_id: &str) -> Option<String> {
    if doc_id.is_empty()
        || !doc_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(doc_id.to_string())
}

/// Extension for storage/display, derived from the uploaded file
/// name ONLY: the part after the last '.', lowercased, ASCII
/// alphanumerics only, max 10 chars. A name like "../../etc/passwd"
/// yields "" (no usable extension) — separators and dots can never
/// survive into the stored path, so a hostile file name cannot
/// escape the originals directory.
pub fn sanitize_extension(file_name: &str) -> String {
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let Some((_, ext)) = base.rsplit_once('.') else {
        return String::new();
    };
    let cleaned: String = ext
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .take(10)
        .collect();
    // An "extension" that is really a whole path segment (no dot in
    // the original, or a traversal fragment) collapses above; a dot
    // hiding inside it (e.g. "a.../x") is filtered out by the
    // alphanumeric pass regardless.
    cleaned
}

/// The file name shown on download: single line, no quotes or
/// control characters (it lands inside a quoted header value).
fn sanitize_display_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "document".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

/// Content type for a stored extension.
pub fn content_type_for_ext(ext: &str) -> &'static str {
    match ext {
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "tif" | "tiff" => "image/tiff",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "docm" => "application/vnd.ms-word.document.macroEnabled.12",
        "odt" => "application/vnd.oasis.opendocument.text",
        "rtf" => "application/rtf",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsm" => "application/vnd.ms-excel.sheet.macroEnabled.12",
        "xlsb" => "application/vnd.ms-excel.sheet.binary.macroEnabled.12",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        _ => "application/octet-stream",
    }
}

fn data_file_name(doc_key: &str, ext: &str) -> String {
    if ext.is_empty() {
        doc_key.to_string()
    } else {
        format!("{doc_key}.{ext}")
    }
}

/// Save `bytes` as the retained original for `doc_id` inside `dir`.
/// `display_name` is the user-facing file name (sidecar); `ext` is
/// the already-sanitized extension to store under ("" for none). Any
/// earlier original for the same document (a deduped re-upload under
/// a different extension) is replaced.
pub fn save_original_in(
    dir: &Path,
    doc_id: &str,
    display_name: &str,
    ext: &str,
    bytes: &[u8],
) -> std::io::Result<PathBuf> {
    let key = safe_doc_key(doc_id)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "unsafe doc id"))?;
    std::fs::create_dir_all(dir)?;
    delete_original_in(dir, &key);
    let path = dir.join(data_file_name(&key, ext));
    std::fs::write(&path, bytes)?;
    std::fs::write(
        dir.join(format!("{key}.name")),
        sanitize_display_name(display_name),
    )?;
    Ok(path)
}

/// Save using the instance's originals dir. `file_name` is the
/// uploaded file's name — the stored extension is derived from it,
/// unless `forced_ext` overrides it (pasted text is stored as .txt).
pub fn save_original(
    doc_id: &str,
    file_name: &str,
    forced_ext: Option<&str>,
    bytes: &[u8],
) -> std::io::Result<PathBuf> {
    let ext = match forced_ext {
        Some(e) => sanitize_extension(&format!("x.{e}")),
        None => sanitize_extension(file_name),
    };
    save_original_in(&originals_dir(), doc_id, file_name, &ext, bytes)
}

/// Resolve a document's retained original: (file path, display name,
/// content type). `None` when nothing was retained (pre-Piece-29
/// documents) or the id is unsafe.
pub fn resolve_original_in(dir: &Path, doc_id: &str) -> Option<(PathBuf, String, &'static str)> {
    let key = safe_doc_key(doc_id)?;
    let display = std::fs::read_to_string(dir.join(format!("{key}.name")))
        .map(|s| sanitize_display_name(&s))
        .ok();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| {
                    n != format!("{key}.name")
                        && (n == key || n.starts_with(&format!("{key}.")))
                })
                .unwrap_or(false)
        })
        .collect();
    entries.sort();
    let path = entries.into_iter().next()?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    // A stored file whose whole name IS the doc key carries no ext.
    let ext = if path.file_name().and_then(|n| n.to_str()) == Some(key.as_str()) {
        String::new()
    } else {
        ext
    };
    let name = display.unwrap_or_else(|| data_file_name(&key, &ext));
    Some((path, name, content_type_for_ext(&ext)))
}

/// Resolve using the instance's originals dir.
pub fn resolve_original(doc_id: &str) -> Option<(PathBuf, String, &'static str)> {
    resolve_original_in(&originals_dir(), doc_id)
}

/// Delete a document's retained original + sidecar. Returns how many
/// files were removed.
pub fn delete_original_in(dir: &Path, doc_id: &str) -> usize {
    let Some(key) = safe_doc_key(doc_id) else {
        return 0;
    };
    let mut removed = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            let is_ours = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == key || n.starts_with(&format!("{key}.")))
                .unwrap_or(false);
            if is_ours && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// Delete using the instance's originals dir.
pub fn delete_original(doc_id: &str) -> usize {
    delete_original_in(&originals_dir(), doc_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "originals-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_resolve_delete_round_trip() {
        let dir = temp_dir("roundtrip");
        let path = save_original_in(&dir, "doc-abc123", "Lease Agreement.PDF", "pdf", b"%PDF-fake")
            .expect("save");
        assert_eq!(path, dir.join("doc-abc123.pdf"));
        let (resolved, name, ctype) = resolve_original_in(&dir, "doc-abc123").expect("resolve");
        assert_eq!(resolved, path);
        assert_eq!(name, "Lease Agreement.PDF");
        assert_eq!(ctype, "application/pdf");
        assert_eq!(std::fs::read(&resolved).unwrap(), b"%PDF-fake");

        // Re-saving under another extension replaces, never accumulates.
        save_original_in(&dir, "doc-abc123", "Lease.txt", "txt", b"plain").expect("re-save");
        let (resolved2, _, ctype2) = resolve_original_in(&dir, "doc-abc123").expect("resolve 2");
        assert_eq!(resolved2, dir.join("doc-abc123.txt"));
        assert_eq!(ctype2, "text/plain");

        assert!(delete_original_in(&dir, "doc-abc123") >= 2);
        assert!(resolve_original_in(&dir, "doc-abc123").is_none());
        assert_eq!(delete_original_in(&dir, "doc-abc123"), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extension_sanitization_never_escapes() {
        assert_eq!(sanitize_extension("lease.PDF"), "pdf");
        assert_eq!(sanitize_extension("archive.tar.gz"), "gz");
        assert_eq!(sanitize_extension("noextension"), "");
        assert_eq!(sanitize_extension("../../etc/passwd"), "");
        assert_eq!(sanitize_extension("..\\..\\evil.pdf"), "pdf");
        assert_eq!(sanitize_extension("file.p/d/f"), "");
        // Over-long + hostile suffixes collapse to alphanumerics, capped.
        let e = sanitize_extension("x.abc/../../../..");
        assert!(e.chars().all(|c| c.is_ascii_alphanumeric()) && e.len() <= 10);
    }

    #[test]
    fn traversal_doc_ids_are_refused() {
        let dir = temp_dir("traversal");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(save_original_in(&dir, "../escape", "x.txt", "txt", b"x").is_err());
        assert!(save_original_in(&dir, "a/b", "x.txt", "txt", b"x").is_err());
        assert!(resolve_original_in(&dir, "../escape").is_none());
        assert_eq!(delete_original_in(&dir, "../../etc"), 0);
        // Nothing was written outside (or inside) for those ids.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_original_resolves_to_none() {
        let dir = temp_dir("missing");
        assert!(resolve_original_in(&dir, "doc-never-saved").is_none());
    }

    #[test]
    fn content_types_cover_the_upload_matrix() {
        assert_eq!(content_type_for_ext("pdf"), "application/pdf");
        assert_eq!(content_type_for_ext("png"), "image/png");
        assert_eq!(content_type_for_ext("docx"), "application/vnd.openxmlformats-officedocument.wordprocessingml.document");
        assert_eq!(content_type_for_ext("txt"), "text/plain");
        assert_eq!(content_type_for_ext("zzz"), "application/octet-stream");
    }
}
