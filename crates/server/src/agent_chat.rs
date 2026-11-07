//! Agent Chat threads (Piece 86).
//!
//! The one-box Agent Chat is a conversation, so its turns are
//! stored as threads — one owner-scoped record per conversation,
//! holding the user's messages, the agent's answers, and the
//! durable summaries of what each turn did (tool trace, citations,
//! linked run ids, linked edit-proposal ids). The heavy artifacts
//! themselves live where they always have: runs in the run store,
//! edit proposals in the edit store; a thread only links to them,
//! so an approval decided later still resolves against the live
//! edit record when the thread reloads.
//!
//! Persisted to `agent_chats.json` in the data directory, mirroring
//! the runs/edits stores (one JSON document, atomic tmp+rename).

use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Threads kept per store (newest survive).
pub const THREADS_CAP: usize = 200;
/// Messages kept per thread (oldest of a very long thread drop).
pub const MESSAGES_CAP: usize = 200;
/// Stored message text cap.
pub const TEXT_CAP: usize = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// user | assistant
    pub role: String,
    pub text: String,
    pub created_at: DateTime<Utc>,
    /// The agent run this assistant turn produced (run store id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Billed tokens of the turn (assistant messages).
    #[serde(default)]
    pub tokens: u64,
    /// finished | budget | budget_tokens | turn_cap | failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
    /// The turn's tool-call trace (assistant messages).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace: Vec<legal_rag_rag::ToolTraceEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub citations: Vec<legal_rag_rag::Citation>,
    /// Edit proposals this turn created (edit store ids).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edit_ids: Vec<String>,
    /// Documents the user referenced with this message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub doc_ids: Vec<String>,
}

impl ChatMessage {
    pub fn user(text: &str, doc_ids: Vec<String>) -> Self {
        Self {
            role: "user".to_string(),
            text: cap_text(text),
            created_at: Utc::now(),
            run_id: None,
            tokens: 0,
            stopped: None,
            trace: Vec::new(),
            citations: Vec::new(),
            edit_ids: Vec::new(),
            doc_ids,
        }
    }

