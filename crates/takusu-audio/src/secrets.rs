//! Secret-bearing newtypes for TTS provider configuration.
//!
//! `ApiKey` masks its value in `Debug` output so that credentials are not
//! leaked via `{:?}` formatting or accidental log statements. `EndpointUrl`
//! validates URL syntax at construction time so that malformed endpoints are
//! rejected before the first HTTP request.
//!
//! Both types serialize as plain strings so existing configuration files keep
//! working unchanged.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

// ── ApiKey ──────────────────────────────────────────────────────────────

/// An API key whose `Debug` representation is masked to prevent accidental
/// leakage through logs or `{:?}` formatting.
///
/// `Display` exposes the underlying value so the key can still be used to
/// build authorization headers. Use `as_str()` for the raw value and
/// `is_empty()` to check for the empty (unset) state.
#[derive(Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApiKey(String);

impl ApiKey {
    /// Wrap an existing string into an `ApiKey` without validation.
    ///
    /// API keys are opaque tokens whose format is provider-specific, so no
    /// structural validation is performed. An empty string represents an
    /// unset key.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Return the raw key value.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the key is the empty (unset) sentinel.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never emit the raw key. Showing the length is safe and useful for
        // debugging "did the key load at all?" without exposing the value.
        if self.0.is_empty() {
            f.write_str("ApiKey(\"\")")
        } else {
            f.debug_tuple("ApiKey")
                .field(&format_args!("<redacted, {} chars>", self.0.len()))
                .finish()
        }
    }
}

impl fmt::Display for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ApiKey {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ApiKey {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl FromStr for ApiKey {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

// ── EndpointUrl ─────────────────────────────────────────────────────────

/// A URL validated at construction time.
///
/// Wraps a `String` but guarantees the value parsed successfully as an
/// absolute [`url::Url`], so malformed endpoints are caught at config load
/// rather than at the first HTTP request. Relative URLs (e.g. `/tts/bytes`)
/// are rejected by `url::Url::parse`. The original string form is preserved
/// so that absolute URLs with query strings are not normalized away.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct EndpointUrl(String);

/// Error returned when a string cannot be parsed as a valid URL.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid endpoint URL: {0}")]
pub struct EndpointUrlError(#[from] url::ParseError);

impl EndpointUrl {
    /// Parse and wrap a URL string.
    pub fn new(url: impl Into<String>) -> Result<Self, EndpointUrlError> {
        let s = url.into();
        // Validate by parsing; the normalized form is discarded so that the
        // original string (including any trailing path/query) is preserved
        // exactly as configured.
        let _ = url::Url::parse(&s)?;
        Ok(Self(s))
    }

    /// Parse and wrap a URL string, falling back to `default` on an empty
    /// input. Non-empty invalid inputs still return an error.
    pub fn new_or_default(url: impl Into<String>, default: &str) -> Result<Self, EndpointUrlError> {
        let s = url.into();
        if s.trim().is_empty() {
            Self::new(default)
        } else {
            Self::new(s)
        }
    }

    /// Return the URL as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for EndpointUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // URLs may carry credentials in the userinfo component
        // (e.g. `https://user:pass@host/`). Mask that section so `Debug`
        // output cannot leak secrets, while still showing the host/path.
        f.debug_tuple("EndpointUrl")
            .field(&mask_userinfo(&self.0))
            .finish()
    }
}

/// Replace the userinfo portion of a URL string with `***` for safe display.
///
/// Returns the original string unchanged when it has no userinfo or cannot be
/// parsed. The scheme, host, port, path, and query are preserved.
fn mask_userinfo(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    // `Url` only exposes userinfo via `username()` / `password()`; clearing
    // them requires `set_username` / `set_password`, which mutate in place.
    let has_userinfo = !parsed.username().is_empty() || parsed.password().is_some();
    if !has_userinfo {
        return url.to_string();
    }
    // `set_username` / `set_password` can fail for special schemes like
    // `file:`; for those, fall back to the original string.
    if parsed.set_username("***").is_err() {
        return url.to_string();
    }
    let _ = parsed.set_password(None);
    parsed.to_string()
}

impl fmt::Display for EndpointUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for EndpointUrl {
    type Err = EndpointUrlError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for EndpointUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for EndpointUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        EndpointUrl::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ApiKey ──────────────────────────────────────────────────────────

    #[test]
    fn api_key_debug_masks_value() {
        let key = ApiKey::new("sk-secret-1234567890");
        let debug = format!("{key:?}");
        assert!(debug.contains("ApiKey"), "debug: {debug}");
        assert!(!debug.contains("sk-secret"), "debug leaked value: {debug}");
        assert!(debug.contains("redacted"), "debug: {debug}");
    }

