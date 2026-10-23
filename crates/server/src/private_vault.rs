//! Confidential vault: per-user, password-encrypted document storage.
//!
//! Guarantees:
//! - A document's title and chunk texts are stored ONLY as AES-256-GCM
//!   ciphertext in `private_vault.json`. Plaintext never touches disk, the
//!   shared corpus snapshots, or the knowledge graph.
//! - The encryption key is derived from a vault password the user sets
//!   (Argon2id KDF, per-user salt). The key lives only in server memory
//!   while the vault is unlocked; locking (or a restart) drops it.
//! - Documents are structurally per-user: every lookup is scoped by the
//!   owner's user id, so no other user — and no admin view — can reach them.
//! - Search runs over the stored embedding vectors; only the top hits are
//!   decrypted, in memory, for the owner's own request.
//!
//! Honest limits: while unlocked, the decrypted top hits are sent to the
//! configured LLM (like any query context), and embeddings are computed by
//! the configured provider at upload time. Embedding vectors are stored
//! alongside the ciphertext (they are not readable text).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::Argon2;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const KEY_CHECK: &[u8] = b"juris-vault-key-check-v1";

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let u = Uuid::new_v4();
    let b = u.as_bytes();
    let mut out = [0u8; N];
    out.copy_from_slice(&b[..N]);
    out
}

fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32], String> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| format!("key derivation failed: {e}"))?;
    Ok(key)
}

/// AES-256-GCM seal: base64( nonce[12] || ciphertext ).
fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> String {
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let nonce = random_bytes::<12>();
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .expect("aes-gcm encryption");
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    B64.encode(out)
}

