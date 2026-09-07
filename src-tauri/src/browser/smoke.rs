//! Dev-only puppet for the built-in browser (Cargo feature `browser-smoke`,
//! never part of a release build).
//!
//! The desktop app cannot be driven from outside without macOS Accessibility
//! rights, which the automation environment does not have, so P0 verification
//! drives the app from the inside instead: when `CODEG_BROWSER_SMOKE_DIR` is
//! set, a task polls `<dir>/cmd.json` for `{ "id": n, "op": "...", ... }`,
//! executes the operation through the same `_core` functions the commands
//! use, and writes `<dir>/result-<n>.json`. Screenshots are taken from the
//! outside with `screencapture -l <window id>`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::browser::registry::BrowserRegistry;
use crate::browser::types::{Bounds, SurfaceChoice};
use crate::commands::browser as browser_commands;

pub fn spawn_if_enabled(app: AppHandle) {
    let Ok(dir) = std::env::var("CODEG_BROWSER_SMOKE_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    tracing::warn!("[browser-smoke] enabled, watching {}", dir.display());
    tauri::async_runtime::spawn(async move {
        let mut last_id: u64 = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let Ok(raw) = std::fs::read_to_string(dir.join("cmd.json")) else {
                continue;
            };
            let Ok(cmd) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            let id = cmd.get("id").and_then(Value::as_u64).unwrap_or(0);
            if id == 0 || id <= last_id {
                continue;
            }
            last_id = id;
            let started = Instant::now();
            let result = match execute(&app, &cmd).await {
                Ok(value) => json!({ "id": id, "ok": true, "result": value }),
                Err(error) => json!({ "id": id, "ok": false, "error": error }),
            };
            let mut result = result;
            result["ms"] = json!(started.elapsed().as_millis() as u64);
            write_result(&dir, id, &result);
        }
    });
}

fn write_result(dir: &Path, id: u64, result: &Value) {
    let tmp = dir.join(format!("result-{id}.json.tmp"));
    let dest = dir.join(format!("result-{id}.json"));
    let body = serde_json::to_string_pretty(result).unwrap_or_default();
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, &dest);
    }
}

fn str_arg(cmd: &Value, key: &str) -> Result<String, String> {
    cmd.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string argument {key:?}"))
}

fn bounds_arg(cmd: &Value) -> Result<Bounds, String> {
    let b = cmd.get("bounds").ok_or("missing bounds")?;
    serde_json::from_value(b.clone()).map_err(|e| format!("bad bounds: {e}"))
}

fn err_string(err: impl std::fmt::Display) -> String {
    err.to_string()
}

