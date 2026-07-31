use axum::http::HeaderMap;

/// Extract an idempotency key from the `Idempotency-Key` header.
///
/// Falls back to `idempotency-key` (lowercase) to match the worker handler.
pub fn operation_id(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("Idempotency-Key")
        .or_else(|| headers.get("idempotency-key"))
        .and_then(|v| v.to_str().ok())
}
