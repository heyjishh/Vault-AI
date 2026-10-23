//! Auth0-compatible OIDC client (authorization code flow).
//!
//! Mirrors the reference product's login wall: the app redirects to the
//! identity provider, which returns an authorization code that is exchanged
//! for verified ID-token claims.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use legal_rag_common::{Error, Result};
use openidconnect::core::{CoreClient, CoreProviderMetadata, CoreResponseType};
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, IssuerUrl, Nonce, RedirectUrl, Scope, TokenResponse,
};

/// OIDC client with in-memory nonce tracking for the login handshake.
#[derive(Clone)]
pub struct OidcClient {
    inner: CoreClient<
        EndpointSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointMaybeSet,
        EndpointMaybeSet,
    >,
    http_client: reqwest::Client,
    nonces: Arc<RwLock<HashMap<String, Nonce>>>,
}

impl OidcClient {
    /// Discover provider metadata (e.g. `https://<tenant>.us.auth0.com`) and build the client.
    pub async fn discover(
        issuer: &str,
        client_id: &str,
        client_secret: Option<&str>,
        redirect_uri: &str,
    ) -> Result<Self> {
        let http_client = reqwest::Client::new();
        let issuer_url = IssuerUrl::new(issuer.to_string())
            .map_err(|e| Error::Config(format!("bad issuer: {e}")))?;
        let metadata = CoreProviderMetadata::discover_async(issuer_url, &http_client)
            .await
            .map_err(|e| Error::Upstream(format!("OIDC discovery failed: {e}")))?;
        let secret = client_secret.map(|s| ClientSecret::new(s.to_string()));
        let inner =
            CoreClient::from_provider_metadata(metadata, ClientId::new(client_id.to_string()), secret)
                .set_redirect_uri(
                    RedirectUrl::new(redirect_uri.to_string())
                        .map_err(|e| Error::Config(format!("bad redirect uri: {e}")))?,
                );
        Ok(Self { inner, http_client, nonces: Arc::new(RwLock::new(HashMap::new())) })
    }

    /// Build the provider login URL; returns `(url, state)` where `state`
    /// must be round-tripped back to [`OidcClient::callback`].
    pub fn login_url(&self) -> (String, String) {
        let (url, csrf, nonce) = self
            .inner
            .authorize_url(
                AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("openid".to_string()))
            .add_scope(Scope::new("profile".to_string()))
            .add_scope(Scope::new("email".to_string()))
            .url();
        let state = csrf.secret().clone();
        self.nonces.write().expect("lock poisoned").insert(state.clone(), nonce);
        (url.to_string(), state)
    }

    /// Exchange the authorization `code`, verify the ID token, and return the
    /// subject + email from its claims.
    pub async fn callback(&self, code: &str, state: &str) -> Result<(String, String)> {
        let nonce = self
            .nonces
            .write()
            .expect("lock poisoned")
            .remove(state)
            .ok_or_else(|| Error::Auth("unknown or expired OIDC state".into()))?;
        let token_response = self
            .inner
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .map_err(|e| Error::Config(format!("token endpoint not configured: {e}")))?
            .request_async(&self.http_client)
            .await
            .map_err(|e| Error::Upstream(format!("code exchange failed: {e}")))?;
        let id_token = token_response
            .id_token()
            .ok_or_else(|| Error::Auth("no id token returned".into()))?;
        let claims = id_token
            .claims(&self.inner.id_token_verifier(), &nonce)
            .map_err(|e| Error::Auth(format!("id token verification failed: {e}")))?;
        let email = claims
            .email()
            .map(|e| e.to_string())
            .ok_or_else(|| Error::Auth("id token has no email claim".into()))?;
        Ok((claims.subject().to_string(), email))
    }
}
