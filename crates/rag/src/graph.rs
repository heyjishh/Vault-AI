//! Lightweight knowledge graph in the spirit of LightRAG.
//!
//! Entities and relations are extracted per chunk by the LLM and stored with
//! provenance (chunk ids), enabling graph-expanded retrieval.

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use async_trait::async_trait;
use legal_rag_common::Result;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::llm::LlmClient;
use crate::vector_store::ChunkRecord;

/// A named entity extracted from a chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub name: String,
    pub kind: String,
    pub description: String,
    pub chunk_ids: Vec<String>,
}

/// A directed relation between two entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub from: String,
    pub to: String,
    pub description: String,
    pub chunk_ids: Vec<String>,
}

/// Graph storage backend.
#[async_trait]
pub trait GraphStore: Send + Sync {
    async fn upsert_entities(&self, entities: Vec<Entity>) -> Result<()>;
    async fn upsert_relations(&self, relations: Vec<Relation>) -> Result<()>;
    async fn neighbors(&self, entity_name: &str) -> Result<Vec<(Entity, Relation)>>;
    async fn entities_in_chunks(&self, chunk_ids: &[String]) -> Result<Vec<Entity>>;
    async fn entity_count(&self) -> usize;
    async fn relation_count(&self) -> usize;
    async fn all_entities(&self) -> Result<Vec<Entity>>;
    async fn all_relations(&self) -> Result<Vec<Relation>>;
    /// Drop provenance for deleted chunks; prunes orphaned entities/relations.
    async fn remove_chunks(&self, chunk_ids: &[String]) -> Result<()>;
}

/// In-memory graph store.
#[derive(Debug, Default)]
pub struct InMemoryGraphStore {
    entities: RwLock<HashMap<String, Entity>>,
    relations: RwLock<Vec<Relation>>,
}

impl InMemoryGraphStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot (entities, relations) for disk persistence.
    pub fn snapshot(&self) -> (Vec<Entity>, Vec<Relation>) {
        let entities = self.entities.read().expect("lock poisoned").values().cloned().collect();
        let relations = self.relations.read().expect("lock poisoned").clone();
        (entities, relations)
    }

    /// Restore from a disk snapshot.
    pub fn restore(&self, entities: Vec<Entity>, relations: Vec<Relation>) {
        *self.entities.write().expect("lock poisoned") = entities
            .into_iter()
            .map(|e| (e.name.to_lowercase(), e))
            .collect();
        *self.relations.write().expect("lock poisoned") = relations;
    }

    fn merge_chunk_ids(existing: &mut Vec<String>, new: &[String]) {
        for id in new {
            if !existing.iter().any(|s| s == id) {
                existing.push(id.clone());
            }
        }
    }
}

#[async_trait]
impl GraphStore for InMemoryGraphStore {
    async fn upsert_entities(&self, entities: Vec<Entity>) -> Result<()> {
        let mut guard = self.entities.write().expect("lock poisoned");
        for entity in entities {
            let key = entity.name.to_lowercase();
            match guard.get_mut(&key) {
                Some(existing) => {
                    if existing.description.len() < entity.description.len() {
                        existing.description = entity.description.clone();
                    }
                    Self::merge_chunk_ids(&mut existing.chunk_ids, &entity.chunk_ids);
                }
                None => {
                    guard.insert(key, entity);
                }
            }
        }
        Ok(())
    }

    async fn upsert_relations(&self, relations: Vec<Relation>) -> Result<()> {
        let mut guard = self.relations.write().expect("lock poisoned");
        for relation in relations {
            let duplicate = guard.iter_mut().find(|r| {
                r.from.to_lowercase() == relation.from.to_lowercase()
                    && r.to.to_lowercase() == relation.to.to_lowercase()
            });
            match duplicate {
                Some(existing) => Self::merge_chunk_ids(&mut existing.chunk_ids, &relation.chunk_ids),
                None => guard.push(relation),
            }
        }
        Ok(())
    }

    async fn neighbors(&self, entity_name: &str) -> Result<Vec<(Entity, Relation)>> {
        let key = entity_name.to_lowercase();
        let entities = self.entities.read().expect("lock poisoned");
        let relations = self.relations.read().expect("lock poisoned");
        let mut out = Vec::new();
        for rel in relations.iter() {
            let other = if rel.from.to_lowercase() == key {
                rel.to.clone()
            } else if rel.to.to_lowercase() == key {
                rel.from.clone()
            } else {
                continue;
            };
            if let Some(entity) = entities.get(&other.to_lowercase()) {
                out.push((entity.clone(), rel.clone()));
            }
        }
        Ok(out)
    }