fn decrypt(key: &[u8; 32], sealed_b64: &str) -> Result<Vec<u8>, String> {
    let raw = B64.decode(sealed_b64).map_err(|e| format!("bad ciphertext: {e}"))?;
    if raw.len() < 13 {
        return Err("ciphertext too short".to_string());
    }
    let (nonce, ct) = raw.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| "decryption failed (wrong key or corrupt data)".to_string())
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0f32;
    let mut na = 0f32;
    let mut nb = 0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivateChunk {
    /// base64 AES-256-GCM ciphertext of the chunk text.
    pub ct: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivateDoc {
    pub id: String,
    /// Encrypted document title.
    pub title_ct: String,
    /// SHA-256 fingerprint of the original file bytes (hex).
    pub sha256: String,
    pub chunks: Vec<PrivateChunk>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultUser {
    user_id: String,
    /// base64 Argon2 salt.
    salt: String,
    /// Encrypted key-check constant; verifies a password without storing it.
    key_check: String,
    docs: Vec<PrivateDoc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct VaultFile {
    users: Vec<VaultUser>,
}

/// Metadata for one private document, titles decrypted (owner, unlocked).
#[derive(Debug, Clone, Serialize)]
pub struct DocMeta {
    pub id: String,
    pub title: String,
    pub sha256: String,
    pub chunks: usize,
    pub created_at: DateTime<Utc>,
}

pub struct PrivateVault {
    path: PathBuf,
    inner: RwLock<VaultFile>,
    /// Session keys, memory only — never persisted.
    keys: RwLock<HashMap<String, [u8; 32]>>,
}

impl PrivateVault {
    pub fn load(path: PathBuf) -> Self {
        let file: VaultFile = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { path, inner: RwLock::new(file), keys: RwLock::new(HashMap::new()) }
    }

    fn save(&self, file: &VaultFile) -> Result<(), String> {
        let text = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn session_key(&self, user_id: &str) -> Option<[u8; 32]> {
        self.keys.read().unwrap().get(user_id).copied()
    }

    pub fn has_password(&self, user_id: &str) -> bool {
        self.inner.read().unwrap().users.iter().any(|u| u.user_id == user_id)
    }

    pub fn is_unlocked(&self, user_id: &str) -> bool {
        self.session_key(user_id).is_some()
    }

    pub fn doc_count(&self, user_id: &str) -> usize {
        self.inner
            .read()
            .unwrap()
            .users
            .iter()
            .find(|u| u.user_id == user_id)
            .map(|u| u.docs.len())
            .unwrap_or(0)
    }

    /// Set the vault password for the first time; also unlocks the session.
    pub fn set_password(&self, user_id: &str, password: &str) -> Result<(), String> {
        let salt = random_bytes::<16>();
        let key = derive_key(password, &salt)?;
        let user = VaultUser {
            user_id: user_id.to_string(),
            salt: B64.encode(salt),
            key_check: encrypt(&key, KEY_CHECK),
            docs: Vec::new(),
        };
        {
            let mut file = self.inner.write().unwrap();
            if file.users.iter().any(|u| u.user_id == user_id) {
                return Err("a vault password is already set".to_string());
            }
            file.users.push(user);
            self.save(&file)?;
        }
        self.keys.write().unwrap().insert(user_id.to_string(), key);
        Ok(())
    }

    /// Verify a password and hold the derived key in memory. `Ok(false)`
    /// means wrong password (or no vault yet).
    pub fn unlock(&self, user_id: &str, password: &str) -> Result<bool, String> {
        let (salt_b64, check) = {
            let file = self.inner.read().unwrap();
            match file.users.iter().find(|u| u.user_id == user_id) {
                Some(u) => (u.salt.clone(), u.key_check.clone()),
                None => return Ok(false),
            }
        };
        let salt = B64.decode(&salt_b64).map_err(|e| e.to_string())?;
        let key = derive_key(password, &salt)?;
        match decrypt(&key, &check) {
            Ok(v) if v == KEY_CHECK => {
                self.keys.write().unwrap().insert(user_id.to_string(), key);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub fn lock(&self, user_id: &str) {
        self.keys.write().unwrap().remove(user_id);
    }

    /// Change the password: verify the current one, then re-encrypt every
    /// title and chunk under a fresh key/salt.
    pub fn change_password(
        &self,
        user_id: &str,
        current: &str,
        new_password: &str,
    ) -> Result<bool, String> {
        if !self.unlock(user_id, current)? {
            return Ok(false);
        }
        let old_key = self.session_key(user_id).expect("just unlocked");
        let new_salt = random_bytes::<16>();
        let new_key = derive_key(new_password, &new_salt)?;
        {
            let mut file = self.inner.write().unwrap();
            let user = file
                .users
                .iter_mut()
                .find(|u| u.user_id == user_id)
                .ok_or("no vault for user")?;
            for doc in user.docs.iter_mut() {
                let title = decrypt(&old_key, &doc.title_ct)?;
                doc.title_ct = encrypt(&new_key, &title);
                for chunk in doc.chunks.iter_mut() {
                    let text = decrypt(&old_key, &chunk.ct)?;
                    chunk.ct = encrypt(&new_key, &text);
                }
            }
            user.salt = B64.encode(new_salt);
            user.key_check = encrypt(&new_key, KEY_CHECK);
            self.save(&file)?;
        }
        self.keys.write().unwrap().insert(user_id.to_string(), new_key);
        Ok(true)
    }

    /// Store a new private document. Requires an unlocked vault.
    pub fn add_doc(
        &self,
        user_id: &str,
        title: &str,
        sha256: String,
        chunks: Vec<(String, Vec<f32>)>,
    ) -> Result<DocMeta, String> {
        let key = self.session_key(user_id).ok_or("vault is locked")?;
        let doc = PrivateDoc {
            id: Uuid::new_v4().to_string(),
            title_ct: encrypt(&key, title.as_bytes()),
            sha256,
            chunks: chunks
                .into_iter()
                .map(|(text, vector)| PrivateChunk { ct: encrypt(&key, text.as_bytes()), vector })
                .collect(),
            created_at: Utc::now(),
        };
        let meta = DocMeta {
            id: doc.id.clone(),
            title: title.to_string(),
            sha256: doc.sha256.clone(),
            chunks: doc.chunks.len(),
            created_at: doc.created_at,
        };
        let mut file = self.inner.write().unwrap();
        let user = file
            .users
            .iter_mut()
            .find(|u| u.user_id == user_id)
            .ok_or("no vault for user")?;
        user.docs.push(doc);
        self.save(&file)?;
        Ok(meta)
    }

    /// List the owner's documents with decrypted titles. Requires unlock.
    pub fn list_docs(&self, user_id: &str) -> Result<Vec<DocMeta>, String> {
        let key = self.session_key(user_id).ok_or("vault is locked")?;
        let file = self.inner.read().unwrap();
        let user = file.users.iter().find(|u| u.user_id == user_id).ok_or("no vault for user")?;
        let mut metas = Vec::with_capacity(user.docs.len());
        for doc in &user.docs {
            let title = String::from_utf8(decrypt(&key, &doc.title_ct)?)
                .unwrap_or_else(|_| "(unreadable title)".to_string());
            metas.push(DocMeta {
                id: doc.id.clone(),
                title,
                sha256: doc.sha256.clone(),
                chunks: doc.chunks.len(),
                created_at: doc.created_at,
            });
        }
        Ok(metas)
    }

    /// Delete one of the owner's documents (ciphertext removal; no key
    /// needed, so it also works while locked).
    pub fn delete_doc(&self, user_id: &str, doc_id: &str) -> bool {
        let mut file = self.inner.write().unwrap();
        let Some(user) = file.users.iter_mut().find(|u| u.user_id == user_id) else {
            return false;
        };
        let before = user.docs.len();
        user.docs.retain(|d| d.id != doc_id);
        let removed = user.docs.len() != before;
        if removed {
            let _ = self.save(&file);
        }
        removed
    }

    /// Cosine search over the owner's chunks; decrypts only the top hits.
    /// Returns (title, text, score). Requires unlock.
    pub fn search(
        &self,
        user_id: &str,
        qvec: &[f32],
        top_k: usize,
    ) -> Result<Vec<(String, String, f32)>, String> {
        let key = self.session_key(user_id).ok_or("vault is locked")?;
        let mut scored: Vec<(f32, String, String)> = Vec::new(); // (score, title_ct, chunk_ct)
        {
            let file = self.inner.read().unwrap();
            let user = file.users.iter().find(|u| u.user_id == user_id).ok_or("no vault for user")?;
            for doc in &user.docs {
                for chunk in &doc.chunks {
                    let s = cosine(qvec, &chunk.vector);
                    if s > 0.25 {
                        scored.push((s, doc.title_ct.clone(), chunk.ct.clone()));
                    }
                }
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        let mut out = Vec::with_capacity(scored.len());
        for (score, title_ct, chunk_ct) in scored {
            let title = String::from_utf8(decrypt(&key, &title_ct)?).unwrap_or_default();
            let text = String::from_utf8(decrypt(&key, &chunk_ct)?).unwrap_or_default();
            out.push((title, text, score));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> PrivateVault {
        let dir = std::env::temp_dir().join(format!("jv-vault-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        PrivateVault::load(dir.join("private_vault.json"))
    }

    #[test]
    fn round_trip_and_isolation() {
        let v = vault();
        v.set_password("user-a", "correct horse").unwrap();
        assert!(v.is_unlocked("user-a"));
        assert!(!v.has_password("user-b"));

        let meta = v
            .add_doc("user-a", "Secret Deed", sha256_hex(b"file"), vec![
                ("the earnest money is Rs 11,00,000".to_string(), vec![1.0, 0.0]),
                ("unrelated text about weather".to_string(), vec![0.0, 1.0]),
            ])
            .unwrap();
        assert_eq!(meta.chunks, 2);
        assert_eq!(v.doc_count("user-a"), 1);
        assert_eq!(v.doc_count("user-b"), 0);

        // On disk: ciphertext only — no plaintext anywhere in the file.
        let raw = std::fs::read_to_string(&v.path).unwrap();
        assert!(!raw.contains("earnest money"), "plaintext leaked to disk");
        assert!(!raw.contains("Secret Deed"), "title leaked to disk");

        // Search finds the right chunk for the owner…
        let hits = v.search("user-a", &[1.0, 0.05], 3).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.contains("earnest money"));
        // …and nothing for anyone else.
        assert!(v.search("user-b", &[1.0, 0.0], 3).is_err());

        // Lock drops the key: search and listing stop working.
        v.lock("user-a");
        assert!(v.search("user-a", &[1.0, 0.0], 3).is_err());
        // Wrong password fails; right password restores.
        assert!(!v.unlock("user-a", "wrong password").unwrap());
        assert!(v.unlock("user-a", "correct horse").unwrap());
        assert_eq!(v.list_docs("user-a").unwrap()[0].title, "Secret Deed");

        // Password change re-encrypts; old password is dead.
        assert!(v.change_password("user-a", "correct horse", "new secret").unwrap());
        v.lock("user-a");
        assert!(!v.unlock("user-a", "correct horse").unwrap());
        assert!(v.unlock("user-a", "new secret").unwrap());
        let hits = v.search("user-a", &[1.0, 0.05], 3).unwrap();
        assert!(hits[0].1.contains("earnest money"));
    }
}
