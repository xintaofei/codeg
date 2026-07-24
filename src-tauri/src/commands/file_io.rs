use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::app_error::AppCommandError;

/// Write a base64-encoded binary blob to a user-chosen path on disk.
///
/// Used by the frontend's "download generated image" flow on desktop:
/// the renderer first invokes `tauri-plugin-dialog`'s `save()` to obtain
/// a destination path from the system save dialog, then calls this command
/// with the base64 payload. Web mode bypasses this command entirely and
/// uses an `<a download>` Blob link.
///
/// `path` must be an absolute filesystem path (the dialog returns one).
/// Parent directory must already exist (it does, since the OS dialog only
/// lets the user pick an existing folder + filename).
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn save_binary_file(path: String, data_base64: String) -> Result<(), AppCommandError> {
    let bytes = STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|e| AppCommandError::invalid_input(format!("invalid base64 payload: {e}")))?;
    std::fs::write(&path, bytes).map_err(AppCommandError::io)?;
    Ok(())
}

/// Write a UTF-8 text payload to a user-chosen path on disk.
///
/// Used by the frontend's "export conversation as Markdown / HTML" flow on
/// desktop. The renderer first invokes `tauri-plugin-dialog`'s `save()`
/// to obtain a destination path from the system save dialog, then calls
/// this command with the text contents. Web mode bypasses this command
/// entirely and uses an `<a download>` Blob link.
///
/// Mirrors `save_binary_file`'s contract: `path` must be an absolute
/// filesystem path returned by the OS dialog, and the parent directory
/// is guaranteed to exist by the dialog. I/O failures (including macOS
/// TCC denials at write time) surface through `AppCommandError::io`,
/// which maps `PermissionDenied` so the caller can disambiguate.
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn save_text_file(path: String, contents: String) -> Result<(), AppCommandError> {
    std::fs::write(&path, contents).map_err(AppCommandError::io)?;
    Ok(())
}

/// Open a filesystem path in VS Code or Cursor without ShellExecute dialogs.
///
/// The JS opener plugin uses `open::with`, which on Windows pops a system
/// "Windows cannot find …" dialog for every missing candidate path. This
/// command instead:
/// 1. Skips absolute candidates that are not real files (silent)
/// 2. Spawns via `std::process::Command` (no ShellExecute error UI)
/// 3. Suppresses console windows for `.cmd` / bare CLI shims on Windows
///
/// `editor` is `"vscode"` or `"cursor"`. Works on Windows, macOS, and Linux.
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_path_in_editor(path: String, editor: String) -> Result<(), AppCommandError> {
    open_path_in_editor_impl(&path, &editor)
}

#[cfg(feature = "tauri-runtime")]
fn open_path_in_editor_impl(path: &str, editor: &str) -> Result<(), AppCommandError> {
    // Validate editor first so invalid args do not depend on path existence
    // (and unit tests stay cross-platform when using a dummy path).
    let editor = editor.trim().to_ascii_lowercase();
    if editor != "vscode" && editor != "cursor" {
        return Err(AppCommandError::invalid_input(format!(
            "unknown editor: {editor}"
        )));
    }

    let file = Path::new(path);
    if !file.exists() {
        return Err(AppCommandError::not_found(format!(
            "file does not exist: {path}"
        )));
    }

    let candidates = editor_launch_candidates(&editor);
    let mut last_err: Option<String> = None;

    for app in candidates {
        // Absolute / relative path-like candidates: skip silently if missing so
        // Windows never shows "cannot find file" dialogs.
        if looks_like_filesystem_path(&app) && !Path::new(&app).is_file() {
            continue;
        }

        match spawn_editor_silent(&app, path) {
            Ok(()) => return Ok(()),
            Err(e) => last_err = Some(format!("{app}: {e}")),
        }
    }

    Err(AppCommandError::not_found(format!(
        "could not open with {editor}: {}",
        last_err.unwrap_or_else(|| "no candidates".into())
    )))
}

#[cfg(feature = "tauri-runtime")]
fn looks_like_filesystem_path(app: &str) -> bool {
    app.contains('/')
        || app.contains('\\')
        || Path::new(app).is_absolute()
        || (app.len() >= 2 && app.as_bytes()[1] == b':')
}

