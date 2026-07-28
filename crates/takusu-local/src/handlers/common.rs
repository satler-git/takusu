use axum::http::HeaderMap;

/// Extract an idempotency key from the `idempotency-key` header.
pub fn operation_id(headers: &HeaderMap) -> Option<&str> {
    headers.get("idempotency-key").and_then(|v| v.to_str().ok())
}
