//! HTTP endpoints for uploading/downloading workspace files.
//!
//! These exist for issue #179 — the web/server build has no native file
//! dialogs, so the file-tree context menu needs network endpoints to move
//! bytes between the operator's browser and the workspace on disk. The
//! Tauri build keeps using the OS file picker, so the routes are gated to
//! web mode in the UI but live in the shared router so the desktop's
//! built-in web service is functional too.
//!
//! All three endpoints share the same path-safety contract: caller passes
//! a `root_path` (the absolute path of an opened workspace) plus a
//! relative path that must not contain `..` or absolute components. The
//! handler joins them, then `canonicalize`s and confirms the resolved
//! path starts with the canonical root, so a symlink inside the user's
//! workspace cannot redirect reads or writes outside it.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use axum::body::{Body, Bytes};
use axum::extract::{Extension, Multipart, Path as AxumPath};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::stream;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::workspace_transfer::{DownloadKind, DownloadTicketIssued, DownloadTicketSpec};

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadWorkspaceFileResult {
    pub path: String,
    pub name: String,
    pub size: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadWorkspaceParams {
    pub root_path: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTicketRequest {
    pub root_path: String,
    pub path: String,
    pub kind: DownloadKind,
}

// ---------------------------------------------------------------------------
// Path safety helpers
// ---------------------------------------------------------------------------

fn validate_relative_components(rel: &Path) -> Result<(), AppCommandError> {
    if rel.is_absolute() {
        return Err(AppCommandError::invalid_input("Path must be relative"));
    }
    for component in rel.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(AppCommandError::invalid_input("Path cannot contain '..'"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(AppCommandError::invalid_input("Invalid path component"));
            }
        }
    }
    Ok(())
}

fn resolve_relative_path(root: &Path, rel: &str) -> Result<PathBuf, AppCommandError> {
    let rel_path = Path::new(rel);
    validate_relative_components(rel_path)?;
    Ok(root.join(rel_path))
}

fn ensure_inside_root(root: &Path, target: &Path) -> Result<(), AppCommandError> {
    let canonical_root = std::fs::canonicalize(root).map_err(AppCommandError::io)?;
    let canonical_target = std::fs::canonicalize(target).map_err(AppCommandError::io)?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(AppCommandError::invalid_input(
            "Resolved path escapes workspace root",
        ));
    }
    Ok(())
}

