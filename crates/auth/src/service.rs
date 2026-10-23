//! High-level authentication operations.

use std::sync::Arc;

use legal_rag_common::{Config, Error, Result};

use crate::jwt::{issue_token_pair, validate_token, Claims, TokenKind, TokenPair};
use crate::password::{hash_password, verify_password};
use crate::store::{User, UserStore, ROLE_ADMIN};

/// Coordinates user management and token issuance.
#[derive(Clone)]
pub struct AuthService {
    store: Arc<dyn UserStore>,
    jwt_secret: String,
    access_ttl_secs: i64,
    refresh_ttl_secs: i64,
    admin_emails: Vec<String>,
}

impl AuthService {
    pub fn new(store: Arc<dyn UserStore>, config: &Config) -> Self {
        Self {
            store,
            jwt_secret: config.jwt.secret.clone(),
            access_ttl_secs: config.jwt.access_ttl_secs,
            refresh_ttl_secs: config.jwt.refresh_ttl_secs,
            admin_emails: config.admin_emails.clone(),
        }
    }

    /// Promote a user to admin when their email is configured as one.
    fn maybe_promote(&self, user: User) -> Result<User> {
        if user.role != ROLE_ADMIN && self.admin_emails.iter().any(|e| e == &user.email) {
            if let Some(promoted) = self.store.set_role(&user.id, ROLE_ADMIN)? {
                return Ok(promoted);
            }
        }
        Ok(user)
    }

    /// Register a new user and return a token pair.
    pub fn register(&self, email: &str, password: &str) -> Result<(User, TokenPair)> {
        let hash = hash_password(password)?;
        let user = self.maybe_promote(self.store.create_user(email, &hash)?)?;
        let tokens = self.issue_for(&user)?;
        Ok((user, tokens))
    }

    /// Verify credentials and return a token pair.
    pub fn login(&self, email: &str, password: &str) -> Result<(User, TokenPair)> {
        let user = self
            .store
            .find_by_email(email)?
            .ok_or_else(|| Error::Auth("invalid email or password".into()))?;
        if !verify_password(password, &user.password_hash)? {
            return Err(Error::Auth("invalid email or password".into()));
        }
        let user = self.maybe_promote(user)?;
        let tokens = self.issue_for(&user)?;
        Ok((user, tokens))
    }

    /// Exchange a refresh token for a fresh pair (rotation).
    pub fn refresh(&self, refresh_token: &str) -> Result<TokenPair> {
        let claims = validate_token(refresh_token, &self.jwt_secret, TokenKind::Refresh)?;
        let user = self
            .store
            .find_by_id(&claims.sub)?
            .ok_or_else(|| Error::Auth("user no longer exists".into()))?;
        self.issue_for(&user)
    }

    /// Resolve a bearer access token to its user.
    pub fn authenticate(&self, access_token: &str) -> Result<User> {
        let claims = validate_token(access_token, &self.jwt_secret, TokenKind::Access)?;
        self.store
            .find_by_id(&claims.sub)?
            .ok_or_else(|| Error::Auth("user no longer exists".into()))
    }

    /// Upsert a user that authenticated via OIDC (passwordless).
    pub fn upsert_oidc_user(&self, email: &str) -> Result<User> {
        if let Some(user) = self.store.find_by_email(email)? {
            return self.maybe_promote(user);
        }
        // Random unusable password hash marker; OIDC users never log in locally.
        let user = self.store.create_user(email, "oidc:external")?;
        self.maybe_promote(user)
    }

    // --- Command Center (admin) operations ---------------------------------

    /// All users, oldest first.
    pub fn list_users(&self) -> Result<Vec<User>> {
        self.store.list_users()
    }

    /// Delete a user by id.
    pub fn delete_user(&self, id: &str) -> Result<bool> {
        self.store.delete_user(id)
    }

    /// Set a user's role; returns the updated user.
    pub fn set_user_role(&self, id: &str, role: &str) -> Result<Option<User>> {
        self.store.set_role(id, role)
    }

    pub fn issue_for(&self, user: &User) -> Result<TokenPair> {
        issue_token_pair(
            &user.id,
            &user.email,
            &self.jwt_secret,
            self.access_ttl_secs,
            self.refresh_ttl_secs,
        )
    }

    /// Validate an access token and return its claims (for middleware).
    pub fn validate_access(&self, token: &str) -> Result<Claims> {
        validate_token(token, &self.jwt_secret, TokenKind::Access)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryUserStore;

    fn service() -> AuthService {
        let config = Config {
            server_addr: String::new(),
            jwt: legal_rag_common::JwtConfig {
                secret: "test-secret".into(),
                access_ttl_secs: 900,
                refresh_ttl_secs: 3600,
            },
            s3: None,
            embedding: legal_rag_common::EmbeddingConfig {
                url: String::new(),
                api_key: None,
                model: String::new(),
                dims: 8,
            },
            llm: legal_rag_common::LlmConfig {
                url: String::new(),
                api_key: None,
                model: String::new(),
            },
            private_embedding: None,
            private_llm: None,
            oidc: None,
            chunk_size: 100,
            chunk_overlap: 10,
            default_top_k: 5,
            admin_emails: vec!["boss@example.com".into()],
        };
        AuthService::new(Arc::new(InMemoryUserStore::new()), &config)
    }

    #[test]
    fn register_login_refresh_flow() {
        let svc = service();
        let (user, _) = svc.register("dev@example.com", "s3cret-pw!").unwrap();
        let (_, pair) = svc.login("dev@example.com", "s3cret-pw!").unwrap();
        assert!(svc.login("dev@example.com", "wrong").is_err());
        let authed = svc.authenticate(&pair.access_token).unwrap();
        assert_eq!(authed.id, user.id);
        let pair2 = svc.refresh(&pair.refresh_token).unwrap();
        assert!(svc.authenticate(&pair2.access_token).is_ok());
    }
}
