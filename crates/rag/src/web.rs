//! Web search + stealth page fetching for web-augmented RAG.
//!
//! Two pieces:
//!
//! 1. **Search** — find fresh URLs for a query (latest amendments, recent
//!    case law, news). The stealth provider drives a real search engine
//!    through `invisible_playwright`, so no search API key is needed.
//! 2. **Fetch** — pull page content through the anti-detect Firefox
//!    (`tools/web/` sidecar). Court and government sites that block plain
//!    HTTP clients and headless Chromium usually let this through.
//!
//! Both go through the [`WebSearch`] trait; [`NoopWebSearch`] disables the
//! feature when the sidecar isn't installed. Fetched pages are cited as
//! `[web:N]` sources, clearly separated from corpus `[doc:N]` citations.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use legal_rag_common::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebResult {
    pub url: String,
    pub title: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedPage {
    pub url: String,
    pub title: String,
    pub text: String,
    /// Publication / judgment date (YYYY-MM-DD) when the page exposes one.
    #[serde(default)]
    pub date: Option<String>,
}

#[async_trait]
pub trait WebSearch: Send + Sync {
    /// Human label for logs/UI.
    fn name(&self) -> &str;
    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<WebResult>>;
    async fn fetch(&self, url: &str) -> Result<FetchedPage>;
}

/// Disabled stub — used when the stealth sidecar isn't installed.
pub struct NoopWebSearch;

#[async_trait]
impl WebSearch for NoopWebSearch {
    fn name(&self) -> &str {
        "none"
    }
    async fn search(&self, _query: &str, _max_results: usize) -> Result<Vec<WebResult>> {
        Err(Error::Config("web search not configured".into()))
    }
    async fn fetch(&self, _url: &str) -> Result<FetchedPage> {
        Err(Error::Config("web search not configured".into()))
    }
}

/// Web search powered by the `invisible_playwright` sidecar in `tools/web`.
/// Search and fetch both run inside the anti-detect Firefox, so
/// bot-guarded legal/government sites can be read like a human would.
pub struct StealthWebSearch {
    /// Directory holding `websearch.py` / `webfetch.py` (a uv project).
    sidecar_dir: PathBuf,
    timeout: Duration,
}

impl StealthWebSearch {
    pub fn new(sidecar_dir: PathBuf) -> Self {
        Self { sidecar_dir, timeout: Duration::from_secs(120) }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// True when the sidecar looks installed (project + scripts present).
    pub fn available(sidecar_dir: &std::path::Path) -> bool {
        sidecar_dir.join("websearch.py").exists() && sidecar_dir.join("webfetch.py").exists()
    }

    async fn run_script(&self, script: &str, args: &[&str]) -> Result<Vec<u8>> {
        // Prefer the uv-managed venv python; fall back to `uv run`.
        let venv_python = self.sidecar_dir.join(".venv/bin/python");
        let script_path = self.sidecar_dir.join(script);
        let mut cmd = if venv_python.exists() {
            let mut c = tokio::process::Command::new(&venv_python);
            c.arg(&script_path);
            c
        } else {
            let mut c = tokio::process::Command::new("uv");
            c.arg("run")
                .arg("--project")
                .arg(&self.sidecar_dir)
                .arg("python")
                .arg(&script_path);
            c
        };
        cmd.args(args);
        cmd.kill_on_drop(true);
        let out = tokio::time::timeout(self.timeout, cmd.output())
            .await
            .map_err(|_| Error::Upstream(format!("web sidecar timed out: {script}")))?
            .map_err(|e| Error::Upstream(format!("web sidecar failed: {e}")))?;
        if !out.status.success() {
            return Err(Error::Upstream(format!(
                "web sidecar error: {}",
                String::from_utf8_lossy(&out.stderr).chars().take(300).collect::<String>()
            )));
        }
        Ok(out.stdout)
    }
}

#[async_trait]
impl WebSearch for StealthWebSearch {
    fn name(&self) -> &str {
        "stealth-playwright"
    }

    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<WebResult>> {
        let max = max_results.clamp(1, 10).to_string();
        let raw = self.run_script("websearch.py", &[query, &max]).await?;
        serde_json::from_slice::<Vec<WebResult>>(&raw)
            .map_err(|e| Error::Upstream(format!("bad websearch output: {e}")))
    }

    async fn fetch(&self, url: &str) -> Result<FetchedPage> {
        let raw = self.run_script("webfetch.py", &[url]).await?;
        serde_json::from_slice::<FetchedPage>(&raw)
            .map_err(|e| Error::Upstream(format!("bad webfetch output: {e}")))
    }
}