/// Walk from `root` toward `target` one segment at a time and reject if any
/// already-existing component is a symlink. `target` must be a descendant of
/// `root` (callers compose it via `resolve_relative_path`).
///
/// This runs *before* `create_dir_all`, which would otherwise follow a
/// symlink mid-chain and silently create new directories outside the
/// workspace. The earlier post-hoc `canonicalize` check caught the
/// escape but the side-effect (empty dir at the symlink target) was
/// already on disk.
fn ensure_no_symlink_in_chain(root: &Path, target: &Path) -> Result<(), AppCommandError> {
    let rel = target
        .strip_prefix(root)
        .map_err(|_| AppCommandError::invalid_input("Target path is not under workspace root"))?;
    let mut current = root.to_path_buf();
    for component in rel.components() {
        let segment = match component {
            Component::Normal(s) => s,
            Component::CurDir => continue,
            _ => {
                return Err(AppCommandError::invalid_input(
                    "Invalid path component while validating upload target",
                ));
            }
        };
        current.push(segment);
        match std::fs::symlink_metadata(&current) {
            Ok(md) => {
                if md.file_type().is_symlink() {
                    return Err(AppCommandError::invalid_input(
                        "Upload path traverses a symlink; refuse to follow it",
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // The remainder of the path doesn't exist yet — nothing
                // for create_dir_all to follow into, so we're safe.
                return Ok(());
            }
            Err(e) => return Err(AppCommandError::io(e)),
        }
    }
    Ok(())
}

/// Strip cross-platform-hostile characters from a single path segment.
/// Empty / all-dots input collapses to `"file"` so the rename can succeed
/// even when the browser hands us a degenerate name.
fn sanitize_segment(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            other => other,
        })
        .collect();
    let trimmed = cleaned
        .trim_matches(|c: char| c.is_whitespace())
        .trim_end_matches('.');
    if trimmed.is_empty() || trimmed.chars().all(|c| c == '.') {
        "file".to_string()
    } else {
        trimmed.to_string()
    }
}

fn sanitize_relative_subpath(raw: &str) -> Result<String, AppCommandError> {
    let raw_parts: Vec<&str> = raw
        .split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if raw_parts.is_empty() {
        return Err(AppCommandError::invalid_input("Invalid upload path"));
    }
    // Reject parent-dir traversal *before* `sanitize_segment` collapses
    // it to "file" — otherwise the check would never fire and a request
    // for `../escape` would silently rewrite to `file/escape`, hiding the
    // operator's intent (and surprising whoever audits the resulting
    // path on disk).
    if raw_parts.contains(&"..") {
        return Err(AppCommandError::invalid_input("Path cannot contain '..'"));
    }
    let parts: Vec<String> = raw_parts.iter().map(|s| sanitize_segment(s)).collect();
    Ok(parts.join("/"))
}

fn header_safe_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_control() || c == '"' || c == '\\' {
                '_'
            } else if c.is_ascii() {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn attachment_header(name: &str) -> Option<HeaderValue> {
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        header_safe_filename(name),
        urlencoding::encode(name)
    ))
    .ok()
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

/// Stream a single file from the operator's browser into the workspace.
///
/// Expected multipart fields (order matters — text fields must precede
/// `file` so the handler can resolve the destination before any bytes
/// land on disk):
///   * `root_path` — absolute path of the opened workspace folder
///   * `target_path` — relative directory under `root_path` to upload
///     into. Empty / missing means workspace root.
///   * `relative_path` — optional relative path *including filename*
///     used for folder uploads to preserve directory structure. When
///     present, the browser's filename is ignored.
///   * `file` — the file payload.
pub async fn upload_workspace_file(
    Extension(state): Extension<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<UploadWorkspaceFileResult>, AppCommandError> {
    let _permit = state
        .workspace_transfer
        .workspace_upload_semaphore
        .acquire()
        .await
        .map_err(|_| {
            AppCommandError::task_execution_failed("Workspace upload semaphore is closed")
        })?;

    let mut root_path: Option<String> = None;
    let mut target_path: Option<String> = None;
    let mut relative_path: Option<String> = None;
    let mut result: Option<UploadWorkspaceFileResult> = None;

    while let Some(mut field) = multipart.next_field().await.map_err(|e| {
        AppCommandError::io_error("Invalid multipart upload").with_detail(e.to_string())
    })? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "root_path" | "rootPath" => {
                root_path = Some(field.text().await.map_err(|e| {
                    AppCommandError::io_error("Failed to read root_path field")
                        .with_detail(e.to_string())
                })?);
            }
            "target_path" | "targetPath" => {
                target_path = Some(field.text().await.map_err(|e| {
                    AppCommandError::io_error("Failed to read target_path field")
                        .with_detail(e.to_string())
                })?);
            }
            "relative_path" | "relativePath" => {
                relative_path = Some(field.text().await.map_err(|e| {
                    AppCommandError::io_error("Failed to read relative_path field")
                        .with_detail(e.to_string())
                })?);
            }
            "file" => {
                if result.is_some() {
                    return Err(AppCommandError::invalid_input(
                        "Multiple `file` fields are not supported per request",
                    ));
                }
                let root_str = root_path.as_deref().ok_or_else(|| {
                    AppCommandError::invalid_input(
                        "root_path field must appear before the file field",
                    )
                })?;
                let root = PathBuf::from(root_str);
                if !root.exists() || !root.is_dir() {
                    return Err(AppCommandError::not_found(
                        "Workspace folder does not exist",
                    ));
                }
                let canonical_root = std::fs::canonicalize(&root).map_err(AppCommandError::io)?;

                let file_name_hint = field
                    .file_name()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "file".to_string());

                let final_rel = compute_final_rel(
                    target_path.as_deref().unwrap_or(""),
                    relative_path.as_deref().unwrap_or(""),
                    &file_name_hint,
                )?;
                let final_abs = resolve_relative_path(&root, &final_rel)?;

                if let Some(parent) = final_abs.parent() {
                    // Reject *before* touching the filesystem if any
                    // existing component along the path is a symlink —
                    // otherwise `create_dir_all` would follow the link
                    // and create directories outside the workspace
                    // before the canonical check below could fire.
                    ensure_no_symlink_in_chain(&root, parent)?;
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        AppCommandError::io_error("Failed to create upload directory")
                            .with_detail(e.to_string())
                    })?;
                    let canonical_parent =
                        std::fs::canonicalize(parent).map_err(AppCommandError::io)?;
                    if !canonical_parent.starts_with(&canonical_root) {
                        return Err(AppCommandError::invalid_input(
                            "Resolved path escapes workspace root",
                        ));
                    }
                }

                if final_abs.is_dir() {
                    return Err(AppCommandError::invalid_input(
                        "Refusing to overwrite an existing directory with a file",
                    ));
                }
                if final_abs.exists() {
                    return Err(AppCommandError::already_exists(
                        "A file with this name already exists",
                    ));
                }

                let staging_name = format!(".codeg-upload-{}.part", uuid::Uuid::new_v4().simple());
                let staging_path = final_abs
                    .parent()
                    .map(|p| p.join(&staging_name))
                    .ok_or_else(|| {
                        AppCommandError::invalid_input("Cannot determine parent directory")
                    })?;

                let mut out = tokio::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&staging_path)
                    .await
                    .map_err(|e| {
                        AppCommandError::io_error("Failed to create staging file")
                            .with_detail(e.to_string())
                    })?;

                let mut written: u64 = 0;
                let stream_result: Result<(), AppCommandError> = async {
                    while let Some(chunk) = field.chunk().await.map_err(|e| {
                        AppCommandError::io_error("Failed to read upload chunk")
                            .with_detail(e.to_string())
                    })? {
                        let new_total = written.saturating_add(chunk.len() as u64);
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
                    Ok(())
                }
                .await;
                drop(out);

                if let Err(err) = stream_result {
                    let _ = tokio::fs::remove_file(&staging_path).await;
                    return Err(err);
                }

                // Empty files are valid in a workspace (`.gitkeep`,
                // `__init__.py`, placeholder configs) — only chat
                // attachments need the "must contain bytes" guard, since
                // those feed an LLM. Don't reject here.

                // Commit the staging file onto the final name atomically.
                // `hard_link` errors with `AlreadyExists` instead of
                // silently overwriting, which closes the TOCTOU window
                // that a bare `rename` leaves open on Unix (rename(2)
                // replaces an existing destination). On filesystems that
                // don't support hard links (Windows FAT32, cross-device,
                // some FUSE mounts) we fall back to `rename` — that path
                // still has the narrow race but it's the best we can do
                // there, and the user is uploading into their own
                // workspace so the race window has no security impact.
                let commit_method: &str;
                match tokio::fs::hard_link(&staging_path, &final_abs).await {
                    Ok(()) => {
                        commit_method = "hard_link";
                        let _ = tokio::fs::remove_file(&staging_path).await;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        let _ = tokio::fs::remove_file(&staging_path).await;
                        return Err(AppCommandError::already_exists(
                            "A file with this name already exists",
                        ));
                    }
                    Err(hard_link_err) => {
                        if let Err(e) = tokio::fs::rename(&staging_path, &final_abs).await {
                            let _ = tokio::fs::remove_file(&staging_path).await;
                            return Err(AppCommandError::io_error("Failed to commit upload")
                                .with_detail(format!(
                                    "hard_link_err={hard_link_err} rename_err={e}"
                                )));
                        }
                        commit_method = "rename";
                    }
                }

                // Defense in depth: re-check that the committed path is
                // inside the root. If a symlink got swapped under us, undo.
                if let Err(err) = ensure_inside_root(&root, &final_abs) {
                    let _ = tokio::fs::remove_file(&final_abs).await;
                    return Err(err);
                }

                // Sanity verification: the API has been observed to
                // return success while leaving nothing on disk. Stat the
                // final path BEFORE responding so a regression surfaces
                // as an error here instead of as a phantom file in the
                // tree that delete/edit can't touch. Use symlink_metadata
                // (NOT exists()) so a dangling link is detected too.
                match tokio::fs::symlink_metadata(&final_abs).await {
                    Ok(_) => {}
                    Err(err) => {
                        eprintln!(
                            "[workspace_files] upload commit verification FAILED: \
                             final_abs={} commit_method={} written={} err={}",
                            final_abs.display(),
                            commit_method,
                            written,
                            err
                        );
                        return Err(AppCommandError::io_error(
                            "Upload appeared to succeed but the file is missing",
                        )
                        .with_detail(format!(
                            "final_abs={} commit_method={} err={}",
                            final_abs.display(),
                            commit_method,
                            err
                        )));
                    }
                }

                let name = final_abs
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("file")
                    .to_string();

                result = Some(UploadWorkspaceFileResult {
                    path: final_rel,
                    name,
                    size: written,
                });
            }
            _ => {
                // Drain unknown fields to keep the parser moving.
                let _ = field.bytes().await;
            }
        }
    }

    result
        .ok_or_else(|| AppCommandError::invalid_input("Missing `file` field"))
        .map(Json)
}

