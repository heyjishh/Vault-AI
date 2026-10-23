//! Recursive character chunking with overlap.

use legal_rag_common::new_id;
use serde::{Deserialize, Serialize};

/// A chunk of a document, the unit of embedding and retrieval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: String,
    pub doc_id: String,
    pub ordinal: usize,
    pub text: String,
    pub char_start: usize,
    pub char_end: usize,
}

/// Splits text on paragraph/sentence/word boundaries with overlap.
#[derive(Debug, Clone)]
pub struct Chunker {
    pub chunk_size: usize,
    pub chunk_overlap: usize,
}

impl Chunker {
    pub fn new(chunk_size: usize, chunk_overlap: usize) -> Self {
        Self { chunk_size: chunk_size.max(64), chunk_overlap: chunk_overlap.min(chunk_size / 2) }
    }

    pub fn chunk(&self, doc_id: &str, text: &str) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        let mut start = 0usize;
        let mut ordinal = 0usize;
        while start < chars.len() {
            let mut end = (start + self.chunk_size).min(chars.len());
            if end < chars.len() {
                // Prefer a clean boundary: paragraph, sentence, then word.
                let window = &chars[start..end];
                let boundary = window
                    .iter()
                    .rposition(|&c| c == '\n')
                    .or_else(|| window.iter().rposition(|&c| c == '.' || c == '!' || c == '?'))
                    .or_else(|| window.iter().rposition(|&c| c == ' '))
                    .map(|i| start + i + 1);
                if let Some(b) = boundary {
                    if b > start + self.chunk_size / 4 {
                        end = b;
                    }
                }
            }
            let piece: String = chars[start..end].iter().collect();
            let piece = piece.trim();
            if !piece.is_empty() {
                chunks.push(Chunk {
                    id: new_id(),
                    doc_id: doc_id.to_string(),
                    ordinal,
                    text: piece.to_string(),
                    char_start: start,
                    char_end: end,
                });
                ordinal += 1;
            }
            if end >= chars.len() {
                break;
            }
            start = end.saturating_sub(self.chunk_overlap);
        }
        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_with_overlap_and_boundaries() {
        let text = "Para one is here.\n\nPara two follows with more words to fill the chunker up nicely.\n\nPara three ends it.";
        let chunker = Chunker::new(60, 15);
        let chunks = chunker.chunk("doc1", text);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].ordinal, 0);
        assert_eq!(chunks[0].doc_id, "doc1");
        // Full coverage: first starts at 0, last ends at text length.
        assert_eq!(chunks[0].char_start, 0);
        assert_eq!(chunks.last().unwrap().char_end, text.chars().count());
    }

    #[test]
    fn empty_text_yields_no_chunks() {
        assert!(Chunker::new(100, 10).chunk("d", "").is_empty());
    }
}
