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

use crate::browser::doc_guest::{DocGuests, DocMode};
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
                        // Never read a URL through tauri: wry panics on a
                        // webview without a committed document, and even the
                        // workspace window is one while its first page loads.
                        "url": Value::Null,
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
                    devtools: cmd.get("devtools").and_then(Value::as_bool).unwrap_or(false),
                    profile: cmd
                        .get("profile")
                        .and_then(Value::as_str)
                        .unwrap_or(crate::browser::profile::DEFAULT_PROFILE_ID)
                        .to_string(),
                },
            )
            .map_err(err_string)?;
            Ok(json!(state))
        }
        "browser_doc_open" => {
            let owner = owner()?;
            let guests = app.state::<DocGuests>();
            let result = browser_commands::doc_open_core(
                app,
                &owner,
                &registry,
                &guests,
                browser_commands::DocOpenParams {
                    tab_id: str_arg(cmd, "tab_id")?,
                    path: str_arg(cmd, "path")?,
                    root: cmd.get("root").and_then(Value::as_str).map(str::to_string),
                    bounds: bounds_arg(cmd)?,
                    background: cmd
                        .get("background")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    devtools: cmd.get("devtools").and_then(Value::as_bool).unwrap_or(false),
                },
            )
            .map_err(err_string)?;
            Ok(json!(result))
        }
        "browser_doc_mode" => {
            let guests = app.state::<DocGuests>();
            let mode = match cmd.get("mode").and_then(Value::as_str) {
                Some("dynamic") => DocMode::Dynamic,
                _ => DocMode::Safe,
            };
            let state = browser_commands::doc_set_mode_core(
                app,
                &registry,
                &guests,
                &str_arg(cmd, "tab_id")?,
                mode,
            )
            .map_err(err_string)?;
            Ok(json!(state))
        }
        "browser_doc_state" => {
            let guests = app.state::<DocGuests>();
            let state = browser_commands::doc_state_core(&guests, &str_arg(cmd, "tab_id")?)
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
            let frame = browser_commands::set_visible_core(
                &owner,
                &registry,
                &str_arg(cmd, "tab_id")?,
                cmd.get("visible").and_then(Value::as_bool).unwrap_or(true),
                cmd.get("handoff_focus")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                cmd.get("freeze").and_then(Value::as_bool).unwrap_or(false),
            )
            .await
            .map_err(err_string)?;
            // The frame itself is large; report its shape, and write it out
            // when asked so a run can look at it.
            match frame {
                Some(frame) => {
                    if let Some(path) = cmd.get("frame_path").and_then(Value::as_str) {
                        use base64::Engine as _;
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(&frame.data)
                            .map_err(err_string)?;
                        std::fs::write(path, bytes).map_err(err_string)?;
                    }
                    Ok(json!({
                        "frame": { "mime": frame.mime, "width": frame.width, "height": frame.height, "base64Len": frame.data.len() }
                    }))
                }
                None => Ok(json!({ "frame": Value::Null })),
            }
        }
        "browser_set_host_rules" => {
            let rules: Vec<crate::browser::policy::HostRule> =
                serde_json::from_value(cmd.get("rules").cloned().unwrap_or(json!([])))
                    .map_err(|e| format!("bad rules: {e}"))?;
            let policy = app.state::<crate::browser::policy::BrowserPolicy>();
            policy.set_user_rules(rules);
            Ok(json!({ "userRules": policy.user_rules().len() }))
        }
        "browser_capabilities" => {
            let policy = app.state::<crate::browser::policy::BrowserPolicy>();
            Ok(serde_json::to_value(browser_commands::capabilities(&policy)).map_err(err_string)?)
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
            browser_commands::reload_core(app, &registry, &str_arg(cmd, "tab_id")?)
                .map_err(err_string)?;
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
        // The two halves of agent access, so that the grant model can be
        // exercised against a real engine rather than only against its own
        // unit tests: share a tab, read it, navigate it away, watch the read
        // be refused. `level` is the wire spelling (`none` / `read` /
        // `control`), the same one the frontend sends.
        "browser_agent_grant" => {
            let level = serde_json::from_value(
                cmd.get("level").cloned().unwrap_or(Value::String("none".into())),
            )
            .map_err(err_string)?;
            let state = browser_commands::set_agent_grant_core(
                app,
                &registry,
                &str_arg(cmd, "tab_id")?,
                level,
            )
            .map_err(err_string)?;
            Ok(json!(state))
        }
        "browser_agent_snapshot" => {
            let request = crate::browser::agent::SnapshotRequest {
                max_chars: cmd
                    .get("max_chars")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize),
            };
            let snapshot = browser_commands::agent_snapshot_core(
                app,
                &registry,
                &str_arg(cmd, "tab_id")?,
                &request,
            )
            .await
            .map_err(err_string)?;
            Ok(json!(snapshot))
        }
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
        "browser_profile" => Ok(json!({
            "isolatedStorage": crate::browser::profile::isolated_storage(),
            "profiles": crate::browser::profile::profiles_supported(),
            "signInUserAgent": crate::browser::profile::sign_in_user_agent_supported(),
            "signInUserAgentEnabled": crate::browser::profile::sign_in_user_agent_enabled(),
            "proxy": crate::browser::profile::proxy_status(),
            "effectiveProxyUrl": crate::network::proxy::effective_proxy_url(),
        })),
        "browser_set_sign_in_ua" => {
            browser_commands::set_sign_in_user_agent_core(
                &registry,
                cmd.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            );
            Ok(Value::Null)
        }
        "browser_remove_profile" => {
            browser_commands::remove_profile_core(app, &registry, &str_arg(cmd, "profile")?)
                .await
                .map_err(err_string)?;
            Ok(Value::Null)
        }
        // The per-record path older macOS uses, run against the shared
        // default store here so it can be exercised on a machine that has an
        // isolated profile.
        #[cfg(target_os = "macos")]
        "browser_default_store_records" => {
            let names = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let sink = names.clone();
            browser_commands::on_main_until_done(app, "list default store records", move |done| {
                crate::browser::shim::macos::default_store_record_names(move |found| {
                    *sink.lock().unwrap_or_else(|p| p.into_inner()) = found;
                    done();
                })
            })
            .await
            .map_err(err_string)?;
            let names = names.lock().unwrap_or_else(|p| p.into_inner()).clone();
            Ok(json!(names))
        }
        #[cfg(target_os = "macos")]
        "browser_clear_shared_records" => {
            browser_commands::on_main_until_done(app, "clear shared records", |done| {
                crate::browser::shim::macos::clear_shared_store_except_app(done)
            })
            .await
            .map_err(err_string)?;
            Ok(Value::Null)
        }
        "browser_find" => {
            let found = browser_commands::find_core(
                &registry,
                &str_arg(cmd, "tab_id")?,
                &str_arg(cmd, "query").unwrap_or_default(),
                cmd.get("forward").and_then(Value::as_bool).unwrap_or(true),
            )
            .await
            .map_err(err_string)?;
            Ok(json!(found))
        }
        // Download records this run produced, oldest first.
        "browser_downloads" => Ok(serde_json::to_value(
            app.state::<crate::browser::BrowserDownloads>().list(),
        )
        .unwrap_or(Value::Null)),
        "browser_clear_downloads" => {
            app.state::<crate::browser::BrowserDownloads>().clear();
            Ok(Value::Null)
        }
        "browser_clear_data" => {
            let profile = cmd
                .get("profile")
                .and_then(Value::as_str)
                .unwrap_or(crate::browser::profile::DEFAULT_PROFILE_ID);
            browser_commands::clear_data_core(app, &registry, profile)
                .await
                .map_err(err_string)?;
            Ok(Value::Null)
        }
        // Ask the frontend to open a URL as a browser tab (exercises the real
        // tab record → surface host → browser_open_tab path).
        // Mint a companion token against the live broker socket, so the real
        // `codeg-mcp` binary can be driven against this running app the way an
        // agent CLI drives it. Verifying the browser tools end to end otherwise
        // means starting a real agent session and asking it nicely; this reaches
        // the same listener, over the same UDS, with the same token policy —
        // the only thing skipped is which process asked for the token.
        //
        // Dev-only twice over: the `browser-smoke` feature is never in a release
        // build, and the puppet does nothing without `CODEG_BROWSER_SMOKE_DIR`.
        "mcp_companion_handle" => {
            let tokens = app
                .try_state::<std::sync::Arc<crate::acp::delegation::listener::TokenRegistry>>()
                .ok_or("no delegation token registry")?;
            let socket = app
                .try_state::<crate::commands::delegation::DelegationSocketPath>()
                .ok_or("no delegation socket path")?;
            let token = uuid::Uuid::new_v4().to_string();
            tokens
                .register(
                    token.clone(),
                    crate::acp::delegation::listener::TokenEntry {
                        parent_connection_id: cmd
                            .get("parent")
                            .and_then(Value::as_str)
                            .unwrap_or("smoke-parent")
                            .to_string(),
                        working_dir: std::env::temp_dir(),
                    },
                )
                .await;
            Ok(json!({ "token": token, "socketPath": socket.0.to_string_lossy() }))
        }
        "frontend_open" => {
            crate::browser::events::emit_open_request(
                app,
                &crate::browser::types::BrowserOpenRequestPayload {
                    url: str_arg(cmd, "url")?,
                    source: "smoke".to_string(),
                    activate: cmd.get("activate").and_then(Value::as_bool).unwrap_or(true),
                    owner_window: cmd.get("owner").and_then(Value::as_str).map(str::to_string),
                    opener_tab_id: cmd.get("opener").and_then(Value::as_str).map(str::to_string),
                    profile: cmd.get("profile").and_then(Value::as_str).map(str::to_string),
                },
            );
            Ok(Value::Null)
        }
        // Evaluate in the MAIN (workspace) webview — drives the frontend.
        // `label` picks another app window (settings, …) instead.
        "main_eval" => {
            let main = match cmd.get("label").and_then(Value::as_str) {
                Some(label) => app
                    .get_webview_window(label)
                    .ok_or_else(|| format!("no window {label:?}"))?,
                None => main_window()?,
            };
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
