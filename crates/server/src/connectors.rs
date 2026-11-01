//! Document-source connectors (Piece 70, 2026-11-01): connect
//! Google Drive, OneDrive (Microsoft) and Dropbox so documents can
//! be imported straight into the Vault — the "connected to
//! everything that matters" pattern JISHAN pointed at. NetDocuments
//! is listed in the UI as partner-only (its API needs a NetDocuments
//! partnership; there is no self-serve OAuth app).
//!
//! Each provider needs an OAuth app registered by the operator
//! (client id + secret). Config comes from the environment
//! (`GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET`,
//! `MICROSOFT_CLIENT_ID` / `MICROSOFT_CLIENT_SECRET`,
//! `DROPBOX_CLIENT_ID` / `DROPBOX_CLIENT_SECRET` — the server's
//! `.env` is loaded at boot) or from `connector_apps.json` in the
//! data directory with the shape
//! `{"google": {"client_id": "…", "client_secret": "…"}}`.
//! The OAuth redirect URI is
//! `{LEGAL_RAG_PUBLIC_URL or the public relay}/api/v1/connectors/{provider}/callback`
//! and must be registered on the provider app.
//!
//! Per-user tokens persist to `connectors.json` in the data
//! directory (access + refresh + the account email). Access tokens
//! are refreshed transparently when expired.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub struct ProviderDef {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    pub auth_url: &'static str,
    pub token_url: &'static str,
    pub scope: &'static str,
}

pub fn providers() -> Vec<ProviderDef> {
    vec![
        ProviderDef {
            id: "google",
            name: "Google Drive",
            blurb: "Import documents from your Google Drive into the Vault.",
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            scope: "https://www.googleapis.com/auth/drive.readonly openid email",
        },
        ProviderDef {
            id: "microsoft",
            name: "OneDrive",
            blurb: "Import documents from OneDrive (personal or work) via Microsoft Graph.",
            auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            scope: "offline_access Files.Read.All User.Read",
        },
        ProviderDef {
            id: "dropbox",
            name: "Dropbox",
            blurb: "Import documents from your Dropbox into the Vault.",
            auth_url: "https://www.dropbox.com/oauth2/authorize",
            token_url: "https://api.dropboxapi.com/2/oauth2/token",
            scope: "files.metadata.read files.content.read account_info.read",
        },
    ]
}

