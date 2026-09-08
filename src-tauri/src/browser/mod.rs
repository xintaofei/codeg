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
//! - `profile`    — the tabs' own data store / directory and their proxy
//! - `downloads`  — destination policy and records for page downloads
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
pub mod downloads;
pub mod events;
pub mod hooks;
pub mod policy;
pub mod profile;
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

pub use downloads::BrowserDownloads;
pub use registry::BrowserRegistry;

/// Label prefix of every browser tab webview / window. Nothing under this
/// prefix may ever appear in `capabilities/*.json`.
pub const TAB_LABEL_PREFIX: &str = "browser-";

pub fn tab_label(tab_id: &str) -> String {
    format!("{TAB_LABEL_PREFIX}{tab_id}")
}

#[cfg(test)]
mod tests {
    use super::TAB_LABEL_PREFIX;

    /// A browser tab must have NO Tauri IPC: its label (and the popup and
    /// document-guest prefixes) may never appear in a capability's window
    /// list, and no capability may use a bare wildcard that would cover it.
    #[test]
    fn browser_labels_are_absent_from_every_capability() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("capabilities dir") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let raw = std::fs::read_to_string(&path).unwrap();
            let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
            let windows = json["windows"].as_array().cloned().unwrap_or_default();
            for pattern in windows.iter().filter_map(|w| w.as_str()) {
                assert_ne!(pattern, "*", "{}: a bare wildcard covers browser tabs", path.display());
                assert_ne!(pattern, "**", "{}: a bare wildcard covers browser tabs", path.display());
                for forbidden in [TAB_LABEL_PREFIX, "browser-popup-", "codeg-doc-"] {
                    assert!(
                        !pattern.starts_with(forbidden),
                        "{}: capability window pattern {pattern:?} grants IPC to browser surfaces",
                        path.display()
                    );
                }
            }
            checked += 1;
        }
        assert!(checked >= 1, "no capability files found");
    }

    #[test]
    fn tab_labels_carry_the_prefix() {
        assert_eq!(super::tab_label("abc"), "browser-abc");
    }
}
