//! `/api/v1/connectors` (Piece 70): document-source connectors —
//! status, OAuth connect/callback, disconnect, file listing and
//! import into the Vault. See `crate::connectors`.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::Redirect;
use legal_rag_auth::AuthUser;
use serde::{Deserialize, Serialize};

use crate::connectors as conn;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Serialize)]
pub struct ProviderStatus {
    pub id: String,
    pub name: String,
    pub blurb: String,
    /// The operator has registered an OAuth app for this provider.
    pub configured: bool,
    pub connected: bool,
    pub account_email: Option<String>,
    /// Set for providers that cannot self-connect (partner APIs).
    pub note: Option<String>,
}

pub async fn list_connectors(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Json<Vec<ProviderStatus>> {
    let mut out: Vec<ProviderStatus> = conn::providers()
        .iter()
        .map(|def| {
            let c = state.connectors.connection(&user.email, def.id);
            ProviderStatus {
                id: def.id.to_string(),
                name: def.name.to_string(),
                blurb: def.blurb.to_string(),
                configured: conn::app_config(state.connectors.path(), def.id).is_some(),
                connected: c.is_some(),
                account_email: c.and_then(|x| x.account_email),
                note: None,
            }
        })
        .collect();
    out.push(ProviderStatus {
        id: "sharepoint".to_string(),
        name: "SharePoint".to_string(),
        blurb: "Import documents from SharePoint sites via Microsoft Graph.".to_string(),
        configured: conn::app_config(state.connectors.path(), "microsoft").is_some(),
        connected: state.connectors.connection(&user.email, "microsoft").is_some(),
        account_email: None,
        note: Some(
            "Connect through OneDrive above — SharePoint site libraries ride the same Microsoft connection; the site picker lands next."
                .to_string(),
        ),
    });
    out.push(ProviderStatus {
        id: "netdocs".to_string(),
        name: "NetDocuments".to_string(),
        blurb: "Import documents from NetDocuments workspaces.".to_string(),
        configured: false,
        connected: false,
        account_email: None,
        note: Some(
            "NetDocuments' API requires a partner agreement with NetDocuments — it cannot be connected self-serve."
                .to_string(),
        ),
    });
    Json(out)
}

#[derive(Serialize)]
pub struct AuthUrlView {
    pub url: String,
}

/// Build the provider's OAuth consent URL for the signed-in user.
pub async fn auth_url(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(provider_id): Path<String>,
) -> Result<Json<AuthUrlView>, ApiError> {
    let def = conn::provider(&provider_id).ok_or_else(|| {
        legal_rag_common::Error::NotFound(format!("provider {provider_id}"))
    })?;
    let (client_id, _secret) = conn::app_config(state.connectors.path(), def.id)
        .ok_or_else(|| {
            // BadRequest, not Config: the message IS the UI's
            // explanation of the setup step (Config maps to a bare
            // 500 "internal server error").
            legal_rag_common::Error::BadRequest(format!(
                "{} is not configured on this server — the operator must register an OAuth app and set {}_CLIENT_ID / {}_CLIENT_SECRET",
                def.name,
                def.id.to_uppercase(),
                def.id.to_uppercase()
            ))
        })?;
    let nonce = state.connectors.new_nonce(&user.email, def.id);
    let mut url = reqwest::Url::parse(def.auth_url)
        .map_err(|e| legal_rag_common::Error::Config(e.to_string()))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("client_id", &client_id);
        qp.append_pair("redirect_uri", &conn::redirect_uri(def.id));
        qp.append_pair("response_type", "code");
        qp.append_pair("scope", def.scope);
        qp.append_pair("state", &nonce);
        match def.id {
            // Google: offline access + consent screen so a refresh
            // token is actually issued.
            "google" => {
                qp.append_pair("access_type", "offline");
                qp.append_pair("prompt", "consent");
            }
            // Dropbox: offline token access for a refresh token.
            "dropbox" => {
                qp.append_pair("token_access_type", "offline");
            }
            _ => {}
        }
    }
    Ok(Json(AuthUrlView { url: url.to_string() }))
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// The OAuth redirect target (no session — the state nonce carries
/// the user). Exchanges the code, stores the connection, and sends
/// the browser back to the app.
pub async fn callback(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Query(q): Query<CallbackQuery>,
) -> Result<Redirect, ApiError> {
    let fail = Redirect::to("/?connector_error=1");
    let Some(def) = conn::provider(&provider_id) else {
        return Ok(fail);
    };
    if q.error.is_some() {
        return Ok(fail);
    }
    let (Some(code), Some(nonce)) = (q.code, q.state) else {
        return Ok(fail);
    };
    let Some((email, nonce_provider)) = state.connectors.take_nonce(&nonce) else {
        return Ok(fail);
    };
    if nonce_provider != def.id {
        return Ok(fail);
    }
    let Some((client_id, client_secret)) =
        conn::app_config(state.connectors.path(), def.id)
    else {
        return Ok(fail);
    };
    let Ok(mut stored) = conn::exchange_token(
        &def,
        &client_id,
        &client_secret,
        &[
            ("grant_type", "authorization_code".to_string()),
            ("code", code),
            ("redirect_uri", conn::redirect_uri(def.id)),
        ],
    )
    .await
    else {
        // A failed exchange lands the user back in the app with the
        // error flag, not on a bare 500 page.
        return Ok(fail);
    };
    stored.account_email = conn::fetch_account_email(&def, &stored.access_token).await;
    state.connectors.set_connection(&email, def.id, stored);
    Ok(Redirect::to("/?connected=1"))
}

pub async fn disconnect(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(provider_id): Path<String>,
) -> Json<serde_json::Value> {
    let removed = state.connectors.remove_connection(&user.email, &provider_id);
    Json(serde_json::json!({ "disconnected": removed }))
}

#[derive(Deserialize)]
pub struct FilesQuery {
    pub q: Option<String>,
}

pub async fn list_files(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(provider_id): Path<String>,
    Query(q): Query<FilesQuery>,
) -> Result<Json<Vec<conn::RemoteFile>>, ApiError> {
    let def = conn::provider(&provider_id).ok_or_else(|| {
        legal_rag_common::Error::NotFound(format!("provider {provider_id}"))
    })?;
    let token = conn::valid_token(&state.connectors, &user.email, def.id)
        .await
        .map_err(|e| legal_rag_common::Error::BadRequest(e))?;
    let files = conn::list_files(&def, &token, q.q.as_deref().unwrap_or(""))
        .await
        .map_err(|e| legal_rag_common::Error::BadRequest(e))?;
    Ok(Json(files))
}

#[derive(Deserialize)]
pub struct ImportRequest {
    pub file_id: String,
    pub name: String,
    pub mime_type: Option<String>,
    pub matter_id: Option<String>,
}

/// Download one file from the provider and ingest it into the
/// Vault through the exact same pipeline as a direct upload
/// (indexing, original retention, filing, billing).
pub async fn import_file(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(provider_id): Path<String>,
    Json(body): Json<ImportRequest>,
) -> Result<Json<crate::routes::vault::IngestReport>, ApiError> {
    let def = conn::provider(&provider_id).ok_or_else(|| {
        legal_rag_common::Error::NotFound(format!("provider {provider_id}"))
    })?;
    crate::routes::billing::ensure_quota(&state, &user.id)?;
    let token = conn::valid_token(&state.connectors, &user.email, def.id)
        .await
        .map_err(|e| legal_rag_common::Error::BadRequest(e))?;
    let remote = conn::RemoteFile {
        id: body.file_id.clone(),
        name: body.name.clone(),
        mime_type: body.mime_type.clone(),
        size: None,
        modified: None,
    };
    let (bytes, file_name) = conn::download_file(&def, &token, &remote)
        .await
        .map_err(|e| legal_rag_common::Error::BadRequest(e))?;
    let title = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem.to_string())
        .unwrap_or_else(|| file_name.clone());
    let upload: crate::routes::vault::UploadRequest = serde_json::from_value(
        serde_json::json!({
            "title": title,
            "file_name": file_name,
            "matter_id": body.matter_id,
        }),
    )
    .map_err(|e| legal_rag_common::Error::Config(e.to_string()))?;
    let report = crate::routes::vault::run_upload(&state, &user, upload, bytes, false).await?;
    Ok(Json(report))
}