pub fn provider(id: &str) -> Option<ProviderDef> {
    providers().into_iter().find(|p| p.id == id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredConnection {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub account_email: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConnData {
    /// user email -> provider id -> connection.
    connections: HashMap<String, HashMap<String, StoredConnection>>,
}

pub struct ConnectorStore {
    data: RwLock<ConnData>,
    path: PathBuf,
    /// OAuth state nonces: nonce -> (user email, provider id).
    nonces: RwLock<HashMap<String, (String, String)>>,
}

#[derive(Deserialize)]
struct AppCreds {
    client_id: String,
    client_secret: String,
}

/// (client_id, client_secret) for a provider, from the environment
/// or `connector_apps.json` next to the store file.
pub fn app_config(path: &PathBuf, provider_id: &str) -> Option<(String, String)> {
    let env_prefix = provider_id.to_uppercase();
    if let (Ok(id), Ok(secret)) = (
        std::env::var(format!("{env_prefix}_CLIENT_ID")),
        std::env::var(format!("{env_prefix}_CLIENT_SECRET")),
    ) {
        if !id.trim().is_empty() && !secret.trim().is_empty() {
            return Some((id, secret));
        }
    }
    let cfg_path = path.with_file_name("connector_apps.json");
    let text = std::fs::read_to_string(cfg_path).ok()?;
    let map: HashMap<String, AppCreds> = serde_json::from_str(&text).ok()?;
    let creds = map.get(provider_id)?;
    Some((creds.client_id.clone(), creds.client_secret.clone()))
}

pub fn public_base() -> String {
    std::env::var("LEGAL_RAG_PUBLIC_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://legal-rag-relay.jurisvault.workers.dev".to_string())
        .trim_end_matches('/')
        .to_string()
}

pub fn redirect_uri(provider_id: &str) -> String {
    format!("{}/api/v1/connectors/{}/callback", public_base(), provider_id)
}

impl ConnectorStore {
    pub fn load(path: PathBuf) -> Self {
        let data: ConnData = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { data: RwLock::new(data), path, nonces: RwLock::new(HashMap::new()) }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    fn save(&self) {
        if let Ok(data) = self.data.read() {
            if let Ok(bytes) = serde_json::to_vec_pretty(&*data) {
                let tmp = self.path.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(tmp, &self.path);
                }
            }
        }
    }

    pub fn connection(&self, user_email: &str, provider_id: &str) -> Option<StoredConnection> {
        self.data
            .read()
            .unwrap()
            .connections
            .get(user_email)
            .and_then(|m| m.get(provider_id))
            .cloned()
    }

    pub fn set_connection(&self, user_email: &str, provider_id: &str, conn: StoredConnection) {
        {
            let mut data = self.data.write().unwrap();
            data.connections
                .entry(user_email.to_string())
                .or_default()
                .insert(provider_id.to_string(), conn);
        }
        self.save();
    }

    pub fn remove_connection(&self, user_email: &str, provider_id: &str) -> bool {
        let removed = {
            let mut data = self.data.write().unwrap();
            data.connections
                .get_mut(user_email)
                .map(|m| m.remove(provider_id).is_some())
                .unwrap_or(false)
        };
        if removed {
            self.save();
        }
        removed
    }

    pub fn new_nonce(&self, user_email: &str, provider_id: &str) -> String {
        let nonce = legal_rag_common::new_id();
        self.nonces
            .write()
            .unwrap()
            .insert(nonce.clone(), (user_email.to_string(), provider_id.to_string()));
        nonce
    }

    pub fn take_nonce(&self, nonce: &str) -> Option<(String, String)> {
        self.nonces.write().unwrap().remove(nonce)
    }
}

/// Exchange an authorization code (or refresh token) at a
/// provider's token endpoint. Returns the stored connection shape.
pub async fn exchange_token(
    def: &ProviderDef,
    client_id: &str,
    client_secret: &str,
    params: &[(&str, String)],
) -> Result<StoredConnection, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let mut form: Vec<(&str, String)> = vec![
        ("client_id", client_id.to_string()),
        ("client_secret", client_secret.to_string()),
    ];
    form.extend_from_slice(params);
    let resp = client
        .post(def.token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;
    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("token response unreadable: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "provider refused the token exchange ({status}): {}",
            body.get("error_description")
                .or_else(|| body.get("error"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
        ));
    }
    let access = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("token response had no access_token")?
        .to_string();
    let expires_in = body.get("expires_in").and_then(|v| v.as_i64()).unwrap_or(3600);
    Ok(StoredConnection {
        access_token: access,
        refresh_token: body
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        account_email: None,
        expires_at: Some(Utc::now() + Duration::seconds(expires_in.saturating_sub(60))),
    })
}

/// The connected account's email, best-effort per provider.
pub async fn fetch_account_email(
    def: &ProviderDef,
    access_token: &str,
) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .ok()?;
    match def.id {
        "google" => {
            let v: serde_json::Value = client
                .get("https://www.googleapis.com/oauth2/v2/userinfo")
                .bearer_auth(access_token)
                .send()
                .await
                .ok()?
                .json()
                .await
                .ok()?;
            v.get("email").and_then(|e| e.as_str()).map(str::to_string)
        }
        "microsoft" => {
            let v: serde_json::Value = client
                .get("https://graph.microsoft.com/v1.0/me")
                .bearer_auth(access_token)
                .send()
                .await
                .ok()?
                .json()
                .await
                .ok()?;
            v.get("mail")
                .or_else(|| v.get("userPrincipalName"))
                .and_then(|e| e.as_str())
                .map(str::to_string)
        }
        "dropbox" => {
            let v: serde_json::Value = client
                .post("https://api.dropboxapi.com/2/users/get_current_account")
                .bearer_auth(access_token)
                .send()
                .await
                .ok()?
                .json()
                .await
                .ok()?;
            v.get("email").and_then(|e| e.as_str()).map(str::to_string)
        }
        _ => None,
    }
}

/// A valid access token for the user's connection — refreshing
/// first when the stored one has expired. Errors explain themselves
/// for the UI ("not connected", "session expired — reconnect").
pub async fn valid_token(
    store: &ConnectorStore,
    user_email: &str,
    provider_id: &str,
) -> Result<String, String> {
    let def = provider(provider_id).ok_or_else(|| format!("unknown provider {provider_id}"))?;
    let conn = store
        .connection(user_email, provider_id)
        .ok_or_else(|| format!("{} is not connected", def.name))?;
    let expired = conn
        .expires_at
        .map(|t| Utc::now() >= t)
        .unwrap_or(false);
    if !expired {
        return Ok(conn.access_token.clone());
    }
    let Some(refresh) = conn.refresh_token.clone() else {
        return Err(format!(
            "the {} session expired — disconnect and connect again",
            def.name
        ));
    };
    let (client_id, client_secret) = app_config(store.path(), provider_id)
        .ok_or_else(|| format!("{} is not configured on this server", def.name))?;
    let mut fresh = exchange_token(
        &def,
        &client_id,
        &client_secret,
        &[
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh.clone()),
        ],
    )
    .await?;
    fresh.refresh_token = fresh.refresh_token.or(Some(refresh));
    fresh.account_email = conn.account_email.clone();
    store.set_connection(user_email, provider_id, fresh.clone());
    Ok(fresh.access_token)
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteFile {
    pub id: String,
    pub name: String,
    pub mime_type: Option<String>,
    pub size: Option<u64>,
    pub modified: Option<String>,
}

/// List files from a connected provider (name search when `q` is
/// non-empty where the provider supports it; client-side otherwise).
pub async fn list_files(
    def: &ProviderDef,
    access_token: &str,
    q: &str,
) -> Result<Vec<RemoteFile>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let needle = q.trim().to_lowercase();
    match def.id {
        "google" => {
            let mut query = "trashed=false and mimeType!='application/vnd.google-apps.folder'".to_string();
            if !needle.is_empty() {
                let esc = q.trim().replace('\'', "\\'");
                query = format!("name contains '{esc}' and {query}");
            }
            let v: serde_json::Value = client
                .get("https://www.googleapis.com/drive/v3/files")
                .bearer_auth(access_token)
                .query(&[
                    ("q", query.as_str()),
                    ("pageSize", "40"),
                    ("orderBy", "modifiedTime desc"),
                    ("fields", "files(id,name,mimeType,size,modifiedTime)"),
                ])
                .send()
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| format!("Drive listing failed: {e}"))?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            Ok(v.get("files")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|f| RemoteFile {
                    id: f.get("id").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                    name: f.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                    mime_type: f.get("mimeType").and_then(|x| x.as_str()).map(str::to_string),
                    size: f.get("size").and_then(|x| x.as_str()).and_then(|s| s.parse().ok()),
                    modified: f.get("modifiedTime").and_then(|x| x.as_str()).map(str::to_string),
                })
                .collect())
        }
        "microsoft" => {
            let url = if needle.is_empty() {
                "https://graph.microsoft.com/v1.0/me/drive/root/children".to_string()
            } else {
                format!(
                    "https://graph.microsoft.com/v1.0/me/drive/root/search(q='{}')",
                    q.trim().replace('\'', "''")
                )
            };
            let v: serde_json::Value = client
                .get(&url)
                .bearer_auth(access_token)
                .query(&[("$top", "40"), ("$select", "id,name,size,lastModifiedDateTime,file")])
                .send()
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| format!("OneDrive listing failed: {e}"))?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            Ok(v.get("value")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter(|f| f.get("file").is_some())
                .map(|f| RemoteFile {
                    id: f.get("id").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                    name: f.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                    mime_type: f
                        .get("file")
                        .and_then(|x| x.get("mimeType"))
                        .and_then(|x| x.as_str())
                        .map(str::to_string),
                    size: f.get("size").and_then(|x| x.as_u64()),
                    modified: f.get("lastModifiedDateTime").and_then(|x| x.as_str()).map(str::to_string),
                })
                .collect())
        }
        "dropbox" => {
            let v: serde_json::Value = client
                .post("https://api.dropboxapi.com/2/files/list_folder")
                .bearer_auth(access_token)
                .json(&serde_json::json!({ "path": "", "recursive": false, "limit": 100 }))
                .send()
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| format!("Dropbox listing failed: {e}"))?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            Ok(v.get("entries")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter(|f| {
                    f.get(".tag").and_then(|x| x.as_str()) == Some("file")
                        && (needle.is_empty()
                            || f.get("name")
                                .and_then(|x| x.as_str())
                                .map(|n| n.to_lowercase().contains(&needle))
                                .unwrap_or(false))
                })
                .map(|f| RemoteFile {
                    id: f.get("path_lower").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                    name: f.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                    mime_type: None,
                    size: f.get("size").and_then(|x| x.as_u64()),
                    modified: f.get("client_modified").and_then(|x| x.as_str()).map(str::to_string),
                })
                .collect())
        }
        _ => Err(format!("unknown provider {}", def.id)),
    }
}

