//! Windows shim on top of WebView2 (`webview2-com` + the `windows` crate).
//!
//! The helper script and the primitive it posts through live in a CDP
//! **isolated world** named `codeg`:
//! `Page.addScriptToEvaluateOnNewDocument {worldName}` injects the helper into
//! that world for every document of every frame, and
//! `Runtime.addBinding {executionContextName}` puts `__codegSend` there — a
//! separate JavaScript global over the same DOM, invisible to page scripts and
//! immune to their prototype tampering, exactly like macOS's `WKContentWorld`.
//! Unlike macOS the world needs no prefix script: the binding *is* the send
//! primitive the helper looks for.
//!
//! Everything else a tab needs and neither tauri nor wry expose is an
//! `ICoreWebView2` call from here: history, stop, the load flag (WebView2 has
//! no `isLoading` property, so the navigation events keep one), typed
//! navigation failures, snapshots, find in page, the sign-in identity, and the
//! fence that keeps a `block` site rule in force inside iframes (WebView2's
//! `NavigationStarting` is main-frame only).
//!
//! Every function runs on the main thread against the live webview. The
//! per-webview state lives in a thread-local keyed by the `ICoreWebView2`
//! pointer — which is also what a channel message reports as its source — and
//! is dropped by `forget_navigation_delegate` when the surface goes. Event
//! handlers capture that key rather than the state itself, so nothing here can
//! keep a webview alive past its tab.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{json, Value};
use tauri::Url;
use tauri_runtime_wry::wry::{self, WebViewExtWindows};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2DevToolsProtocolEventReceiver, ICoreWebView2Environment15,
    ICoreWebView2Find, ICoreWebView2Frame, ICoreWebView2Frame2, ICoreWebView2Frame7,
    ICoreWebView2PermissionRequestedEventArgs3,
    ICoreWebView2Settings2, ICoreWebView2_28, ICoreWebView2_4,
    COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT, COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG,
    COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_MULTIPLE_AUTOMATIC_DOWNLOADS,
    COREWEBVIEW2_PERMISSION_STATE_ALLOW, COREWEBVIEW2_PERMISSION_STATE_DENY,
    COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, COREWEBVIEW2_WEB_ERROR_STATUS,
    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT,
    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED,
    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID,
    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED,
    COREWEBVIEW2_WEB_ERROR_STATUS_CLIENT_CERTIFICATE_CONTAINS_ERRORS,
    COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED,
    COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
};
use webview2_com::{
    take_pwstr, CallDevToolsProtocolMethodCompletedHandler, CapturePreviewCompletedHandler,
    DevToolsProtocolEventReceivedEventHandler, FindStartCompletedHandler, FrameChildFrameCreatedEventHandler,
    FrameCreatedEventHandler, FrameNavigationStartingEventHandler, NavigationCompletedEventHandler,
    NavigationStartingEventHandler, PermissionRequestedEventHandler,
    WebResourceRequestedEventHandler,
};
use windows::core::{Interface, BOOL, HSTRING, PWSTR};
use windows::Win32::Foundation::RECT;
use windows::Win32::System::Com::{IStream, STREAM_SEEK_SET};
use windows::Win32::UI::Shell::SHCreateMemStream;

use super::super::channel::MessageSink;
use super::super::hooks::LoadFailure;
use super::super::profile;
use super::super::types::BrowserErrorKind;
pub use super::{NavigationEvent, NavigationSink};

/// Name of the isolated world the helper and the binding live in. Must match
/// nothing a page can name: CDP worlds are addressed by this string alone.
pub const WORLD_NAME: &str = "codeg";
/// The function `Runtime.addBinding` exposes in that world; the helper's send
/// primitive (`src/browser-injected/helper.js` looks for exactly this name).
pub const BINDING_NAME: &str = "__codegSend";

