//! Minimal HS256 JWT signing/verification helpers.
//!
//! Uses `hmac` + `sha2` so it works on native targets and `wasm32-unknown-unknown`
//! (Cloudflare Workers). No `jsonwebtoken`/`ring` dependency, avoiding WASM
//! compatibility issues.

use web_time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// Default JWT audience.
pub const DEFAULT_AUD: &str = "takusu";
/// Default JWT issuer.
pub const DEFAULT_ISS: &str = "takusu";
/// Default scope for regular tokens.
pub const SCOPE_READ_WRITE: &str = "read-write";
/// Scope for root/admin tokens.
pub const SCOPE_ROOT: &str = "root";

/// Default TTL for regular tokens (90 days).
pub const DEFAULT_TOKEN_TTL_SECONDS: i64 = 60 * 60 * 24 * 90;
/// Default TTL for root tokens (1 year).
pub const DEFAULT_ROOT_TOKEN_TTL_SECONDS: i64 = 60 * 60 * 24 * 365;
/// Clock skew leeway for `iat` and `exp` checks (60 seconds).
const CLOCK_SKEW_LEEWAY_SECONDS: i64 = 60;

const ALG: &str = "HS256";
const TYP: &str = "JWT";

/// JWT claims used by takusu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// Subject / user identifier. For takusu this is the same as `jti`.
    pub sub: String,
    /// JWT ID; stored in the `tokens` table for revocation.
    pub jti: String,
    /// Token scope: `root` or `read-write`.
    pub scope: String,
    /// Optional human-readable label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Audience.
    pub aud: String,
    /// Issuer.
    pub iss: String,
    /// Issued at (Unix seconds).
    pub iat: i64,
    /// Expiration (Unix seconds). `None` means no expiration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<i64>,
}

impl Claims {
    /// Returns true if the token has the `root` scope.
    pub fn is_root(&self) -> bool {
        self.scope == SCOPE_ROOT
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    alg: String,
    typ: String,
}

/// Errors that can occur during JWT signing or verification.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JwtError {
    #[error("invalid JWT format")]
    InvalidFormat,
    #[error("invalid base64 encoding")]
    InvalidBase64,
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
    #[error("unsupported JWT algorithm")]
    UnsupportedAlgorithm,
    #[error("invalid JWT signature")]
    InvalidSignature,
    #[error("JWT expired")]
    Expired,
    #[error("JWT issued-at is in the future")]
    IssuedAtFuture,
    #[error("system clock error")]
    ClockError,
    #[error("invalid audience: expected {expected}, got {actual}")]
    InvalidAudience { expected: String, actual: String },
    #[error("invalid issuer: expected {expected}, got {actual}")]
    InvalidIssuer { expected: String, actual: String },
    #[error("missing scope claim")]
    MissingScope,
}

fn now_seconds() -> Result<i64, JwtError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .map_err(|_| JwtError::ClockError)
}

fn sign_message(secret: &str, message: &str) -> String {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any size");
    mac.update(message.as_bytes());
    let result = mac.finalize();
    let bytes = result.into_bytes();
    URL_SAFE_NO_PAD.encode(bytes)
}

fn verify_message(secret: &str, message: &str, signature: &str) -> Result<(), JwtError> {
    type HmacSha256 = Hmac<Sha256>;
    // HMAC accepts keys of any size; this error is unreachable.
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any size");
    mac.update(message.as_bytes());
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(signature.as_bytes())
        .map_err(|_| JwtError::InvalidSignature)?;
    mac.verify_slice(&sig_bytes)
        .map_err(|_| JwtError::InvalidSignature)
}

/// Sign `claims` with `secret` using HS256, returning a compact JWT string.
pub fn sign(secret: &str, claims: &Claims) -> Result<String, JwtError> {
    if claims.scope.is_empty() {
        return Err(JwtError::MissingScope);
    }
    let header = Header {
        alg: ALG.to_string(),
        typ: TYP.to_string(),
    };
    let header_json =
        serde_json::to_string(&header).map_err(|e| JwtError::InvalidJson(e.to_string()))?;
    let claims_json =
        serde_json::to_string(claims).map_err(|e| JwtError::InvalidJson(e.to_string()))?;
    let header_b64 = URL_SAFE_NO_PAD.encode(header_json);
    let claims_b64 = URL_SAFE_NO_PAD.encode(claims_json);
    let message = format!("{header_b64}.{claims_b64}");
    let signature = sign_message(secret, &message);
    Ok(format!("{message}.{signature}"))
}

/// Verify a JWT signed with `secret`, checking audience `aud` and expiration.
pub fn verify(secret: &str, token: &str, aud: &str) -> Result<Claims, JwtError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(JwtError::InvalidFormat);
    }

    let header_json = URL_SAFE_NO_PAD
        .decode(parts[0].as_bytes())
        .map_err(|_| JwtError::InvalidBase64)?;
    let claims_json = URL_SAFE_NO_PAD
        .decode(parts[1].as_bytes())
        .map_err(|_| JwtError::InvalidBase64)?;

    let message = format!("{}.{}", parts[0], parts[1]);
    verify_message(secret, &message, parts[2])?;

    let header: Header =
        serde_json::from_slice(&header_json).map_err(|e| JwtError::InvalidJson(e.to_string()))?;
    if header.alg != ALG || header.typ != TYP {
        return Err(JwtError::UnsupportedAlgorithm);
    }

    let claims: Claims =
        serde_json::from_slice(&claims_json).map_err(|e| JwtError::InvalidJson(e.to_string()))?;

    if claims.scope.is_empty() {
        return Err(JwtError::MissingScope);
    }
    if claims.aud != aud {
        return Err(JwtError::InvalidAudience {
            expected: aud.to_string(),
            actual: claims.aud.clone(),
        });
    }
    if claims.iss != DEFAULT_ISS {
        return Err(JwtError::InvalidIssuer {
            expected: DEFAULT_ISS.to_string(),
            actual: claims.iss.clone(),
        });
    }

    let now = now_seconds()?;
    if claims.iat > now.saturating_add(CLOCK_SKEW_LEEWAY_SECONDS) {
        return Err(JwtError::IssuedAtFuture);
    }
    if let Some(exp) = claims.exp
        && now > exp.saturating_add(CLOCK_SKEW_LEEWAY_SECONDS)
    {
        return Err(JwtError::Expired);
    }

    Ok(claims)
}