    async fn entities_in_chunks(&self, chunk_ids: &[String]) -> Result<Vec<Entity>> {
        let wanted: HashSet<&str> = chunk_ids.iter().map(|s| s.as_str()).collect();
        let entities = self.entities.read().expect("lock poisoned");
        Ok(entities
            .values()
            .filter(|e| e.chunk_ids.iter().any(|id| wanted.contains(id.as_str())))
            .cloned()
            .collect())
    }

    async fn entity_count(&self) -> usize {
        self.entities.read().expect("lock poisoned").len()
    }

    async fn relation_count(&self) -> usize {
        self.relations.read().expect("lock poisoned").len()
    }

    async fn all_entities(&self) -> Result<Vec<Entity>> {
        let mut entities: Vec<Entity> =
            self.entities.read().expect("lock poisoned").values().cloned().collect();
        entities.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entities)
    }

    async fn all_relations(&self) -> Result<Vec<Relation>> {
        let mut relations: Vec<Relation> =
            self.relations.read().expect("lock poisoned").clone();
        relations.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
        Ok(relations)
    }

    async fn remove_chunks(&self, chunk_ids: &[String]) -> Result<()> {
        let removed: std::collections::HashSet<&str> =
            chunk_ids.iter().map(|s| s.as_str()).collect();
        {
            let mut entities = self.entities.write().expect("lock poisoned");
            entities.retain(|_, e| {
                e.chunk_ids.retain(|id| !removed.contains(id.as_str()));
                !e.chunk_ids.is_empty()
            });
        }
        {
            let mut relations = self.relations.write().expect("lock poisoned");
            relations.retain_mut(|r| {
                r.chunk_ids.retain(|id| !removed.contains(id.as_str()));
                !r.chunk_ids.is_empty()
            });
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Extraction {
    #[serde(default)]
    entities: Vec<RawEntity>,
    #[serde(default)]
    relations: Vec<RawRelation>,
}

#[derive(Deserialize)]
struct RawEntity {
    name: String,
    #[serde(default, alias = "type")]
    kind: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct RawRelation {
    #[serde(default, alias = "subject")]
    from: String,
    #[serde(default, alias = "object")]
    to: String,
    #[serde(default, alias = "condition")]
    description: String,
}

const EXTRACTION_SYSTEM: &str = "You extract legal knowledge graph elements from contract text. \
Respond with ONLY a JSON object of the form \
{\"entities\":[{\"name\":\"...\",\"kind\":\"party|clause|date|obligation|...\",\"description\":\"...\"}],\
\"relations\":[{\"from\":\"entity name\",\"to\":\"entity name\",\"description\":\"...\"}]}.";

fn clean_json(raw: &str) -> &str {
    let t = raw.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

/// Tolerant parse of LLM-produced JSON: direct, then escape repair.
/// Shared by graph extraction and research framing/adjudication.
pub(crate) fn parse_llm_json<T>(raw: &str) -> Option<T>
where
    T: serde::de::DeserializeOwned,
{
    let cleaned = clean_json(raw);
    if let Ok(v) = serde_json::from_str::<T>(cleaned) {
        return Some(v);
    }
    let repaired = repair_escapes(cleaned);
    serde_json::from_str::<T>(&repaired).ok()
}

/// Best-effort parse of LLM extraction JSON: tolerant parse first, then
/// truncation repair (cut at the last complete element, close brackets).
fn parse_extraction(raw: &str) -> Option<Extraction> {
    if let Some(e) = parse_llm_json::<Extraction>(raw) {
        return Some(e);
    }
    let repaired = repair_escapes(clean_json(raw));
    // Truncation: keep everything through the last complete `}` element
    // (dropping the trailing comma), then close the array and object.
    if let Some(idx) = repaired.rfind("},") {
        let mut s = repaired[..=idx].to_string();
        s.push_str("]}");
        if let Ok(e) = serde_json::from_str::<Extraction>(&s) {
            return Some(e);
        }
    }
    None
}

/// Escape stray backslashes so `\X` (invalid JSON escapes from the model)
/// becomes `\\X` instead of failing the parse.
pub(crate) fn repair_escapes(s: &str) -> String {
    const VALID: &str = "\"\\/bfnrtu";
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some(&n) if VALID.contains(n) => {
                    out.push(c);
                    out.push(chars.next().unwrap_or(n));
                }
                _ => out.push_str("\\\\"),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Extract entities/relations for one chunk via the LLM. Best-effort: parse
/// failures yield empty vecs rather than failing indexing.
pub async fn extract_graph_elements(
    llm: &dyn LlmClient,
    chunk: &ChunkRecord,
) -> (Vec<Entity>, Vec<Relation>) {
    let user = format!("Extract entities and relations from this legal text:\n\n{}", chunk.text);
    let raw = match llm.complete(EXTRACTION_SYSTEM, &user).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "graph extraction llm call failed");
            return (Vec::new(), Vec::new());
        }
    };
    let parsed: Extraction = match parse_extraction(&raw) {
        Some(p) => p,
        None => {
            warn!("graph extraction parse failed");
            return (Vec::new(), Vec::new());
        }
    };
    let entities = parsed
        .entities
        .into_iter()
        .map(|e| Entity {
            name: e.name,
            kind: if e.kind.is_empty() { "unknown".into() } else { e.kind },
            description: e.description,
            chunk_ids: vec![chunk.id.clone()],
        })
        .collect();
    let relations = parsed
        .relations
        .into_iter()
        .filter(|r| !r.from.trim().is_empty() && !r.to.trim().is_empty())
        .map(|r| Relation {
            from: r.from,
            to: r.to,
            description: r.description,
            chunk_ids: vec![chunk.id.clone()],
        })
        .collect();
    (entities, relations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlm;

    #[tokio::test]
    async fn graph_upsert_and_neighbors() {
        let g = InMemoryGraphStore::new();
        g.upsert_entities(vec![Entity {
            name: "Acme Corp".into(),
            kind: "party".into(),
            description: "buyer".into(),
            chunk_ids: vec!["c1".into()],
        }])
        .await
        .unwrap();
        g.upsert_entities(vec![Entity {
            name: "Globex".into(),
            kind: "party".into(),
            description: "seller".into(),
            chunk_ids: vec!["c1".into()],
        }])
        .await
        .unwrap();
        g.upsert_relations(vec![Relation {
            from: "Acme Corp".into(),
            to: "Globex".into(),
            description: "purchases from".into(),
            chunk_ids: vec!["c1".into()],
        }])
        .await
        .unwrap();
        let neighbors = g.neighbors("acme corp").await.unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].0.name, "Globex");
        assert_eq!(g.entity_count().await, 2);
    }

    #[tokio::test]
    async fn extraction_parses_llm_json() {
        let llm = MockLlm::new(
            r#"{"entities":[{"name":"Acme","kind":"party","description":"buyer"}],"relations":[{"from":"Acme","to":"Beta","description":"pays"}]}"#,
        );
        let chunk = ChunkRecord {
            id: "c1".into(),
            doc_id: "d1".into(),
            ordinal: 0,
            text: "Acme pays Beta.".into(),
            embedding: vec![],
            metadata: Default::default(),
        };
        let (entities, relations) = extract_graph_elements(&llm, &chunk).await;
        assert_eq!(entities.len(), 1);
        assert_eq!(relations.len(), 1);
        assert_eq!(entities[0].chunk_ids, vec!["c1".to_string()]);
    }

    #[tokio::test]
    async fn extraction_accepts_alternative_schema() {
        // Some models emit type/subject/object instead of kind/from/to.
        let llm = MockLlm::new(
            "```json\n{\"entities\":[{\"type\":\"Gov\",\"name\":\"Central Government\"}],\
             \"relations\":[{\"subject\":\"Assessee\",\"object\":\"Income-tax\",\"condition\":\"on securities\"}]}\n```",
        );
        let chunk = ChunkRecord {
            id: "c1".into(),
            doc_id: "d1".into(),
            ordinal: 0,
            text: "text".into(),
            embedding: vec![],
            metadata: Default::default(),
        };
        let (entities, relations) = extract_graph_elements(&llm, &chunk).await;
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].kind, "Gov");
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].from, "Assessee");
        assert_eq!(relations[0].to, "Income-tax");
    }

    #[test]
    fn parse_extraction_repairs_truncation() {
        let truncated = r#"{"entities":[{"name":"A","kind":"party","description":"x"},{"name":"B","kind":"party","descri"#;
        let e = parse_extraction(truncated).expect("truncation repair should yield partial");
        assert_eq!(e.entities.len(), 1);
        assert_eq!(e.entities[0].name, "A");
    }

    #[test]
    fn parse_extraction_repairs_bad_escapes() {
        let raw = r#"{"entities":[{"name":"A","kind":"party","description":"100\% sure"}],"relations":[]}"#;
        let e = parse_extraction(raw).expect("escape repair should parse");
        assert_eq!(e.entities[0].description, "100\\% sure");
    }
}
