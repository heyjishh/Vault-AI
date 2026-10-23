//! Axum extractor that resolves the bearer token to an authenticated user.
//!
//! The router state must provide `Arc<AuthService>` via `FromRef`
//! (e.g. `impl FromRef<AppState> for Arc<AuthService>`).

use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use legal_rag_common::Error;
use serde_json::json;

use crate::service::AuthService;
use crate::store::User;

/// Rejection for failed authentication extraction.
pub struct AuthRejection(Error, StatusCode);

impl From<Error> for AuthRejection {
    fn from(e: Error) -> Self {
        Self(e, StatusCode::UNAUTHORIZED)
    }
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        let message = self.0.to_string();
        (self.1, Json(json!({ "error": message }))).into_response()
    }
}

/// The authenticated user for the current request.
#[derive(Debug, Clone)]
pub struct AuthUser(pub User);
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    Arc<AuthService>: FromRef<S>,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let State(auth) = State::<Arc<AuthService>>::from_request_parts(parts, state)
            .await
            .map_err(|_| AuthRejection::from(Error::Auth("auth service unavailable".into())))?;
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AuthRejection::from(Error::Auth("missing Authorization header".into())))?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AuthRejection::from(Error::Auth("expected Bearer token".into())))?;
        let user = auth.authenticate(token).map_err(AuthRejection::from)?;
        Ok(AuthUser(user))
    }
}

/// An authenticated **admin** user — rejects non-admins with 403.
/// Gates the Command Center (`/api/v1/admin/*`) routes.
#[derive(Debug, Clone)]
pub struct AdminUser(pub User);

impl<S> FromRequestParts<S> for AdminUser
where
    S: Send + Sync,
    Arc<AuthService>: FromRef<S>,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let AuthUser(user) = AuthUser::from_request_parts(parts, state).await?;
        if user.role != crate::store::ROLE_ADMIN {
            return Err(AuthRejection(
                Error::BadRequest("admin role required".into()),
                StatusCode::FORBIDDEN,
            ));
        }
        Ok(AdminUser(user))
    }
}