/// Whether a subframe may navigate to this address — the same decision the
/// main frame's navigation handler makes, asked for a frame WebView2 would
/// otherwise never report. `surface_child` provides it; `false` cancels.
pub type FrameNavigationSink = std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Whether a page may start SEVERAL downloads at once (the address it asks
/// from, and whether the engine considers the request user-initiated).
/// `false` refuses. Answered by the host so the engine keeps its own prompt
/// — drawn inside the page's rectangle, in nobody's design language, and
/// invisible to codeg — out of an embedded tab.
pub type DownloadPermissionSink =
    std::sync::Arc<dyn Fn(&str, bool) -> bool + Send + Sync>;

#[derive(Default)]
struct SurfaceState {
    /// Between a main-frame `NavigationStarting` and its `NavigationCompleted`.
    /// WebView2 has no load property of its own; this stands in for one.
    loading: Cell<bool>,
    /// Where the navigation in flight is heading (its last start or redirect),
    /// so a failure can name the address that never arrived.
    started_url: RefCell<Option<String>>,
    /// The host stopped the load in flight. WebView2 reports the abort as an
    /// ordinary connection failure, and an error page over a load the user
    /// stopped on purpose would be the engine contradicting the user.
    stopped: Cell<bool>,
    /// CDP frame id of the top-level frame. The safety of the channel rests on
    /// it: a `bindingCalled` counts as main-frame only when its execution
    /// context belongs to this frame.
    main_frame: RefCell<Option<String>>,
    /// `codeg`-world execution contexts: CDP context id → the frame it is for.
    contexts: RefCell<HashMap<i64, String>>,
    /// The helper, kept for the documents the injection cannot reach in time
    /// (see `recover_world`).
    helper: RefCell<Option<String>>,
    /// The engine's own user agent, read before anything overrode it, so the
    /// sign-in identity can be taken back off again.
    default_user_agent: RefCell<Option<String>>,
    /// What `find_string` searched for last: the same term again is "next
    /// match", a different one starts a new search.
    find_term: RefCell<Option<String>>,
    channel_installed: Cell<bool>,
    sink: RefCell<Option<MessageSink>>,
    navigation: RefCell<Option<NavigationSink>>,
    /// Kept alive for the life of the surface: dropping a receiver ends the
    /// subscription that feeds the channel.
    receivers: RefCell<Vec<ICoreWebView2DevToolsProtocolEventReceiver>>,
}

thread_local! {
    /// State by `ICoreWebView2` pointer. Main thread only.
    static STATES: RefCell<HashMap<usize, Rc<SurfaceState>>> = RefCell::new(HashMap::new());
}

/// The state of a webview, cloned out so no borrow of the map is held while a
/// sink runs — a sink reaches the registry, which reaches back in here.
fn state(key: usize) -> Option<Rc<SurfaceState>> {
    STATES.with(|states| states.borrow().get(&key).cloned())
}

fn state_of(key: usize) -> Rc<SurfaceState> {
    STATES.with(|states| states.borrow_mut().entry(key).or_default().clone())
}

fn core(webview: &wry::WebView) -> ICoreWebView2 {
    WebViewExtWindows::webview(webview)
}

/// The engine object behind a wry `WebView`, for a caller that must let go of
/// the surface it came from before using it (see `install_world`).
pub fn engine_webview(webview: &wry::WebView) -> ICoreWebView2 {
    core(webview)
}

/// Identity of the platform webview behind a wry `WebView`, matching the
/// `source` a message sink receives.
pub fn webview_pointer(webview: &wry::WebView) -> usize {
    core(webview).as_raw() as usize
}

fn pwstr_out(read: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> Option<String> {
    let mut value = PWSTR::null();
    read(&mut value).ok()?;
    Some(take_pwstr(value))
}

// ---------------------------------------------------------------------------
// History, stop, load state
// ---------------------------------------------------------------------------

pub fn go_back(webview: &wry::WebView) {
    // SAFETY: main thread, live webview.
    let _ = unsafe { core(webview).GoBack() };
}

pub fn go_forward(webview: &wry::WebView) {
    // SAFETY: main thread, live webview.
    let _ = unsafe { core(webview).GoForward() };
}

pub fn can_go_back(webview: &wry::WebView) -> bool {
    let mut can = BOOL::from(false);
    // SAFETY: main thread, live webview; `can` is a valid out parameter.
    unsafe { core(webview).CanGoBack(&mut can) }.is_ok() && can.as_bool()
}

pub fn can_go_forward(webview: &wry::WebView) -> bool {
    let mut can = BOOL::from(false);
    // SAFETY: main thread, live webview; `can` is a valid out parameter.
    unsafe { core(webview).CanGoForward(&mut can) }.is_ok() && can.as_bool()
}

pub fn stop_loading(webview: &wry::WebView) {
    let webview2 = core(webview);
    if let Some(state) = state(webview2.as_raw() as usize) {
        state.stopped.set(true);
    }
    // SAFETY: main thread, live webview.
    let _ = unsafe { webview2.Stop() };
}

/// Whether a navigation is in flight, from the flag the navigation hooks keep.
/// `false` for a webview whose hooks were never installed — the load watcher
/// then falls back to judging by what committed.
pub fn is_loading(webview: &wry::WebView) -> bool {
    state(webview_pointer(webview)).is_some_and(|s| s.loading.get())
}

/// The committed URL, or `None` before anything has committed.
pub fn current_url(webview: &wry::WebView) -> Option<String> {
    // SAFETY: main thread, live webview; WebView2 hands back an owned string.
    pwstr_out(|out| unsafe { core(webview).Source(out) }).filter(|u| !u.is_empty())
}

/// Diagnostic view of the native state (dev puppet only).
pub fn debug_view(webview: &wry::WebView) -> Value {
    let key = webview_pointer(webview);
    let controller = WebViewExtWindows::controller(webview);
    let mut visible = BOOL::from(false);
    let mut bounds = RECT::default();
    // SAFETY: main thread, live controller; both are valid out parameters.
    unsafe {
        let _ = controller.IsVisible(&mut visible);
        let _ = controller.Bounds(&mut bounds);
    }
    let state = state(key);
    json!({
        "isLoading": state.as_ref().is_some_and(|s| s.loading.get()),
        "hasUrl": current_url(webview).is_some(),
        "source": current_url(webview),
        "visible": visible.as_bool(),
        "bounds": [bounds.left, bounds.top, bounds.right - bounds.left, bounds.bottom - bounds.top],
        "channelInstalled": state.as_ref().is_some_and(|s| s.channel_installed.get()),
        "mainFrame": state.as_ref().and_then(|s| s.main_frame.borrow().clone()),
        "worldContexts": state.as_ref().map(|s| s.contexts.borrow().len()).unwrap_or(0),
    })
}

// ---------------------------------------------------------------------------
// The page ↔ host channel, over CDP
// ---------------------------------------------------------------------------

/// Install the helper in the `codeg` isolated world and the binding it posts
/// through. Every step is awaited: the caller navigates only once this
/// returns, so no document can load before the world is ready. `Ok(true)` —
/// there is no page-world fallback on Windows, a failure is reported as one.
///
/// Takes the engine object rather than the wry `WebView` on purpose: waiting
/// for WebView2's completions means pumping the message loop, and whatever
/// that pump dispatches — a tab being closed, another being opened — must
/// find the caller's surface map free.
pub fn install_world(
    webview2: ICoreWebView2,
    helper: &str,
    sink: MessageSink,
) -> Result<bool, String> {
    let key = webview2.as_raw() as usize;
    let state = state_of(key);
    if state.channel_installed.get() {
        return Ok(true);
    }
    *state.sink.borrow_mut() = Some(sink);
    *state.helper.borrow_mut() = Some(helper.to_string());
    // Subscribed before `Runtime.enable`, so the contexts it announces for
    // documents that already exist are seen too.
    for event in [
        "Runtime.executionContextCreated",
        "Runtime.executionContextDestroyed",
        "Runtime.executionContextsCleared",
        "Runtime.bindingCalled",
        "Page.frameNavigated",
    ] {
        subscribe(&webview2, key, event)?;
    }
    call_and_wait(&webview2, "Runtime.enable", "{}")?;
    call_and_wait(&webview2, "Page.enable", "{}")?;
    // The top-level frame id, the yardstick every `bindingCalled` is measured
    // against. `Page.frameNavigated` keeps it current afterwards.
    let tree = call_and_wait(&webview2, "Page.getFrameTree", "{}")?;
    if let Some(id) = serde_json::from_str::<Value>(&tree)
        .ok()
        .and_then(|v| v["frameTree"]["frame"]["id"].as_str().map(str::to_string))
    {
        *state.main_frame.borrow_mut() = Some(id);
    }
    call_and_wait(
        &webview2,
        "Page.addScriptToEvaluateOnNewDocument",
        &json!({ "source": helper, "worldName": WORLD_NAME }).to_string(),
    )?;
    call_and_wait(
        &webview2,
        "Runtime.addBinding",
        &json!({ "name": BINDING_NAME, "executionContextName": WORLD_NAME }).to_string(),
    )?;
    state.channel_installed.set(true);
    Ok(true)
}

/// Call a CDP method and pump the message loop until its completion arrives
/// (wry does the same for its own document scripts). The raw result JSON is
/// returned; an error there is an error here, so `install_world` cannot
/// report a world it did not get.
fn call_and_wait(webview: &ICoreWebView2, method: &str, params: &str) -> Result<String, String> {
    let answer = Rc::new(RefCell::new(String::new()));
    let received = answer.clone();
    let webview = webview.clone();
    let name = HSTRING::from(method);
    let params = HSTRING::from(params);
    CallDevToolsProtocolMethodCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| {
            // SAFETY: main thread, live webview; the handler outlives the call.
            unsafe { webview.CallDevToolsProtocolMethod(&name, &params, &handler) }
                .map_err(Into::into)
        }),
        Box::new(move |result, json| {
            result?;
            *received.borrow_mut() = json;
            Ok(())
        }),
    )
    .map_err(|err| format!("{method} failed: {err:?}"))?;
    let answer = answer.borrow().clone();
    Ok(answer)
}

