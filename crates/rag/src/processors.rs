//! Modal processors — mirrors `raganything.modalprocessors`.
//!
//! Each processor takes one multimodal content block plus extracted context,
//! generates a textual description via the LLM (the vision model for images),
//! and returns a [`ProcessedModal`] the pipeline indexes as a first-class
//! chunk/entity.

use std::sync::Arc;

use async_trait::async_trait;
use legal_rag_common::Result;

use crate::content::ContentBlock;
use crate::context::ContextExtractor;
use crate::llm::LlmClient;

/// Description produced for one multimodal block.
#[derive(Debug, Clone)]
pub struct ProcessedModal {
    /// Stable entity name, e.g. `image_3`, `table_1`.
    pub entity_name: String,
    /// LLM-generated description, indexed as chunk text.
    pub description: String,
    /// Block kind: image / table / equation.
    pub kind: &'static str,
    pub page_idx: Option<u32>,
}

/// Generates a description for one multimodal block.
#[async_trait]
pub trait ModalProcessor: Send + Sync {
    fn content_type(&self) -> &'static str;
    async fn process(
        &self,
        block: &ContentBlock,
        index: usize,
        blocks: &[ContentBlock],
        doc_id: &str,
    ) -> Result<ProcessedModal>;
}

struct Base {
    llm: Arc<dyn LlmClient>,
    context_extractor: ContextExtractor,
}

impl Base {
    async fn describe(&self, kind: &str, raw: &str, context: &str, doc_id: &str) -> Result<String> {
        let system = "You describe document content blocks for a legal retrieval index. \
            Be precise and factual; keep descriptions under 200 words.";
        let user = format!(
            "Describe this {kind} extracted from a legal document (doc_id={doc_id}).\n\
             Surrounding context:\n{context}\n\n{kind} content:\n{raw}\n\nDescription:"
        );
        let out = self.llm.complete(system, &user).await?;
        Ok(out.trim().to_string())
    }

    fn context(&self, blocks: &[ContentBlock], index: usize) -> String {
        self.context_extractor.extract_context(blocks, index)
    }
}

/// Image blocks → vision/LLM description.
pub struct ImageModalProcessor {
    base: Base,
}

impl ImageModalProcessor {
    pub fn new(llm: Arc<dyn LlmClient>, context_extractor: ContextExtractor) -> Self {
        Self { base: Base { llm, context_extractor } }
    }
}

#[async_trait]
impl ModalProcessor for ImageModalProcessor {
    fn content_type(&self) -> &'static str {
        "image"
    }

    async fn process(
        &self,
        block: &ContentBlock,
        index: usize,
        blocks: &[ContentBlock],
        doc_id: &str,
    ) -> Result<ProcessedModal> {
        let (caption, page_idx, source) = match block {
            ContentBlock::Image { caption, page_idx, source } => (caption, *page_idx, source.clone()),
            _ => unreachable!("image processor got {block:?}"),
        };
        let raw = caption
            .clone()
            .map(|c| format!("Caption: {c}\nImage reference: {source}"))
            .unwrap_or_else(|| format!("Image reference: {source}"));
        let context = self.base.context(blocks, index);
        let description = self.base.describe("image", &raw, &context, doc_id).await?;
        Ok(ProcessedModal {
            entity_name: format!("image_{index}"),
            description,
            kind: "image",
            page_idx,
        })
    }
}

/// Table blocks → LLM description of the table's contents.
pub struct TableModalProcessor {
    base: Base,
}

impl TableModalProcessor {
    pub fn new(llm: Arc<dyn LlmClient>, context_extractor: ContextExtractor) -> Self {
        Self { base: Base { llm, context_extractor } }
    }
}

#[async_trait]
impl ModalProcessor for TableModalProcessor {
    fn content_type(&self) -> &'static str {
        "table"
    }

    async fn process(
        &self,
        block: &ContentBlock,
        index: usize,
        blocks: &[ContentBlock],
        doc_id: &str,
    ) -> Result<ProcessedModal> {
        let (body, page_idx) = match block {
            ContentBlock::Table { body, page_idx, .. } => (body.clone(), *page_idx),
            _ => unreachable!("table processor got {block:?}"),
        };
        let context = self.base.context(blocks, index);
        let description = self.base.describe("table", &body, &context, doc_id).await?;
        Ok(ProcessedModal {
            entity_name: format!("table_{index}"),
            description,
            kind: "table",
            page_idx,
        })
    }
}

/// Equation blocks → LLM verbalization of the formula.
pub struct EquationModalProcessor {
    base: Base,
}

impl EquationModalProcessor {
    pub fn new(llm: Arc<dyn LlmClient>, context_extractor: ContextExtractor) -> Self {
        Self { base: Base { llm, context_extractor } }
    }
}

#[async_trait]
impl ModalProcessor for EquationModalProcessor {
    fn content_type(&self) -> &'static str {
        "equation"
    }

    async fn process(
        &self,
        block: &ContentBlock,
        index: usize,
        blocks: &[ContentBlock],
        doc_id: &str,
    ) -> Result<ProcessedModal> {
        let (latex, page_idx) = match block {
            ContentBlock::Equation { latex, page_idx } => (latex.clone(), *page_idx),
            _ => unreachable!("equation processor got {block:?}"),
        };
        let context = self.base.context(blocks, index);
        let description = self.base.describe("equation", &latex, &context, doc_id).await?;
        Ok(ProcessedModal {
            entity_name: format!("equation_{index}"),
            description,
            kind: "equation",
            page_idx,
        })
    }
}

/// Fallback processor for anything without a specialist.
pub struct GenericModalProcessor {
    base: Base,
}

impl GenericModalProcessor {
    pub fn new(llm: Arc<dyn LlmClient>, context_extractor: ContextExtractor) -> Self {
        Self { base: Base { llm, context_extractor } }
    }
}

#[async_trait]
impl ModalProcessor for GenericModalProcessor {
    fn content_type(&self) -> &'static str {
        "generic"
    }

    async fn process(
        &self,
        block: &ContentBlock,
        index: usize,
        blocks: &[ContentBlock],
        doc_id: &str,
    ) -> Result<ProcessedModal> {
        let raw = format!("{:?}", block);
        let context = self.base.context(blocks, index);
        let description = self.base.describe("content block", &raw, &context, doc_id).await?;
        Ok(ProcessedModal {
            entity_name: format!("block_{index}"),
            description,
            kind: "generic",
            page_idx: block.page_idx(),
        })
    }
}
