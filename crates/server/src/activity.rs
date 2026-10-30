//! Activity capture: every user question, research run, workflow, draft,
//! interaction and upload is recorded here. It powers the user dashboard
//! ("what you researched"), the admin insights, and the golden ruleset
//! feedback loop (real queries become evaluation rules). Persisted to
//! `activity.json` in the data directory.

use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const MAX_EVENTS: usize = 5000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub id: String,
    pub user_id: String,
    pub user_email: String,
    /// query | research | workflow | interact | draft | upload
    pub kind: String,
    /// The question, workflow input, draft title, or document name.
    pub title: String,
    /// Mode, workflow id, interact action, draft kind… ("" when n/a).
    pub detail: String,
    /// Matter this work belongs to, when scoped.
    #[serde(default)]
    pub matter_id: Option<String>,
    /// Short excerpt of the produced answer/output.
    pub result_excerpt: String,
    /// Full restorable result (answer + citations, research report,
    /// workflow run, draft…) so a history click can reopen the exact
    /// state instead of just the page. `None` for uploads and for
    /// private-vault queries, whose content never enters this log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    pub citations: usize,
    pub tokens: u64,
    pub created_at: DateTime<Utc>,
}

impl ActivityEvent {
    pub fn new(
        user_id: &str,
        user_email: &str,
        kind: &str,
        title: &str,
        detail: &str,
        result: &str,
        citations: usize,
        tokens: u64,
    ) -> Self {
        Self {
            id: legal_rag_common::new_id(),
            user_id: user_id.to_string(),
            user_email: user_email.to_string(),
            kind: kind.to_string(),
            title: excerpt(title, 300),
            detail: detail.to_string(),
            matter_id: None,
            result_excerpt: excerpt(result, 400),
            payload: None,
            citations,
            tokens,
            created_at: Utc::now(),
        }
    }

    /// Tag this event with the matter it belongs to.
    pub fn with_matter(mut self, matter_id: Option<String>) -> Self {
        self.matter_id = matter_id;
        self
    }

    /// Attach the full restorable result. Oversized payloads are dropped
    /// rather than bloating the log (the excerpt still records the work).
    pub fn with_payload(mut self, payload: Option<serde_json::Value>) -> Self {
        self.payload = payload.filter(|p| p.to_string().len() <= 48_000);
        self
    }
}

/// First `n` chars of `s`, on a char boundary.
pub fn excerpt(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}

pub struct ActivityStore {
    events: RwLock<Vec<ActivityEvent>>,
    path: PathBuf,
}

impl ActivityStore {
    pub fn load(path: PathBuf) -> Self {
        let events: Vec<ActivityEvent> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { events: RwLock::new(events), path }
    }