/// Fire and forget: a CDP call whose answer is handled by `done`.
fn call_async(
    webview: &ICoreWebView2,
    method: &str,
    params: &str,
    done: impl FnOnce(Result<String, String>) + 'static,
) -> Result<(), String> {
    let name = HSTRING::from(method);
    let params = HSTRING::from(params);
    let method = method.to_string();
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
        move |result, json| {
            match result {
                Ok(()) => done(Ok(json)),
                Err(err) => done(Err(format!("{method} failed: {err}"))),
            }
            Ok(())
        },
    ));
    // SAFETY: main thread, live webview; WebView2 owns the handler until it
    // has called it.
    unsafe { webview.CallDevToolsProtocolMethod(&name, &params, &handler) }
        .map_err(|e| e.to_string())
}

/// Route one CDP event into `on_event`, keeping the receiver alive with the
/// surface.
fn subscribe(webview: &ICoreWebView2, key: usize, event: &str) -> Result<(), String> {
    let name = HSTRING::from(event);
    // SAFETY: main thread, live webview.
    let receiver = unsafe { webview.GetDevToolsProtocolEventReceiver(&name) }
        .map_err(|e| format!("cannot subscribe to {event}: {e}"))?;
    let received = event.to_string();
    let mut token = 0i64;
    // SAFETY: main thread; the handler is owned by the receiver, which the
    // surface state keeps until the webview goes.
    unsafe {
        receiver.add_DevToolsProtocolEventReceived(
            &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    if let Some(json) = pwstr_out(|out| args.ParameterObjectAsJson(out)) {
                        on_event(key, &received, &json);
                    }
                }
                Ok(())
            })),
            &mut token,
        )
    }
    .map_err(|e| format!("cannot subscribe to {event}: {e}"))?;
    state_of(key).receivers.borrow_mut().push(receiver);
    Ok(())
}

fn on_event(key: usize, event: &str, raw: &str) {
    let Some(state) = state(key) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return;
    };
    match event {
        "Runtime.executionContextCreated" => {
            let context = &value["context"];
            if context["name"].as_str() != Some(WORLD_NAME) {
                return;
            }
            let (Some(id), Some(frame)) = (
                context["id"].as_i64(),
                context["auxData"]["frameId"].as_str(),
            ) else {
                return;
            };
            state.contexts.borrow_mut().insert(id, frame.to_string());
        }
        "Runtime.executionContextDestroyed" => {
            if let Some(id) = value["executionContextId"].as_i64() {
                state.contexts.borrow_mut().remove(&id);
            }
        }
        "Runtime.executionContextsCleared" => state.contexts.borrow_mut().clear(),
        "Page.frameNavigated" => {
            let frame = &value["frame"];
            // No parent = the top-level frame. Its id survives ordinary
            // navigations, but a process swap can mint a new one.
            if frame["parentId"].is_null() {
                if let Some(id) = frame["id"].as_str() {
                    *state.main_frame.borrow_mut() = Some(id.to_string());
                }
            }
        }
        "Runtime.bindingCalled" => {
            if value["name"].as_str() != Some(BINDING_NAME) {
                return;
            }
            let Some(payload) = value["payload"].as_str() else {
                return;
            };
            // Which frame spoke is decided here, from the execution context
            // the engine reports — never from the envelope's own `top` field,
            // which the page's frame could set to anything. An unknown context
            // is not the main frame.
            let frame = value["executionContextId"]
                .as_i64()
                .and_then(|id| state.contexts.borrow().get(&id).cloned());
            let main_frame = match (frame, state.main_frame.borrow().clone()) {
                (Some(frame), Some(main)) => frame == main,
                _ => false,
            };
            let sink = state.sink.borrow().clone();
            if let Some(sink) = sink {
                sink(payload.to_string(), main_frame, key);
            }
        }
        _ => {}
    }
}

