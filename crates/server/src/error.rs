//! Maps domain errors to HTTP responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use legal_rag_common::Error as DomainError;
use serde_json::json;

pub struct ApiError(pub DomainError);

impl From<DomainError> for ApiError {
    fn from(e: DomainError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self.0 {
            DomainError::Config(m) => (StatusCode::INTERNAL_SERVER_ERROR, m.clone()),
            DomainError::Auth(m) => (StatusCode::UNAUTHORIZED, m.clone()),
            DomainError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            DomainError::Forbidden(m) => (StatusCode::FORBIDDEN, m.clone()),
            DomainError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            DomainError::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            DomainError::Quota(m) => (StatusCode::PAYMENT_REQUIRED, m.clone()),
            DomainError::Upstream(m) => (StatusCode::BAD_GATEWAY, m.clone()),
            DomainError::Storage(m) => (StatusCode::INTERNAL_SERVER_ERROR, m.clone()),
            DomainError::Other(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        // Never leak internal detail for 5xx — except Upstream, whose
        // messages are our own provider-failure strings (users need them,
        // e.g. "cannot reach YouPI-PG gateway" at checkout).
        let body = if status.is_server_error() && !matches!(self.0, DomainError::Upstream(_)) {
            json!({ "error": "internal server error" })
        } else {
            json!({ "error": message })
        };
        (status, Json(body)).into_response()
    }
}