    pub fn assistant(
        text: &str,
        run_id: Option<String>,
        tokens: u64,
        stopped: &str,
        trace: Vec<legal_rag_rag::ToolTraceEntry>,
        citations: Vec<legal_rag_rag::Citation>,
        edit_ids: Vec<String>,
    ) -> Self {
        Self {
            role: "assistant".to_string(),
            text: cap_text(text),
            created_at: Utc::now(),
            run_id,
            tokens,
            stopped: Some(stopped.to_string()),
            trace,
            citations,
            edit_ids,
            doc_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatThread {
    pub id: String,
    pub owner_email: String,
    /// The Workspace (matter) the conversation is confined to.
    #[serde(default)]
    pub matter_id: Option<String>,
    pub title: String,
    pub messages: Vec<ChatMessage>,
    /// Append-only session event log (Piece 88, the harness's
    /// session layer): the source the follow-up model context is
    /// projected from via `derive_context`. Absent in pre-88
    /// threads — those load with an empty log and fall back to the
    /// message-based context until new turns log events.
    #[serde(default)]
    pub events: Vec<legal_rag_rag::SessionEvent>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A thread without its messages — the sidebar list row.
#[derive(Debug, Clone, Serialize)]
pub struct ChatThreadSummary {
    pub id: String,
    #[serde(default)]
    pub matter_id: Option<String>,
    pub title: String,
    pub message_count: usize,
    pub updated_at: DateTime<Utc>,
}

impl From<&ChatThread> for ChatThreadSummary {
    fn from(t: &ChatThread) -> Self {
        Self {
            id: t.id.clone(),
            matter_id: t.matter_id.clone(),
            title: t.title.clone(),
            message_count: t.messages.len(),
            updated_at: t.updated_at,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ChatData {
    threads: Vec<ChatThread>,
}

pub struct ChatStore {
    data: RwLock<ChatData>,
    path: PathBuf,
}

fn cap_text(s: &str) -> String {
    if s.chars().count() <= TEXT_CAP {
        s.to_string()
    } else {
        s.chars().take(TEXT_CAP).collect()
    }
}

fn title_of(first_message: &str) -> String {
    let t: String = first_message.trim().chars().take(80).collect();
    if t.is_empty() {
        "Agent chat".to_string()
    } else {
        t
    }
}

impl ChatStore {
    pub fn load(path: PathBuf) -> Self {
        let data: ChatData = std::fs::read(&path)
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

    /// Open a new thread; its title comes from the first message.
    pub fn create(&self, owner_email: &str, matter_id: Option<String>, first_message: &str) -> ChatThread {
        let now = Utc::now();
        let thread = ChatThread {
            id: legal_rag_common::new_id(),
            owner_email: owner_email.to_string(),
            matter_id,
            title: title_of(first_message),
            messages: Vec::new(),
            events: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        {
            let mut data = self.data.write().unwrap();
            data.threads.push(thread.clone());
            if data.threads.len() > THREADS_CAP {
                let excess = data.threads.len() - THREADS_CAP;
                data.threads.drain(0..excess);
            }
        }
        self.save();
        thread
    }

    /// Append one message; returns the updated thread, or None when
    /// the thread is not the owner's.
    pub fn append(&self, owner_email: &str, thread_id: &str, message: ChatMessage) -> Option<ChatThread> {
        let mut data = self.data.write().unwrap();
        let t = data
            .threads
            .iter_mut()
            .find(|t| t.id == thread_id && t.owner_email == owner_email)?;
        t.messages.push(message);
        if t.messages.len() > MESSAGES_CAP {
            let excess = t.messages.len() - MESSAGES_CAP;
            t.messages.drain(0..excess);
        }
        t.updated_at = Utc::now();
        let out = t.clone();
        drop(data);
        self.save();
        Some(out)
    }

    /// Append session events to a thread's log (Piece 88);
    /// returns the updated thread, or None when the thread is not
    /// the owner's.
    pub fn append_events(
        &self,
        owner_email: &str,
        thread_id: &str,
        events: Vec<legal_rag_rag::SessionEvent>,
    ) -> Option<ChatThread> {
        if events.is_empty() {
            return self.get(owner_email, thread_id);
        }
        let mut data = self.data.write().unwrap();
        let t = data
            .threads
            .iter_mut()
            .find(|t| t.id == thread_id && t.owner_email == owner_email)?;
        t.events.extend(events);
        t.updated_at = Utc::now();
        let out = t.clone();
        drop(data);
        self.save();
        Some(out)
    }

    /// One thread, owner-checked.
    pub fn get(&self, owner_email: &str, id: &str) -> Option<ChatThread> {
        let data = self.data.read().unwrap();
        data.threads
            .iter()
            .find(|t| t.id == id && t.owner_email == owner_email)
            .cloned()
    }

    /// The owner's threads, most recently updated first.
    pub fn list_for(&self, owner_email: &str, limit: usize) -> Vec<ChatThreadSummary> {
        let data = self.data.read().unwrap();
        let mut mine: Vec<&ChatThread> = data
            .threads
            .iter()
            .filter(|t| t.owner_email == owner_email)
            .collect();
        mine.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        mine.into_iter()
            .take(limit.clamp(1, 200))
            .map(ChatThreadSummary::from)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ChatStore {
        let name = std::thread::current().name().unwrap_or("t").replace(|c: char| !c.is_alphanumeric(), "_");
        ChatStore::load(PathBuf::from(format!(
            "/tmp/legal-rag-agent-chat-test-{}-{name}.json",
            std::process::id()
        )))
    }

    #[test]
    fn threads_append_and_stay_owner_scoped() {
        let s = store();
        let t = s.create("a@x.test", Some("m-1".to_string()), "change the notice period in the tenancy note");
        assert_eq!(t.title, "change the notice period in the tenancy note");
        assert_eq!(t.matter_id.as_deref(), Some("m-1"));
        let t = s.append("a@x.test", &t.id, ChatMessage::user("change the notice period", vec!["doc-1".into()])).unwrap();
        assert_eq!(t.messages.len(), 1);
        assert_eq!(t.messages[0].doc_ids, vec!["doc-1".to_string()]);
        let t = s
            .append(
                "a@x.test",
                &t.id,
                ChatMessage::assistant(
                    "Proposed.",
                    Some("run-1".into()),
                    1234,
                    "finished",
                    Vec::new(),
                    Vec::new(),
                    vec!["edit-1".into()],
                ),
            )
            .unwrap();
        assert_eq!(t.messages.len(), 2);
        assert_eq!(t.messages[1].run_id.as_deref(), Some("run-1"));
        assert_eq!(t.messages[1].edit_ids, vec!["edit-1".to_string()]);
        // Another owner can neither read nor append.
        assert!(s.get("b@x.test", &t.id).is_none());
        assert!(s.append("b@x.test", &t.id, ChatMessage::user("intrude", vec![])).is_none());
        // Summaries list newest first and count messages.
        let list = s.list_for("a@x.test", 50);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].message_count, 2);
        // The thread survives a reload from disk.
        let s2 = ChatStore::load(s.path.clone());
        assert_eq!(s2.get("a@x.test", &t.id).unwrap().messages.len(), 2);
    }

    #[test]
    fn legacy_threads_without_events_load_and_gain_a_log() {
        // A pre-Piece-88 agent_chats.json thread has no `events`
        // field at all; it must still load, with an empty log.
        let path = PathBuf::from(format!(
            "/tmp/legal-rag-agent-chat-legacy-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let legacy = r#"{"threads":[{"id":"t-old","owner_email":"a@x.test","matter_id":null,"title":"Old thread","messages":[],"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}]}"#;
        std::fs::write(&path, legacy).unwrap();
        let s = ChatStore::load(path.clone());
        let t = s.get("a@x.test", "t-old").expect("legacy thread loads");
        assert!(t.events.is_empty());
        // New turns append events; the log persists across reload.
        let t = s
            .append_events(
                "a@x.test",
                "t-old",
                vec![
                    legal_rag_rag::SessionEvent::UserMessage { text: "hi".to_string() },
                    legal_rag_rag::SessionEvent::TurnEnd { stopped: "finished".to_string() },
                ],
            )
            .unwrap();
        assert_eq!(t.events.len(), 2);
        let s2 = ChatStore::load(path);
        assert_eq!(s2.get("a@x.test", "t-old").unwrap().events.len(), 2);
        // Another owner cannot append events.
        assert!(s2
            .append_events(
                "b@x.test",
                "t-old",
                vec![legal_rag_rag::SessionEvent::UserMessage { text: "intrude".to_string() }]
            )
            .is_none());
    }
}
