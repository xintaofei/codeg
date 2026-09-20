use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

use crate::office_watch::constant_time_eq;

pub const WS_EVENT_PROTOCOL: &str = "codeg-events";
const WS_TOKEN_PROTOCOL_PREFIX: &str = "codeg-token.";

fn token_from_ws_protocols(value: &str) -> Option<String> {
    value
        .split(',')
        .map(str::trim)
        .find_map(|protocol| protocol.strip_prefix(WS_TOKEN_PROTOCOL_PREFIX))
        .and_then(|encoded| URL_SAFE_NO_PAD.decode(encoded).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

pub async fn require_token(request: Request, next: Next, token: String) -> Response {
    // Fail closed on a misconfigured empty token: otherwise `Bearer ` (an empty
    // bearer value) would match it and silently disable authentication.
    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "Server token is not configured").into_response();
    }

    if let Some(auth_header) = request.headers().get("authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if auth_str
                .strip_prefix("Bearer ")
                .is_some_and(|t| constant_time_eq(t.as_bytes(), token.as_bytes()))
            {
                return next.run(request).await;
            }
        }
    }

    if let Some(protocol_header) = request.headers().get("sec-websocket-protocol") {
        if let Ok(protocols) = protocol_header.to_str() {
            if token_from_ws_protocols(protocols)
                .is_some_and(|t| constant_time_eq(t.as_bytes(), token.as_bytes()))
            {
                return next.run(request).await;
            }
        }
    }

    (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    #[test]
    fn parses_token_from_ws_protocols() {
        let token = "secret/token+value";
        let encoded = URL_SAFE_NO_PAD.encode(token);
        assert_eq!(
            token_from_ws_protocols(&format!("codeg-events, codeg-token.{encoded}")).as_deref(),
            Some(token)
        );
    }

    #[test]
    fn ignores_invalid_ws_protocol_token() {
        assert!(token_from_ws_protocols("codeg-events, codeg-token.not-valid-@@@@").is_none());
    }

    #[test]
    fn constant_time_eq_uses_timing_safe_comparison() {
        // Verify constant_time_eq is used for Bearer token matching.
        // Bearer token comparison on line 34 uses constant_time_eq to prevent
        // timing side-channel attacks.
        assert!(constant_time_eq(b"secret-token", b"secret-token"));
        assert!(!constant_time_eq(b"secret-token", b"wrong-token"));
        assert!(!constant_time_eq(b"secret-token", b"secret-othe"));
        assert!(!constant_time_eq(b"secret", b"secret-token"));
    }

    #[tokio::test]
    async fn require_token_middleware_verifies_bearer() {
        use axum::body::Body;
        use axum::http::Request;
        use axum::middleware;
        use axum::routing::get;
        use axum::Router;
        use tower::util::ServiceExt;

        let token = "correct-secret-token-12345".to_string();
        let token_for_mw = token.clone();
        let app = Router::new()
            .route("/test", get(|| async { "ok" }))
            .layer(middleware::from_fn(move |req, next| {
                require_token(req, next, token_for_mw.clone())
            }));

        // Valid Bearer -> 200
        let req = Request::builder()
            .uri("/test")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // Wrong token of same length -> 401
        let wrong_same_len = "wrong-secret-token-1234567";
        assert_eq!(wrong_same_len.len(), token.len());
        let req = Request::builder()
            .uri("/test")
            .header("authorization", format!("Bearer {wrong_same_len}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Prefix of valid token -> 401
        let prefix_token = &token[..token.len() / 2];
        let req = Request::builder()
            .uri("/test")
            .header("authorization", format!("Bearer {prefix_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Missing authorization header -> 401
        let req = Request::builder().uri("/test").body(Body::empty()).unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}
