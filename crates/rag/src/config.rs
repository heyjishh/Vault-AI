//! `RAGAnythingConfig` — faithful mirror of rag-anything's configuration.
//!
//! Field-for-field equivalent of `RAGAnythingConfig` in
//! `raganything/config.py`: parser selection, multimodal toggles,
//! batch processing, and context extraction.

use std::env;

use serde::{Deserialize, Serialize};

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_bool(key: &str, default: bool) -> bool {
    env::var(key)
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Mirrors `RAGAnythingConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagAnythingConfig {
    // Directory configuration
    pub working_dir: String,
    // Parser configuration
    pub parse_method: String,
    pub parser_output_dir: String,
    pub parser: String,
    pub display_content_stats: bool,
    // Multimodal processing configuration
    pub enable_image_processing: bool,
    pub enable_table_processing: bool,
    pub enable_equation_processing: bool,
    pub enable_audio_processing: bool,
    pub enable_video_processing: bool,
    // Batch processing configuration
    pub max_concurrent_files: usize,
    pub supported_file_extensions: Vec<String>,
    pub recursive_folder_processing: bool,
    // Context extraction configuration
    pub context_window: usize,
    pub context_mode: String,
    pub max_context_tokens: usize,
    pub include_headers: bool,
    pub include_captions: bool,
    pub filter_content_types: Vec<String>,
    pub content_format: String,
    // Path handling
    pub use_full_path: bool,
    // OCR fallback for scanned/image PDFs
    pub ocr_enabled: bool,
    pub ocr_min_text_chars: usize,
    pub ocr_dpi: u32,
    /// OCR engine command (Surya sidecar): reads a PNG on stdin, writes
    /// recognized text on stdout. Surya is the only OCR engine.
    /// Env: RAG_ANYTHING_OCR_PRIMARY_CMD.
    pub ocr_primary_cmd: String,
    pub pdftoppm_cmd: String,
}

impl Default for RagAnythingConfig {
    fn default() -> Self {
        Self {
            working_dir: "./rag_storage".into(),
            parse_method: "auto".into(),
            parser_output_dir: "./output".into(),
            parser: "auto".into(),
            display_content_stats: true,
            enable_image_processing: true,
            enable_table_processing: true,
            enable_equation_processing: true,
            enable_audio_processing: false,
            enable_video_processing: false,
            max_concurrent_files: 4,
            supported_file_extensions: vec![
                ".pdf", ".txt", ".md", ".doc", ".docx", ".png", ".jpg", ".jpeg",
            ]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
            recursive_folder_processing: true,
            context_window: 1,
            context_mode: "page".into(),
            max_context_tokens: 2000,
            include_headers: true,
            include_captions: true,
            filter_content_types: vec!["text".into()],
            content_format: "raganything".into(),
            use_full_path: false,
            ocr_enabled: true,
            ocr_min_text_chars: 50,
            ocr_dpi: 300,
            // Surya is the only OCR engine (see ocr.rs for the
            // benchmark rationale; Tesseract was removed 2026-10-20).
            // The wrapper script wires the deployment env the bare
            // sidecar needs (venv PATH, llama.cpp backend, local GGUF
            // weights) and is a passthrough where Surya is installed
            // normally — `bash` as argv[0] also keeps CommandOcr's
            // `available()` probe (`bash --version`) from executing
            // the sidecar itself.
            ocr_primary_cmd: "bash tools/ocr/surya_sidecar.sh".into(),
            pdftoppm_cmd: "pdftoppm".into(),
        }
    }
}

impl RagAnythingConfig {
    /// Load from environment (`RAG_ANYTHING_*`), falling back to defaults.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        cfg.working_dir = env_or("RAG_ANYTHING_WORKING_DIR", &cfg.working_dir);
        cfg.parse_method = env_or("RAG_ANYTHING_PARSE_METHOD", &cfg.parse_method);
        cfg.parser = env_or("RAG_ANYTHING_PARSER", &cfg.parser);
        cfg.enable_image_processing =
            env_bool("RAG_ANYTHING_ENABLE_IMAGE_PROCESSING", cfg.enable_image_processing);
        cfg.enable_table_processing =
            env_bool("RAG_ANYTHING_ENABLE_TABLE_PROCESSING", cfg.enable_table_processing);
        cfg.enable_equation_processing = env_bool(
            "RAG_ANYTHING_ENABLE_EQUATION_PROCESSING",
            cfg.enable_equation_processing,
        );
        cfg.max_concurrent_files =
            env_usize("RAG_ANYTHING_MAX_CONCURRENT_FILES", cfg.max_concurrent_files);
        cfg.context_window = env_usize("RAG_ANYTHING_CONTEXT_WINDOW", cfg.context_window);
        cfg.context_mode = env_or("RAG_ANYTHING_CONTEXT_MODE", &cfg.context_mode);
        cfg.max_context_tokens =
            env_usize("RAG_ANYTHING_MAX_CONTEXT_TOKENS", cfg.max_context_tokens);
        cfg.include_headers = env_bool("RAG_ANYTHING_INCLUDE_HEADERS", cfg.include_headers);
        cfg.include_captions = env_bool("RAG_ANYTHING_INCLUDE_CAPTIONS", cfg.include_captions);
        cfg.ocr_enabled = env_bool("RAG_ANYTHING_OCR_ENABLED", cfg.ocr_enabled);
        cfg.ocr_min_text_chars =
            env_usize("RAG_ANYTHING_OCR_MIN_TEXT_CHARS", cfg.ocr_min_text_chars);
        cfg.ocr_dpi = env_usize("RAG_ANYTHING_OCR_DPI", cfg.ocr_dpi as usize) as u32;
        cfg.ocr_primary_cmd = env_or("RAG_ANYTHING_OCR_PRIMARY_CMD", &cfg.ocr_primary_cmd);
        cfg.pdftoppm_cmd = env_or("RAG_ANYTHING_PDFTOPPM_CMD", &cfg.pdftoppm_cmd);
        cfg
    }
}
