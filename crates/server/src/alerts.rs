//! Workspace alerts (Piece 79): case and query watches anchored to a
//! Workspace (matter). A watch is re-run periodically against the vault
//! and the web; anything cited that the watch has not seen before lands
//! as a hit for the owner. The FIRST check of a watch only establishes
//! its baseline — everything already there is marked seen and produces
//! no hits — so a new watch never floods the owner with old material.
//!
//! Persisted to `alerts.json` in the data directory, mirroring the
//! prompts/templates stores (one JSON document, atomic tmp+rename save).

use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How many citation keys a watch remembers. Once the cap is hit the
/// oldest keys fall off — a very old source could then re-fire once,
/// which is the acceptable price of a bounded store.
pub const SEEN_CAP: usize = 500;
/// Hits kept per watch (newest first when listed).
pub const HITS_PER_WATCH_CAP: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertWatch {
    pub id: String,
    pub owner_email: String,
    /// The Workspace (matter) this watch belongs to.
    pub matter_id: String,
    /// `case` | `query`
    pub kind: String,
    /// Case name / citation, or the legal question being watched.
    pub text: String,
    pub created_at: DateTime<Utc>,
    /// `None` until the first check establishes the baseline.
    #[serde(default)]
    pub last_checked_at: Option<DateTime<Utc>>,
    /// Citation keys (chunk ids) this watch has already seen.
    #[serde(default)]
    pub seen_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertHit {
    pub id: String,
    pub watch_id: String,
    /// The citation key (chunk id) that fired this hit.
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub snippet: String,
    /// Origin URL for web hits; `None` for vault hits.
    #[serde(default)]
    pub url: Option<String>,
    /// `corpus` | `web`
    pub source: String,
    pub found_at: DateTime<Utc>,
    #[serde(default)]
    pub read: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AlertData {
    watches: Vec<AlertWatch>,
    hits: Vec<AlertHit>,
}

pub struct AlertStore {
    data: RwLock<AlertData>,
    path: PathBuf,
}

impl AlertStore {
    pub fn load(path: PathBuf) -> Self {
        let data: AlertData = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            data: RwLock::new(data),
            path,
        }
    }

    fn save(&self) {
        if let Ok(data) = self.data.read() {
            if let Ok(bytes) = serde_json::to_vec(&*data) {
                let tmp = self.path.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(tmp, &self.path);
                }
            }
        }
    }

    pub fn create(
        &self,
        owner_email: &str,
        matter_id: &str,
        kind: &str,
        text: &str,
    ) -> AlertWatch {
        let watch = AlertWatch {
            id: legal_rag_common::new_id(),
            owner_email: owner_email.to_string(),
            matter_id: matter_id.to_string(),
            kind: if kind == "case" { "case" } else { "query" }.to_string(),
            text: text.trim().to_string(),
            created_at: Utc::now(),
            last_checked_at: None,
            seen_keys: Vec::new(),
        };
        {
            let mut data = self.data.write().unwrap();
            data.watches.push(watch.clone());
        }
        self.save();
        watch
    }

    pub fn get(&self, id: &str) -> Option<AlertWatch> {
        self.data
            .read()
            .unwrap()
            .watches
            .iter()
            .find(|w| w.id == id)
            .cloned()
    }

    /// Watches owned by `owner_email`, optionally narrowed to one matter.
    pub fn list_for(&self, owner_email: &str, matter_id: Option<&str>) -> Vec<AlertWatch> {
        let mut out: Vec<AlertWatch> = self
            .data
            .read()
            .unwrap()
            .watches
            .iter()
            .filter(|w| w.owner_email == owner_email)
            .filter(|w| matter_id.map(|m| w.matter_id == m).unwrap_or(true))
            .cloned()
            .collect();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        out
    }

    /// Every watch, for the background checker.
    pub fn all_watches(&self) -> Vec<AlertWatch> {
        self.data.read().unwrap().watches.clone()
    }

    /// Delete a watch (owner-scoped) together with its hits.
    pub fn delete(&self, owner_email: &str, id: &str) -> bool {
        let removed = {
            let mut data = self.data.write().unwrap();
            let before = data.watches.len();
            data.watches
                .retain(|w| !(w.id == id && w.owner_email == owner_email));
            if data.watches.len() != before {
                data.hits.retain(|h| h.watch_id != id);
                true
            } else {
                false
            }
        };
        if removed {
            self.save();
        }
        removed
    }

    /// Record a completed check: fold `observed_keys` into the watch's
    /// seen set and append `new_hits` (already filtered to unseen keys
    /// by the caller). Returns how many hits were stored.
    pub fn record_check(
        &self,
        watch_id: &str,
        observed_keys: Vec<String>,
        new_hits: Vec<AlertHit>,
    ) -> usize {
        let stored = {
            let mut data = self.data.write().unwrap();
            let Some(watch) = data.watches.iter_mut().find(|w| w.id == watch_id) else {
                return 0;
            };
            for key in observed_keys {
                if !watch.seen_keys.contains(&key) {
                    watch.seen_keys.push(key);
                }
            }
            if watch.seen_keys.len() > SEEN_CAP {
                let excess = watch.seen_keys.len() - SEEN_CAP;
                watch.seen_keys.drain(0..excess);
            }
            watch.last_checked_at = Some(Utc::now());
            let n = new_hits.len();
            data.hits.extend(new_hits);
            // Bound hits per watch, oldest first out.
            let mut mine: Vec<(DateTime<Utc>, String)> = data
                .hits
                .iter()
                .filter(|h| h.watch_id == watch_id)
                .map(|h| (h.found_at, h.id.clone()))
                .collect();
            if mine.len() > HITS_PER_WATCH_CAP {
                mine.sort();
                let excess = mine.len() - HITS_PER_WATCH_CAP;
                let drop: std::collections::HashSet<String> =
                    mine.into_iter().take(excess).map(|(_, id)| id).collect();
                data.hits.retain(|h| !drop.contains(&h.id));
            }
            n
        };
        self.save();
        stored
    }

    /// Hits for one watch, newest first.
    pub fn hits_for(&self, watch_id: &str) -> Vec<AlertHit> {
        let mut out: Vec<AlertHit> = self
            .data
            .read()
            .unwrap()
            .hits
            .iter()
            .filter(|h| h.watch_id == watch_id)
            .cloned()
            .collect();
        out.sort_by(|a, b| b.found_at.cmp(&a.found_at));
        out
    }

    pub fn unread_count(&self, owner_email: &str, matter_id: &str) -> usize {
        let data = self.data.read().unwrap();
        let watch_ids: std::collections::HashSet<&str> = data
            .watches
            .iter()
            .filter(|w| w.owner_email == owner_email && w.matter_id == matter_id)
            .map(|w| w.id.as_str())
            .collect();
        data.hits
            .iter()
            .filter(|h| watch_ids.contains(h.watch_id.as_str()) && !h.read)
            .count()
    }

    /// Mark every hit of a watch read (owner-scoped via the watch).
    pub fn mark_watch_read(&self, owner_email: &str, watch_id: &str) -> bool {
        let changed = {
            let mut data = self.data.write().unwrap();
            let owned = data
                .watches
                .iter()
                .any(|w| w.id == watch_id && w.owner_email == owner_email);
            if !owned {
                return false;
            }
            let mut any = false;
            for h in data.hits.iter_mut().filter(|h| h.watch_id == watch_id) {
                if !h.read {
                    h.read = true;
                    any = true;
                }
            }
            any
        };
        if changed {
            self.save();
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> AlertStore {
        let path = std::env::temp_dir()
            .join(format!("alerts-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        AlertStore::load(path)
    }

    fn hit(watch_id: &str, key: &str) -> AlertHit {
        AlertHit {
            id: legal_rag_common::new_id(),
            watch_id: watch_id.to_string(),
            key: key.to_string(),
            title: format!("Doc {key}"),
            snippet: "snippet".to_string(),
            url: None,
            source: "corpus".to_string(),
            found_at: Utc::now(),
            read: false,
        }
    }

    #[test]
    fn create_list_delete_are_owner_scoped() {
        let s = store();
        let w = s.create("a@test.io", "m1", "case", "State v. Rao");
        assert_eq!(s.list_for("a@test.io", None).len(), 1);
        assert_eq!(s.list_for("b@test.io", None).len(), 0);
        assert_eq!(s.list_for("a@test.io", Some("m1")).len(), 1);
        assert_eq!(s.list_for("a@test.io", Some("other")).len(), 0);
        assert!(!s.delete("b@test.io", &w.id), "not the owner");
        assert!(s.delete("a@test.io", &w.id));
        assert_eq!(s.list_for("a@test.io", None).len(), 0);
    }

    #[test]
    fn record_check_baselines_then_reports_only_new() {
        let s = store();
        let w = s.create("a@test.io", "m1", "query", "Is X maintainable?");
        // Baseline check: everything observed becomes seen, no hits stored.
        let n = s.record_check(&w.id, vec!["k1".into(), "k2".into()], vec![]);
        assert_eq!(n, 0);
        let after = s.get(&w.id).unwrap();
        assert!(after.last_checked_at.is_some());
        assert_eq!(after.seen_keys.len(), 2);
        // Second check: k2 already seen, k3 new with a hit.
        let n = s.record_check(
            &w.id,
            vec!["k2".into(), "k3".into()],
            vec![hit(&w.id, "k3")],
        );
        assert_eq!(n, 1);
        let hits = s.hits_for(&w.id);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "k3");
        assert_eq!(s.unread_count("a@test.io", "m1"), 1);
        assert!(s.mark_watch_read("a@test.io", &w.id));
        assert_eq!(s.unread_count("a@test.io", "m1"), 0);
    }

    #[test]
    fn delete_removes_hits_too() {
        let s = store();
        let w = s.create("a@test.io", "m1", "case", "Rao v. State");
        s.record_check(&w.id, vec!["k1".into()], vec![hit(&w.id, "k1")]);
        assert_eq!(s.hits_for(&w.id).len(), 1);
        assert!(s.delete("a@test.io", &w.id));
        assert_eq!(s.hits_for(&w.id).len(), 0);
    }
}
