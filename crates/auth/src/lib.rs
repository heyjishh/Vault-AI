//! Authentication: local users (Argon2 + JWT) and Auth0-compatible OIDC.
//!
//! Mirrors the REA finding that the reference product gates everything behind
//! an Auth0 (OIDC) login wall, while keeping a local JWT path for development
//! and self-hosted deployments.

pub mod extractor;
pub mod jwt;
pub mod oidc;
pub mod password;
pub mod service;
pub mod store;

pub use extractor::{AdminUser, AuthUser};
pub use jwt::{Claims, TokenPair};
pub use oidc::OidcClient;
pub use service::AuthService;
pub use store::{InMemoryUserStore, User, UserSnapshot, UserStore, ROLE_ADMIN, ROLE_USER};