/// Build the `codeg` world for a document the injection did not reach and run
/// the helper in it.
///
/// `Page.addScriptToEvaluateOnNewDocument` only covers documents whose
/// navigation starts after it is registered, and an adopted popup's first
/// document is already on its way by the time the engine hands the webview
/// over — so that one document would have no channel for its whole life, and
/// a popup is exactly where the address bar and the gesture ring are needed.
/// `Page.createIsolatedWorld` makes the same named world for a frame that
/// exists; the binding is registered by world NAME, so `__codegSend` is in it
/// and the helper finds what it expects.
fn recover_world(webview: &ICoreWebView2, key: usize) {
    let Some(state) = state(key) else {
        return;
    };
    let (Some(frame), Some(helper)) = (
        state.main_frame.borrow().clone(),
        state.helper.borrow().clone(),
    ) else {
        return;
    };
    let params = json!({ "frameId": frame, "worldName": WORLD_NAME }).to_string();
    let webview = webview.clone();
    let _ = call_async(
        &webview.clone(),
        "Page.createIsolatedWorld",
        &params,
        move |answer| {
            let context = answer.ok().and_then(|json| {
                serde_json::from_str::<Value>(&json)
                    .ok()?["executionContextId"]
                    .as_i64()
            });
            let Some(context) = context else {
                return;
            };
            tracing::debug!("[browser] page channel recovered in context {context}");
            let params = json!({ "expression": helper, "contextId": context }).to_string();
            let _ = call_async(&webview, "Runtime.evaluate", &params, |_| {});
        },
    );
}

/// The `codeg` world of the main frame, once the helper has run there.
fn main_world_context(state: &SurfaceState) -> Option<i64> {
    let main = state.main_frame.borrow().clone()?;
    state
        .contexts
        .borrow()
        .iter()
        .find(|(_, frame)| **frame == main)
        .map(|(id, _)| *id)
}

/// Evaluate `expression` in the `codeg` world of the main frame. The result
/// arrives as the JSON string `{"ok":true,"value":…}` or
/// `{"ok":false,"error":…}` — the same envelope macOS produces, built inside
/// the page so no platform value conversion is needed.
pub fn eval_in_world(
    webview: &wry::WebView,
    expression: &str,
    callback: impl Fn(Result<String, String>) + Send + 'static,
) -> Result<(), String> {
    let webview2 = core(webview);
    let key = webview2.as_raw() as usize;
    let context = state(key)
        .and_then(|s| main_world_context(&s))
        .ok_or("the page has no isolated world yet")?;
    let wrapped = format!(
        "(function(){{try{{return JSON.stringify({{ok:true,value:(function(){{return ({expression});}})()}})}}catch(e){{return JSON.stringify({{ok:false,error:String(e&&e.stack||e)}})}}}})()"
    );
    let params = json!({
        "expression": wrapped,
        "contextId": context,
        "returnByValue": true,
    })
    .to_string();
    call_async(&webview2, "Runtime.evaluate", &params, move |answer| {
        callback(answer.and_then(|json| {
            let value: Value =
                serde_json::from_str(&json).map_err(|e| format!("unreadable result: {e}"))?;
            if let Some(details) = value["exceptionDetails"].as_object() {
                return Err(details
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("evaluation failed")
                    .to_string());
            }
            value["result"]["value"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "non-string result".to_string())
        }));
    })
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

/// Viewport snapshot as PNG bytes.
pub fn snapshot_png(
    webview: &wry::WebView,
    callback: impl Fn(Result<Vec<u8>, String>) + Send + 'static,
) -> Result<(), String> {
    capture(
        webview,
        COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
        callback,
    )
}

/// Viewport snapshot as JPEG: `(bytes, pixel width, pixel height)`, for the
/// freeze frame a placeholder shows while its surface is hidden.
/// `CapturePreview` has no quality knob — WebView2 picks one — so `quality` is
/// only what the caller would have asked for.
pub fn snapshot_jpeg(
    webview: &wry::WebView,
    quality: f64,
    callback: impl Fn(Result<(Vec<u8>, u32, u32), String>) + Send + 'static,
) -> Result<(), String> {
    let _ = quality;
    // The size the engine will capture, read now: by the time the bytes come
    // back the callback has no webview to ask.
    let fallback = viewport_size(webview);
    capture(
        webview,
        COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG,
        move |result| {
            callback(result.and_then(|bytes| {
                let (width, height) = jpeg_size(&bytes)
                    .or(fallback)
                    .ok_or_else(|| "snapshot has no readable size".to_string())?;
                Ok((bytes, width, height))
            }))
        },
    )
}

fn viewport_size(webview: &wry::WebView) -> Option<(u32, u32)> {
    let mut bounds = RECT::default();
    // SAFETY: main thread, live controller; a valid out parameter.
    unsafe { WebViewExtWindows::controller(webview).Bounds(&mut bounds) }.ok()?;
    let width = u32::try_from(bounds.right - bounds.left).ok()?;
    let height = u32::try_from(bounds.bottom - bounds.top).ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

fn capture(
    webview: &wry::WebView,
    format: COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT,
    done: impl FnOnce(Result<Vec<u8>, String>) + 'static,
) -> Result<(), String> {
    let webview2 = core(webview);
    // SAFETY: shlwapi's growable in-memory stream; `None` starts it empty.
    let stream = unsafe { SHCreateMemStream(None) }
        .ok_or_else(|| "cannot allocate a snapshot buffer".to_string())?;
    let filled = stream.clone();
    let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
        let outcome = match result {
            Ok(()) => read_stream(&filled),
            Err(err) => Err(err.to_string()),
        };
        done(outcome);
        Ok(())
    }));
    // SAFETY: main thread, live webview; WebView2 owns both the stream and the
    // handler until it has called back.
    unsafe { webview2.CapturePreview(format, &stream, &handler) }.map_err(|e| e.to_string())
}