async fn execute(app: &AppHandle, cmd: &Value) -> Result<Value, String> {
    let op = str_arg(cmd, "op")?;
    let main_window = || {
        app.get_webview_window("main")
            .ok_or_else(|| "no main window".to_string())
    };
    let owner = || -> Result<tauri::WebviewWindow, String> {
        match cmd.get("owner").and_then(Value::as_str) {
            Some(label) => app
                .get_webview_window(label)
                .ok_or_else(|| format!("no window {label:?}")),
            None => main_window(),
        }
    };
    let registry = app.state::<BrowserRegistry>();

    match op.as_str() {
        "ping" => Ok(json!("pong")),
        "sleep" => {
            let ms = cmd.get("ms").and_then(Value::as_u64).unwrap_or(500);
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(Value::Null)
        }
        "list_windows" => {
            let mut out: Vec<Value> = app
                .webview_windows()
                .into_iter()
                .map(|(label, w)| {
                    json!({
                        "label": label,
                        "visible": w.is_visible().ok(),
                        "focused": w.is_focused().ok(),
                        "minimized": w.is_minimized().ok(),
                        "position": w.outer_position().ok().map(|p| [p.x, p.y]),
                        "size": w.inner_size().ok().map(|s| [s.width, s.height]),
                        "scale": w.scale_factor().ok(),
                        "url": w.url().ok().map(|u| u.to_string()),
                    })
                })
                .collect();
            out.sort_by(|a, b| a["label"].as_str().cmp(&b["label"].as_str()));
            Ok(Value::Array(out))
        }
        "webviews" => {
            let mut windows: Vec<String> = app.webview_windows().keys().cloned().collect();
            windows.sort();
            let tabs: Vec<String> = registry
                .list()
                .into_iter()
                .map(|s| crate::browser::tab_label(&s.tab_id))
                .collect();
            Ok(json!({ "windows": windows, "tabs": tabs }))
        }
        "open_settings" => {
            let main = main_window()?;
            crate::commands::windows::open_settings_window(
                app.clone(),
                main,
                app.state(),
                None,
                None,
                None,
                None,
                app.state(),
            )
            .await
            .map_err(err_string)?;
            Ok(Value::Null)
        }
        "open_import_sessions" => {
            crate::commands::windows::open_import_sessions_window(
                app.clone(),
                app.state(),
                None,
                None,
                None,
            )
            .await
            .map_err(err_string)?;
            Ok(Value::Null)
        }
        "open_project_boot" => {
            crate::commands::windows::open_project_boot_window(
                app.clone(),
                app.state(),
                None,
                None,
                None,
            )
            .await
            .map_err(err_string)?;
            Ok(Value::Null)
        }
        "open_commit" => {
            let folder_id = cmd
                .get("folder_id")
                .and_then(Value::as_i64)
                .ok_or("missing folder_id")? as i32;
            let main = main_window()?;
            crate::commands::windows::open_commit_window(
                app.clone(),
                main,
                app.state(),
                app.state(),
                folder_id,
                None,
                None,
            )
            .await
            .map_err(err_string)?;
            Ok(Value::Null)
        }
        "open_pet" => {
            crate::commands::windows::open_pet_window(app.clone(), app.state())
                .await
                .map_err(err_string)?;
            Ok(Value::Null)
        }
        "close_window" | "focus_window" | "minimize_window" | "unminimize_window"
        | "hide_window" | "show_window" => {
            let label = str_arg(cmd, "label")?;
            let w = app
                .get_webview_window(&label)
                .ok_or_else(|| format!("no window {label:?}"))?;
            let r = match op.as_str() {
                "close_window" => w.close(),
                "focus_window" => w.set_focus(),
                "minimize_window" => w.minimize(),
                "unminimize_window" => w.unminimize(),
                "hide_window" => w.hide(),
                _ => w.show(),
            };
            r.map_err(err_string)?;
            Ok(Value::Null)
        }
        "browser_open" => {
            let owner = owner()?;
            let surface = match cmd.get("surface").and_then(Value::as_str) {
                Some("child") => SurfaceChoice::Child,
                Some("window") => SurfaceChoice::Window,
                _ => SurfaceChoice::Auto,
            };
            let state = browser_commands::open_tab_core(
                app,
                &owner,
                &registry,
                browser_commands::OpenTabParams {
                    tab_id: str_arg(cmd, "tab_id")?,
                    url: str_arg(cmd, "url")?,
                    bounds: bounds_arg(cmd)?,
                    background: cmd
                        .get("background")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    surface,
                },
            )
            .map_err(err_string)?;
            Ok(json!(state))
        }
        "browser_set_bounds" => {
            browser_commands::set_bounds_core(&registry, &str_arg(cmd, "tab_id")?, bounds_arg(cmd)?)
                .map_err(err_string)?;
            Ok(Value::Null)
        }
        "browser_set_visible" => {
            let owner = owner()?;
            browser_commands::set_visible_core(
                &owner,
                &registry,
                &str_arg(cmd, "tab_id")?,
                cmd.get("visible").and_then(Value::as_bool).unwrap_or(true),
                cmd.get("handoff_focus")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
            .map_err(err_string)?;
            Ok(Value::Null)
        }
        "browser_navigate" => {
            let state = browser_commands::navigate_core(
                app,
                &registry,
                &str_arg(cmd, "tab_id")?,
                &str_arg(cmd, "url")?,
            )
            .map_err(err_string)?;
            Ok(json!(state))
        }
        "browser_reload" => {
            browser_commands::reload_core(&registry, &str_arg(cmd, "tab_id")?).map_err(err_string)?;
            Ok(Value::Null)
        }
        "browser_close" => {
            browser_commands::close_core(app, &registry, &str_arg(cmd, "tab_id")?)
                .map_err(err_string)?;
            Ok(Value::Null)
        }
        "browser_state" => {
            let state = browser_commands::state_core(&registry, &str_arg(cmd, "tab_id")?)
                .map_err(err_string)?;
            Ok(json!(state))
        }
        "browser_list" => Ok(json!(registry.list())),
        "browser_url" => {
            let surface = registry
                .surface(&str_arg(cmd, "tab_id")?)
                .ok_or("no such tab")?;
            Ok(json!(surface.url().map_err(err_string)?.to_string()))
        }
        "browser_eval" => {
            let surface = registry
                .surface(&str_arg(cmd, "tab_id")?)
                .ok_or("no such tab")?;
            let js = str_arg(cmd, "js")?;
            let (tx, rx) = std::sync::mpsc::channel::<String>();
            surface
                .eval_with_callback(&js, move |value| {
                    let _ = tx.send(value);
                })
                .map_err(err_string)?;
            let timeout = Duration::from_millis(cmd.get("timeout_ms").and_then(Value::as_u64).unwrap_or(8000));
            let value = tokio::task::spawn_blocking(move || rx.recv_timeout(timeout))
                .await
                .map_err(err_string)?
                .map_err(|_| "eval timed out".to_string())?;
            Ok(serde_json::from_str(&value).unwrap_or(Value::String(value)))
        }
        "browser_eval_world" => {
            let surface = registry
                .surface(&str_arg(cmd, "tab_id")?)
                .ok_or("no such tab")?;
            let js = str_arg(cmd, "js")?;
            let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
            surface
                .eval_in_world(&js, move |value| {
                    let _ = tx.send(value);
                })
                .map_err(err_string)?;
            let timeout = Duration::from_millis(cmd.get("timeout_ms").and_then(Value::as_u64).unwrap_or(8000));
            let value = tokio::task::spawn_blocking(move || rx.recv_timeout(timeout))
                .await
                .map_err(err_string)?
                .map_err(|_| "world eval timed out".to_string())??;
            Ok(serde_json::from_str(&value).unwrap_or(Value::String(value)))
        }
        "browser_snapshot" => {
            let surface = registry
                .surface(&str_arg(cmd, "tab_id")?)
                .ok_or("no such tab")?;
            let path = str_arg(cmd, "path")?;
            let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<u8>, String>>();
            surface
                .snapshot_png(move |png| {
                    let _ = tx.send(png);
                })
                .map_err(err_string)?;
            let png = tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(10)))
                .await
                .map_err(err_string)?
                .map_err(|_| "snapshot timed out".to_string())??;
            std::fs::write(&path, &png).map_err(err_string)?;
            Ok(json!({ "path": path, "bytes": png.len() }))
        }
        "browser_back" => {
            browser_commands::go_back_core(&registry, &str_arg(cmd, "tab_id")?).map_err(err_string)?;
            Ok(Value::Null)
        }
        "browser_forward" => {
            browser_commands::go_forward_core(&registry, &str_arg(cmd, "tab_id")?).map_err(err_string)?;
            Ok(Value::Null)
        }
        "browser_stop" => {
            browser_commands::stop_core(app, &registry, &str_arg(cmd, "tab_id")?).map_err(err_string)?;
            Ok(Value::Null)
        }
        "browser_gestures" => {
            let gestures: Vec<Value> = registry
                .recent_gestures(&str_arg(cmd, "tab_id")?)
                .into_iter()
                .map(|g| json!({ "age_ms": g.received.elapsed().as_millis() as u64, "payload": g.payload }))
                .collect();
            Ok(Value::Array(gestures))
        }
        // Ask the frontend to open a URL as a browser tab (exercises the real
        // tab record → surface host → browser_open_tab path).
        "frontend_open" => {
            crate::browser::events::emit_open_request(
                app,
                &crate::browser::types::BrowserOpenRequestPayload {
                    url: str_arg(cmd, "url")?,
                    source: "smoke".to_string(),
                    activate: cmd.get("activate").and_then(Value::as_bool).unwrap_or(true),
                    owner_window: cmd.get("owner").and_then(Value::as_str).map(str::to_string),
                    opener_tab_id: cmd.get("opener").and_then(Value::as_str).map(str::to_string),
                },
            );
            Ok(Value::Null)
        }
        // Evaluate in the MAIN (workspace) webview — drives the frontend.
        "main_eval" => {
            let main = main_window()?;
            let js = str_arg(cmd, "js")?;
            let (tx, rx) = std::sync::mpsc::channel::<String>();
            main.eval_with_callback(&js, move |value| {
                let _ = tx.send(value);
            })
            .map_err(err_string)?;
            let timeout = Duration::from_millis(cmd.get("timeout_ms").and_then(Value::as_u64).unwrap_or(8000));
            let value = tokio::task::spawn_blocking(move || rx.recv_timeout(timeout))
                .await
                .map_err(err_string)?
                .map_err(|_| "main eval timed out".to_string())?;
            Ok(serde_json::from_str(&value).unwrap_or(Value::String(value)))
        }
        "browser_debug" => {
            let tab_id = str_arg(cmd, "tab_id")?;
            let surface = registry.surface(&tab_id).ok_or("no such tab")?;
            let (visible, bounds) = registry
                .update(&tab_id, |t| (t.visible, t.last_bounds))
                .ok_or("no such tab")?;
            Ok(json!({
                "registry": { "visible": visible, "lastBounds": bounds },
                "native": surface.debug_view().map_err(err_string)?,
            }))
        }
        "browser_focus" => {
            let surface = registry
                .surface(&str_arg(cmd, "tab_id")?)
                .ok_or("no such tab")?;
            surface.set_focus().map_err(err_string)?;
            Ok(Value::Null)
        }
        other => Err(format!("unknown op {other:?}")),
    }
}
