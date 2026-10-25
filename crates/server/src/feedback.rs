//! Answer feedback: thumbs up/down votes on Ask answers.
//!
//! The Ask UI posts one vote per answer; the backend stores the vote
//! with the question and an excerpt of the answer it judged, so votes
//! stay interpretable after the fact. One vote per (user, question,
//! answer excerpt): re-voting the same answer updates the earlier vote
//! instead of appending a duplicate. Persisted to `feedback.json` in
//! the data directory. Admins read the full list (with up/down counts)
//! at `GET /api/v1/admin/feedback`.

use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Characters of the answer kept with a vote — enough to identify what
/// was judged without storing unbounded answer text.
pub const ANSWER_EXCERPT_CHARS: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackVote {
    pub id: String,
    pub user_email: String,
    pub question: String,
    pub answer_excerpt: String,
    /// "up" | "down"
    pub vote: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FeedbackData {
    votes: Vec<FeedbackVote>,
}

pub struct FeedbackStore {
    data: RwLock<FeedbackData>,
    path: PathBuf,
}

impl FeedbackStore {
    pub fn load(path: PathBuf) -> Self {
        let data: FeedbackData = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { data: RwLock::new(data), path }
    }

    fn save(&self) {
        if let Ok(data) = self.data.read() {
            if let Ok(bytes) = serde_json::to_vec_pretty(&*data) {
                let tmp = self.path.with_extension("json.tmp");
                if let Ok(()) = std::fs::write(&tmp, bytes) {
                    let _ = std::fs::rename(tmp, &self.path);
                }
            }
        }
    }

    pub fn excerpt(answer: &str) -> String {
        answer.chars().take(ANSWER_EXCERPT_CHARS).collect()
    }

    /// Record a vote, updating the existing vote when the same user
    /// votes on the same (question, answer excerpt) again.
    pub fn vote(&self, user_email: &str, question: &str, answer: &str, vote: &str) -> FeedbackVote {
        let answer_excerpt = Self::excerpt(answer);
        let now = Utc::now();
        let saved = {
            let mut data = self.data.write().unwrap();
            if let Some(existing) = data.votes.iter_mut().find(|v| {
                v.user_email == user_email
                    && v.question == question
                    && v.answer_excerpt == answer_excerpt
            }) {
                existing.vote = vote.to_string();
                existing.updated_at = now;
                existing.clone()
            } else {
                let fresh = FeedbackVote {
                    id: legal_rag_common::new_id(),
                    user_email: user_email.to_string(),
                    question: question.to_string(),
                    answer_excerpt,
                    vote: vote.to_string(),
                    created_at: now,
                    updated_at: now,
                };
                data.votes.push(fresh.clone());
                fresh
            }
        };
        self.save();
        saved
    }

    /// Newest first.
    pub fn list(&self) -> Vec<FeedbackVote> {
        let mut votes = self.data.read().unwrap().votes.clone();
        votes.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        votes
    }

    pub fn counts(&self) -> (usize, usize) {
        let data = self.data.read().unwrap();
        let up = data.votes.iter().filter(|v| v.vote == "up").count();
        let down = data.votes.iter().filter(|v| v.vote == "down").count();
        (up, down)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vote_upserts_same_answer_and_counts() {
        let path = std::env::temp_dir()
            .join(format!("feedback-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = FeedbackStore::load(path.clone());
        let first = store.vote("a@test.io", "What is theft?", "Theft is ...", "up");
        let second = store.vote("a@test.io", "What is theft?", "Theft is ...", "down");
        assert_eq!(first.id, second.id, "re-vote must update, not duplicate");
        assert_eq!(second.vote, "down");
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.counts(), (0, 1));
        store.vote("b@test.io", "What is theft?", "Theft is ...", "up");
        store.vote("a@test.io", "Another question?", "Different answer.", "up");
        assert_eq!(store.list().len(), 3, "different user/question/answer = new vote");
        assert_eq!(store.counts(), (2, 1));
        // Long answers are excerpted, and the excerpt is the upsert key.
        let long = "x".repeat(ANSWER_EXCERPT_CHARS + 500);
        let v = store.vote("c@test.io", "Q?", &long, "up");
        assert_eq!(v.answer_excerpt.chars().count(), ANSWER_EXCERPT_CHARS);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("json.tmp"));
    }
}