fn read_stream(stream: &IStream) -> Result<Vec<u8>, String> {
    // SAFETY: a live stream this process created; every buffer below is valid
    // for the length passed with it.
    unsafe {
        stream
            .Seek(0, STREAM_SEEK_SET, None)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            stream
                .Read(
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    Some(&mut read),
                )
                .ok()
                .map_err(|e| e.to_string())?;
            if read == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buffer[..read as usize]);
        }
    }
}

/// Pixel size of a JPEG, from its frame header. `CapturePreview` reports no
/// dimensions and the placeholder needs the aspect ratio; the controller's
/// bounds are the fallback.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut at = 2usize;
    while at + 3 < bytes.len() {
        // Fill bytes between segments.
        if bytes[at] != 0xFF || bytes[at + 1] == 0xFF {
            at += 1;
            continue;
        }
        let marker = bytes[at + 1];
        // Standalone markers: no length follows.
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            at += 2;
            continue;
        }
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        // SOF0…SOF15 carry the size; DHT, JPG and DAC share the range.
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let height = u16::from_be_bytes([*bytes.get(at + 5)?, *bytes.get(at + 6)?]);
            let width = u16::from_be_bytes([*bytes.get(at + 7)?, *bytes.get(at + 8)?]);
            return Some((u32::from(width), u32::from(height)));
        }
        at = at.checked_add(2 + length.max(2))?;
    }
    None
}

// ---------------------------------------------------------------------------
// Find in page
// ---------------------------------------------------------------------------

fn finder(webview: &wry::WebView) -> Option<ICoreWebView2Find> {
    // SAFETY: main thread, live webview; the cast fails on runtimes without
    // the interface, which is the version check.
    unsafe { core(webview).cast::<ICoreWebView2_28>().ok()?.Find() }.ok()
}

fn match_count(find: &ICoreWebView2Find) -> i32 {
    let mut count = 0i32;
    // SAFETY: main thread, live object; a valid out parameter.
    unsafe { find.MatchCount(&mut count) }.ok();
    count
}

/// Highlight the next (or previous) occurrence of `query`, the way Ctrl+F does
/// in Edge: the engine owns the search and the selection, so this never
/// touches the DOM and cannot be observed or broken by the page. Its own find
/// bar stays suppressed — codeg draws that. `callback` gets whether anything
/// matched.
pub fn find_string(
    webview: &wry::WebView,
    query: &str,
    forward: bool,
    callback: impl Fn(bool) + Send + 'static,
) -> Result<(), String> {
    let find = finder(webview).ok_or("this WebView2 runtime cannot search a page")?;
    let state = state_of(webview_pointer(webview));
    let repeat = state.find_term.borrow().as_deref() == Some(query);
    if repeat {
        // SAFETY: main thread, live find object.
        unsafe {
            if forward {
                find.FindNext()
            } else {
                find.FindPrevious()
            }
        }
        .map_err(|e| e.to_string())?;
        callback(match_count(&find) > 0);
        return Ok(());
    }
    let environment = WebViewExtWindows::environment(webview);
    // SAFETY: main thread, live environment and options.
    let options = unsafe {
        let options = environment
            .cast::<ICoreWebView2Environment15>()
            .map_err(|_| "this WebView2 runtime cannot search a page".to_string())?
            .CreateFindOptions()
            .map_err(|e| e.to_string())?;
        options
            .SetFindTerm(&HSTRING::from(query))
            .map_err(|e| e.to_string())?;
        let _ = options.SetIsCaseSensitive(false);
        let _ = options.SetShouldMatchWord(false);
        let _ = options.SetShouldHighlightAllMatches(true);
        let _ = options.SetSuppressDefaultFindDialog(true);
        options
    };
    *state.find_term.borrow_mut() = Some(query.to_string());
    let counted = find.clone();
    let handler = FindStartCompletedHandler::create(Box::new(move |result| {
        callback(result.is_ok() && match_count(&counted) > 0);
        Ok(())
    }));
    // SAFETY: main thread; WebView2 owns the options and the handler until the
    // search has finished.
    unsafe { find.Start(&options, &handler) }.map_err(|e| e.to_string())
}

