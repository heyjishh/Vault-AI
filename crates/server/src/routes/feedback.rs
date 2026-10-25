//! `/api/v1/knowledge/feedback` (any logged-in user votes) and
//! `/api/v1/admin/feedback` (admins read the votes). The thumbs
//! up/down loop on Ask answers: votes are stored with the question
//! and an excerpt of the judged answer, one vote per user per answer
//! (re-voting updates). See `crate::feedback`.

use axum::Json;
use axum::extract::State;
use legal_rag_auth::{AdminUser, AuthUser};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::feedback::FeedbackVote;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct VoteRequest {
    pub question: String,
    pub answer: String,
    /// "up" | "down"
    pub vote: String,
}

pub async fn submit_vote(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<VoteRequest>,
) -> Result<Json<FeedbackVote>, ApiError> {
    if body.question.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("question is empty".into()).into());
    }
    if body.answer.trim().is_empty() {
        return Err(legal_rag_common::Error::BadRequest("answer is empty".into()).into());
    }
    if body.vote != "up" && body.vote != "down" {
        return Err(legal_rag_common::Error::BadRequest("vote must be \"up\" or \"down\"".into()).into());
    }
    Ok(Json(state.feedback.vote(
        &user.email,
        body.question.trim(),
        &body.answer,
        &body.vote,
    )))
}

#[derive(Serialize)]
pub struct FeedbackListView {
    pub votes: Vec<FeedbackVote>,
    pub up: usize,
    pub down: usize,
}

pub async fn list_votes(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Json<FeedbackListView> {
    let (up, down) = state.feedback.counts();
    Json(FeedbackListView { votes: state.feedback.list(), up, down })
}
