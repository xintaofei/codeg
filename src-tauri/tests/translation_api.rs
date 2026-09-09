//! HTTP API integration tests for the content-translation middleware (PR1).
//!
//! Same harness as `api_integration.rs`: the real Axum router wired to an
//! in-memory SQLite database, driven through `axum-test::TestServer`.
//!
//! Scope:
//! - Auth matrix on a translation endpoint (they are protected like the rest)
//! - Settings save/get roundtrip, including the masked API key
//! - Identity translation returns `skipped: true` without contacting any
//!   endpoint (the text is already in the target language)
//! - Translating while unconfigured fails with a configuration error
//!
//! Not covered: live endpoint traffic (unit tests in `src/translation/` drive
//! stub loopback endpoints for that).

use std::sync::Arc;

use axum_test::TestServer;
use codeg_lib::app_state::AppState;
use codeg_lib::db::test_helpers::fresh_in_memory_db;
use codeg_lib::web::router::build_router;
use codeg_lib::web::shutdown::ShutdownSignal;
use serde_json::{json, Value};

const TEST_TOKEN: &str = "integration-test-token";

async fn build_test_server() -> (TestServer, tempfile::TempDir, tempfile::TempDir) {
    let data_dir = tempfile::tempdir().expect("data dir");
    let static_dir = tempfile::tempdir().expect("static dir");

    let db = fresh_in_memory_db().await;
    let state = Arc::new(AppState::new_for_test(db, data_dir.path().to_path_buf()));
    let shutdown = Arc::new(ShutdownSignal::new());

    let router = build_router(
        state,
        TEST_TOKEN.to_string(),
        static_dir.path().to_path_buf(),
        shutdown,
    );

    let server = TestServer::new(router).expect("test server");
    (server, data_dir, static_dir)
}

fn auth_header() -> String {
    format!("Bearer {TEST_TOKEN}")
}

/// Enabled settings whose endpoint is never contacted by these tests: the
/// identity test below short-circuits before the network, by construction.
fn enabled_settings() -> Value {
    json!({
        "enabled": true,
        "baseUrl": "https://api.example.com/v1",
        "apiKey": "sk-secret",
        "model": "gpt-4o-mini",
        "targetLang": "zh-CN",
        "translateBody": true,
    })
}

async fn save_settings(server: &TestServer, settings: &Value) -> Value {
    let resp = server
        .post("/api/translation_update_settings")
        .add_header("authorization", auth_header())
        .json(&json!({ "settings": settings }))
        .await;
    assert_eq!(resp.status_code(), 200, "settings save must succeed");
    resp.json::<Value>()
}

// ────────────────────────────────────────────────────────────────────────────
// Auth matrix
// ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn translation_endpoints_reject_missing_token() {
    let (server, _data, _static) = build_test_server().await;
    let resp = server
        .post("/api/translation_get_settings")
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn translation_endpoints_reject_wrong_token() {
    let (server, _data, _static) = build_test_server().await;
    let resp = server
        .post("/api/translation_translate")
        .add_header("authorization", "Bearer wrong-token")
        .json(&json!({ "texts": ["hello"] }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

// ────────────────────────────────────────────────────────────────────────────
// Settings roundtrip
// ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn settings_save_get_roundtrip_masks_the_key() {
    let (server, _data, _static) = build_test_server().await;

    let saved = save_settings(&server, &enabled_settings()).await;
    assert!(saved["enabled"].as_bool().unwrap_or(false));
    assert_eq!(
        saved["apiKey"].as_str().unwrap_or_default(),
        "••••••••",
        "the saved reply is masked for display"
    );

    let resp = server
        .post("/api/translation_get_settings")
        .add_header("authorization", auth_header())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 200);
    let read: Value = resp.json();
    assert!(read["enabled"].as_bool().unwrap_or(false));
    assert_eq!(read["model"].as_str().unwrap_or_default(), "gpt-4o-mini");
    assert_eq!(
        read["apiKey"].as_str().unwrap_or_default(),
        "••••••••",
        "the stored key never leaves the backend"
    );
}

#[tokio::test]
async fn an_unconfigured_install_reads_as_disabled() {
    let (server, _data, _static) = build_test_server().await;
    let resp = server
        .post("/api/translation_get_settings")
        .add_header("authorization", auth_header())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 200);
    let read: Value = resp.json();
    assert!(!read["enabled"].as_bool().unwrap_or(true));
    assert_eq!(read["baseUrl"].as_str().unwrap_or_default(), "");
}

// ────────────────────────────────────────────────────────────────────────────
// Translate
// ────────────────────────────────────────────────────────────────────────────