/// Drop the find highlight and end the session, so the next search starts
/// fresh. A runtime without the interface has nothing to clear.
pub fn clear_find(webview: &wry::WebView) -> Result<(), String> {
    if let Some(state) = state(webview_pointer(webview)) {
        *state.find_term.borrow_mut() = None;
    }
    let Some(find) = finder(webview) else {
        return Ok(());
    };
    // SAFETY: main thread, live find object.
    unsafe { find.Stop() }.map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// The identity a page is shown (the sign-in exception)
// ---------------------------------------------------------------------------

/// Re-decide the identity for the page the webview shows (the preference
/// changed), from its own URL. Main thread. A webview with nothing committed
/// is left alone: its first navigation decides.
pub fn refresh_user_agent(webview: &wry::WebView) {
    let webview2 = core(webview);
    let key = webview2.as_raw() as usize;
    if let Some(url) = current_url(webview).and_then(|u| Url::parse(&u).ok()) {
        apply_user_agent(&webview2, key, &url);
    }
}

/// The engine's own user agent, read once before anything overrode it.
fn default_user_agent(state: &SurfaceState, settings: &ICoreWebView2Settings2) -> Option<String> {
    if let Some(cached) = state.default_user_agent.borrow().clone() {
        return Some(cached);
    }
    // SAFETY: main thread, live settings; WebView2 hands back an owned string.
    let value = pwstr_out(|out| unsafe { settings.UserAgent(out) })?;
    *state.default_user_agent.borrow_mut() = Some(value.clone());
    Some(value)
}

/// Give the webview the user agent `profile::user_agent_for` wants for a
/// main-frame navigation to `url`.
///
/// This is the half the PAGE sees: `navigator.userAgent` and everything the
/// document derives from it. Set as the navigation starts, it does reach the
/// document being navigated to (measured) — but not the request that is
/// already on its way, and `NavigationStarting`'s own request headers are
/// read-only. What goes on the wire is therefore written in
/// `apply_request_user_agent`, per request, where headers can still be
/// changed.
fn apply_user_agent(webview: &ICoreWebView2, key: usize, url: &Url) {
    // SAFETY: main thread, live webview.
    let Ok(settings) = (unsafe { webview.Settings() }).and_then(|s| s.cast::<ICoreWebView2Settings2>())
    else {
        return;
    };
    let state = state_of(key);
    let Some(default) = default_user_agent(&state, &settings) else {
        return;
    };
    let wanted = profile::user_agent_for(url);
    let value = wanted.unwrap_or(default.as_str());
    // SAFETY: main thread, live settings.
    let current = pwstr_out(|out| unsafe { settings.UserAgent(out) });
    if current.as_deref() != Some(value) {
        tracing::debug!(
            "[browser] user agent for {}: {}",
            url.host_str().unwrap_or("?"),
            if wanted.is_some() { "sign-in identity" } else { "engine's own" }
        );
        // SAFETY: main thread, live settings.
        let _ = unsafe { settings.SetUserAgent(&HSTRING::from(value)) };
    }
}

/// The other half: the identity a document request actually carries. Written
/// on the request itself, which is the only place WebView2 lets it be changed
/// in time — the setting above reaches the page but not the navigation that
/// is already in flight, so without this the sign-in host would be asked with
/// the engine's own identity and refuse the very first load.
///
/// Both directions are written, not only the sign-in one: a navigation away
/// from a sign-in host must not carry the borrowed identity out with it.
fn apply_request_user_agent(
    key: usize,
    args: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2WebResourceRequestedEventArgs,
) {
    // SAFETY: main thread; the args are live for the duration of the event.
    let Ok(request) = (unsafe { args.Request() }) else {
        return;
    };
    let Some(url) = pwstr_out(|out| unsafe { request.Uri(out) }).and_then(|u| Url::parse(&u).ok())
    else {
        return;
    };
    let wanted = profile::user_agent_for(&url);
    let default = state(key).and_then(|state| state.default_user_agent.borrow().clone());
    let Some(value) = wanted.map(str::to_string).or(default) else {
        return;
    };
    // SAFETY: main thread; the request and its headers are live.
    if let Ok(headers) = unsafe { request.Headers() } {
        let _ = unsafe { headers.SetHeader(&HSTRING::from("User-Agent"), &HSTRING::from(value)) };
    }
}

// ---------------------------------------------------------------------------
// Navigation hooks: what wry does not report
// ---------------------------------------------------------------------------
//
// wry forwards two of WebView2's navigation events (content loading, navigation
// completed) as "page load started / finished" and hands `NavigationStarting`
// to the host as a yes/no policy question carrying the URL alone. A tab needs
// three more things: where a page-initiated navigation is heading (so the
// address bar follows the page and not only the toolbar), why a load failed
// (WebView2 commits its OWN error page and tells nobody, so without this the
// tab reports a perfectly successful load of a page the user never asked for),
// and whether a navigation is still in flight (there is no `isLoading`).
//
// Subframes are the fourth: `NavigationStarting` fires for the main frame only,
// so an iframe would slip past the scheme list and the `block` site rules
// entirely. Every frame is caught as it is created — including frames of
// frames — and asked the same question.

/// Install the navigation hooks. `navigation` receives what wry does not
/// report; `frames` decides subframe navigations; `downloads` answers the
/// engine's multiple-downloads permission. Idempotent per webview.
pub fn install_navigation_hooks(
    webview: &wry::WebView,
    navigation: NavigationSink,
    frames: FrameNavigationSink,
    downloads: DownloadPermissionSink,
) -> Result<(), String> {
    let webview2 = core(webview);
    let key = webview2.as_raw() as usize;
    let state = state_of(key);
    if state.navigation.borrow().is_some() {
        return Ok(());
    }
    *state.navigation.borrow_mut() = Some(navigation);
    // Read the engine's own identity before anything can override it.
    // SAFETY: main thread, live webview.
    if let Ok(settings) = (unsafe { webview2.Settings() }).and_then(|s| s.cast::<ICoreWebView2Settings2>()) {
        default_user_agent(&state, &settings);
    }

    let mut token = 0i64;
    let started = webview2.clone();
    // SAFETY: main thread, live webview; the handler is owned by it.
    unsafe {
        webview2.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    on_navigation_starting(&started, key, &args);
                }
                Ok(())
            })),
            &mut token,
        )
    }
    .map_err(|e| format!("cannot watch navigation starts: {e}"))?;
    let completed = webview2.clone();
    // SAFETY: main thread, live webview; the handler is owned by it.
    unsafe {
        webview2.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    on_navigation_completed(&completed, key, &args);
                }
                Ok(())
            })),
            &mut token,
        )
    }
    .map_err(|e| format!("cannot watch navigation completions: {e}"))?;
    // Document requests, for the identity they carry. Filtered to documents:
    // the sign-in exception is about which page is being asked for, and every
    // other request would be a per-subresource callback for nothing.
    // SAFETY: main thread, live webview; the handler is owned by it.
    unsafe {
        if webview2
            .AddWebResourceRequestedFilter(
                &HSTRING::from("*"),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
            )
            .is_ok()
        {
            let _ = webview2.add_WebResourceRequested(
                &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        apply_request_user_agent(key, &args);
                    }
                    Ok(())
                })),
                &mut token,
            );
        }
    }
    // The second download from the same page raises a permission request, and
    // until it is answered the engine draws its own bubble over the page and
    // holds `DownloadStarting` back — so a user who clicked would see nothing
    // happen and codeg would have no idea there was anything to show. Only
    // this one kind is taken over: a camera or a microphone is the user's to
    // grant, and the engine's prompt is the right place to do it.
    // SAFETY: main thread, live webview; the handler is owned by it.
    unsafe {
        let _ = webview2.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                args.PermissionKind(&mut kind)?;
                if kind != COREWEBVIEW2_PERMISSION_KIND_MULTIPLE_AUTOMATIC_DOWNLOADS {
                    return Ok(());
                }
                // Every request is decided fresh. The engine would otherwise
                // write the first answer into the profile and stop asking,
                // freezing whichever way one page happened to be treated —
                // and the test here is about the click that came before this
                // request, not about the site.
                if let Ok(args3) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
                    let _ = args3.SetSavesInProfile(false);
                }
                let uri = pwstr_out(|out| args.Uri(out)).unwrap_or_default();
                let mut initiated = BOOL::default();
                let _ = args.IsUserInitiated(&mut initiated);
                let allowed = downloads(&uri, initiated.as_bool());
                args.SetState(if allowed {
                    COREWEBVIEW2_PERMISSION_STATE_ALLOW
                } else {
                    COREWEBVIEW2_PERMISSION_STATE_DENY
                })?;
                Ok(())
            })),
            &mut token,
        );
    }
    // Subframes: only reachable through the frame objects, and only for frames
    // created after this subscription — which is why it goes in before the
    // first navigation.
    // SAFETY: main thread, live webview; the handler is owned by it.
    if let Ok(webview4) = webview2.cast::<ICoreWebView2_4>() {
        let _ = unsafe {
            webview4.add_FrameCreated(
                &FrameCreatedEventHandler::create(Box::new(move |_, args| {
                    if let Some(frame) = args.and_then(|args| args.Frame().ok()) {
                        watch_frame(&frame, frames.clone());
                    }
                    Ok(())
                })),
                &mut token,
            )
        };
    }
    Ok(())
}

