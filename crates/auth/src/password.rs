//! Argon2 password hashing.

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use legal_rag_common::{Error, Result};

/// Hash a password with Argon2id.
pub fn hash_password(password: &str) -> Result<String> {
    if password.len() < 8 {
        return Err(Error::BadRequest("password must be at least 8 characters".into()));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Auth(format!("hashing failed: {e}")))
}

/// Verify a password against its Argon2 hash.
pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(hash).map_err(|_| Error::Auth("invalid password hash".into()))?;
    Ok(Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let hash = hash_password("correct-horse-123").unwrap();
        assert!(verify_password("correct-horse-123", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
        assert!(hash_password("short").is_err());
    }
}