fn compute_final_rel(
    target_dir: &str,
    relative_path: &str,
    file_name_hint: &str,
) -> Result<String, AppCommandError> {
    let target_dir_clean = target_dir.trim().trim_end_matches(['/', '\\']);
    let body = if !relative_path.trim().is_empty() {
        sanitize_relative_subpath(relative_path)?
    } else {
        let last = file_name_hint
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(file_name_hint);
        sanitize_segment(last)
    };
    let combined = if target_dir_clean.is_empty() {
        body
    } else {
        let dir = sanitize_relative_subpath(target_dir_clean)?;
        format!("{dir}/{body}")
    };
    // Final sanity check — re-validate the joined path as relative
    // components only.
    validate_relative_components(Path::new(&combined))?;
    Ok(combined)
}

// ---------------------------------------------------------------------------
// Download tickets
// ---------------------------------------------------------------------------

pub async fn create_download_ticket(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<DownloadTicketRequest>,
) -> Result<Json<DownloadTicketIssued>, AppCommandError> {
    let issued = create_download_ticket_core(
        state.workspace_transfer.clone(),
        params,
        "/api/workspace_download".to_string(),
    )
    .await?;
    Ok(Json(issued))
}

async fn create_download_ticket_core(
    manager: Arc<crate::workspace_transfer::WorkspaceTransferManager>,
    params: DownloadTicketRequest,
    base_url: String,
) -> Result<DownloadTicketIssued, AppCommandError> {
    let (root_path, target_path, filename) = match params.kind {
        DownloadKind::File => {
            let root = PathBuf::from(&params.root_path);
            let target = resolve_download_file_target(&params.root_path, &params.path)?;
            let filename = target
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("download")
                .to_string();
            (root, target, filename)
        }
        DownloadKind::Dir => {
            let root = PathBuf::from(&params.root_path);
            let (target, dir_name) = resolve_download_dir_target(&params.root_path, &params.path)?;
            (root, target, format!("{dir_name}.zip"))
        }
    };

    let ticket = manager
        .issue_download_ticket(DownloadTicketSpec {
            root_path,
            target_path,
            relative_path: params.path,
            kind: params.kind,
            filename,
        })
        .await;

    let url_base = base_url.trim_end_matches('/');
    Ok(DownloadTicketIssued {
        url: format!("{url_base}/{}", ticket.ticket),
        ..ticket
    })
}

