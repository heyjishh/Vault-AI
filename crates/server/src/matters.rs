//! Matters: the workspace unit of Juris Vault AI. Every document, question,
//! interaction and draft can belong to a matter, and the matter accumulates
//! a living memory — its documents, the entities they mention, and the full
//! activity history scoped to it. Persisted to `matters.json` in the data
//! directory, mirroring the activity/billing stores.

use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Matter {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub client: String,
    /// Matter type: Litigation, Corporate, Tax, Advisory… (free text).
    #[serde(default)]
    pub kind: String,
    /// Active | On hold | Closed
    #[serde(default = "default_status")]
    pub status: String,
    /// Free-form matter notes — part of the living memory.
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub document_ids: Vec<String>,
    #[serde(default)]
    pub is_default: bool,
    /// Owning Space (multi-tenant container). `None` = legacy brief with
    /// no space boundary — visible to every authenticated user.
    #[serde(default)]
    pub space_id: Option<String>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_status() -> String {
    "Active".to_string()
}

pub struct MatterStore {
    matters: RwLock<Vec<Matter>>,
    path: PathBuf,
}

impl MatterStore {
    pub fn load(path: PathBuf) -> Self {
        let matters: Vec<Matter> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            matters: RwLock::new(matters),
            path,
        }
    }

    fn save(&self) {
        if let Ok(matters) = self.matters.read() {
            if let Ok(bytes) = serde_json::to_vec(&*matters) {
                let tmp = self.path.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(tmp, &self.path);
                }
            }
        }
    }

    pub fn list(&self) -> Vec<Matter> {
        let mut out = self.matters.read().unwrap().clone();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    pub fn get(&self, id: &str) -> Option<Matter> {
        self.matters
            .read()
            .unwrap()
            .iter()
            .find(|m| m.id == id)
            .cloned()
    }

    pub fn default_id(&self) -> Option<String> {
        self.matters
            .read()
            .unwrap()
            .iter()
            .find(|m| m.is_default)
            .map(|m| m.id.clone())
    }

    pub fn create(
        &self,
        title: &str,
        client: &str,
        kind: &str,
        notes: &str,
        created_by: &str,
        space_id: Option<String>,
    ) -> Matter {
        let now = Utc::now();
        let matter = Matter {
            id: legal_rag_common::new_id(),
            title: title.to_string(),
            client: client.to_string(),
            kind: kind.to_string(),
            status: "Active".to_string(),
            notes: notes.to_string(),
            document_ids: Vec::new(),
            is_default: false,
            space_id,
            created_by: created_by.to_string(),
            created_at: now,
            updated_at: now,
        };
        self.matters.write().unwrap().push(matter.clone());
        self.save();
        matter
    }

    /// Partial update; returns the updated matter. `space` moves the
    /// brief between spaces: outer `None` = unchanged, inner `None` =
    /// move out of any space.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &self,
        id: &str,
        title: Option<String>,
        client: Option<String>,
        kind: Option<String>,
        status: Option<String>,
        notes: Option<String>,
        space: Option<Option<String>>,
    ) -> Option<Matter> {
        let mut matters = self.matters.write().unwrap();
        let matter = matters.iter_mut().find(|m| m.id == id)?;
        if let Some(v) = space {
            matter.space_id = v;
        }
        if let Some(v) = title {
            matter.title = v;
        }
        if let Some(v) = client {
            matter.client = v;
        }
        if let Some(v) = kind {
            matter.kind = v;
        }
        if let Some(v) = status {
            matter.status = v;
        }
        if let Some(v) = notes {
            matter.notes = v;
        }
        matter.updated_at = Utc::now();
        let out = matter.clone();
        drop(matters);
        self.save();
        Some(out)
    }

    pub fn delete(&self, id: &str) -> bool {
        let mut matters = self.matters.write().unwrap();
        let before = matters.len();
        matters.retain(|m| m.id != id);
        let removed = matters.len() != before;
        drop(matters);
        if removed {
            self.save();
        }
        removed
    }

    pub fn add_document(&self, id: &str, doc_id: &str) -> Option<Matter> {
        let mut matters = self.matters.write().unwrap();
        let matter = matters.iter_mut().find(|m| m.id == id)?;
        if !matter.document_ids.iter().any(|d| d == doc_id) {
            matter.document_ids.push(doc_id.to_string());
        }
        matter.updated_at = Utc::now();
        let out = matter.clone();
        drop(matters);
        self.save();
        Some(out)
    }

    pub fn remove_document(&self, id: &str, doc_id: &str) -> Option<Matter> {
        let mut matters = self.matters.write().unwrap();
        let matter = matters.iter_mut().find(|m| m.id == id)?;
        matter.document_ids.retain(|d| d != doc_id);
        matter.updated_at = Utc::now();
        let out = matter.clone();
        drop(matters);
        self.save();
        Some(out)
    }

    /// Which matter (if any) a document belongs to.
    pub fn matter_of_document(&self, doc_id: &str) -> Option<String> {
        self.matters
            .read()
            .unwrap()
            .iter()
            .find(|m| m.document_ids.iter().any(|d| d == doc_id))
            .map(|m| m.id.clone())
    }

    /// Move every brief out of a space (used when the space is deleted):
    /// the briefs survive as legacy, spaceless briefs.
    pub fn clear_space(&self, space_id: &str) {
        let mut matters = self.matters.write().unwrap();
        let mut touched = false;
        for m in matters.iter_mut() {
            if m.space_id.as_deref() == Some(space_id) {
                m.space_id = None;
                m.updated_at = Utc::now();
                touched = true;
            }
        }
        drop(matters);
        if touched {
            self.save();
        }
    }

    /// Every matter that files this document.
    pub fn matters_of_document(&self, doc_id: &str) -> Vec<Matter> {
        self.matters
            .read()
            .unwrap()
            .iter()
            .filter(|m| m.document_ids.iter().any(|d| d == doc_id))
            .cloned()
            .collect()
    }

    /// Boot migration: guarantee a default matter exists and adopt every
    /// already-indexed document that belongs to no matter. Returns the
    /// default matter id.
    pub fn ensure_default(&self, existing_doc_ids: &[String]) -> String {
        let default_id = match self.default_id() {
            Some(id) => id,
            None => {
                let now = Utc::now();
                let matter = Matter {
                    id: legal_rag_common::new_id(),
                    title: "General".to_string(),
                    client: String::new(),
                    kind: "General".to_string(),
                    status: "Active".to_string(),
                    notes: "Default workspace for documents not filed under a specific brief."
                        .to_string(),
                    document_ids: Vec::new(),
                    is_default: true,
                    space_id: None,
                    created_by: "system".to_string(),
                    created_at: now,
                    updated_at: now,
                };
                let id = matter.id.clone();
                self.matters.write().unwrap().push(matter);
                id
            }
        };
        for doc_id in existing_doc_ids {
            if self.matter_of_document(doc_id).is_none() {
                self.add_document(&default_id, doc_id);
            }
        }
        self.save();
        default_id
    }
}
