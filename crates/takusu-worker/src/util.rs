//! Pure utility functions shared between the WASM worker and native tests.
//!
//! These functions do not depend on `worker` / `wasm_bindgen` so they can be
//! compiled and tested on the host target.

use sha2::{Digest, Sha256};

/// SHA-256 hash of a token, returned as lowercase hex.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let result = hasher.finalize();
    takusu_types::jwt::hex(&result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_token_is_deterministic() {
        let a = hash_token("hello");
        let b = hash_token("hello");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_token_output_format() {
        let hash = hash_token("test-token");
        // SHA-256 hex: 64 chars, all lowercase hex
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }

    #[test]
    fn hash_token_different_inputs_differ() {
        let a = hash_token("token-a");
        let b = hash_token("token-b");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_token_empty_string() {
        let hash = hash_token("");
        assert_eq!(hash.len(), 64);
        // Known SHA-256 of empty string
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