pub async fn consume_download_ticket(
    Extension(state): Extension<Arc<AppState>>,
    AxumPath(ticket): AxumPath<String>,
) -> Result<Response, AppCommandError> {
    let Some(ticket) = state
        .workspace_transfer
        .consume_download_ticket(&ticket)
        .await
    else {
        return Err(AppCommandError::not_found(
            "Download ticket is invalid or expired",
        ));
    };

    match ticket.kind {
        DownloadKind::File => {
            let target = resolve_download_file_target(
                &ticket.root_path.to_string_lossy(),
                &ticket.relative_path,
            )?;
            stream_file_response(&target, &ticket.filename).await
        }
        DownloadKind::Dir => {
            let (target, _) = resolve_download_dir_target(
                &ticket.root_path.to_string_lossy(),
                &ticket.relative_path,
            )?;
            stream_zip_response(state.workspace_transfer.clone(), target, ticket.filename).await
        }
    }
}

fn ensure_workspace_root(root: &Path) -> Result<(), AppCommandError> {
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found(
            "Workspace folder does not exist",
        ));
    }
    Ok(())
}

fn resolve_download_file_target(
    root_path: &str,
    rel_path: &str,
) -> Result<PathBuf, AppCommandError> {
    let root = PathBuf::from(root_path);
    ensure_workspace_root(&root)?;
    let target = resolve_relative_path(&root, rel_path)?;
    if !target.exists() {
        return Err(AppCommandError::not_found("File does not exist"));
    }
    if !target.is_file() {
        return Err(AppCommandError::invalid_input("Path is not a file"));
    }
    ensure_inside_root(&root, &target)?;
    Ok(target)
}

