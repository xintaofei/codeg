//! Built-in browser.
//!
//! A browser tab is a Rust-owned webview that renders an arbitrary web page
//! next to the chat: on macOS / Windows a wry child webview embedded in the
//! workspace window at the bounds of a placeholder element (see Cargo.toml's
//! `browser-child` note for why wry is driven directly rather than through
//! tauri's `add_child`), on Linux (and as
//! the fallback everywhere) an owned top-level window. The frontend only ever
//! talks to it through the `browser_*` commands and the `browser://*` events;
//! the page itself has no Tauri IPC at all — `browser-*` labels are absent from
//! every capability on purpose, so a page script cannot reach any command.
//!
//! Module map:
//! - `types`      — wire types shared with `src/lib/browser/types.ts`
//! - `policy`     — pure decisions (scheme allow-list, …)
//! - `registry`   — tab id → surface + last known state
//! - `surface`    — the enum over the concrete surfaces and their common ops
//! - `surface_child` / `surface_window` — the concrete builders
//! - `channel`    — page → host messages from the isolated-world helper
//! - `shim`       — per-platform WebKit / WebView2 calls (worlds, eval, snapshot)
//! - `hooks`      — webview callbacks (page load, title) → registry + events
//! - `events`     — state fan-out to the frontend
//! - `smoke`      — dev-only puppet driven by a JSON control file (feature
//!   `browser-smoke`, never in a release build)

pub mod channel;
pub mod events;
pub mod hooks;
pub mod policy;
pub mod registry;
pub mod surface;
#[cfg(all(
    feature = "browser-child",
    any(target_os = "macos", target_os = "windows")
))]
pub mod surface_child;
pub mod surface_window;
pub mod types;

pub mod shim;

#[cfg(feature = "browser-smoke")]
pub mod smoke;

pub use registry::BrowserRegistry;

/// Label prefix of every browser tab webview / window. Nothing under this
/// prefix may ever appear in `capabilities/*.json`.
pub const TAB_LABEL_PREFIX: &str = "browser-";

pub fn tab_label(tab_id: &str) -> String {
    format!("{TAB_LABEL_PREFIX}{tab_id}")
}