/// Download one file's bytes. Google Workspace files are exported
/// as PDF (the ingest pipeline's strongest format); everything else
/// downloads as-is. Returns (bytes, effective file name).
pub async fn download_file(
    def: &ProviderDef,
    access_token: &str,
    file: &RemoteFile,
) -> Result<(Vec<u8>, String), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    match def.id {
        "google" => {
            let is_workspace = file
                .mime_type
                .as_deref()
                .map(|m| m.starts_with("application/vnd.google-apps."))
                .unwrap_or(false);
            if is_workspace {
                let bytes = client
                    .get(format!(
                        "https://www.googleapis.com/drive/v3/files/{}/export",
                        file.id
                    ))
                    .bearer_auth(access_token)
                    .query(&[("mimeType", "application/pdf")])
                    .send()
                    .await
                    .map_err(|e| e.to_string())?
                    .error_for_status()
                    .map_err(|e| format!("Drive export failed: {e}"))?
                    .bytes()
                    .await
                    .map_err(|e| e.to_string())?;
                let name = if file.name.to_lowercase().ends_with(".pdf") {
                    file.name.clone()
                } else {
                    format!("{}.pdf", file.name)
                };
                Ok((bytes.to_vec(), name))
            } else {
                let bytes = client
                    .get(format!(
                        "https://www.googleapis.com/drive/v3/files/{}",
                        file.id
                    ))
                    .bearer_auth(access_token)
                    .query(&[("alt", "media")])
                    .send()
                    .await
                    .map_err(|e| e.to_string())?
                    .error_for_status()
                    .map_err(|e| format!("Drive download failed: {e}"))?
                    .bytes()
                    .await
                    .map_err(|e| e.to_string())?;
                Ok((bytes.to_vec(), file.name.clone()))
            }
        }
        "microsoft" => {
            let bytes = client
                .get(format!(
                    "https://graph.microsoft.com/v1.0/me/drive/items/{}/content",
                    file.id
                ))
                .bearer_auth(access_token)
                .send()
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| format!("OneDrive download failed: {e}"))?
                .bytes()
                .await
                .map_err(|e| e.to_string())?;
            Ok((bytes.to_vec(), file.name.clone()))
        }
        "dropbox" => {
            let bytes = client
                .post("https://content.dropboxapi.com/2/files/download")
                .bearer_auth(access_token)
                .header(
                    "Dropbox-API-Arg",
                    serde_json::json!({ "path": file.id }).to_string(),
                )
                .send()
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| format!("Dropbox download failed: {e}"))?
                .bytes()
                .await
                .map_err(|e| e.to_string())?;
            Ok((bytes.to_vec(), file.name.clone()))
        }
        _ => Err(format!("unknown provider {}", def.id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (ConnectorStore, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "connectors-test-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        (ConnectorStore::load(path.clone()), path)
    }

    #[test]
    fn providers_cover_drive_onedrive_dropbox() {
        let ids: Vec<&str> = providers().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["google", "microsoft", "dropbox"]);
        assert!(provider("netdocs").is_none(), "partner-only, not an OAuth provider");
    }

    #[test]
    fn connection_roundtrip_and_nonce_single_use() {
        let (s, path) = store();
        assert!(s.connection("a@test.io", "google").is_none());
        s.set_connection(
            "a@test.io",
            "google",
            StoredConnection {
                access_token: "tok".into(),
                refresh_token: Some("ref".into()),
                account_email: Some("a@test.io".into()),
                expires_at: None,
            },
        );
        let got = s.connection("a@test.io", "google").unwrap();
        assert_eq!(got.account_email.as_deref(), Some("a@test.io"));
        // Another user sees nothing.
        assert!(s.connection("b@test.io", "google").is_none());
        // Nonces are single-use.
        let n = s.new_nonce("a@test.io", "google");
        assert_eq!(s.take_nonce(&n).unwrap().0, "a@test.io");
        assert!(s.take_nonce(&n).is_none());
        assert!(s.remove_connection("a@test.io", "google"));
        assert!(!s.remove_connection("a@test.io", "google"));
        let _ = std::fs::remove_file(&path);
    }
}
