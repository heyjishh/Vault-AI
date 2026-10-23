//! Disk persistence for the in-memory stores.
//!
//! Layout under `LEGAL_RAG_DATA_DIR` (default `./data`):
//!
//! - `chunks.json` — `Vec<ChunkRecord>` (only when the in-memory vector
//!   store is used; SereneDB persists itself)
//! - `graph.json` — `{ entities, relations }`
//! - `documents.json` — `{ documents, status }` (doc registry)
//! - `users.json` — `Vec<User>` (written by the auth crate's helper)
//!
//! JSON is used so snapshots stay human-inspectable. Writes are atomic
//! (temp file + rename) so a crash mid-save never corrupts the last good
//! snapshot.

use std::path::{Path, PathBuf};

use legal_rag_common::{Error, Result};
use serde::{de::DeserializeOwned, Serialize};

/// Data directory from `LEGAL_RAG_DATA_DIR`, default `./data`.
pub fn data_dir() -> PathBuf {
    std::env::var("LEGAL_RAG_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./data"))
}

/// Atomically write `value` as pretty JSON to `dir/name`.
pub fn save_json<T: Serialize>(dir: &Path, name: &str, value: &T) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::Storage(format!("create data dir: {e}")))?;
    let tmp = dir.join(format!("{name}.tmp"));
    let final_path = dir.join(name);
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|e| Error::Storage(format!("encode {name}: {e}")))?;
    std::fs::write(&tmp, bytes).map_err(|e| Error::Storage(format!("write {name}: {e}")))?;
    std::fs::rename(&tmp, &final_path).map_err(|e| Error::Storage(format!("commit {name}: {e}")))?;
    Ok(())
}

/// Load `dir/name` if it exists; `Ok(None)` when there's no snapshot yet.
pub fn load_json<T: DeserializeOwned>(dir: &Path, name: &str) -> Result<Option<T>> {
    let path = dir.join(name);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Storage(format!("read {name}: {e}"))),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| Error::Storage(format!("decode {name}: {e}")))
}
