use axum::extract::Multipart;
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use std::collections::BTreeMap;

use crate::app_error::{
    AppCommandError, UPLOAD_I18N_KEY_QUOTA_EXCEEDED, UPLOAD_I18N_KEY_TOO_LARGE,
};
use crate::commands::folders as folder_commands;
use crate::paths::codeg_uploads_root;

// ---------------------------------------------------------------------------
// Param structs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadFilePreviewParams {
    pub root_path: String,
    pub path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadFileBase64Params {
    pub path: String,
    pub max_bytes: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadFileForEditParams {
    pub root_path: String,
    pub path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveFileContentParams {
    pub root_path: String,
    pub path: String,
    pub content: String,
    pub expected_etag: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveFileCopyParams {
    pub root_path: String,
    pub path: String,
    pub content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameFileTreeEntryParams {
    pub root_path: String,
    pub path: String,
    pub new_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteFileTreeEntryParams {
    pub root_path: String,
    pub path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFileTreeEntryParams {
    pub root_path: String,
    pub path: String,
    pub name: String,
    pub kind: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn read_file_preview(
    Json(params): Json<ReadFilePreviewParams>,
) -> Result<Json<folder_commands::FilePreviewContent>, AppCommandError> {
    let result = folder_commands::read_file_preview(params.root_path, params.path).await?;
    Ok(Json(result))
}

pub async fn read_file_base64(
    Json(params): Json<ReadFileBase64Params>,
) -> Result<Json<String>, AppCommandError> {
    let result = folder_commands::read_file_base64(params.path, params.max_bytes).await?;
    Ok(Json(result))
}

pub async fn read_file_for_edit(
    Json(params): Json<ReadFileForEditParams>,
) -> Result<Json<folder_commands::FileEditContent>, AppCommandError> {
    let result = folder_commands::read_file_for_edit(params.root_path, params.path).await?;
    Ok(Json(result))
}

pub async fn save_file_content(
    Json(params): Json<SaveFileContentParams>,
) -> Result<Json<folder_commands::FileSaveResult>, AppCommandError> {
    let result = folder_commands::save_file_content(
        params.root_path,
        params.path,
        params.content,
        params.expected_etag,
    )
    .await?;
    Ok(Json(result))
}

pub async fn save_file_copy(
    Json(params): Json<SaveFileCopyParams>,
) -> Result<Json<folder_commands::FileSaveResult>, AppCommandError> {
    let result =
        folder_commands::save_file_copy(params.root_path, params.path, params.content).await?;
    Ok(Json(result))
}

pub async fn rename_file_tree_entry(
    Json(params): Json<RenameFileTreeEntryParams>,
) -> Result<Json<String>, AppCommandError> {
    let result =
        folder_commands::rename_file_tree_entry(params.root_path, params.path, params.new_name)
            .await?;
    Ok(Json(result))
}

pub async fn delete_file_tree_entry(
    Json(params): Json<DeleteFileTreeEntryParams>,
) -> Result<Json<()>, AppCommandError> {
    folder_commands::delete_file_tree_entry(params.root_path, params.path).await?;
    Ok(Json(()))
}

pub async fn create_file_tree_entry(
    Json(params): Json<CreateFileTreeEntryParams>,
) -> Result<Json<String>, AppCommandError> {
    let result = folder_commands::create_file_tree_entry(
        params.root_path,
        params.path,
        params.name,
        params.kind,
    )
    .await?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Attachment upload
// ---------------------------------------------------------------------------

/// Hard cap on a single uploaded attachment.
///
/// Aligned with axum's default 2MB multipart body limit and with the practical
/// constraint that the file is later embedded as context for an AI agent —
/// anything larger would not fit a typical model's context window anyway.
/// The check inside the streaming loop is defense-in-depth: axum's
/// `DefaultBodyLimit` rejects the request before reaching here, but a future
/// limit change must not silently allow oversized writes to disk.
pub const UPLOAD_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Env-controlled soft cap on the *total* bytes resident under
/// `uploads_root/`. Per-file `UPLOAD_MAX_BYTES` bounds one payload; this
/// bounds long-term accumulation so a compromised or shared token can't
/// repeatedly upload small files until the host runs out of disk. Unset
/// or `0` disables the cap — preserves the original "no GC" behavior
/// for operators who want it.
///
/// The check is intentionally conservative: it fires before any bytes
/// are streamed to disk, assuming the worst-case `UPLOAD_MAX_BYTES`.
/// That over-rejects in the last `UPLOAD_MAX_BYTES` of headroom (e.g. a
/// 100 KB upload may get rejected when only 1 MB remains under the
/// cap), but it keeps the code free of mid-stream cleanup races and
/// gives operators a hard ceiling.
const UPLOAD_TOTAL_BYTES_ENV: &str = "CODEG_UPLOAD_MAX_TOTAL_BYTES";

fn upload_total_max_bytes_from_env() -> Option<u64> {
    parse_upload_total_max_bytes(std::env::var(UPLOAD_TOTAL_BYTES_ENV).ok().as_deref())
}

/// Pure-function form of the env parser so unit tests don't need to
/// mutate process-global state (which would race the test harness's
/// concurrent runner).
fn parse_upload_total_max_bytes(raw: Option<&str>) -> Option<u64> {
    let parsed: u64 = raw?.trim().parse().ok()?;
    if parsed == 0 {
        None
    } else {
        Some(parsed)
    }
}

/// Sum the size of every regular file under `uploads_root/` except the
/// `.tmp/` staging directory. Walks at most one level of buckets — that
/// is the structure produced by `stream_and_finalize` — but the inner
/// walk follows whatever entries exist, so a hand-edited deeper tree
/// is still counted faithfully.
///
/// Failures during the walk are logged and skipped: a permission error
/// on one file shouldn't block the upload pipeline. The returned total
/// is a lower bound in that case, which means the cap may admit one
/// extra upload before tripping. That's strictly better than refusing
/// to serve.
async fn current_uploads_total_bytes(uploads_root: &std::path::Path) -> u64 {
    let mut total: u64 = 0;
    let mut bucket_iter = match tokio::fs::read_dir(uploads_root).await {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 0,
        Err(e) => {
            eprintln!(
                "[uploads] failed to enumerate uploads root {}: {}",
                uploads_root.display(),
                e
            );
            return 0;
        }
    };
    while let Some(entry) = bucket_iter.next_entry().await.transpose() {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[uploads] read_dir entry error: {e}");
                continue;
            }
        };
        let name = entry.file_name();
        if name == ".tmp" {
            // Staging files are unreferenced and purged at startup —
            // exclude them so a partial upload doesn't inflate the
            // counter and reject the very next request.
            continue;
        }
        let file_type = match entry.file_type().await {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if file_type.is_file() {
            // A loose file at the top level (legacy layout or admin
            // copy-in) still counts.
            if let Ok(meta) = entry.metadata().await {
                total = total.saturating_add(meta.len());
            }
            continue;
        }
        if !file_type.is_dir() {
            continue;
        }
        let mut file_iter = match tokio::fs::read_dir(entry.path()).await {
            Ok(it) => it,
            Err(_) => continue,
        };
        while let Some(f) = file_iter.next_entry().await.transpose() {
            let f = match f {
                Ok(f) => f,
                Err(_) => continue,
            };
            if let Ok(meta) = f.metadata().await {
                if meta.is_file() {
                    total = total.saturating_add(meta.len());
                }
            }
        }
    }
    total
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadAttachmentResult {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub mime_type: Option<String>,
}

/// Sanitize a client-supplied filename so it lands inside the target
/// directory and contains no shell-hostile bytes.
///
/// Strategy: keep only the final path component, strip everything that isn't
/// a safe printable character, and bound the length. The caller still prefixes
/// a UUID, so an empty result is fine — `"<uuid>-"` alone is a valid name.
fn sanitize_upload_filename(raw: &str) -> String {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| c == '.' || c.is_whitespace());
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            other => other,
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let limited: String = trimmed.chars().take(120).collect();
    if limited.is_empty() {
        "file".to_string()
    } else {
        limited
    }
}

/// Sanitize a session identifier used as the upload bucket directory name.
///
/// Different semantics from filenames: a session id should never contain `.`
/// or whitespace, so reuse of `sanitize_upload_filename` would silently merge
/// distinct sessions whose ids degenerate to an empty string. Only allow
/// `[A-Za-z0-9_-]`; everything else collapses to `_`. Empty input falls back
/// to `"anon"`.
fn sanitize_session_bucket(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| match c {
            c if c.is_ascii_alphanumeric() => c,
            '-' | '_' => c,
            _ => '_',
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    let limited: String = trimmed.chars().take(80).collect();
    if limited.is_empty() {
        "anon".to_string()
    } else {
        limited
    }
}

/// Confirm `candidate` resolves (after symlink expansion) inside `root`.
/// Returns the canonical path on success. Both paths must exist on disk.
async fn ensure_path_inside(
    candidate: &std::path::Path,
    root: &std::path::Path,
) -> Result<std::path::PathBuf, AppCommandError> {
    let candidate_canon = tokio::fs::canonicalize(candidate).await.map_err(|e| {
        AppCommandError::io_error("Failed to canonicalize upload path")
            .with_detail(e.to_string())
    })?;
    let root_canon = tokio::fs::canonicalize(root).await.map_err(|e| {
        AppCommandError::io_error("Failed to canonicalize uploads root")
            .with_detail(e.to_string())
    })?;
    if !candidate_canon.starts_with(&root_canon) {
        return Err(AppCommandError::io_error(
            "Resolved upload path escapes uploads root",
        )
        .with_detail(candidate_canon.to_string_lossy().to_string()));
    }
    Ok(candidate_canon)
}

/// Remove any leftover staging files in `<uploads_root>/.tmp/`.
///
/// Called once at server startup. Staging files represent in-flight uploads
/// that were interrupted by a crash/restart — they are unreferenced by
/// definition and safe to drop. Distinct from the per-bucket history under
/// `<uploads_root>/<bucket>/`, which the user explicitly opted to retain.
///
/// Failures are logged and swallowed: a missing `.tmp/` directory is the
/// expected case on a fresh install, and permission issues should not block
/// the server from starting.
pub async fn purge_upload_staging() {
    let tmp_dir = codeg_uploads_root().join(".tmp");
    match tokio::fs::remove_dir_all(&tmp_dir).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            eprintln!(
                "[uploads] failed to purge staging dir {}: {}",
                tmp_dir.display(),
                e
            );
        }
    }
}

pub async fn upload_attachment(
    mut multipart: Multipart,
) -> Result<Json<UploadAttachmentResult>, AppCommandError> {
    let uploads_root = codeg_uploads_root();
    // Ensure root exists before canonicalize/ensure_path_inside can compare.
    tokio::fs::create_dir_all(&uploads_root).await.map_err(|e| {
        AppCommandError::io_error("Failed to create uploads root")
            .with_detail(e.to_string())
    })?;

    // Quota check, before staging any bytes. We assume the worst-case
    // payload size (`UPLOAD_MAX_BYTES`) since the actual size isn't
    // known until the multipart body is drained — admitting a request
    // we'd reject mid-stream would waste disk and require cleanup races.
    if let Some(cap) = upload_total_max_bytes_from_env() {
        let used = current_uploads_total_bytes(&uploads_root).await;
        let projected = used.saturating_add(UPLOAD_MAX_BYTES);
        if projected > cap {
            let mut params = BTreeMap::new();
            params.insert("used".to_string(), used.to_string());
            params.insert("limit".to_string(), cap.to_string());
            return Err(AppCommandError::io_error(
                "Upload quota exceeded for this server",
            )
            .with_detail(format!("used={used} limit={cap}"))
            .with_i18n(UPLOAD_I18N_KEY_QUOTA_EXCEEDED, params));
        }
    }

    // Pre-stage the file under <uploads_root>/.tmp/<uuid>.part so we can
    // stream bytes to disk without knowing the final bucket up front (the
    // session_id form field may arrive after the file). On success we rename
    // into place; on any error we delete it.
    let tmp_dir = uploads_root.join(".tmp");
    tokio::fs::create_dir_all(&tmp_dir).await.map_err(|e| {
        AppCommandError::io_error("Failed to create tmp directory")
            .with_detail(e.to_string())
    })?;
    let staging_id = uuid::Uuid::new_v4().simple().to_string();
    let staging_path = tmp_dir.join(format!("{staging_id}.part"));

    // Wrap the streaming work so any early return cleans up the staged file.
    let result = stream_and_finalize(&mut multipart, &uploads_root, &staging_path).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&staging_path).await;
    }
    result.map(Json)
}

/// Drain the multipart body and produce the final upload result. Splits out
/// of `upload_attachment` so a single staging-file cleanup wraps every early
/// return.
async fn stream_and_finalize(
    multipart: &mut Multipart,
    uploads_root: &std::path::Path,
    staging_path: &std::path::Path,
) -> Result<UploadAttachmentResult, AppCommandError> {
    let mut session_id: Option<String> = None;
    let mut raw_name: Option<String> = None;
    let mut mime_type: Option<String> = None;
    let mut written: u64 = 0;
    let mut file_seen = false;

    while let Some(mut field) = multipart.next_field().await.map_err(|e| {
        AppCommandError::io_error("Invalid multipart upload").with_detail(e.to_string())
    })? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "session_id" | "sessionId" => {
                let value = field.text().await.map_err(|e| {
                    AppCommandError::io_error("Failed to read session_id field")
                        .with_detail(e.to_string())
                })?;
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    session_id = Some(sanitize_session_bucket(trimmed));
                }
            }
            "file" => {
                if file_seen {
                    return Err(AppCommandError::io_error(
                        "Multiple `file` fields are not supported per request",
                    ));
                }
                file_seen = true;
                raw_name = Some(field.file_name().unwrap_or("file").to_string());
                mime_type = field.content_type().map(|s| s.to_string());

                let mut out = tokio::fs::File::create(staging_path).await.map_err(|e| {
                    AppCommandError::io_error("Failed to create staging file")
                        .with_detail(e.to_string())
                })?;
                while let Some(chunk) = field.chunk().await.map_err(|e| {
                    AppCommandError::io_error("Failed to read upload chunk")
                        .with_detail(e.to_string())
                })? {
                    let new_total = written.saturating_add(chunk.len() as u64);
                    if new_total > UPLOAD_MAX_BYTES {
                        // Symmetric with the proxy's pre/post-decode caps
                        // in `commands/remote_proxy.rs`: any of the three
                        // layers can fire first depending on how the
                        // request reached us (web direct, Tauri-proxied,
                        // or local path read), and they all surface as
                        // the same i18n key so the toast text in the UI
                        // is uniform.
                        let mut params = BTreeMap::new();
                        params.insert("size".to_string(), new_total.to_string());
                        params.insert("limit".to_string(), UPLOAD_MAX_BYTES.to_string());
                        return Err(AppCommandError::io_error(
                            "Upload exceeds the maximum allowed size",
                        )
                        .with_detail(format!(
                            "size={new_total} limit={UPLOAD_MAX_BYTES}"
                        ))
                        .with_i18n(UPLOAD_I18N_KEY_TOO_LARGE, params));
                    }
                    out.write_all(&chunk).await.map_err(|e| {
                        AppCommandError::io_error("Failed to write chunk")
                            .with_detail(e.to_string())
                    })?;
                    written = new_total;
                }
                out.flush().await.map_err(|e| {
                    AppCommandError::io_error("Failed to flush staging file")
                        .with_detail(e.to_string())
                })?;
            }
            _ => {
                // Drain unknown fields to avoid stalling the multipart parser.
                let _ = field.bytes().await;
            }
        }
    }

    if !file_seen {
        return Err(AppCommandError::io_error(
            "Missing `file` field in multipart upload",
        ));
    }
    if written == 0 {
        return Err(AppCommandError::io_error("Uploaded file is empty"));
    }

    let safe_name = sanitize_upload_filename(raw_name.as_deref().unwrap_or("file"));
    let bucket = session_id.unwrap_or_else(|| "anon".to_string());
    let dir = uploads_root.join(&bucket);
    tokio::fs::create_dir_all(&dir).await.map_err(|e| {
        AppCommandError::io_error("Failed to create uploads directory")
            .with_detail(e.to_string())
    })?;

    let unique = uuid::Uuid::new_v4().simple().to_string();
    let final_name = format!("{}-{}", unique, safe_name);
    let final_path = dir.join(&final_name);

    tokio::fs::rename(&staging_path, &final_path)
        .await
        .map_err(|e| {
            AppCommandError::io_error("Failed to move staged upload into place")
                .with_detail(e.to_string())
        })?;

    // Defense in depth: even though every component above was sanitized, run
    // the final canonical path through the jail check so any future relaxing
    // of sanitization can't silently escape the uploads root.
    let canon = ensure_path_inside(&final_path, uploads_root).await?;

    Ok(UploadAttachmentResult {
        path: canon.to_string_lossy().to_string(),
        name: safe_name,
        size: written,
        mime_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_strips_traversal() {
        assert_eq!(sanitize_upload_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_upload_filename("..\\..\\boot.ini"), "boot.ini");
    }

    #[test]
    fn sanitize_filename_handles_empty_and_dots() {
        assert_eq!(sanitize_upload_filename("..."), "file");
        assert_eq!(sanitize_upload_filename(""), "file");
        assert_eq!(sanitize_upload_filename("   "), "file");
    }

    #[test]
    fn sanitize_filename_replaces_hostile_chars() {
        assert_eq!(sanitize_upload_filename("a:b*c?\"d"), "a_b_c__d");
    }

    #[test]
    fn sanitize_session_bucket_allows_safe_chars() {
        assert_eq!(sanitize_session_bucket("abc-123_XY"), "abc-123_XY");
    }

    #[test]
    fn sanitize_session_bucket_collapses_unsafe() {
        assert_eq!(sanitize_session_bucket("../etc"), "etc");
        assert_eq!(sanitize_session_bucket("...."), "anon");
        assert_eq!(sanitize_session_bucket(""), "anon");
    }

    // ─── current_uploads_total_bytes ───────────────────────────────────

    async fn write_bytes(path: &std::path::Path, n: usize) {
        tokio::fs::create_dir_all(path.parent().expect("parent"))
            .await
            .unwrap();
        tokio::fs::write(path, vec![0u8; n]).await.unwrap();
    }

    #[tokio::test]
    async fn current_uploads_total_bytes_is_zero_for_missing_root() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert_eq!(current_uploads_total_bytes(&missing).await, 0);
    }

    #[tokio::test]
    async fn current_uploads_total_bytes_sums_files_under_buckets() {
        let dir = tempfile::tempdir().unwrap();
        write_bytes(&dir.path().join("session-a/file1"), 100).await;
        write_bytes(&dir.path().join("session-a/file2"), 250).await;
        write_bytes(&dir.path().join("session-b/file3"), 700).await;
        assert_eq!(current_uploads_total_bytes(dir.path()).await, 1050);
    }

    #[tokio::test]
    async fn current_uploads_total_bytes_skips_staging_tmp() {
        // `.tmp/` holds in-flight uploads that get purged at server
        // startup; including them in the running total would let a
        // partially-streamed upload reject the very next request.
        let dir = tempfile::tempdir().unwrap();
        write_bytes(&dir.path().join(".tmp/staging.part"), 9999).await;
        write_bytes(&dir.path().join("session-a/file"), 5).await;
        assert_eq!(current_uploads_total_bytes(dir.path()).await, 5);
    }

    #[tokio::test]
    async fn current_uploads_total_bytes_counts_loose_top_level_files() {
        // Anything copied in by an admin or left by an older layout
        // still counts toward the cap so the quota stays honest.
        let dir = tempfile::tempdir().unwrap();
        write_bytes(&dir.path().join("legacy.bin"), 42).await;
        assert_eq!(current_uploads_total_bytes(dir.path()).await, 42);
    }

    // ─── parse_upload_total_max_bytes ─────────────────────────────────
    //
    // Tests the pure parser, NOT the env reader — mutating
    // `CODEG_UPLOAD_MAX_TOTAL_BYTES` from a test would race the harness's
    // parallel runner.

    #[test]
    fn parse_upload_total_max_bytes_handles_all_branches() {
        assert_eq!(parse_upload_total_max_bytes(None), None, "unset → None");
        assert_eq!(parse_upload_total_max_bytes(Some("")), None, "empty → None");
        assert_eq!(parse_upload_total_max_bytes(Some("   ")), None, "whitespace → None");
        assert_eq!(parse_upload_total_max_bytes(Some("0")), None, "zero → None");
        assert_eq!(
            parse_upload_total_max_bytes(Some("  1048576  ")),
            Some(1_048_576),
            "trim + parse"
        );
        assert_eq!(
            parse_upload_total_max_bytes(Some("not-a-number")),
            None,
            "invalid → None"
        );
        // Negative numbers don't fit u64 — parse fails, falls through.
        assert_eq!(
            parse_upload_total_max_bytes(Some("-1")),
            None,
            "negative → None"
        );
    }
}
