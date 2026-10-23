//! `/api/v1/auth/*` — registration, login, refresh, OIDC.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::response::Redirect;
use axum::Json;
use legal_rag_auth::{AuthUser, TokenPair};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct Credentials {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub user: UserView,
    pub tokens: TokenPair,
}

#[derive(Serialize)]
pub struct UserView {
    pub id: String,
    pub email: String,
    pub role: String,
}

impl From<legal_rag_auth::User> for UserView {
    fn from(u: legal_rag_auth::User) -> Self {
        Self { id: u.id, email: u.email, role: u.role }
    }
}

pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<Credentials>,
) -> Result<Json<AuthResponse>, ApiError> {
    let (user, tokens) = state.auth.register(&body.email, &body.password)?;
    Ok(Json(AuthResponse { user: user.into(), tokens }))
}

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<Credentials>,
) -> Result<Json<AuthResponse>, ApiError> {
    let (user, tokens) = state.auth.login(&body.email, &body.password)?;
    Ok(Json(AuthResponse { user: user.into(), tokens }))
}

pub async fn refresh(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> Result<Json<TokenPair>, ApiError> {
    Ok(Json(state.auth.refresh(&body.refresh_token)?))
}

pub async fn me(AuthUser(user): AuthUser) -> Json<UserView> {
    Json(user.into())
}

/// Start the OIDC login: redirect to the identity provider (e.g. Auth0).
pub async fn oidc_login(State(state): State<AppState>) -> Result<Redirect, ApiError> {
    let oidc = state.oidc.as_ref().ok_or_else(|| {
        legal_rag_common::Error::Config("OIDC is not configured".to_string())
    })?;
    let (url, _state) = oidc.login_url();
    Ok(Redirect::to(&url))
}

/// OIDC callback: verify the code, provision the user, issue local tokens.
pub async fn oidc_callback(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<AuthResponse>, ApiError> {
    let oidc = state.oidc.as_ref().ok_or_else(|| {
        legal_rag_common::Error::Config("OIDC is not configured".to_string())
    })?;
    let code = params.get("code").ok_or_else(|| {
        legal_rag_common::Error::BadRequest("missing code".to_string())
    })?;
    let st = params.get("state").ok_or_else(|| {
        legal_rag_common::Error::BadRequest("missing state".to_string())
    })?;
    let (_sub, email) = oidc.callback(code, st).await?;
    let user = state.auth.upsert_oidc_user(&email)?;
    let tokens = state.auth.issue_for(&user)?;
    Ok(Json(AuthResponse { user: user.into(), tokens }))
}