fn resolve_download_dir_target(
    root_path: &str,
    rel_path: &str,
) -> Result<(PathBuf, String), AppCommandError> {
    let root = PathBuf::from(root_path);
    ensure_workspace_root(&root)?;
    if rel_path.is_empty() {
        let name = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("workspace")
            .to_string();
        return Ok((root, name));
    }

    let resolved = resolve_relative_path(&root, rel_path)?;
    if !resolved.exists() {
        return Err(AppCommandError::not_found("Directory does not exist"));
    }
    if !resolved.is_dir() {
        return Err(AppCommandError::invalid_input("Path is not a directory"));
    }
    ensure_inside_root(&root, &resolved)?;
    let name = resolved
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("folder")
        .to_string();
    Ok((resolved, name))
}

// ---------------------------------------------------------------------------
// Download (single file)
// ---------------------------------------------------------------------------

pub async fn download_workspace_file(
    Json(params): Json<DownloadWorkspaceParams>,
) -> Result<Response, AppCommandError> {
    let target = resolve_download_file_target(&params.root_path, &params.path)?;
    let name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("download")
        .to_string();
    stream_file_response(&target, &name).await
}

pub(crate) async fn stream_file_response(
    target: &Path,
    name: &str,
) -> Result<Response, AppCommandError> {
    let metadata = tokio::fs::metadata(&target)
        .await
        .map_err(AppCommandError::io)?;
    let size = metadata.len();
    let file = tokio::fs::File::open(&target)
        .await
        .map_err(AppCommandError::io)?;

    let body_stream = stream::unfold(file, |mut file| async move {
        let mut buf = vec![0u8; 64 * 1024];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                let bytes: Bytes = buf.into();
                Some((Ok::<_, std::io::Error>(bytes), file))
            }
            Err(e) => Some((Err(e), file)),
        }
    });
    let body = Body::from_stream(body_stream);

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(v) = HeaderValue::from_str(&size.to_string()) {
        headers.insert(header::CONTENT_LENGTH, v);
    }
    if let Some(v) = attachment_header(name) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }

    Ok((StatusCode::OK, headers, body).into_response())
}

// ---------------------------------------------------------------------------
// Download (directory as ZIP)
// ---------------------------------------------------------------------------

pub async fn download_workspace_dir(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<DownloadWorkspaceParams>,
) -> Result<Response, AppCommandError> {
    let (dir_path, dir_name) = resolve_download_dir_target(&params.root_path, &params.path)?;
    stream_zip_response(
        state.workspace_transfer.clone(),
        dir_path,
        format!("{dir_name}.zip"),
    )
    .await
}

async fn stream_zip_response(
    manager: Arc<crate::workspace_transfer::WorkspaceTransferManager>,
    dir_path: PathBuf,
    zip_name: String,
) -> Result<Response, AppCommandError> {
    let body = Body::from_stream(zip_body_stream(manager, dir_path));

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    if let Some(v) = attachment_header(&zip_name) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok((StatusCode::OK, headers, body).into_response())
}

fn zip_body_stream(
    manager: Arc<crate::workspace_transfer::WorkspaceTransferManager>,
    dir_path: PathBuf,
) -> ReaderStream<tokio::io::DuplexStream> {
    let (reader, writer) = tokio::io::duplex(8 * 64 * 1024);
    tokio::spawn(async move {
        let result = write_zip_archive_to_stream(manager, dir_path.clone(), writer).await;
        if let Err(err) = result {
            eprintln!(
                "[workspace_files] streaming zip failed for {}: {}{}",
                dir_path.display(),
                err.message,
                err.detail
                    .as_deref()
                    .map(|detail| format!(" ({detail})"))
                    .unwrap_or_default()
            );
        }
    });
    ReaderStream::with_capacity(reader, 64 * 1024)
}

