//! Wire types for the built-in browser. `src/lib/browser/types.ts` mirrors
//! these one to one; both sides use camelCase field names and kebab-case enum
//! values.

use serde::{Deserialize, Serialize};

/// Which concrete surface renders a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceKind {
    /// wry child webview embedded in the owner window (macOS / Windows).
    Child,
    /// Owned top-level window (`WebviewWindowBuilder::parent`).
    Window,
}

/// How the page ↔ host channel was installed for a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelKind {
    /// Isolated-world helper + native message handler.
    Native,
    /// Installation failed; only navigation interception is available.
    Degraded,
    /// Platform too old for isolated worlds (macOS < 11); page-world helper.
    Legacy,
}

/// Placement of the surface inside the owner window, in logical pixels — the
/// same unit `getBoundingClientRect()` reports (the workspace content area is
/// the whole window and the app's zoom changes the root font size, not the
/// webview scale).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BrowserErrorKind {
    Dns,
    Tls,
    Blocked,
    Failed,
    PopupDenied,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserErrorInfo {
    pub kind: BrowserErrorKind,
    pub message: String,
    pub url: Option<String>,
}

/// Everything the toolbar / status layer renders for one tab. Emitted in full
/// on every change (`browser://state`); the frontend keeps it in a store keyed
/// by `tab_id` rather than inside the workspace tab record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTabState {
    pub tab_id: String,
    /// Label of the window the tab belongs to (`main`, `remote-workspace-*`).
    pub owner_window: String,
    pub surface: SurfaceKind,
    pub channel: ChannelKind,
    /// Last committed URL.
    pub url: String,
    /// URL the last navigation was asked for (differs from `url` while loading
    /// or after a redirect).
    pub requested_url: String,
    pub title: String,
    pub favicon: Option<String>,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub origin: Option<String>,
    pub zoom: f64,
    pub error: Option<BrowserErrorInfo>,
    /// Set when the tab's traffic egresses through a remote workspace host.
    pub remote_host: Option<String>,
}

/// Answer to `browser_capabilities`: what this build on this machine can do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCapabilities {
    pub available: bool,
    pub surface: Option<SurfaceKind>,
    pub platform: String,
    pub channel: ChannelKind,
    /// Human-readable reasons behind a degraded answer (for diagnostics UI).
    pub reasons: Vec<String>,
}

/// Caller's surface preference for `browser_open_tab`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceChoice {
    #[default]
    Auto,
    Child,
    Window,
}

pub const STATE_EVENT: &str = "browser://state";
pub const CLOSED_EVENT: &str = "browser://closed";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserClosedPayload {
    pub tab_id: String,
    pub owner_window: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_are_camel_and_kebab() {
        let state = BrowserTabState {
            tab_id: "t1".into(),
            owner_window: "main".into(),
            surface: SurfaceKind::Child,
            channel: ChannelKind::Native,
            url: "about:blank".into(),
            requested_url: "https://example.com/".into(),
            title: String::new(),
            favicon: None,
            loading: true,
            can_go_back: false,
            can_go_forward: false,
            origin: None,
            zoom: 1.0,
            error: None,
            remote_host: None,
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["tabId"], "t1");
        assert_eq!(json["ownerWindow"], "main");
        assert_eq!(json["surface"], "child");
        assert_eq!(json["channel"], "native");
        assert_eq!(json["requestedUrl"], "https://example.com/");
        assert_eq!(json["canGoBack"], false);

        let bounds: Bounds =
            serde_json::from_str(r#"{"x":1,"y":2.5,"width":300,"height":200}"#).unwrap();
        assert_eq!(bounds.y, 2.5);
        let choice: SurfaceChoice = serde_json::from_str(r#""window""#).unwrap();
        assert_eq!(choice, SurfaceChoice::Window);
    }
}
