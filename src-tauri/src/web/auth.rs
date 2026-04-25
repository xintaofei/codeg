use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use subtle::ConstantTimeEq;

/// Constant-time comparison of two strings to prevent timing attacks.
/// Always compares full lengths to avoid leaking length information via early-exit timing.
#[inline]
fn constant_time_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

pub async fn require_token(request: Request, next: Next, token: String) -> Response {
    // Allow WebSocket upgrade requests to authenticate via query param.
    // The token value is URL-encoded by the client, so decode before comparing.
    if let Some(query) = request.uri().query() {
        for pair in query.split('&') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            if key != "token" {
                continue;
            }
            if let Ok(decoded) = urlencoding::decode(value) {
                if constant_time_eq(&decoded, &token) {
                    return next.run(request).await;
                }
            }
        }
    }

    // Check Authorization header
    if let Some(auth_header) = request.headers().get("authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(t) = auth_str.strip_prefix("Bearer ") {
                if constant_time_eq(t, &token) {
                    return next.run(request).await;
                }
            }
        }
    }

    (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response()
}
