//! Platform-specific WebKit / WebView2 calls that neither tauri nor wry
//! expose: isolated-world scripts and message handlers, world-scoped
//! evaluation, snapshots, back / forward. Every function here runs on the
//! main thread against the live platform webview and is only ever reached
//! through `surface_child::ChildHandle` (or, later, `with_webview` for owned
//! windows).

#[cfg(target_os = "macos")]
pub mod macos;