    fn save(&self) {
        if let Ok(events) = self.events.read() {
            if let Ok(bytes) = serde_json::to_vec(&*events) {
                let tmp = self.path.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(tmp, &self.path);
                }
            }
        }
    }

    pub fn record(&self, event: ActivityEvent) {
        {
            let mut events = self.events.write().unwrap();
            events.insert(0, event);
            events.truncate(MAX_EVENTS);
        }
        self.save();
    }

    /// Events belonging to one matter, newest first (its living history).
    pub fn for_matter(&self, matter_id: &str, limit: usize) -> Vec<ActivityEvent> {
        self.events
            .read()
            .unwrap()
            .iter()
            .filter(|e| e.matter_id.as_deref() == Some(matter_id))
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn for_user(&self, user_id: &str, limit: usize) -> Vec<ActivityEvent> {
        self.events
            .read()
            .unwrap()
            .iter()
            .filter(|e| e.user_id == user_id)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn recent(&self, limit: usize) -> Vec<ActivityEvent> {
        self.events.read().unwrap().iter().take(limit).cloned().collect()
    }

    pub fn all(&self) -> Vec<ActivityEvent> {
        self.events.read().unwrap().clone()
    }

    pub fn by_id(&self, id: &str) -> Option<ActivityEvent> {
        self.events.read().unwrap().iter().find(|e| e.id == id).cloned()
    }

    /// Kind of the system record written when an admin purges a
    /// user's data. Such records are SYSTEM data: the purge never
    /// removes them (the audit trail of the purge itself must stay),
    /// whoever authored them.
    pub const PURGE_KIND: &str = "admin_purge";

    /// How many of a user's entries a purge would remove (their
    /// history, excluding system purge records).
    pub fn count_for_user(&self, user_id: &str) -> usize {
        self.events
            .read()
            .unwrap()
            .iter()
            .filter(|e| e.user_id == user_id && e.kind != Self::PURGE_KIND)
            .count()
    }

    /// Remove a user's history (admin user-data purge). Returns how
    /// many entries were removed. System purge records and every
    /// other user's entries are preserved. Idempotent: a second call
    /// removes nothing.
    pub fn remove_for_user(&self, user_id: &str) -> usize {
        let removed = {
            let mut events = self.events.write().unwrap();
            let before = events.len();
            events.retain(|e| e.user_id != user_id || e.kind == Self::PURGE_KIND);
            before - events.len()
        };
        if removed > 0 {
            self.save();
        }
        removed
    }

    /// Remove ALL history entries belonging to one matter (Brief) —
    /// used when the brief is deleted: its Ask / Interact / research /
    /// draft / upload history goes with it, so a deleted brief leaves
    /// nothing restorable behind. A brief is a shared scope, so the
    /// removal crosses users. System purge records
    /// (`PURGE_KIND`) are never removed, even when tagged to the
    /// matter — that audit trail is system data. Unscoped events and
    /// other matters' events are untouched. Billing, token-usage and
    /// payment records live in other stores and are NOT touched here —
    /// only this history store. Returns how many entries were removed.
    /// Idempotent: a second call removes nothing.
    pub fn remove_for_matter(&self, matter_id: &str) -> usize {
        let removed = {
            let mut events = self.events.write().unwrap();
            let before = events.len();
            events.retain(|e| {
                e.matter_id.as_deref() != Some(matter_id) || e.kind == Self::PURGE_KIND
            });
            before - events.len()
        };
        if removed > 0 {
            self.save();
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (ActivityStore, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "activity-test-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        (ActivityStore::load(path.clone()), path)
    }

    fn ev(user: &str, kind: &str) -> ActivityEvent {
        ActivityEvent::new(user, "u@example.test", kind, "t", "", "r", 0, 0)
    }

    /// The purge contract: a user's own history goes; system purge
    /// records and other users' entries stay; the operation is
    /// idempotent and the count preview matches the removal.
    #[test]
    fn remove_for_user_keeps_system_records_and_other_users() {
        let (store, path) = temp_store();
        store.record(ev("user-b", "query"));
        store.record(ev("user-b", "upload"));
        store.record(ev("user-b", ActivityStore::PURGE_KIND));
        store.record(ev("admin-1", "query"));
        store.record(ev("admin-1", ActivityStore::PURGE_KIND));

        assert_eq!(store.count_for_user("user-b"), 2);
        assert_eq!(store.remove_for_user("user-b"), 2);

        let remaining = store.all();
        // user-b's purge record (system data) + both admin entries.
        assert_eq!(remaining.len(), 3);
        assert!(remaining
            .iter()
            .all(|e| e.user_id != "user-b" || e.kind == ActivityStore::PURGE_KIND));
        assert_eq!(
            remaining
                .iter()
                .filter(|e| e.user_id == "admin-1")
                .count(),
            2
        );

        // Idempotent, and persisted to disk.
        assert_eq!(store.remove_for_user("user-b"), 0);
        let reloaded = ActivityStore::load(path.clone());
        assert_eq!(reloaded.all().len(), 3);
        let _ = std::fs::remove_file(&path);
    }

    /// The brief-delete contract (Piece 27): deleting a brief removes
    /// every history entry tagged to it, across users — but never a
    /// system purge record, never an unscoped entry, and never another
    /// brief's history. Idempotent and persisted.
    #[test]
    fn remove_for_matter_takes_only_that_matters_history() {
        let (store, path) = temp_store();
        store.record(ev("user-a", "query").with_matter(Some("matter-1".into())));
        store.record(ev("user-b", "interact").with_matter(Some("matter-1".into())));
        store.record(ev("user-a", "research").with_matter(Some("matter-2".into())));
        store.record(ev("user-a", "query")); // unscoped
        store
            .record(ev("admin-1", ActivityStore::PURGE_KIND).with_matter(Some("matter-1".into())));

        assert_eq!(store.remove_for_matter("matter-1"), 2);

        let remaining = store.all();
        assert_eq!(remaining.len(), 3);
        assert!(remaining
            .iter()
            .all(|e| e.matter_id.as_deref() != Some("matter-1")
                || e.kind == ActivityStore::PURGE_KIND));
        assert_eq!(
            remaining
                .iter()
                .filter(|e| e.matter_id.as_deref() == Some("matter-2"))
                .count(),
            1
        );
        assert_eq!(remaining.iter().filter(|e| e.matter_id.is_none()).count(), 1);

        // Idempotent, and persisted to disk.
        assert_eq!(store.remove_for_matter("matter-1"), 0);
        let reloaded = ActivityStore::load(path.clone());
        assert_eq!(reloaded.all().len(), 3);
        let _ = std::fs::remove_file(&path);
    }
}