#[cfg(feature = "tauri-runtime")]
fn editor_launch_candidates(editor: &str) -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        if editor == "vscode" {
            vec!["Visual Studio Code".into()]
        } else {
            vec!["Cursor".into()]
        }
    }

    #[cfg(target_os = "linux")]
    {
        if editor == "vscode" {
            vec!["code".into(), "code-insiders".into()]
        } else {
            vec!["cursor".into()]
        }
    }

    #[cfg(target_os = "windows")]
    {
        let mut out = Vec::new();
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            if editor == "vscode" {
                out.push(
                    local
                        .join("Programs")
                        .join("Microsoft VS Code")
                        .join("Code.exe")
                        .to_string_lossy()
                        .into_owned(),
                );
                out.push(
                    local
                        .join("Programs")
                        .join("Microsoft VS Code")
                        .join("bin")
                        .join("code.cmd")
                        .to_string_lossy()
                        .into_owned(),
                );
            } else {
                for brand in ["cursor", "Cursor"] {
                    out.push(
                        local
                            .join("Programs")
                            .join(brand)
                            .join("Cursor.exe")
                            .to_string_lossy()
                            .into_owned(),
                    );
                    out.push(
                        local
                            .join("Programs")
                            .join(brand)
                            .join("resources")
                            .join("app")
                            .join("bin")
                            .join("cursor.cmd")
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        if editor == "vscode" {
            out.push("code.cmd".into());
            out.push("code".into());
        } else {
            out.push("cursor.cmd".into());
            out.push("cursor".into());
        }
        out
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = editor;
        Vec::new()
    }
}

/// Spawn an editor without ShellExecute dialogs or console flashes.
#[cfg(feature = "tauri-runtime")]
fn spawn_editor_silent(app: &str, file_path: &str) -> Result<(), std::io::Error> {
    #[cfg(target_os = "macos")]
    {
        // `open -a "App Name" -- /path/to/file` — no Finder error dialog if we
        // only call this with real app names (macOS candidates are names only).
        let status = Command::new("open")
            .args(["-a", app, "--", file_path])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "open -a {app} exited with {status}"
            )))
        }
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // Hide console host for .cmd / PATH shims. Do NOT put CREATE_NO_WINDOW
        // on GUI .exe launches — only suppress the cmd.exe console flash.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;

        let lower = app.to_ascii_lowercase();
        let is_batch = lower.ends_with(".cmd") || lower.ends_with(".bat");
        let mut cmd = if is_batch {
            let mut c = Command::new("cmd.exe");
            c.args(["/C", app, file_path]);
            c
        } else {
            let mut c = Command::new(app);
            c.arg(file_path);
            c
        };
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let flags = if is_batch || !looks_like_filesystem_path(app) {
            CREATE_NO_WINDOW | DETACHED_PROCESS
        } else {
            DETACHED_PROCESS
        };
        cmd.creation_flags(flags);
        cmd.spawn()?;
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new(app)
            .arg(file_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        all(unix, not(target_os = "macos"))
    )))]
    {
        let _ = (app, file_path);
        Err(std::io::Error::other("unsupported platform"))
    }
}

#[cfg(all(test, feature = "tauri-runtime"))]
mod tests {
    use super::*;

    #[test]
    fn looks_like_filesystem_path_detects_absolute_and_drive_paths() {
        assert!(looks_like_filesystem_path(r"C:\Users\a\Code.exe"));
        assert!(looks_like_filesystem_path("/usr/bin/code"));
        assert!(looks_like_filesystem_path(r"Programs\cursor\Cursor.exe"));
        assert!(!looks_like_filesystem_path("code"));
        assert!(!looks_like_filesystem_path("cursor.cmd"));
    }

    #[test]
    fn open_path_in_editor_rejects_unknown_editor() {
        // Path need not exist: editor is validated first.
        let err = open_path_in_editor_impl("/tmp/x", "notepad").expect_err("unknown");
        assert!(matches!(
            err.code,
            crate::app_error::AppErrorCode::InvalidInput
        ));
    }

    #[test]
    fn open_path_in_editor_rejects_missing_file() {
        let err = open_path_in_editor_impl(
            "/this/path/definitely/does/not/exist-codeg-open-editor",
            "vscode",
        )
        .expect_err("missing file");
        assert!(matches!(
            err.code,
            crate::app_error::AppErrorCode::NotFound
        ));
    }

    #[tokio::test]
    async fn save_text_file_writes_utf8_payload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.md");
        let contents = "# Title\n\n中文内容 emoji 🎉\n".to_string();
        save_text_file(path.to_string_lossy().into_owned(), contents.clone())
            .await
            .expect("write");
        let read = std::fs::read_to_string(&path).expect("read");
        assert_eq!(read, contents);
    }

    #[tokio::test]
    async fn save_text_file_overwrites_existing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.md");
        std::fs::write(&path, "old").expect("seed");
        save_text_file(path.to_string_lossy().into_owned(), "new".into())
            .await
            .expect("overwrite");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "new");
    }

    #[tokio::test]
    async fn save_text_file_surfaces_io_error_on_missing_parent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bad = dir.path().join("does/not/exist/out.md");
        let err = save_text_file(bad.to_string_lossy().into_owned(), "x".into())
            .await
            .expect_err("must fail");
        assert!(matches!(err.code, crate::app_error::AppErrorCode::NotFound));
    }
}
