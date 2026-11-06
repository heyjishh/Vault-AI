//! Agent run history (Piece 80). Workflow runs and autonomous runs
//! (orchestrated and legacy) used to exist only inside the activity
//! log's payloads; this store keeps a durable, owner-scoped record
//! of every run — what was asked, how it ended, what it cost, and
//! the report it produced — behind the Agents page's History view.
//!
//! Persisted to `agent_runs.json` in the data directory, mirroring
//! the alerts/prompts stores (one JSON document, atomic tmp+rename).

use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Records kept (newest survive; a busy team's history is bounded).
pub const RUNS_CAP: usize = 500;
/// Stored report cap — enough for any real report, bounded on disk.
pub const REPORT_CAP: usize = 60_000;
/// Stored input cap.
pub const INPUT_CAP: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub owner_email: String,
    /// workflow | autonomous | autonomous_orchestrated |
    /// autonomous_tools (Piece 84) | document_edit (Piece 85)
    pub kind: String,
    /// Workflow name, or the autonomous mode label.
    pub name: String,
    pub input: String,
    /// completed | budget | budget_tokens | partial | drafter_error | failed
    pub status: String,
    pub steps: usize,
    /// Provider-reported tokens for the run (the billed total).
    pub tokens: u64,
    pub report: String,
    #[serde(default)]
    pub error: Option<String>,
    /// Piece 84: the tool-call trace of a tool-agent run (empty for
    /// every other run kind). Persisted with the record so the Agents
    /// UI's run view can show exactly what the agent called.
    #[serde(default)]
    pub trace: Vec<legal_rag_rag::ToolTraceEntry>,
    pub created_at: DateTime<Utc>,
}

/// A run without its report — the History list row.
#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub input: String,
    pub status: String,
    pub steps: usize,
    pub tokens: u64,
    pub created_at: DateTime<Utc>,
}

impl From<&RunRecord> for RunSummary {
    fn from(r: &RunRecord) -> Self {
        Self {
            id: r.id.clone(),
            kind: r.kind.clone(),
            name: r.name.clone(),
            input: r.input.clone(),
            status: r.status.clone(),
            steps: r.steps,
            tokens: r.tokens,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RunData {
    runs: Vec<RunRecord>,
}

pub struct RunStore {
    data: RwLock<RunData>,
    path: PathBuf,
}

/// Map an engine's stopped string onto a history status.
pub fn status_from_stopped(stopped: &str) -> String {
    match stopped {
        "finished" => "completed".to_string(),
        other => other.to_string(),
    }
}

fn cap_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        s.to_string()
    } else {
        // The result (marker included) never exceeds `cap` characters.
        let marker = "\n\n[truncated]";
        let keep = cap.saturating_sub(marker.chars().count());
        let kept: String = s.chars().take(keep).collect();
        format!("{kept}{marker}")
    }
}

impl RunStore {
    pub fn load(path: PathBuf) -> Self {
        let data: RunData = std::fs::read(&path)
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

    /// Append a finished (or failed) run; returns the stored record.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        owner_email: &str,
        kind: &str,
        name: &str,
        input: &str,
        status: &str,
        steps: usize,
        tokens: u64,
        report: &str,
        error: Option<String>,
    ) -> RunRecord {
        self.record_with_trace(owner_email, kind, name, input, status, steps, tokens, report, error, Vec::new())
    }

    /// [`Self::record`] plus a Piece 84 tool-call trace.
    #[allow(clippy::too_many_arguments)]
    pub fn record_with_trace(
        &self,
        owner_email: &str,
        kind: &str,
        name: &str,
        input: &str,
        status: &str,
        steps: usize,
        tokens: u64,
        report: &str,
        error: Option<String>,
        trace: Vec<legal_rag_rag::ToolTraceEntry>,
    ) -> RunRecord {
        let record = RunRecord {
            id: legal_rag_common::new_id(),
            owner_email: owner_email.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            input: cap_chars(input, INPUT_CAP),
            status: status.to_string(),
            steps,
            tokens,
            report: cap_chars(report, REPORT_CAP),
            error,
            trace,
            created_at: Utc::now(),
        };
        {
            let mut data = self.data.write().unwrap();
            data.runs.push(record.clone());
            if data.runs.len() > RUNS_CAP {
                let excess = data.runs.len() - RUNS_CAP;
                data.runs.drain(0..excess);
            }
        }
        self.save();
        record
    }

    /// The owner's runs, newest first, as summaries.
    pub fn list_for(&self, owner_email: &str, limit: usize) -> Vec<RunSummary> {
        let data = self.data.read().unwrap();
        data.runs
            .iter()
            .rev()
            .filter(|r| r.owner_email == owner_email)
            .take(limit.clamp(1, 200))
            .map(RunSummary::from)
            .collect()
    }

    /// One full record, owner-checked (None when not the owner's).
    pub fn get(&self, owner_email: &str, id: &str) -> Option<RunRecord> {
        let data = self.data.read().unwrap();
        data.runs
            .iter()
            .find(|r| r.id == id && r.owner_email == owner_email)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> RunStore {
        // Unique path per test AND per process run, so parallel tests
        // never share a file and reruns never load stale records.
        let name = std::thread::current().name().unwrap_or("t").replace(|c: char| !c.is_alphanumeric(), "_");
        RunStore::load(PathBuf::from(format!("/tmp/legal-rag-runs-test-{}-{name}.json", std::process::id())))
    }

    #[test]
    fn records_list_newest_first_and_owner_scoped() {
        let s = store();
        s.record("a@x.test", "workflow", "W1", "in1", "completed", 4, 100, "report one", None);
        s.record("b@x.test", "workflow", "W2", "in2", "completed", 2, 50, "report two", None);
        s.record("a@x.test", "autonomous", "A1", "in3", "budget_tokens", 1, 10, "partial", None);
        let mine = s.list_for("a@x.test", 50);
        assert_eq!(mine.len(), 2);
        assert_eq!(mine[0].name, "A1");
        assert_eq!(mine[0].status, "budget_tokens");
        assert_eq!(mine[1].name, "W1");
        // Detail is owner-checked.
        let id = mine[1].id.clone();
        assert_eq!(s.get("a@x.test", &id).unwrap().report, "report one");
        assert!(s.get("b@x.test", &id).is_none());
    }

    #[test]
    fn caps_report_and_input() {
        let s = store();
        let big = "x".repeat(70_000);
        let r = s.record("a@x.test", "workflow", "W", &big, "completed", 1, 1, &big, None);
        assert!(r.report.chars().count() < 61_000);
        assert!(r.input.chars().count() <= INPUT_CAP);
        assert_eq!(status_from_stopped("finished"), "completed");
        assert_eq!(status_from_stopped("budget"), "budget");
    }
}
