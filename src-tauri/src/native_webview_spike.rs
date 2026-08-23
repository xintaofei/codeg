use crate::native_webview_capability::{capability_snapshot, SPIKE_LABEL};
use base64::Engine;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{mpsc::sync_channel, Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{
    webview::{NewWindowResponse, WebviewBuilder},
    AppHandle, LogicalPosition, LogicalSize, Manager, Webview, WebviewUrl, WebviewWindow, Window,
    WindowEvent,
};
use webview2_com::{CallDevToolsProtocolMethodCompletedHandler, CoTaskMemPWSTR};
use windows::core::Interface;

const SPIKE_ENV: &str = "CODEG_NATIVE_WEBVIEW2_SPIKE";
const EVIDENCE_ENV: &str = "CODEG_NATIVE_WEBVIEW2_SPIKE_EVIDENCE_DIR";
const GEOMETRY_SEQUENCE_ENV: &str = "CODEG_NATIVE_WEBVIEW2_SPIKE_GEOMETRY_SEQUENCE";
const Z_ORDER_SEQUENCE_ENV: &str = "CODEG_NATIVE_WEBVIEW2_SPIKE_Z_ORDER_SEQUENCE";
const SPIKE_PAGE: &str = include_str!("../spikes/native-webview2/page.html");
const HOST_Z_ORDER_FIXTURE: &str = r#"
(() => {
  document.open();
  document.write(`<!doctype html>
    <meta charset="utf-8">
    <style>
      * { box-sizing: border-box; }
      html, body { width: 100%; height: 100%; margin: 0; }
      body { display: grid; place-items: center; background: #101828; color: #f8fafc; font: 600 18px/1.5 system-ui, sans-serif; }
      #z-order-overlay { position: fixed; inset: 0; display: grid; place-items: center; background: #7c3aed; }
      #z-order-card { max-width: 720px; padding: 40px; border: 3px solid #fef08a; border-radius: 20px; background: #4c1d95; text-align: center; box-shadow: 0 24px 80px #0008; }
      strong { display: block; margin-bottom: 12px; color: #fef08a; font-size: 30px; }
    </style>
    <main id="z-order-overlay" role="dialog" aria-modal="true" aria-label="B2-17 host overlay">
      <div id="z-order-card"><strong>HOST MODAL VISIBLE</strong>Child WebView2 已隐藏；宿主 modal/menu 可完整覆盖原 surface。</div>
    </main>`);
  document.close();
})()
"#;

pub fn maybe_attach(main: &WebviewWindow, effective_data_dir: &Path) -> tauri::Result<()> {
    if !spike_enabled() {
        return Ok(());
    }

    let evidence_dir = evidence_dir();
    fs::create_dir_all(&evidence_dir)?;
    let profile_dir = effective_data_dir
        .join("browser")
        .join("native-webview2-spike-profile");
    fs::create_dir_all(&profile_dir)?;

    let generation = uuid::Uuid::new_v4().to_string();
    write_json(
        &evidence_dir.join("launch.json"),
        &json!({
            "backend": "embedded_webview2",
            "feature": "native-webview-spike",
            "generation": generation,
            "label": SPIKE_LABEL,
            "profileDir": profile_dir,
            "startedAtUnixMs": now_unix_ms(),
        }),
    )?;

    let builder = WebviewBuilder::new(
        SPIKE_LABEL,
        WebviewUrl::External("about:blank".parse().expect("valid about:blank URL")),
    )
    .data_directory(profile_dir)
    .focused(true)
    .devtools(false)
    .browser_extensions_enabled(false)
    .enable_clipboard_access()
    .disable_drag_drop_handler()
    .on_navigation(|url| url.as_str() == "about:blank")
    .on_new_window(|_, _| NewWindowResponse::Deny)
    .on_download(|_, _| false);

    let parent = main.as_ref().window();
    let (position, size) = logical_surface_bounds(&parent);
    let child = parent.add_child(builder, position, size)?;
    let probe_webview = child.clone();
    let probe_host = main.clone();
    let probe_evidence_dir = evidence_dir.clone();
    let probe_generation = generation.clone();
    let _ = thread::Builder::new()
        .name("native-webview2-spike".into())
        .spawn(move || {
            run_probe(
                probe_webview,
                probe_host,
                probe_evidence_dir,
                probe_generation,
            )
        });
    child.set_focus()?;
    parent.set_title("Codeg — Native WebView2 B2-17 Spike")?;
    append_geometry(&evidence_dir, &parent, &child, "attached");
    if geometry_sequence_enabled() {
        let sequence_parent = parent.clone();
        let _ = thread::Builder::new()
            .name("native-webview2-geometry-sequence".into())
            .spawn(move || {
                thread::sleep(Duration::from_millis(500));
                let _ = sequence_parent.unmaximize();
                thread::sleep(Duration::from_millis(300));
                let _ = sequence_parent.set_size(LogicalSize::new(1000.0, 720.0));
                thread::sleep(Duration::from_millis(300));
                let _ = sequence_parent.set_position(LogicalPosition::new(120.0, 120.0));
                thread::sleep(Duration::from_millis(300));
                let _ = sequence_parent.maximize();
                thread::sleep(Duration::from_millis(300));
                let _ = sequence_parent.unmaximize();
                thread::sleep(Duration::from_millis(300));
                if let Ok(monitors) = sequence_parent.available_monitors() {
                    for monitor in monitors {
                        let origin = monitor.position();
                        let inset = (120.0 * monitor.scale_factor()).round() as i32;
                        let _ = sequence_parent.set_position(tauri::PhysicalPosition::new(
                            origin.x + inset,
                            origin.y + inset,
                        ));
                        thread::sleep(Duration::from_millis(600));
                    }
                }
            });
    }

    Ok(())
}

pub(crate) fn spike_enabled() -> bool {
    std::env::var(SPIKE_ENV)
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn geometry_sequence_enabled() -> bool {
    std::env::var(GEOMETRY_SEQUENCE_ENV)
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn z_order_sequence_enabled() -> bool {
    std::env::var(Z_ORDER_SEQUENCE_ENV)
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

pub(crate) fn sync_geometry_on_window_event(window: &Window, event: &WindowEvent) {
    if !spike_enabled() || window.label() != "main" {
        return;
    }

    let reason = match event {
        WindowEvent::Resized(_) => Some("resized"),
        WindowEvent::Moved(_) => Some("moved"),
        WindowEvent::ScaleFactorChanged { .. } => Some("scale_factor_changed"),
        _ => None,
    };
    let Some(reason) = reason else {
        return;
    };
    let Some(child) = window.app_handle().get_webview(SPIKE_LABEL) else {
        return;
    };

    let (position, size) = logical_surface_bounds(window);
    let _ = child.set_position(position);
    let _ = child.set_size(size);
    append_geometry(&evidence_dir(), window, &child, reason);
}

pub(crate) fn close_for_exit(app: &AppHandle) {
    if !spike_enabled() {
        return;
    }

    let evidence_dir = evidence_dir();
    let registered_before = app.get_webview(SPIKE_LABEL).is_some();
    let close_result = app
        .get_webview(SPIKE_LABEL)
        .map(|webview| webview.close().map_err(|error| error.to_string()))
        .transpose();
    let registered_after = app.get_webview(SPIKE_LABEL).is_some();
    append_jsonl(
        &evidence_dir.join("lifecycle.jsonl"),
        &json!({
            "event": "exit_requested",
            "registeredBefore": registered_before,
            "closeResult": match close_result {
                Ok(_) => "PASS",
                Err(_) => "FAIL",
            },
            "closeError": close_result.err(),
            "registeredAfter": registered_after,
            "timestampUnixMs": now_unix_ms(),
        }),
    );
}

fn evidence_dir() -> PathBuf {
    std::env::var_os(EVIDENCE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("codeg-native-webview2-spike"))
}

fn logical_surface_bounds(window: &Window) -> (LogicalPosition<f64>, LogicalSize<f64>) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let inner = window
        .inner_size()
        .map(|size| size.to_logical::<f64>(scale))
        .unwrap_or_else(|_| LogicalSize::new(1260.0, 860.0));
    let margin = 16.0;
    let top = 96.0_f64.min((inner.height - margin).max(margin));
    let available_width = (inner.width - margin * 2.0).max(320.0);
    let width = (inner.width * 0.58).clamp(320.0, available_width);
    let height = (inner.height - top - margin).max(320.0);
    let x = (inner.width - width - margin).max(margin);
    (
        LogicalPosition::new(x, top),
        LogicalSize::new(width, height),
    )
}

fn run_probe(webview: Webview, host: WebviewWindow, evidence_dir: PathBuf, generation: String) {
    thread::sleep(Duration::from_millis(250));
    match automated_probe(&webview, &evidence_dir, &generation) {
        Ok(evidence) => {
            let _ = write_json(&evidence_dir.join("automated.json"), &evidence);
            if z_order_sequence_enabled() {
                if let Err(error) = run_z_order_probe(&host, &webview, &evidence_dir, &generation) {
                    let _ = write_json(
                        &evidence_dir.join("z-order.json"),
                        &json!({
                            "generation": generation,
                            "status": "FAIL",
                            "error": error,
                            "finishedAtUnixMs": now_unix_ms(),
                        }),
                    );
                    set_page_status(&webview, "fail", "z-order 检查失败；请查看证据目录。");
                    return;
                }
            }
            set_page_status(
                &webview,
                "pass",
                "自动检查通过。现在请直接点击、输入、选择、滚动并测试中文输入法。",
            );
            let _ = webview.set_focus();
            monitor_manual_input(webview, evidence_dir, generation);
        }
        Err(error) => {
            let _ = write_json(
                &evidence_dir.join("automated.json"),
                &json!({
                    "backend": "embedded_webview2",
                    "generation": generation,
                    "status": "FAIL",
                    "error": error,
                    "finishedAtUnixMs": now_unix_ms(),
                }),
            );
            set_page_status(&webview, "fail", &format!("自动检查失败：{error}"));
        }
    }
}

fn automated_probe(
    webview: &Webview,
    evidence_dir: &Path,
    generation: &str,
) -> Result<Value, String> {
    let capability = capability_snapshot()?;
    if capability["passed"] != Value::Bool(true) {
        return Err("the main window capability still covers the child webview".into());
    }

    webview.hide().map_err(|error| error.to_string())?;
    webview.show().map_err(|error| error.to_string())?;
    let window = webview.window();
    let (position, size) = logical_surface_bounds(&window);
    webview
        .set_position(position)
        .map_err(|error| error.to_string())?;
    webview.set_size(size).map_err(|error| error.to_string())?;
    webview
        .navigate("about:blank".parse().expect("valid about:blank URL"))
        .map_err(|error| error.to_string())?;
    thread::sleep(Duration::from_millis(300));

    let mut controllers = BTreeSet::new();
    for method in ["Runtime.enable", "DOM.enable", "Page.enable"] {
        call_and_track(webview, method, json!({}), &mut controllers)?;
    }

    let page_literal = serde_json::to_string(SPIKE_PAGE).map_err(|error| error.to_string())?;
    call_and_track(
        webview,
        "Runtime.evaluate",
        json!({
            "expression": format!(
                "document.open();document.write({page_literal});document.close();'ready'"
            ),
            "returnByValue": true,
        }),
        &mut controllers,
    )?;
    thread::sleep(Duration::from_millis(150));

    let document = call_and_track(
        webview,
        "DOM.getDocument",
        json!({ "depth": 1, "pierce": true }),
        &mut controllers,
    )?;
    let root_node_id = document
        .pointer("/root/nodeId")
        .and_then(Value::as_i64)
        .ok_or_else(|| "DOM.getDocument did not return a root node".to_string())?;
    let query = call_and_track(
        webview,
        "DOM.querySelector",
        json!({ "nodeId": root_node_id, "selector": "#native-spike-root" }),
        &mut controllers,
    )?;
    let spike_node_id = query
        .get("nodeId")
        .and_then(Value::as_i64)
        .filter(|node_id| *node_id > 0)
        .ok_or_else(|| "the native spike page root was not found".to_string())?;
    let before = call_and_track(
        webview,
        "DOM.getOuterHTML",
        json!({ "nodeId": spike_node_id }),
        &mut controllers,
    )?;
    if !before["outerHTML"]
        .as_str()
        .is_some_and(|html| html.contains("等待 Agent 侧 CDP 修改"))
    {
        return Err("CDP DOM read did not observe the expected marker".into());
    }

    let generation_literal =
        serde_json::to_string(generation).map_err(|error| error.to_string())?;
    call_and_track(
        webview,
        "Runtime.evaluate",
        json!({
            "expression": format!(
                "(() => {{ const marker = document.getElementById('agent-marker'); marker.textContent = 'Agent CDP 已修改同一页面'; marker.dataset.generation = {generation_literal}; return marker.textContent; }})()"
            ),
            "returnByValue": true,
        }),
        &mut controllers,
    )?;
    let after = call_and_track(
        webview,
        "DOM.getOuterHTML",
        json!({ "nodeId": spike_node_id }),
        &mut controllers,
    )?;
    if !after["outerHTML"]
        .as_str()
        .is_some_and(|html| html.contains("Agent CDP 已修改同一页面"))
    {
        return Err("the Agent-side CDP mutation was not visible in the same DOM".into());
    }

    let targets = call_and_track(webview, "Target.getTargets", json!({}), &mut controllers)?;
    let page_target_count = targets["targetInfos"]
        .as_array()
        .map(|targets| {
            targets
                .iter()
                .filter(|target| target["type"] == "page")
                .count()
        })
        .ok_or_else(|| "Target.getTargets did not return targetInfos".to_string())?;
    if page_target_count != 1 {
        return Err(format!(
            "expected one WebView2 page target in the isolated environment, found {page_target_count}"
        ));
    }

    let ipc = call_and_track(
        webview,
        "Runtime.evaluate",
        json!({
            "expression": "(async () => { const internals = globalThis.__TAURI_INTERNALS__; if (!internals || typeof internals.invoke !== 'function') return JSON.stringify({ internals: false, invocationAttempted: false, allowed: false, denial: 'not_exposed' }); try { await internals.invoke('browser_get_status'); return JSON.stringify({ internals: true, invocationAttempted: true, allowed: true, denial: null }); } catch (error) { return JSON.stringify({ internals: true, invocationAttempted: true, allowed: false, denial: String(error).slice(0, 240) }); } })()",
            "awaitPromise": true,
            "returnByValue": true,
        }),
        &mut controllers,
    )?;
    let ipc_value = runtime_value(&ipc)
        .and_then(Value::as_str)
        .ok_or_else(|| "IPC isolation probe returned no value".to_string())?;
    let ipc: Value = serde_json::from_str(ipc_value).map_err(|error| error.to_string())?;
    if ipc["allowed"] == Value::Bool(true) {
        return Err("the child webview inherited Codeg Tauri command access".into());
    }

    let screenshot = call_and_track(
        webview,
        "Page.captureScreenshot",
        json!({
            "format": "png",
            "fromSurface": true,
            "captureBeyondViewport": false,
        }),
        &mut controllers,
    )?;
    let screenshot_data = screenshot["data"]
        .as_str()
        .ok_or_else(|| "Page.captureScreenshot returned no data".to_string())?;
    let screenshot_bytes = base64::engine::general_purpose::STANDARD
        .decode(screenshot_data)
        .map_err(|error| error.to_string())?;
    let screenshot_path = evidence_dir.join("same-controller.png");
    fs::write(&screenshot_path, &screenshot_bytes).map_err(|error| error.to_string())?;

    let layout = call_and_track(
        webview,
        "Page.getLayoutMetrics",
        json!({}),
        &mut controllers,
    )?;
    let browser = call_and_track(webview, "Browser.getVersion", json!({}), &mut controllers)?;
    if controllers.len() != 1 {
        return Err(format!(
            "CDP calls crossed {} controller identities",
            controllers.len()
        ));
    }

    let monitors = window
        .available_monitors()
        .map_err(|error| error.to_string())?;
    let mut monitor_scales: Vec<f64> = monitors
        .iter()
        .map(|monitor| monitor.scale_factor())
        .collect();
    monitor_scales.sort_by(f64::total_cmp);
    monitor_scales.dedup();
    let monitor_details: Vec<Value> = monitors
        .iter()
        .map(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            json!({
                "name": monitor.name(),
                "scaleFactor": monitor.scale_factor(),
                "position": { "x": position.x, "y": position.y },
                "size": { "width": size.width, "height": size.height },
            })
        })
        .collect();
    let physical_position = webview.position().map_err(|error| error.to_string())?;
    let physical_size = webview.size().map_err(|error| error.to_string())?;
    let initial_geometry = geometry_snapshot(&window, webview, "automated_probe");

    Ok(json!({
        "backend": "embedded_webview2",
        "generation": generation,
        "status": "PASS",
        "controllerIdentity": controllers.iter().next().copied(),
        "checks": {
            "childCreatePositionResizeShowHideNavigateDestroyApi": "PASS_EXCEPT_DESTROY_RESERVED_FOR_APP_EXIT",
            "sameControllerCdp": true,
            "cdpDomRead": true,
            "agentSideDomMutation": true,
            "cdpScreenshot": true,
            "singlePageTarget": true,
            "tauriIpcDenied": true,
        },
        "capabilityIsolation": capability,
        "ipcProbe": ipc,
        "pageTargetCount": page_target_count,
        "browserProduct": browser["product"],
        "browserProtocolVersion": browser["protocolVersion"],
        "layoutViewport": layout["cssLayoutViewport"],
        "windowScaleFactor": window.scale_factor().map_err(|error| error.to_string())?,
        "availableMonitorCount": monitor_details.len(),
        "availableMonitorScaleFactors": monitor_scales,
        "availableMonitors": monitor_details,
        "surfacePhysicalBounds": {
            "x": physical_position.x,
            "y": physical_position.y,
            "width": physical_size.width,
            "height": physical_size.height,
        },
        "initialGeometry": initial_geometry,
        "screenshot": screenshot_path,
        "manualInputGate": "OPEN",
        "finishedAtUnixMs": now_unix_ms(),
    }))
}

fn call_and_track(
    webview: &Webview,
    method: &str,
    params: Value,
    controllers: &mut BTreeSet<usize>,
) -> Result<Value, String> {
    let response = call_cdp(webview, method, params)?;
    controllers.insert(response.controller_identity);
    let value: Value = serde_json::from_str(&response.body)
        .map_err(|error| format!("{method} returned invalid JSON: {error}"))?;
    if let Some(error) = value.get("error") {
        return Err(format!("{method} failed: {error}"));
    }
    Ok(value)
}

struct CdpResponse {
    body: String,
    controller_identity: usize,
}

fn call_cdp(webview: &Webview, method: &str, params: Value) -> Result<CdpResponse, String> {
    let method = method.to_string();
    let params = serde_json::to_string(&params).map_err(|error| error.to_string())?;
    let (tx, rx) = sync_channel(1);
    webview
        .with_webview(move |platform| {
            let controller = platform.controller();
            let controller_identity = controller.as_raw() as usize;
            let result = (|| -> Result<CdpResponse, String> {
                let core =
                    unsafe { controller.CoreWebView2() }.map_err(|error| error.to_string())?;
                let response = Arc::new(Mutex::new(None::<String>));
                let completed_response = response.clone();
                let method_value = method.clone();
                let params_value = params.clone();
                CallDevToolsProtocolMethodCompletedHandler::wait_for_async_operation(
                    Box::new(move |handler| unsafe {
                        let method = CoTaskMemPWSTR::from(method_value.as_str());
                        let params = CoTaskMemPWSTR::from(params_value.as_str());
                        core.CallDevToolsProtocolMethod(
                            *method.as_ref().as_pcwstr(),
                            *params.as_ref().as_pcwstr(),
                            &handler,
                        )
                        .map_err(webview2_com::Error::WindowsError)
                    }),
                    Box::new(move |status, body| {
                        status?;
                        if let Ok(mut slot) = completed_response.lock() {
                            *slot = Some(body);
                        }
                        Ok(())
                    }),
                )
                .map_err(|error| error.to_string())?;
                let body = response
                    .lock()
                    .map_err(|_| "CDP response mutex was poisoned".to_string())?
                    .take()
                    .ok_or_else(|| "WebView2 returned no CDP response".to_string())?;
                Ok(CdpResponse {
                    body,
                    controller_identity,
                })
            })();
            let _ = tx.send(result);
        })
        .map_err(|error| error.to_string())?;
    rx.recv_timeout(Duration::from_secs(15))
        .map_err(|error| format!("timed out waiting for WebView2 CDP: {error}"))?
}

fn runtime_value(value: &Value) -> Option<&Value> {
    value.pointer("/result/value")
}

fn set_page_status(webview: &Webview, state: &str, message: &str) {
    let state = serde_json::to_string(state).unwrap_or_else(|_| "\"fail\"".into());
    let message = serde_json::to_string(message).unwrap_or_else(|_| "\"probe error\"".into());
    let expression = format!(
        "(() => {{ const status = document.getElementById('probe-status'); if (!status) return; status.dataset.state = {state}; status.textContent = {message}; }})()"
    );
    let _ = call_cdp(
        webview,
        "Runtime.evaluate",
        json!({ "expression": expression, "returnByValue": true }),
    );
}

fn run_z_order_probe(
    host: &WebviewWindow,
    webview: &Webview,
    evidence_dir: &Path,
    generation: &str,
) -> Result<(), String> {
    host.navigate("about:blank".parse().expect("valid about:blank URL"))
        .map_err(|error| error.to_string())?;
    thread::sleep(Duration::from_millis(300));
    host.eval(HOST_Z_ORDER_FIXTURE)
        .map_err(|error| error.to_string())?;
    thread::sleep(Duration::from_millis(300));
    append_jsonl(
        &evidence_dir.join("z-order.jsonl"),
        &json!({
            "generation": generation,
            "phase": "host_overlay_prepared_child_visible",
            "timestampUnixMs": now_unix_ms(),
        }),
    );

    webview.hide().map_err(|error| error.to_string())?;
    append_jsonl(
        &evidence_dir.join("z-order.jsonl"),
        &json!({
            "generation": generation,
            "phase": "host_overlay_visible_child_hidden",
            "timestampUnixMs": now_unix_ms(),
        }),
    );
    thread::sleep(Duration::from_secs(15));

    host.eval(
        "document.getElementById('z-order-overlay')?.remove(); document.body.dataset.zOrderOverlay = 'dismissed';",
    )
    .map_err(|error| error.to_string())?;
    append_jsonl(
        &evidence_dir.join("z-order.jsonl"),
        &json!({
            "generation": generation,
            "phase": "host_overlay_dismissed",
            "timestampUnixMs": now_unix_ms(),
        }),
    );
    webview.show().map_err(|error| error.to_string())?;
    webview.set_focus().map_err(|error| error.to_string())?;
    let continuity = call_cdp(
        webview,
        "Runtime.evaluate",
        json!({
            "expression": "document.getElementById('agent-marker')?.textContent ?? null",
            "returnByValue": true,
        }),
    )?;
    let continuity_value: Value =
        serde_json::from_str(&continuity.body).map_err(|error| error.to_string())?;
    let marker = runtime_value(&continuity_value)
        .and_then(Value::as_str)
        .unwrap_or_default();
    if marker != "Agent CDP 已修改同一页面" {
        return Err("child controller did not preserve the DOM across hide/show".into());
    }
    write_json(
        &evidence_dir.join("z-order.json"),
        &json!({
            "generation": generation,
            "status": "PASS",
            "strategy": "host overlay explicitly hides the child surface, then restores the same controller",
            "childHide": true,
            "childShow": true,
            "hostOverlayDismissedBeforeChildShow": true,
            "sameControllerIdentity": continuity.controller_identity,
            "domContinuity": true,
            "finishedAtUnixMs": now_unix_ms(),
        }),
    )
    .map_err(|error| error.to_string())?;
    append_jsonl(
        &evidence_dir.join("z-order.jsonl"),
        &json!({
            "generation": generation,
            "phase": "child_restored_same_controller",
            "controllerIdentity": continuity.controller_identity,
            "timestampUnixMs": now_unix_ms(),
        }),
    );
    Ok(())
}

fn monitor_manual_input(webview: Webview, evidence_dir: PathBuf, generation: String) {
    let mut previous = String::new();
    let mut consecutive_errors = 0_u32;
    for _ in 0..1_200 {
        thread::sleep(Duration::from_millis(500));
        let response = match call_cdp(
            &webview,
            "Runtime.evaluate",
            json!({
                "expression": "JSON.stringify(globalThis.__nativeSpikeState?.() ?? null)",
                "returnByValue": true,
            }),
        ) {
            Ok(response) => {
                consecutive_errors = 0;
                response
            }
            Err(error) => {
                consecutive_errors += 1;
                append_jsonl(
                    &evidence_dir.join("manual-monitor.jsonl"),
                    &json!({
                        "generation": generation,
                        "consecutiveErrors": consecutive_errors,
                        "error": error,
                        "timestampUnixMs": now_unix_ms(),
                    }),
                );
                if consecutive_errors >= 20 {
                    break;
                }
                continue;
            }
        };
        let response: Value = match serde_json::from_str(&response.body) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let Some(state) = runtime_value(&response).and_then(Value::as_str) else {
            continue;
        };
        if state == previous {
            continue;
        }
        previous = state.to_string();
        let state: Value = match serde_json::from_str(state) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let events = state["events"].as_array().cloned().unwrap_or_default();
        let trusted_event_count = events
            .iter()
            .filter(|event| event["isTrusted"] == Value::Bool(true))
            .count();
        let composition_event_count = events
            .iter()
            .filter(|event| {
                event["type"]
                    .as_str()
                    .is_some_and(|kind| kind.starts_with("composition"))
            })
            .count();
        let _ = write_json(
            &evidence_dir.join("manual-state.json"),
            &json!({
                "generation": generation,
                "status": "OBSERVING",
                "trustedEventCount": trusted_event_count,
                "compositionEventCount": composition_event_count,
                "state": state,
                "updatedAtUnixMs": now_unix_ms(),
            }),
        );
    }
}

fn append_geometry(evidence_dir: &Path, window: &Window, webview: &Webview, reason: &str) {
    append_jsonl(
        &evidence_dir.join("geometry.jsonl"),
        &geometry_snapshot(window, webview, reason),
    );
}

fn geometry_snapshot(window: &Window, webview: &Webview, reason: &str) -> Value {
    let scale_factor = window.scale_factor().ok();
    let (logical_position, logical_size) = logical_surface_bounds(window);
    let expected_physical = scale_factor.map(|scale| {
        json!({
            "x": (logical_position.x * scale).round() as i32,
            "y": (logical_position.y * scale).round() as i32,
            "width": (logical_size.width * scale).round() as u32,
            "height": (logical_size.height * scale).round() as u32,
        })
    });
    let surface_position = webview.position().ok();
    let surface_size = webview.size().ok();
    let geometry_matches_expected = match (
        expected_physical.as_ref(),
        surface_position.as_ref(),
        surface_size.as_ref(),
    ) {
        (Some(expected), Some(position), Some(size)) => {
            let expected_x = expected["x"].as_i64().unwrap_or_default();
            let expected_y = expected["y"].as_i64().unwrap_or_default();
            let expected_width = expected["width"].as_u64().unwrap_or_default();
            let expected_height = expected["height"].as_u64().unwrap_or_default();
            (i64::from(position.x) - expected_x).abs() <= 2
                && (i64::from(position.y) - expected_y).abs() <= 2
                && (u64::from(size.width).abs_diff(expected_width)) <= 2
                && (u64::from(size.height).abs_diff(expected_height)) <= 2
        }
        _ => false,
    };
    let current_monitor = window.current_monitor().ok().flatten().map(|monitor| {
        let position = monitor.position();
        let size = monitor.size();
        json!({
            "name": monitor.name(),
            "scaleFactor": monitor.scale_factor(),
            "position": { "x": position.x, "y": position.y },
            "size": { "width": size.width, "height": size.height },
        })
    });
    json!({
        "reason": reason,
        "scaleFactor": scale_factor,
        "windowOuterPosition": window.outer_position().ok().map(|position| json!({ "x": position.x, "y": position.y })),
        "windowInnerSize": window.inner_size().ok().map(|size| json!({ "width": size.width, "height": size.height })),
        "windowMaximized": window.is_maximized().ok(),
        "windowVisible": window.is_visible().ok(),
        "currentMonitor": current_monitor,
        "expectedSurfaceLogicalBounds": {
            "x": logical_position.x,
            "y": logical_position.y,
            "width": logical_size.width,
            "height": logical_size.height,
        },
        "expectedSurfacePhysicalBounds": expected_physical,
        "surfacePosition": surface_position.map(|position| json!({ "x": position.x, "y": position.y })),
        "surfaceSize": surface_size.map(|size| json!({ "width": size.width, "height": size.height })),
        "geometryMatchesExpected": geometry_matches_expected,
        "timestampUnixMs": now_unix_ms(),
    })
}

fn append_jsonl(path: &Path, value: &Value) {
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{value}");
}

fn write_json(path: &Path, value: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    fs::write(path, bytes)
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