async fn write_zip_archive_to_stream(
    manager: Arc<crate::workspace_transfer::WorkspaceTransferManager>,
    dir: PathBuf,
    sink: tokio::io::DuplexStream,
) -> Result<(), AppCommandError> {
    use futures_lite::io::AsyncWriteExt as _;

    let _permit =
        manager.zip_semaphore.acquire().await.map_err(|_| {
            AppCommandError::task_execution_failed("Workspace ZIP semaphore is closed")
        })?;

    let mut writer = ZipFileWriter::with_tokio(sink);
    let mut symlinks_skipped: u64 = 0;

    for entry in walkdir::WalkDir::new(&dir).follow_links(false) {
        let entry = entry.map_err(|e| {
            AppCommandError::io_error("Failed to walk directory").with_detail(e.to_string())
        })?;
        let path = entry.path();
        let rel = match path.strip_prefix(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            symlinks_skipped = symlinks_skipped.saturating_add(1);
            continue;
        }
        if file_type.is_dir() {
            let entry = ZipEntryBuilder::new(format!("{rel_str}/").into(), Compression::Deflate)
                .unix_permissions(0o755);
            writer.write_entry_whole(entry, &[]).await.map_err(|e| {
                AppCommandError::io_error("Failed to add dir to zip").with_detail(e.to_string())
            })?;
        } else if file_type.is_file() {
            let entry =
                ZipEntryBuilder::new(rel_str.into(), Compression::Deflate).unix_permissions(0o644);
            let mut entry_writer = writer.write_entry_stream(entry).await.map_err(|e| {
                AppCommandError::io_error("Failed to start zip entry").with_detail(e.to_string())
            })?;
            let mut f = tokio::fs::File::open(path)
                .await
                .map_err(AppCommandError::io)?;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = f.read(&mut buf).await.map_err(AppCommandError::io)?;
                if n == 0 {
                    break;
                }
                entry_writer.write_all(&buf[..n]).await.map_err(|e| {
                    AppCommandError::io_error("Failed to write zip entry")
                        .with_detail(e.to_string())
                })?;
            }
            entry_writer.close().await.map_err(|e| {
                AppCommandError::io_error("Failed to close zip entry").with_detail(e.to_string())
            })?;
        }
    }
    if symlinks_skipped > 0 {
        eprintln!(
            "[workspace_files] download_workspace_dir: skipped {} symlink entries under {}",
            symlinks_skipped,
            dir.display()
        );
    }
    writer.close().await.map_err(|e| {
        AppCommandError::io_error("Failed to finalize zip").with_detail(e.to_string())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt as _;

    #[test]
    fn sanitize_segment_replaces_hostile_chars_and_handles_dots() {
        // sanitize_segment is the *per-segment* sanitizer — it is not
        // expected to extract the basename; that's `sanitize_relative_subpath`'s
        // job. Hostile chars are replaced and degenerate inputs collapse
        // to "file" so the rename succeeds.
        assert_eq!(sanitize_segment("a:b*c?\"d"), "a_b_c__d");
        assert_eq!(sanitize_segment("..."), "file");
        assert_eq!(sanitize_segment(""), "file");
        assert_eq!(sanitize_segment("normal.txt"), "normal.txt");
    }

    #[test]
    fn sanitize_relative_subpath_joins_clean() {
        assert_eq!(sanitize_relative_subpath("a/b/c.txt").unwrap(), "a/b/c.txt");
        assert_eq!(
            sanitize_relative_subpath("a\\b\\c.txt").unwrap(),
            "a/b/c.txt"
        );
        assert_eq!(sanitize_relative_subpath("./a/./b").unwrap(), "a/b");
    }

    #[test]
    fn sanitize_relative_subpath_rejects_empty_and_traversal() {
        assert!(sanitize_relative_subpath("").is_err());
        assert!(sanitize_relative_subpath("/").is_err());
        assert!(sanitize_relative_subpath("../escape").is_err());
    }

    #[test]
    fn compute_final_rel_uses_file_name_when_no_relative() {
        assert_eq!(
            compute_final_rel("dir", "", "report.txt").unwrap(),
            "dir/report.txt"
        );
        assert_eq!(
            compute_final_rel("", "", "report.txt").unwrap(),
            "report.txt"
        );
    }

    #[test]
    fn compute_final_rel_prefers_relative_path() {
        assert_eq!(
            compute_final_rel("dir", "sub/a.txt", "ignored").unwrap(),
            "dir/sub/a.txt"
        );
        assert_eq!(
            compute_final_rel("", "a/b/c.txt", "ignored").unwrap(),
            "a/b/c.txt"
        );
    }

    #[test]
    fn validate_relative_components_rejects_dotdot_and_absolute() {
        assert!(validate_relative_components(Path::new("../escape")).is_err());
        assert!(validate_relative_components(Path::new("/etc/passwd")).is_err());
        assert!(validate_relative_components(Path::new("a/b")).is_ok());
    }

    #[tokio::test]
    async fn create_download_ticket_rejects_path_traversal() {
        let root = tempfile::tempdir().unwrap();
        let manager = std::sync::Arc::new(
            crate::workspace_transfer::WorkspaceTransferManager::new_for_tests(
                std::time::Duration::from_secs(60),
            ),
        );
        let err = create_download_ticket_core(
            manager,
            DownloadTicketRequest {
                root_path: root.path().to_string_lossy().to_string(),
                path: "../escape".to_string(),
                kind: crate::workspace_transfer::DownloadKind::File,
            },
            "/api/workspace_download".to_string(),
        )
        .await
        .unwrap_err();
        assert!(err.message.contains(".."));
    }

    #[tokio::test]
    async fn create_download_ticket_for_file_is_consumed_once() {
        let root = tempfile::tempdir().unwrap();
        let file_path = root.path().join("a.txt");
        tokio::fs::write(&file_path, b"hello").await.unwrap();
        let manager = std::sync::Arc::new(
            crate::workspace_transfer::WorkspaceTransferManager::new_for_tests(
                std::time::Duration::from_secs(60),
            ),
        );
        let issued = create_download_ticket_core(
            manager.clone(),
            DownloadTicketRequest {
                root_path: root.path().to_string_lossy().to_string(),
                path: "a.txt".to_string(),
                kind: crate::workspace_transfer::DownloadKind::File,
            },
            "/api/workspace_download".to_string(),
        )
        .await
        .unwrap();
        assert_eq!(issued.filename, "a.txt");
        assert_eq!(
            issued.url,
            format!("/api/workspace_download/{}", issued.ticket)
        );
        assert!(manager
            .consume_download_ticket(&issued.ticket)
            .await
            .is_some());
        assert!(manager
            .consume_download_ticket(&issued.ticket)
            .await
            .is_none());
    }

    async fn build_zip_bytes_for_test(dir: PathBuf) -> Result<Vec<u8>, AppCommandError> {
        let manager = std::sync::Arc::new(
            crate::workspace_transfer::WorkspaceTransferManager::new_for_tests(
                std::time::Duration::from_secs(60),
            ),
        );
        let mut stream = zip_body_stream(manager, dir);
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(AppCommandError::io)?;
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    #[tokio::test]
    async fn streaming_zip_skips_symlinks_and_includes_regular_files() {
        let root = tempfile::tempdir().unwrap();
        tokio::fs::create_dir(root.path().join("dir"))
            .await
            .unwrap();
        tokio::fs::write(root.path().join("dir").join("a.txt"), b"hello")
            .await
            .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", root.path().join("dir").join("link")).unwrap();

        let bytes = build_zip_bytes_for_test(root.path().join("dir"))
            .await
            .unwrap();
        let reader = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(reader).unwrap();
        assert!(archive.by_name("a.txt").is_ok());
        assert!(archive.by_name("link").is_err());
    }

    #[tokio::test]
    async fn streaming_zip_channel_drop_stops_writer() {
        let root = tempfile::tempdir().unwrap();
        for i in 0..128 {
            tokio::fs::write(root.path().join(format!("f{i}.txt")), vec![b'x'; 1024])
                .await
                .unwrap();
        }
        let manager = std::sync::Arc::new(
            crate::workspace_transfer::WorkspaceTransferManager::new_for_tests(
                std::time::Duration::from_secs(60),
            ),
        );
        let stream = zip_body_stream(manager, root.path().to_path_buf());
        drop(stream);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_no_symlink_in_chain_rejects_intermediate_symlink() {
        use std::fs;
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("tempdir root");
        let outside = tempfile::tempdir().expect("tempdir outside");

        // root/link -> outside
        symlink(outside.path(), root.path().join("link")).expect("symlink");

        // Target: root/link/sub — does NOT exist, but the intermediate
        // `link` component is a symlink that would carry create_dir_all
        // out of the root.
        let target = root.path().join("link").join("sub");
        let err = ensure_no_symlink_in_chain(root.path(), &target)
            .expect_err("should reject symlink in chain");
        assert!(
            err.message.contains("symlink"),
            "unexpected error: {}",
            err.message
        );

        // Sanity: no symlink in chain → ok.
        fs::create_dir(root.path().join("real")).expect("real dir");
        let ok_target = root.path().join("real").join("nested").join("file.txt");
        assert!(ensure_no_symlink_in_chain(root.path(), &ok_target).is_ok());
    }
}
