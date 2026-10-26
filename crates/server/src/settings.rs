//! Admin customization settings: how the user panel looks and answers.
//!
//! Persisted atomically to `settings.json` in the data dir. Everything
//! here is admin-controlled from the Command Center:
//! - the default query mode (users never see the mode picker),
//! - an optional system-prompt override for answers,
//! - the LLM model (hot-swappable) and embedding model (applies at boot),
//! - branding (product name + tagline) for the user panel,
//! - custom sidebar tabs (external links or markdown pages).

use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Brand {
    pub product_name: String,
    pub tagline: String,
}

impl Default for Brand {
    fn default() -> Self {
        Self {
            product_name: "Juris Vault AI".to_string(),
            tagline: "Legal Research, With Memory".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabKind {
    Link,
    Page,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiTab {
    pub id: String,
    pub label: String,
    pub kind: TabKind,
    /// For `Link` tabs: the URL to open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// For `Page` tabs: markdown content rendered inside the panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default = "default_true")]
    pub visible: bool,
}

fn default_true() -> bool {
    true
}

/// Per-surface prompt overrides. `None` on any surface = its built-in
/// default prompt. Surfaces: Ask answers, Interact (document analyst),
/// Draft (drafting assistant), Research (planner + adjudicator), and
/// Agents (standing instructions injected into every workflow run).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Prompts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub research_frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub research_adjudicate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// One of naive | local | global | hybrid | mix | bypass.
    pub default_mode: String,
    /// Legacy single-prompt field (the Ask prompt) — migrated into
    /// `prompts.ask` on load and no longer written by the panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub prompts: Prompts,
    /// LLM model id override (hot-swapped); `None` = deployment default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_model: Option<String>,
    /// Embedding model id override — applied at boot; changing it
    /// requires re-indexing the corpus (vectors are not interchangeable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    /// Per-run token cap for sub-agent orchestration runs (admin knob;
    /// `None` = the built-in default of 100,000 tokens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestration_run_cap_tokens: Option<u64>,
    #[serde(default)]
    pub brand: Brand,
    #[serde(default)]
    pub tabs: Vec<UiTab>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_mode: "mix".to_string(),
            orchestration_run_cap_tokens: None,
            system_prompt: None,
            prompts: Prompts::default(),
            llm_model: None,
            embedding_model: None,
            brand: Brand::default(),
            tabs: Vec::new(),
        }
    }
}

pub const QUERY_MODES: [&str; 6] = ["naive", "local", "global", "hybrid", "mix", "bypass"];

pub struct SettingsStore {
    path: PathBuf,
    inner: RwLock<Settings>,
}

impl SettingsStore {
    pub fn load(path: PathBuf) -> Self {
        let mut settings: Settings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        // Migrate the legacy single system prompt into the Ask slot.
        if settings.prompts.ask.is_none() && settings.system_prompt.is_some() {
            settings.prompts.ask = settings.system_prompt.take();
        }
        Self { path, inner: RwLock::new(settings) }
    }

    pub fn get(&self) -> Settings {
        self.inner.read().unwrap().clone()
    }

    pub fn update(&self, settings: Settings) -> Result<Settings, String> {
        if !QUERY_MODES.contains(&settings.default_mode.as_str()) {
            return Err(format!("unknown query mode: {}", settings.default_mode));
        }
        if settings.brand.product_name.trim().is_empty() {
            return Err("product name cannot be empty".to_string());
        }
        for tab in &settings.tabs {
            if tab.label.trim().is_empty() {
                return Err("every tab needs a label".to_string());
            }
            match tab.kind {
                TabKind::Link if tab.url.as_deref().unwrap_or("").trim().is_empty() => {
                    return Err(format!("link tab '{}' needs a URL", tab.label));
                }
                TabKind::Page if tab.content.as_deref().unwrap_or("").trim().is_empty() => {
                    return Err(format!("page tab '{}' needs content", tab.label));
                }
                _ => {}
            }
        }
        let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        *self.inner.write().unwrap() = settings.clone();
        Ok(settings)
    }
}
