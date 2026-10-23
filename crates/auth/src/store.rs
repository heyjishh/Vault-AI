//! User model and storage.

use std::collections::HashMap;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use legal_rag_common::{new_id, now_utc, Error, Result};
use serde::{Deserialize, Serialize};

/// Role for a regular workspace user.
pub const ROLE_USER: &str = "user";
/// Role for a Command Center (admin) user.
pub const ROLE_ADMIN: &str = "admin";

fn default_role() -> String {
    ROLE_USER.to_string()
}

/// A registered user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
    /// `user` or `admin` — gates the Command Center API.
    #[serde(default = "default_role")]
    pub role: String,
}

/// Storage backend for users. Swap the in-memory impl for Postgres/etc.
pub trait UserStore: Send + Sync {
    fn create_user(&self, email: &str, password_hash: &str) -> Result<User>;
    fn find_by_email(&self, email: &str) -> Result<Option<User>>;
    fn find_by_id(&self, id: &str) -> Result<Option<User>>;
    /// All users, for the Command Center admin console.
    fn list_users(&self) -> Result<Vec<User>>;
    /// Remove a user by id; returns whether one existed.
    fn delete_user(&self, id: &str) -> Result<bool>;
    /// Set a user's role (`user` | `admin`).
    fn set_role(&self, id: &str, role: &str) -> Result<Option<User>>;
}

/// Thread-safe in-memory user store (development / tests).
#[derive(Debug, Default)]
pub struct InMemoryUserStore {
    by_id: RwLock<HashMap<String, User>>,
    by_email: RwLock<HashMap<String, String>>,
}

impl InMemoryUserStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot all users *including* password hashes for disk persistence.
    /// (The API-facing `User` serialization skips the hash; this one must not.)
    pub fn snapshot(&self) -> Vec<UserSnapshot> {
        self.by_id
            .read()
            .expect("lock poisoned")
            .values()
            .map(|u| UserSnapshot {
                id: u.id.clone(),
                email: u.email.clone(),
                password_hash: u.password_hash.clone(),
                created_at: u.created_at,
                role: u.role.clone(),
            })
            .collect()
    }

    /// Restore users from a disk snapshot.
    pub fn restore(&self, users: Vec<UserSnapshot>) {
        let mut by_id = self.by_id.write().expect("lock poisoned");
        let mut by_email = self.by_email.write().expect("lock poisoned");
        for u in users {
            by_email.insert(u.email.clone(), u.id.clone());
            by_id.insert(
                u.id.clone(),
                User {
                    id: u.id,
                    email: u.email,
                    password_hash: u.password_hash,
                    created_at: u.created_at,
                    role: u.role,
                },
            );
        }
    }
}

/// Serializable user record for snapshots — includes the password hash,
/// unlike the API-facing `User`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSnapshot {
    pub id: String,
    pub email: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
    /// Absent in snapshots written before roles existed.
    #[serde(default = "default_role")]
    pub role: String,
}

impl UserStore for InMemoryUserStore {
    fn create_user(&self, email: &str, password_hash: &str) -> Result<User> {
        let email = email.trim().to_lowercase();
        if email.is_empty() || !email.contains('@') {
            return Err(Error::BadRequest("invalid email address".into()));
        }
        let mut by_email = self.by_email.write().expect("lock poisoned");
        if by_email.contains_key(&email) {
            return Err(Error::Conflict("email already registered".into()));
        }
        let user = User {
            id: new_id(),
            email: email.clone(),
            password_hash: password_hash.to_string(),
            created_at: now_utc(),
            role: ROLE_USER.to_string(),
        };
        by_email.insert(email, user.id.clone());
        self.by_id.write().expect("lock poisoned").insert(user.id.clone(), user.clone());
        Ok(user)
    }

    fn find_by_email(&self, email: &str) -> Result<Option<User>> {
        let email = email.trim().to_lowercase();
        let by_email = self.by_email.read().expect("lock poisoned");
        let by_id = self.by_id.read().expect("lock poisoned");
        Ok(by_email.get(&email).and_then(|id| by_id.get(id)).cloned())
    }

    fn find_by_id(&self, id: &str) -> Result<Option<User>> {
        Ok(self.by_id.read().expect("lock poisoned").get(id).cloned())
    }

    fn list_users(&self) -> Result<Vec<User>> {
        let mut users: Vec<User> = self
            .by_id
            .read()
            .expect("lock poisoned")
            .values()
            .cloned()
            .collect();
        users.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(users)
    }

    fn delete_user(&self, id: &str) -> Result<bool> {
        let mut by_id = self.by_id.write().expect("lock poisoned");
        let user = match by_id.remove(id) {
            Some(u) => u,
            None => return Ok(false),
        };
        self.by_email
            .write()
            .expect("lock poisoned")
            .remove(&user.email);
        Ok(true)
    }

    fn set_role(&self, id: &str, role: &str) -> Result<Option<User>> {
        if role != ROLE_USER && role != ROLE_ADMIN {
            return Err(Error::BadRequest(format!("unknown role '{role}'")));
        }
        let mut by_id = self.by_id.write().expect("lock poisoned");
        let user = match by_id.get_mut(id) {
            Some(u) => u,
            None => return Ok(None),
        };
        user.role = role.to_string();
        Ok(Some(user.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_find_user() {
        let store = InMemoryUserStore::new();
        let user = store.create_user("jishan@example.com", "hash").unwrap();
        assert_eq!(store.find_by_email("JISHAN@example.com").unwrap().unwrap().id, user.id);
        assert!(store.create_user("jishan@example.com", "other").is_err());
    }
}
