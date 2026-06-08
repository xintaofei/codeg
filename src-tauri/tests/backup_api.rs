//! Backup & restore HTTP API integration tests.
//!
//! Exercises the full web round-trip against the real Axum router:
//! create export → download the archive → upload it back → inspect → stage a
//! restore. Uses an on-disk DB (VACUUM INTO can't snapshot a `sqlite::memory:`
//! pool) and `axum-test::TestServer` (no TCP socket).

use std::sync::Arc;

use axum_test::multipart::{MultipartForm, Part};
use axum_test::TestServer;
use codeg_lib::app_state::AppState;
use codeg_lib::db::test_helpers::fresh_disk_db;
use codeg_lib::web::router::build_router;
use codeg_lib::web::shutdown::ShutdownSignal;
use serde_json::{json, Value};

const TEST_TOKEN: &str = "backup-test-token";

fn auth() -> (&'static str, String) {
    ("authorization", format!("Bearer {TEST_TOKEN}"))
}

async fn build_server() -> (TestServer, tempfile::TempDir, tempfile::TempDir) {
    let data_dir = tempfile::tempdir().expect("data dir");
    let static_dir = tempfile::tempdir().expect("static dir");

    let db = fresh_disk_db(data_dir.path()).await;
    // Seed a row so the snapshot has observable content.
    codeg_lib::db::service::folder_service::add_folder(&db.conn, "/tmp/proj")
        .await
        .expect("seed folder");

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

#[tokio::test]
async fn export_download_upload_inspect_stage_roundtrip() {
    let (server, data_dir, _static) = build_server().await;
    let (k, v) = auth();

    // 1. Create an export → download ticket.
    let resp = server
        .post("/api/backup_create_ticket")
        .add_header(k, &v)
        .json(&json!({ "includeExternalTranscripts": false }))
        .await;
    assert_eq!(resp.status_code(), 200, "create ticket: {:?}", resp.text());
    let ticket: Value = resp.json();
    let url = ticket["url"].as_str().expect("url");
    assert!(url.starts_with("/api/backup_download/"));

    // 2. Download the archive (public route, no token needed).
    let dl = server.get(url).await;
    assert_eq!(dl.status_code(), 200);
    let bytes = dl.as_bytes().to_vec();
    assert_eq!(&bytes[..2], b"PK", "plaintext archive should be a ZIP");

    // 3. Upload it back.
    let form = MultipartForm::new().add_part(
        "file",
        Part::bytes(bytes).file_name("codeg-backup.codeg.zip"),
    );
    let up = server
        .post("/api/backup_upload")
        .add_header(k, &v)
        .multipart(form)
        .await;
    assert_eq!(up.status_code(), 200, "upload: {:?}", up.text());
    let upload_id = up.json::<Value>()["uploadId"]
        .as_str()
        .expect("uploadId")
        .to_string();

    // 4. Inspect → compatible (uses our own latest migration).
    let ins = server
        .post("/api/backup_inspect")
        .add_header(k, &v)
        .json(&json!({ "uploadId": upload_id }))
        .await;
    assert_eq!(ins.status_code(), 200, "inspect: {:?}", ins.text());
    let preview: Value = ins.json();
    assert_eq!(preview["encrypted"], json!(false));
    assert_eq!(preview["compatible"], json!(true), "preview: {preview}");

    // 5. Stage a restore → needs restart + a pending marker is written.
    let stage = server
        .post("/api/backup_restore_stage")
        .add_header(k, &v)
        .json(&json!({ "uploadId": upload_id }))
        .await;
    assert_eq!(stage.status_code(), 200, "stage: {:?}", stage.text());
    assert_eq!(stage.json::<Value>()["needsRestart"], json!(true));
    assert!(data_dir
        .path()
        .join(".codeg-restore-pending.json")
        .is_file());
}

#[tokio::test]
async fn inspect_rejects_invalid_upload_id() {
    let (server, _data, _static) = build_server().await;
    let (k, v) = auth();
    let resp = server
        .post("/api/backup_inspect")
        .add_header(k, &v)
        .json(&json!({ "uploadId": "../../etc/passwd" }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn endpoints_require_token() {
    let (server, _data, _static) = build_server().await;
    let resp = server
        .post("/api/backup_create_ticket")
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 401);
}