/// Drop the state of a webview that is going away (call before the wry
/// `WebView` is dropped, on the main thread).
pub fn forget_navigation_delegate(webview: &wry::WebView) {
    let key = webview_pointer(webview);
    STATES.with(|states| states.borrow_mut().remove(&key));
}

fn watch_frame(frame: &ICoreWebView2Frame, decide: FrameNavigationSink) {
    let mut token = 0i64;
    let asked = decide.clone();
    // SAFETY: main thread; WebView2 keeps the frame object alive for as long
    // as the frame exists, and with it the handler.
    if let Ok(frame2) = frame.cast::<ICoreWebView2Frame2>() {
        let _ = unsafe {
            frame2.add_NavigationStarting(
                &FrameNavigationStartingEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let uri = pwstr_out(|out| args.Uri(out)).unwrap_or_default();
                    if !asked(&uri) {
                        args.SetCancel(true)?;
                    }
                    Ok(())
                })),
                &mut token,
            )
        };
    }
    // A frame inside the frame is not reported by the webview's own
    // `FrameCreated`; without this an embedded page could nest its way past
    // the rules.
    // SAFETY: main thread, as above.
    if let Ok(frame7) = frame.cast::<ICoreWebView2Frame7>() {
        let _ = unsafe {
            frame7.add_FrameCreated(
                &FrameChildFrameCreatedEventHandler::create(Box::new(move |_, args| {
                    if let Some(child) = args.and_then(|args| args.Frame().ok()) {
                        watch_frame(&child, decide.clone());
                    }
                    Ok(())
                })),
                &mut token,
            )
        };
    }
}

fn report(key: usize, event: NavigationEvent) {
    let sink = state(key).and_then(|s| s.navigation.borrow().clone());
    if let Some(sink) = sink {
        sink(event);
    }
}

fn on_navigation_starting(
    webview: &ICoreWebView2,
    key: usize,
    args: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2NavigationStartingEventArgs,
) {
    let mut cancelled = BOOL::from(false);
    let mut redirected = BOOL::from(false);
    // SAFETY: main thread; the args are live and both are valid out parameters.
    unsafe {
        let _ = args.Cancel(&mut cancelled);
        let _ = args.IsRedirected(&mut redirected);
    }
    // wry's own handler ran first and put the host's answer on the args. A
    // refused redirect ends the load in flight; a refused new navigation never
    // started one.
    if cancelled.as_bool() {
        if redirected.as_bool() {
            report(key, NavigationEvent::Interrupted);
        }
        return;
    }
    // SAFETY: main thread; the args are live.
    let Some(uri) = pwstr_out(|out| unsafe { args.Uri(out) }) else {
        return;
    };
    if let Ok(url) = Url::parse(&uri) {
        apply_user_agent(webview, key, &url);
    }
    if let Some(state) = state(key) {
        state.loading.set(true);
        *state.started_url.borrow_mut() = Some(uri.clone());
        // A new navigation is not the one that was stopped.
        state.stopped.set(false);
        // The engine drops the find session with the document; the next
        // search has to start one rather than ask it for a next match.
        *state.find_term.borrow_mut() = None;
    }
    report(
        key,
        if redirected.as_bool() {
            NavigationEvent::Redirected(uri)
        } else {
            NavigationEvent::Started(uri)
        },
    );
}

