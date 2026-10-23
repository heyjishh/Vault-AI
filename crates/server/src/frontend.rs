//! Static file serving for the built React frontend (`frontend/dist`).
//!
//! `LEGAL_RAG_FRONTEND_DIR` (default `frontend/dist`, relative to server CWD).
//! Missing directory falls back to a placeholder page telling the operator to run `npm run build`.

use std::path::PathBuf;

use axum::{
    body::Body,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

fn frontend_dir() -> PathBuf {
    std::env::var("LEGAL_RAG_FRONTEND_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("frontend/dist"))
}

fn admin_dir() -> PathBuf {
    std::env::var("LEGAL_RAG_ADMIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("frontend/admin/dist"))
}

fn content_type(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("map") => "application/json",
        _ => "application/octet-stream",
    }
}

async fn serve_file(path: &std::path::Path) -> Option<Response> {
    let bytes = tokio::fs::read(path).await.ok()?;
    Some(
        Response::builder()
            .header(header::CONTENT_TYPE, content_type(path))
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(bytes))
            .unwrap(),
    )
}

/// Root: serve `index.html`.
pub async fn index() -> Response {
    let dir = frontend_dir();
    if let Some(r) = serve_file(&dir.join("index.html")).await {
        return r;
    }
    fallback_page()
}

/// Catch-all: serve static assets, fall back to `index.html` for SPA routes.
pub async fn fallback(uri: axum::http::Uri) -> Response {
    let dir = frontend_dir();
    let rel = uri.path().trim_start_matches('/');
    // Never allow path traversal.
    if rel.contains("..") || rel.starts_with('/') || rel.contains('\\') {
        return StatusCode::NOT_FOUND.into_response();
    }
    let candidate = dir.join(rel);
    if !rel.is_empty() && candidate.is_file() {
        if let Some(r) = serve_file(&candidate).await {
            return r;
        }
    }
    if let Some(r) = serve_file(&dir.join("index.html")).await {
        return r;
    }
    fallback_page()
}

fn fallback_page() -> Response {
    let html = r#"<!doctype html><html><body style="font-family:system-ui;padding:4rem">
<h1>Juris Vault AI frontend not built</h1>
<p>Run <code>npm run build</code> in <code>frontend/</code>, then restart the server.</p>
</body></html>"#;
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .body(Body::from(html))
        .unwrap()
}

/// Command Center root: serve the admin app's `index.html`.
pub async fn admin_index() -> Response {
    let dir = admin_dir();
    if let Some(r) = serve_file(&dir.join("index.html")).await {
        return r;
    }
    fallback_page()
}

/// Command Center catch-all: admin static assets + SPA fallback.
pub async fn admin_fallback(uri: axum::http::Uri) -> Response {
    let dir = admin_dir();
    let rel = uri
        .path()
        .strip_prefix("/admin/")
        .unwrap_or("")
        .to_string();
    if rel.contains("..") || rel.contains('\\') {
        return StatusCode::NOT_FOUND.into_response();
    }
    let candidate = dir.join(&rel);
    if !rel.is_empty() && candidate.is_file() {
        if let Some(r) = serve_file(&candidate).await {
            return r;
        }
    }
    if let Some(r) = serve_file(&dir.join("index.html")).await {
        return r;
    }
    fallback_page()
}
