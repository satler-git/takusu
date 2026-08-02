mod common;

use common::*;

#[test]
#[ignore]
fn auth_integration() {
    let _g = start_wrangler();
    let token = root_token();

    // /health requires no auth
    {
        let (status, body) = http_get("/health", None).unwrap();
        assert_eq!(status, 200, "body: {body}");
        assert_eq!(body, "ok");
    }

    // /api/auth/verify — no auth → 401
    {
        let (status, body) = http_get("/api/auth/verify", None).unwrap();
        assert_eq!(status, 401, "body: {body}");
        assert!(body.contains("unauthorized"), "body: {body}");
    }

    // /api/auth/verify — root token → 200
    {
        let (status, _body) = http_get("/api/auth/verify", Some(&token)).unwrap();
        assert_eq!(status, 200);
    }

    // /api/auth/verify — bad token → 401
    {
        let (status, body) = http_get("/api/auth/verify", Some("tsk_invalid_token")).unwrap();
        assert_eq!(status, 401, "body: {body}");
        assert!(body.contains("unauthorized"), "body: {body}");
    }

    // /api/tasks — no auth → 401
    {
        let (status, body) = http_get("/api/tasks", None).unwrap();
        assert_eq!(status, 401, "body: {body}");
        assert!(body.contains("unauthorized"), "body: {body}");
    }

    // /api/tasks — bad token → 401
    {
        let (status, body) = http_get("/api/tasks", Some("tsk_wrong")).unwrap();
        assert_eq!(status, 401, "body: {body}");
        assert!(body.contains("unauthorized"), "body: {body}");
    }

    // /api/tasks — root token → 200
    {
        let (status, _body) = http_get("/api/tasks", Some(&token)).unwrap();
        assert_eq!(status, 200);
    }

    // /api/habits — no auth → 401
    {
        let (status, body) = http_get("/api/habits", None).unwrap();
        assert_eq!(status, 401, "body: {body}");
        assert!(body.contains("unauthorized"), "body: {body}");
    }

    // /api/schedule — no auth → 401
    {
        let (status, body) = http_get("/api/schedule", None).unwrap();
        assert_eq!(status, 401, "body: {body}");
        assert!(body.contains("unauthorized"), "body: {body}");
    }

    // /api/settings — no auth → 401
    {
        let (status, body) = http_get("/api/settings", None).unwrap();
        assert_eq!(status, 401, "body: {body}");
        assert!(body.contains("unauthorized"), "body: {body}");
    }

    // /api/tokens — no auth → 401
    {
        let (status, body) = http_get("/api/tokens", None).unwrap();
        assert_eq!(status, 401, "body: {body}");
        assert!(body.contains("unauthorized"), "body: {body}");
    }
}