    #[test]
    fn api_key_debug_shows_empty_for_empty() {
        let key = ApiKey::new("");
        assert_eq!(format!("{key:?}"), r#"ApiKey("")"#);
    }

    #[test]
    fn api_key_display_exposes_value() {
        let key = ApiKey::new("sk-secret");
        assert_eq!(format!("{key}"), "sk-secret");
    }

    #[test]
    fn api_key_as_str_and_is_empty() {
        let key = ApiKey::new("sk-secret");
        assert_eq!(key.as_str(), "sk-secret");
        assert!(!key.is_empty());

        let empty = ApiKey::default();
        assert!(empty.is_empty());
        assert_eq!(empty.as_str(), "");
    }

    #[test]
    fn api_key_from_string_and_str() {
        let from_string = ApiKey::from(String::from("sk-1"));
        let from_str = ApiKey::from("sk-2");
        assert_eq!(from_string.as_str(), "sk-1");
        assert_eq!(from_str.as_str(), "sk-2");
    }

    #[test]
    fn api_key_serde_roundtrip() {
        let key = ApiKey::new("sk-secret");
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, r#""sk-secret""#);
        let back: ApiKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, key);
    }

    #[test]
    fn api_key_from_str_infallible() {
        let key: ApiKey = "sk-secret".parse().unwrap();
        assert_eq!(key.as_str(), "sk-secret");
    }

    // ── EndpointUrl ─────────────────────────────────────────────────────

    #[test]
    fn endpoint_url_accepts_valid_url() {
        let url = EndpointUrl::new("https://api.cartesia.ai/tts/bytes").unwrap();
        assert_eq!(url.as_str(), "https://api.cartesia.ai/tts/bytes");
    }

    #[test]
    fn endpoint_url_rejects_invalid_url() {
        assert!(EndpointUrl::new("not a url").is_err());
        assert!(EndpointUrl::new("://missing-scheme").is_err());
    }

    #[test]
    fn endpoint_url_new_or_default_uses_default_for_empty() {
        let url = EndpointUrl::new_or_default("", "https://default.example.com").unwrap();
        assert_eq!(url.as_str(), "https://default.example.com");
    }

    #[test]
    fn endpoint_url_new_or_default_uses_input_when_non_empty() {
        let url =
            EndpointUrl::new_or_default("https://api.example.com", "https://default.example.com")
                .unwrap();
        assert_eq!(url.as_str(), "https://api.example.com");
    }

    #[test]
    fn endpoint_url_new_or_default_rejects_invalid_non_empty() {
        assert!(EndpointUrl::new_or_default("not a url", "https://default.example.com").is_err());
    }

    #[test]
    fn endpoint_url_debug_shows_value_without_userinfo() {
        let url = EndpointUrl::new("https://api.example.com").unwrap();
        let debug = format!("{url:?}");
        assert!(debug.contains("https://api.example.com"));
    }

    #[test]
    fn endpoint_url_debug_masks_userinfo() {
        let url = EndpointUrl::new("https://user:secret@api.example.com/path").unwrap();
        let debug = format!("{url:?}");
        assert!(
            !debug.contains("secret") && !debug.contains("user:"),
            "debug leaked userinfo: {debug}"
        );
        assert!(debug.contains("api.example.com"), "debug: {debug}");
        assert!(debug.contains("***"), "debug: {debug}");
    }

    #[test]
    fn endpoint_url_debug_masks_userinfo_without_password() {
        let url = EndpointUrl::new("https://user@api.example.com/path").unwrap();
        let debug = format!("{url:?}");
        assert!(!debug.contains("user@"), "debug leaked userinfo: {debug}");
        assert!(debug.contains("***"), "debug: {debug}");
    }

    #[test]
    fn endpoint_url_display_exposes_userinfo() {
        // Display intentionally preserves the original string so that the
        // URL can still be used to build HTTP requests.
        let url = EndpointUrl::new("https://user:secret@api.example.com").unwrap();
        assert_eq!(format!("{url}"), "https://user:secret@api.example.com");
    }

    #[test]
    fn endpoint_url_display_shows_value() {
        let url = EndpointUrl::new("https://api.example.com").unwrap();
        assert_eq!(format!("{url}"), "https://api.example.com");
    }

    #[test]
    fn endpoint_url_serde_roundtrip() {
        let url = EndpointUrl::new("https://api.example.com").unwrap();
        let json = serde_json::to_string(&url).unwrap();
        assert_eq!(json, r#""https://api.example.com""#);
        let back: EndpointUrl = serde_json::from_str(&json).unwrap();
        assert_eq!(back, url);
    }

    #[test]
    fn endpoint_url_serde_rejects_invalid() {
        let err = serde_json::from_str::<EndpointUrl>(r#""not a url""#);
        assert!(err.is_err());
    }

    #[test]
    fn endpoint_url_from_str_roundtrip() {
        let url: EndpointUrl = "https://api.example.com".parse().unwrap();
        assert_eq!(url.as_str(), "https://api.example.com");
    }
}
