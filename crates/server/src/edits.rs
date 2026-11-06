//! Document edit proposals and version lineage (Piece 85).
//!
//! An agent-proposed document edit is a durable, owner-scoped record
//! with the same lifecycle as a Piece 80 plan: proposed (with the
//! edited text and a passage diff held for review), then APPROVED —
//! written back as a new version — or REJECTED and discarded. The
//! record doubles as the version lineage and the provenance receipt:
//! every approved edit names its lineage root, its version number,
//! the instruction, the approver, and what the write-back actually
//! did (format regenerated, text fallback, private re-encryption).
//!
//! Persisted to `document_edits.json` in the data directory, mirroring
//! the runs/prompts stores (one JSON document, atomic tmp+rename).
//! The pending edited text is held ONLY while the edit is pending;
//! deciding (either way) drops it — after approval the new document
//! itself holds the text.

use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Records kept (newest survive).
pub const EDITS_CAP: usize = 200;
/// A pending edit's stored text cap — generous for real documents,
/// bounded on disk.
pub const EDIT_TEXT_CAP: usize = 1_000_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditRecord {
    pub id: String,
    pub owner_email: String,
    pub owner_id: String,
    /// shared | private
    pub source: String,
    /// The document version this edit applies to.
    pub doc_id: String,
    /// Lineage root: the first version's document id. An edit of v1
    /// has root == doc_id; an edit of a later version keeps the root.
    pub root_doc_id: String,
    pub doc_title: String,
    pub instruction: String,
    /// pending | approved | rejected
    pub status: String,
    /// The proposed edited text; held only while pending.
    #[serde(default)]
    pub edited_text: String,
    pub diff: legal_rag_rag::PassageDiff,
    /// Run-history id of the run recorded for the proposal.
    #[serde(default)]
    pub run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub decided_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub approver: Option<String>,
    /// On approval: the NEW document created from the edited text.
    #[serde(default)]
    pub result_doc_id: Option<String>,
    /// On approval: the new version's number in the lineage (2, 3, …).
    #[serde(default)]
    pub version: Option<usize>,
    /// On approval: what the write-back did, in one human line.
    #[serde(default)]
    pub receipt: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct EditData {
    edits: Vec<EditRecord>,
}

pub struct EditStore {
    data: RwLock<EditData>,
    path: PathBuf,
}

fn cap_text(s: &str) -> String {
    if s.chars().count() <= EDIT_TEXT_CAP {
        s.to_string()
    } else {
        s.chars().take(EDIT_TEXT_CAP).collect()
    }
}

impl EditStore {
    pub fn load(path: PathBuf) -> Self {
        let data: EditData = std::fs::read(&path)
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

    /// Record a new pending edit; returns the stored record.
    #[allow(clippy::too_many_arguments)]
    pub fn propose(
        &self,
        owner_email: &str,
        owner_id: &str,
        source: &str,
        doc_id: &str,
        root_doc_id: &str,
        doc_title: &str,
        instruction: &str,
        edited_text: &str,
        diff: legal_rag_rag::PassageDiff,
    ) -> EditRecord {
        let record = EditRecord {
            id: legal_rag_common::new_id(),
            owner_email: owner_email.to_string(),
            owner_id: owner_id.to_string(),
            source: source.to_string(),
            doc_id: doc_id.to_string(),
            root_doc_id: root_doc_id.to_string(),
            doc_title: doc_title.to_string(),
            instruction: instruction.to_string(),
            status: "pending".to_string(),
            edited_text: cap_text(edited_text),
            diff,
            run_id: None,
            created_at: Utc::now(),
            decided_at: None,
            approver: None,
            result_doc_id: None,
            version: None,
            receipt: None,
        };
        {
            let mut data = self.data.write().unwrap();
            data.edits.push(record.clone());
            if data.edits.len() > EDITS_CAP {
                let excess = data.edits.len() - EDITS_CAP;
                data.edits.drain(0..excess);
            }
        }
        self.save();
        record
    }

    /// Attach the proposal's run-history id (recorded just after the
    /// proposal, once the run store has minted it).
    pub fn set_run_id(&self, id: &str, run_id: &str) {
        let mut data = self.data.write().unwrap();
        if let Some(e) = data.edits.iter_mut().find(|e| e.id == id) {
            e.run_id = Some(run_id.to_string());
            drop(data);
            self.save();
        }
    }

    /// One record, owner-checked.
    pub fn get(&self, owner_email: &str, id: &str) -> Option<EditRecord> {
        let data = self.data.read().unwrap();
        data.edits
            .iter()
            .find(|e| e.id == id && e.owner_email == owner_email)
            .cloned()
    }

    /// The owner's edits, newest first; `status` filters when given.
    pub fn list_for(&self, owner_email: &str, status: Option<&str>) -> Vec<EditRecord> {
        let data = self.data.read().unwrap();
        data.edits
            .iter()
            .rev()
            .filter(|e| e.owner_email == owner_email)
            .filter(|e| status.map(|s| e.status == s).unwrap_or(true))
            .take(100)
            .cloned()
            .collect()
    }

    /// The lineage root of a document: if this document is the result
    /// of an approved edit, that edit's root; otherwise the document
    /// is its own root.
    pub fn root_of(&self, doc_id: &str) -> String {
        let data = self.data.read().unwrap();
        data.edits
            .iter()
            .find(|e| e.result_doc_id.as_deref() == Some(doc_id) && e.status == "approved")
            .map(|e| e.root_doc_id.clone())
            .unwrap_or_else(|| doc_id.to_string())
    }

    /// The next version number for a lineage (approved edits + 2:
    /// the root is v1).
    pub fn next_version(&self, root_doc_id: &str) -> usize {
        let data = self.data.read().unwrap();
        data.edits
            .iter()
            .filter(|e| e.root_doc_id == root_doc_id && e.status == "approved")
            .count()
            + 2
    }

    /// Approved edits of a lineage, oldest first (version order).
    pub fn lineage(&self, root_doc_id: &str) -> Vec<EditRecord> {
        let data = self.data.read().unwrap();
        let mut out: Vec<EditRecord> = data
            .edits
            .iter()
            .filter(|e| e.root_doc_id == root_doc_id && e.status == "approved")
            .cloned()
            .collect();
        out.sort_by_key(|e| e.version.unwrap_or(0));
        out
    }

    /// Decide a pending edit. On approval the caller has ALREADY
    /// written the new version and passes its id, version, and
    /// receipt. Either way the held edited text is dropped. Returns
    /// the updated record, or None when the edit is not the owner's
    /// or no longer pending.
    pub fn decide(
        &self,
        owner_email: &str,
        id: &str,
        approver: &str,
        approved: bool,
        outcome: Option<(String, usize, String)>,
    ) -> Option<EditRecord> {
        let mut data = self.data.write().unwrap();
        let e = data
            .edits
            .iter_mut()
            .find(|e| e.id == id && e.owner_email == owner_email && e.status == "pending")?;
        e.status = if approved { "approved" } else { "rejected" }.to_string();
        e.decided_at = Some(Utc::now());
        e.approver = Some(approver.to_string());
        if approved && outcome.is_none() {
            // Approved with no write-back outcome: the edit made no
            // textual change, so no new version was created.
            e.receipt = Some(
                "Approved; the edit made no textual change, so no new version was created.".to_string(),
            );
        }
        if let Some((result_doc_id, version, receipt)) = outcome {
            e.result_doc_id = Some(result_doc_id);
            e.version = Some(version);
            e.receipt = Some(receipt);
        }
        e.edited_text = String::new();
        let out = e.clone();
        drop(data);
        self.save();
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> EditStore {
        let name = std::thread::current().name().unwrap_or("t").replace(|c: char| !c.is_alphanumeric(), "_");
        EditStore::load(PathBuf::from(format!("/tmp/legal-rag-edits-test-{}-{name}.json", std::process::id())))
    }

    fn diff() -> legal_rag_rag::PassageDiff {
        legal_rag_rag::compute_diff("a\n\nb", "a\n\nc")
    }

    #[test]
    fn propose_decide_and_lineage() {
        let s = store();
        let p = s.propose("a@x.test", "u1", "shared", "doc-1", "doc-1", "Note", "change b", "a\n\nc", diff());
        assert_eq!(p.status, "pending");
        assert_eq!(s.next_version("doc-1"), 2);
        // Another owner's decide must not touch it.
        assert!(s.decide("b@x.test", &p.id, "b@x.test", true, None).is_none());
        let done = s
            .decide("a@x.test", &p.id, "a@x.test", true, Some(("doc-2".into(), 2, "receipt".into())))
            .unwrap();
        assert_eq!(done.status, "approved");
        assert_eq!(done.result_doc_id.as_deref(), Some("doc-2"));
        assert!(done.edited_text.is_empty(), "held text is dropped once decided");
        // Lineage: doc-2's root is doc-1; the lineage's next version is 3.
        assert_eq!(s.root_of("doc-2"), "doc-1");
        assert_eq!(s.root_of("doc-1"), "doc-1");
        assert_eq!(s.next_version("doc-1"), 3);
        assert_eq!(s.lineage("doc-1").len(), 1);
        // A decided edit cannot be decided again.
        assert!(s.decide("a@x.test", &p.id, "a@x.test", false, None).is_none());
    }

    #[test]
    fn rejection_discards_and_records_the_outcome() {
        let s = store();
        let p = s.propose("a@x.test", "u1", "private", "pv-1", "pv-1", "Secret", "fix", "new text", diff());
        let done = s.decide("a@x.test", &p.id, "a@x.test", false, None).unwrap();
        assert_eq!(done.status, "rejected");
        assert!(done.result_doc_id.is_none());
        assert!(done.edited_text.is_empty());
        assert_eq!(s.list_for("a@x.test", Some("rejected")).len(), 1);
        assert_eq!(s.list_for("a@x.test", Some("pending")).len(), 0);
    }

    #[test]
    fn private_vault_locked_gate() {
        // The route's private path gates on PrivateVault::doc_text:
        // locked → refusal; unlocked → the decrypted text.
        let dir = std::env::temp_dir().join(format!("jv-edit-gate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let v = crate::private_vault::PrivateVault::load(dir.join("private_vault.json"));
        v.set_password("u1", "correct horse").unwrap();
        let meta = v
            .add_doc("u1", "Secret Deed", crate::private_vault::sha256_hex(b"deed"), vec![
                ("the earnest money clause".to_string(), vec![1.0, 0.0]),
            ])
            .unwrap();
        let (title, text) = v.doc_text("u1", &meta.id).unwrap();
        assert_eq!(title, "Secret Deed");
        assert!(text.contains("earnest money"));
        v.lock("u1");
        let err = v.doc_text("u1", &meta.id).unwrap_err();
        assert!(err.contains("locked"), "{err}");
    }
}