/// The identity short-circuit is observable end to end: an already-Chinese
/// text against a zh-CN target comes back verbatim with `skipped: true`,
/// without contacting the (fictional) endpoint — no request is possible, so
/// the test would hang or fail visibly if the short-circuit leaked.
#[tokio::test]
async fn identity_translation_returns_skipped_true() {
    let (server, _data, _static) = build_test_server().await;
    save_settings(&server, &enabled_settings()).await;

    let text = "Git merge 是一个纯知识性问题，直接保留原文即可。";
    let resp = server
        .post("/api/translation_translate")
        .add_header("authorization", auth_header())
        .json(&json!({
            "texts": [text],
            "uiLocale": "zh-CN",
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "the identity path must succeed");
    let body: Value = resp.json();
    let results = body.as_array().expect("array of results");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["text"].as_str().unwrap_or_default(), text);
    assert!(
        results[0]["skipped"].as_bool().unwrap_or(false),
        "the identity short-circuit must be observable over HTTP"
    );
    assert!(!results[0]["fromCache"].as_bool().unwrap_or(true));
    assert!(
        results[0].get("error").is_none(),
        "a skipped slot carries no error"
    );
}

/// Translating while the feature was never configured must fail with an
/// actionable configuration error — not a network error, not a 200 with an
/// error slot (there is no batch to be fault tolerant about: the feature
/// itself is off).
#[tokio::test]
async fn translating_while_unconfigured_fails_with_a_configuration_error() {
    let (server, _data, _static) = build_test_server().await;
    let resp = server
        .post("/api/translation_translate")
        .add_header("authorization", auth_header())
        .json(&json!({
            "texts": ["hello"],
            "uiLocale": "zh-CN",
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "ConfigurationMissing maps to 422");
    let body: Value = resp.json();
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("not enabled"),
        "the actionable message surfaces: {body}"
    );
}

/// An enabled feature with no callable endpoint is the same configuration
/// hole: the request must fail up front, before any network attempt.
#[tokio::test]
async fn translating_without_a_callable_endpoint_fails_up_front() {
    let (server, _data, _static) = build_test_server().await;
    // A row that is enabled but names no endpoint: `validate` must refuse the
    // save itself, so this doubles as the validation contract on this route.
    let resp = server
        .post("/api/translation_update_settings")
        .add_header("authorization", auth_header())
        .json(&json!({ "settings": json!({
            "enabled": true,
            "targetLang": "zh-CN",
        }) }))
        .await;
    assert_eq!(resp.status_code(), 422);
    let body: Value = resp.json();
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("provider"),
        "the validation message names the missing endpoint: {body}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Observability routes
// ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cache_stats_and_metrics_routes_answer_with_pr1_shapes() {
    let (server, _data, _static) = build_test_server().await;

    let resp = server
        .post("/api/translation_cache_stats")
        .add_header("authorization", auth_header())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 200);
    let stats: Value = resp.json();
    // The cache stats shape: memory/disk entry counts plus bytes.
    assert!(stats.get("memoryEntries").is_some(), "{stats}");
    assert!(stats.get("diskEntries").is_some(), "{stats}");
    assert!(stats.get("diskBytes").is_some(), "{stats}");

    let resp = server
        .post("/api/translation_metrics")
        .add_header("authorization", auth_header())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 200);
    let metrics: Value = resp.json();
    // PR1 metrics are one flat row — no per-provider tables, no series.
    assert!(metrics.get("dispatchedTotal").is_some(), "{metrics}");
    assert!(metrics.get("okTotal").is_some(), "{metrics}");
    assert!(metrics.get("failedTotal").is_some(), "{metrics}");
    assert!(metrics.get("gateRejectedTotal").is_some(), "{metrics}");
    assert!(metrics.get("cacheHits").is_some(), "{metrics}");
    assert!(metrics.get("servedTotal").is_some(), "{metrics}");
    assert!(metrics.get("avgLatencyMs").is_some(), "{metrics}");
    assert!(
        metrics.get("providers").is_none() && metrics.get("series").is_none(),
        "per-provider shapes come back with the rotation pool, not in PR1: {metrics}"
    );
}

#[tokio::test]
async fn pool_status_route_answers_with_the_single_endpoint_shape() {
    let (server, _data, _static) = build_test_server().await;
    save_settings(&server, &enabled_settings()).await;

    let resp = server
        .post("/api/translation_pool_status")
        .add_header("authorization", auth_header())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 200);
    let status: Value = resp.json();
    // One object (not the pool-era array): configured + runtime state.
    assert!(status["configured"].as_bool().unwrap_or(false));
    assert_eq!(status["consecutiveFailures"].as_u64().unwrap_or(1), 0);
    assert_eq!(status["cooldownRemainingMs"].as_u64().unwrap_or(1), 0);
    assert!(!status["disabled"].as_bool().unwrap_or(true));
    assert!(
        status["providerId"].is_string(),
        "the selected endpoint is named: {status}"
    );
}