/// Encode a byte slice as a lowercase hex string without per-byte allocations.
pub fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(HEX[(b >> 4) as usize]));
        s.push(char::from(HEX[(b & 0xf) as usize]));
    }
    s
}

/// Generate a 256-bit signing secret as a hex string.
pub fn generate_secret() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("failed to get random bytes");
    hex(&bytes)
}

fn new_jti() -> String {
    format!("tsk_{}", uuid::Uuid::now_v7())
}

/// Generate a root JWT with `scope=root`. Defaults to 1-year expiration.
pub fn generate_root_jwt(secret: &str, label: Option<&str>) -> Result<String, JwtError> {
    let now = now_seconds()?;
    let jti = new_jti();
    let claims = Claims {
        sub: jti.clone(),
        jti,
        scope: SCOPE_ROOT.to_string(),
        label: label.map(|s| s.to_string()),
        aud: DEFAULT_AUD.to_string(),
        iss: DEFAULT_ISS.to_string(),
        iat: now,
        exp: Some(now.saturating_add(DEFAULT_ROOT_TOKEN_TTL_SECONDS)),
    };
    sign(secret, &claims)
}

/// Generate a regular token JWT. Returns `(jwt, jti)`.
///
/// `expires_at` is an optional Unix timestamp. When `None` the token expires
/// after [`DEFAULT_TOKEN_TTL_SECONDS`].
pub fn generate_token_jwt(
    secret: &str,
    scope: &str,
    label: Option<&str>,
    expires_at: Option<i64>,
) -> Result<(String, String), JwtError> {
    let now = now_seconds()?;
    let jti = new_jti();
    let exp = expires_at.unwrap_or_else(|| now.saturating_add(DEFAULT_TOKEN_TTL_SECONDS));
    let claims = Claims {
        sub: jti.clone(),
        jti: jti.clone(),
        scope: scope.to_string(),
        label: label.map(|s| s.to_string()),
        aud: DEFAULT_AUD.to_string(),
        iss: DEFAULT_ISS.to_string(),
        iat: now,
        exp: Some(exp),
    };
    let token = sign(secret, &claims)?;
    Ok((token, jti))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_round_trip() {
        let secret = generate_secret();
        let (token, _jti) =
            generate_token_jwt(&secret, SCOPE_READ_WRITE, Some("test"), None).unwrap();
        let claims = verify(&secret, &token, DEFAULT_AUD).unwrap();
        assert_eq!(claims.scope, SCOPE_READ_WRITE);
        assert_eq!(claims.label.as_deref(), Some("test"));
    }

    #[test]
    fn verify_fails_with_wrong_secret() {
        let secret = generate_secret();
        let (token, _jti) = generate_token_jwt(&secret, SCOPE_READ_WRITE, None, None).unwrap();
        let wrong = generate_secret();
        assert!(matches!(
            verify(&wrong, &token, DEFAULT_AUD),
            Err(JwtError::InvalidSignature)
        ));
    }

    #[test]
    fn verify_fails_with_tampered_payload() {
        let secret = generate_secret();
        let (mut token, _jti) = generate_token_jwt(&secret, SCOPE_READ_WRITE, None, None).unwrap();
        // Append a character to the payload; this should break the signature.
        token.push('x');
        assert!(verify(&secret, &token, DEFAULT_AUD).is_err());
    }

    #[test]
    fn expired_token_is_rejected() {
        let secret = generate_secret();
        let jti = new_jti();
        let claims = Claims {
            sub: jti.clone(),
            jti,
            scope: SCOPE_READ_WRITE.to_string(),
            label: None,
            aud: DEFAULT_AUD.to_string(),
            iss: DEFAULT_ISS.to_string(),
            iat: 0,
            exp: Some(1),
        };
        let token = sign(&secret, &claims).unwrap();
        assert!(matches!(
            verify(&secret, &token, DEFAULT_AUD),
            Err(JwtError::Expired)
        ));
    }

    #[test]
    fn wrong_audience_is_rejected() {
        let secret = generate_secret();
        let (token, _jti) = generate_token_jwt(&secret, SCOPE_READ_WRITE, None, None).unwrap();
        assert!(matches!(
            verify(&secret, &token, "wrong-aud"),
            Err(JwtError::InvalidAudience { .. })
        ));
    }

    #[test]
    fn root_jwt_has_root_scope() {
        let secret = generate_secret();
        let token = generate_root_jwt(&secret, None).unwrap();
        let claims = verify(&secret, &token, DEFAULT_AUD).unwrap();
        assert!(claims.is_root());
    }
}