fn on_navigation_completed(
    webview: &ICoreWebView2,
    key: usize,
    args: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2NavigationCompletedEventArgs,
) {
    let mut succeeded = BOOL::from(false);
    let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
    // SAFETY: main thread; the args are live and both are valid out parameters.
    unsafe {
        let _ = args.IsSuccess(&mut succeeded);
        let _ = args.WebErrorStatus(&mut status);
    }
    let mut url = None;
    let mut worldless = false;
    let mut stopped = false;
    if let Some(state) = state(key) {
        state.loading.set(false);
        url = state.started_url.borrow_mut().take();
        worldless = state.channel_installed.get() && main_world_context(&state).is_none();
        stopped = state.stopped.replace(false);
    }
    if stopped && !succeeded.as_bool() {
        tracing::debug!("[browser] navigation stopped by the host ({status:?})");
        return;
    }
    if succeeded.as_bool() {
        // A document the injection did not reach: give it a world now rather
        // than leave it without a channel for as long as it is shown.
        if worldless {
            recover_world(webview, key);
        }
        return;
    }
    // Did anything commit for the navigation that failed? Chromium puts its
    // own error page in place of a load that failed on its own, so the address
    // that failed is the one showing. A navigation that was ABANDONED —
    // because it turned into a download, or the page started another one —
    // commits nothing and leaves the tab on the document it had. Only the
    // first is a failure of the page: an error page over a perfectly good
    // document the user is still reading is worse than saying nothing. (The
    // engine's status cannot tell them apart: it says the connection was
    // aborted for both.)
    // SAFETY: main thread, live webview.
    let showing = pwstr_out(|out| unsafe { webview.Source(out) });
    if url.is_some() && url != showing {
        tracing::debug!("[browser] navigation abandoned before it committed ({status:?})");
        return;
    }
    let Some(kind) = classify_web_error(status) else {
        // Superseded, stopped by the user, or refused by the host's own
        // handler: not a failure of the page.
        tracing::debug!("[browser] navigation ended without a page ({status:?})");
        return;
    };
    tracing::debug!("[browser] navigation failed: {status:?} -> {kind:?}");
    report(
        key,
        NavigationEvent::Failed(LoadFailure {
            kind,
            // WebView2 has no message for a status, and the status layer words
            // the error itself, in the user's language.
            message: String::new(),
            url,
            provisional: true,
        }),
    );
}

/// Classify a WebView2 navigation failure. `None` means "not a failure of the
/// page" — the cancelled status covers a superseded load, a stop, a navigation
/// our own handler refused, and a response that became a download. The kinds
/// are the same ones macOS derives from `NSURLErrorDomain`.
fn classify_web_error(status: COREWEBVIEW2_WEB_ERROR_STATUS) -> Option<BrowserErrorKind> {
    const TLS: [COREWEBVIEW2_WEB_ERROR_STATUS; 5] = [
        COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT,
        COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED,
        COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID,
        COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED,
        COREWEBVIEW2_WEB_ERROR_STATUS_CLIENT_CERTIFICATE_CONTAINS_ERRORS,
    ];
    if status == COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED {
        return None;
    }
    if status == COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED {
        return Some(BrowserErrorKind::Dns);
    }
    if TLS.contains(&status) {
        return Some(BrowserErrorKind::Tls);
    }
    Some(BrowserErrorKind::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT, COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT,
        COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN,
        COREWEBVIEW2_WEB_ERROR_STATUS_VALID_AUTHENTICATION_CREDENTIALS_REQUIRED,
    };

    /// The engine's statuses, by kind — and the one that is NOT a page failure.
    #[test]
    fn load_errors_classify_by_engine_status() {
        use BrowserErrorKind::*;
        assert_eq!(
            classify_web_error(COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED),
            Some(Dns)
        );
        assert_eq!(
            classify_web_error(COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED),
            Some(Tls)
        );
        assert_eq!(
            classify_web_error(COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID),
            Some(Tls)
        );
        assert_eq!(
            classify_web_error(COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT),
            Some(Failed)
        );
        assert_eq!(
            classify_web_error(COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT),
            Some(Failed)
        );
        assert_eq!(
            classify_web_error(COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN),
            Some(Failed)
        );
        assert_eq!(
            classify_web_error(
                COREWEBVIEW2_WEB_ERROR_STATUS_VALID_AUTHENTICATION_CREDENTIALS_REQUIRED
            ),
            Some(Failed)
        );
        // Superseded / stopped / refused by us / became a download.
        assert_eq!(
            classify_web_error(COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED),
            None
        );
    }

    /// The freeze frame needs the captured size, and `CapturePreview` reports
    /// none: it comes out of the JPEG's own frame header.
    #[test]
    fn jpeg_dimensions_come_from_the_frame_header() {
        // SOI, an APP0 segment to skip over, then a baseline SOF0 of 320×200.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0xC8, 0x01, 0x40]);
        assert_eq!(jpeg_size(&jpeg), Some((320, 200)));
        // Progressive JPEGs carry the size in SOF2, and a Huffman table in the
        // same marker range must not be mistaken for one.
        let progressive = [
            0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC2, 0x00, 0x11, 0x08, 0x01,
            0x00, 0x02, 0x00,
        ];
        assert_eq!(jpeg_size(&progressive), Some((512, 256)));
        assert_eq!(jpeg_size(&[0xFF, 0xD8]), None);
        assert_eq!(jpeg_size(b"not a jpeg"), None);
        assert_eq!(jpeg_size(&[]), None);
    }
}
