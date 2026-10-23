//! JWT issuance and validation (HS256).

use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use legal_rag_common::{Error, Result};
use serde::{Deserialize, Serialize};

/// Token type marker: access vs refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    Access,
    Refresh,
}

/// JWT claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub email: String,
    pub kind: TokenKind,
    pub exp: i64,
    pub iat: i64,
}

/// An issued token pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
}

fn encode_claims(claims: &Claims, secret: &str) -> Result<String> {
    encode(&Header::default(), claims, &EncodingKey::from_secret(secret.as_bytes()))
        .map_err(|e| Error::Auth(format!("token encoding failed: {e}")))
}

/// Issue an access + refresh token pair for a user.
pub fn issue_token_pair(
    user_id: &str,
    email: &str,
    secret: &str,
    access_ttl_secs: i64,
    refresh_ttl_secs: i64,
) -> Result<TokenPair> {
    let now = Utc::now().timestamp();
    let access = Claims {
        sub: user_id.to_string(),
        email: email.to_string(),
        kind: TokenKind::Access,
        iat: now,
        exp: now + access_ttl_secs,
    };
    let refresh = Claims {
        kind: TokenKind::Refresh,
        exp: now + refresh_ttl_secs,
        ..access.clone()
    };
    Ok(TokenPair {
        access_token: encode_claims(&access, secret)?,
        refresh_token: encode_claims(&refresh, secret)?,
        token_type: "Bearer".to_string(),
        expires_in: access_ttl_secs,
    })
}

/// Validate a token and enforce its expected kind.
pub fn validate_token(token: &str, secret: &str, expected: TokenKind) -> Result<Claims> {
    let mut validation = Validation::default();
    validation.validate_exp = true;
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|_| Error::Auth("invalid or expired token".into()))?;
    if data.claims.kind != expected {
        return Err(Error::Auth("wrong token kind".into()));
    }
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrip_and_kind_enforcement() {
        let pair = issue_token_pair("u1", "a@b.c", "secret", 900, 3600).unwrap();
        let claims = validate_token(&pair.access_token, "secret", TokenKind::Access).unwrap();
        assert_eq!(claims.sub, "u1");
        assert!(validate_token(&pair.access_token, "secret", TokenKind::Refresh).is_err());
        assert!(validate_token(&pair.refresh_token, "secret", TokenKind::Refresh).is_ok());
        assert!(validate_token(&pair.access_token, "wrong", TokenKind::Access).is_err());
    }
}
