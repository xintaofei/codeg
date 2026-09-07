//! Browser profile: where tabs keep cookies, caches and storage, and which
//! proxy their traffic goes through. P1 ships one profile, `default`.
//!
//! Why tabs need a container of their own: the workspace webview keeps the
//! app's own localStorage and IndexedDB in WebKit's default data store (macOS)
//! / the app's WebView2 user-data folder (Windows), so "clear browsing data"
//! must never run against the store the app lives in, and a page opened in a
//! tab should not share a cookie jar with the app or with remote-workspace
//! windows. A proxy is a property of that same container
//! (`WKWebsiteDataStore.proxyConfigurations`, WebView2 environment arguments,
//! the WebKitGTK network session), which is the second reason.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

pub const DEFAULT_PROFILE_ID: &str = "default";

/// `WKWebsiteDataStore` identifier of the default profile (macOS 14+):
/// uuid5(NAMESPACE_URL, "https://codeg.app/browser-profile/default"). Fixed so
/// the same store is found again after a restart or an update.
pub const DEFAULT_DATA_STORE_IDENTIFIER: [u8; 16] = [
    0xb5, 0xb1, 0xc6, 0x31, 0xe0, 0x8c, 0x58, 0xf2, 0xba, 0x41, 0x9d, 0x11, 0x62, 0x85, 0x6f, 0x26,
];

/// Directory holding the profile's data on Windows (WebView2 user-data
/// folder) and Linux (WebKitGTK data directory). Unused on macOS, where the
/// data store identifier plays this role.
pub fn directory(profile_id: &str) -> PathBuf {
    crate::paths::codeg_browser_profiles_root().join(profile_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProxyScheme {
    Http,
    Socks5,
}

/// A proxy in the shape every webview engine's hook can take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserProxy {
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
}

impl BrowserProxy {
    /// `scheme://host:port` — what WebView2's `--proxy-server` and tauri's
    /// `proxy_url` accept.
    pub fn to_url_string(&self) -> String {
        let scheme = match self.scheme {
            ProxyScheme::Http => "http",
            ProxyScheme::Socks5 => "socks5",
        };
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!("{scheme}://{host}:{}", self.port)
    }
}

/// Parse the app's (already normalized) proxy URL into what a webview can
/// use. `http`, `socks5` and `socks5h` are accepted — the engines resolve
/// names proxy-side for SOCKS regardless of the `h`. `https` (TLS to the proxy
/// itself) is refused: none of the three engines' proxy hooks express it, so
/// pretending it is plain HTTP CONNECT would send cleartext to a TLS port.
pub fn parse_proxy(raw: &str) -> Result<BrowserProxy, String> {
    let url = tauri::Url::parse(raw.trim()).map_err(|e| format!("invalid proxy URL {raw:?}: {e}"))?;
    let scheme = match url.scheme() {
        "http" => ProxyScheme::Http,
        "socks5" | "socks5h" => ProxyScheme::Socks5,
        other => {
            return Err(format!(
                "the built-in browser cannot use a {other}:// proxy (http and socks5 only)"
            ))
        }
    };
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| format!("proxy URL {raw:?} has no host"))?
        .trim_matches(|c| c == '[' || c == ']')
        .to_string();
    let port = url.port().unwrap_or(match scheme {
        ProxyScheme::Http => 80,
        ProxyScheme::Socks5 => 1080,
    });
    Ok(BrowserProxy { scheme, host, port })
}

/// The proxy browser tabs should use right now.
///
/// The source is the process environment: the app's "system proxy" setting
/// writes `HTTP(S)_PROXY` / `ALL_PROXY` when enabled and clears them when
/// disabled, and values a shell or service manager exported are honoured the
/// same way the app honours them for its own requests and for agent
/// processes. `Err` means a proxy is configured but the browser cannot use it.
pub fn current_proxy() -> Result<Option<BrowserProxy>, String> {
    match crate::network::proxy::effective_proxy_url() {
        None => Ok(None),
        Some(url) => parse_proxy(&url).map(Some),
    }
}

/// How the platform takes a proxy change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProxyApplies {
    /// Open and new tabs use the new proxy for their next connections.
    Live,
    /// Tabs opened after the change use it; open tabs keep the old one.
    NextTab,
    /// The engine fixes the proxy for the process; restart to change it.
    Restart,
    /// This platform (version) cannot proxy browser tabs.
    Unsupported,
}

/// Wire shape of the proxy part of `browser_capabilities`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProxyStatus {
    /// Proxy browser tabs use, as `scheme://host:port`; `None` = direct.
    pub url: Option<String>,
    pub applies: ProxyApplies,
    /// Why `url` is `None` although a proxy is configured (unsupported
    /// scheme or platform), or why the shown proxy is not the configured one
    /// (Windows until a restart).
    pub reason: Option<String>,
}

/// Windows: WebView2 reads the proxy from the environment's browser
/// arguments, which are fixed for a user-data folder for the life of the
/// process — a later environment with different arguments fails to create.
/// The first browser webview therefore freezes the proxy for this run.
#[cfg(target_os = "windows")]
static FROZEN_PROXY: std::sync::OnceLock<Option<BrowserProxy>> = std::sync::OnceLock::new();

/// Windows: the proxy every browser webview of this process is built with.
#[cfg(target_os = "windows")]
pub fn frozen_proxy() -> Option<BrowserProxy> {
    FROZEN_PROXY
        .get_or_init(|| current_proxy().ok().flatten())
        .clone()
}

/// Browser arguments for WebView2. wry's own default flags come first, then
/// silent Integrated Windows Authentication is switched off for every host
/// (an arbitrary page must not be able to make the engine present the user's
/// Windows credentials), then the proxy. wry only injects `--proxy-server`
/// itself when no arguments are given at all, so once we hand it a string we
/// own the whole thing — hence one function for the entire string.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn windows_browser_args(proxy: Option<&BrowserProxy>) -> String {
    let mut args = String::from(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --auth-server-allowlist=",
    );
    if let Some(proxy) = proxy {
        args.push_str(" --proxy-server=");
        args.push_str(&proxy.to_url_string());
    }
    args
}

pub fn proxy_status() -> BrowserProxyStatus {
    platform_proxy_status()
}

#[cfg(target_os = "macos")]
fn platform_proxy_status() -> BrowserProxyStatus {
    if !crate::browser::shim::macos::supports_isolated_profile() {
        return BrowserProxyStatus {
            url: None,
            applies: ProxyApplies::Unsupported,
            reason: Some("proxying browser tabs needs macOS 14 or later".to_string()),
        };
    }
    status_for(ProxyApplies::Live, current_proxy())
}

#[cfg(target_os = "windows")]
fn platform_proxy_status() -> BrowserProxyStatus {
    let frozen = frozen_proxy();
    let mut status = status_for(ProxyApplies::Restart, current_proxy());
    if FROZEN_PROXY.get().is_some() && status.url != frozen.as_ref().map(BrowserProxy::to_url_string) {
        status.reason = Some("restart codeg for browser tabs to use the new proxy".to_string());
        status.url = frozen.map(|p| p.to_url_string());
    }
    status
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_proxy_status() -> BrowserProxyStatus {
    status_for(ProxyApplies::NextTab, current_proxy())
}

fn status_for(applies: ProxyApplies, proxy: Result<Option<BrowserProxy>, String>) -> BrowserProxyStatus {
    match proxy {
        Ok(proxy) => BrowserProxyStatus {
            url: proxy.map(|p| p.to_url_string()),
            applies,
            reason: None,
        },
        Err(reason) => BrowserProxyStatus {
            url: None,
            applies,
            reason: Some(reason),
        },
    }
}

/// Whether browsing data lives apart from the app's own web storage.
pub fn isolated_storage() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::browser::shim::macos::supports_isolated_profile()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Get the profile ready for a tab: its directory exists (Windows / Linux)
/// and, on macOS, its data store exists and points at the current proxy.
/// Idempotent and cheap after the first call.
pub fn prepare(app: &AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let proxy = current_proxy();
        run_on_main(app, move || {
            crate::browser::shim::macos::ensure_profile(proxy_or_none(proxy))
        })?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        let dir = directory(DEFAULT_PROFILE_ID);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create browser profile directory {}: {e}", dir.display()))
    }
}

/// The app's proxy setting changed. macOS re-points the profile's store, which
/// open tabs pick up for their next connections; the other platforms take the
/// change at the next tab (Linux) or restart (Windows) — see `ProxyApplies`.
pub fn proxy_settings_changed(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let proxy = current_proxy();
        if let Err(err) = run_on_main(app, move || {
            crate::browser::shim::macos::ensure_profile(proxy_or_none(proxy))
        })
        .and_then(|r| r)
        {
            tracing::warn!("[browser] could not apply the proxy to the browser profile: {err}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
}

/// An unusable proxy (unsupported scheme) means direct connections, not a
/// failed tab: the settings section explains why.
#[cfg(target_os = "macos")]
fn proxy_or_none(proxy: Result<Option<BrowserProxy>, String>) -> Option<BrowserProxy> {
    match proxy {
        Ok(proxy) => proxy,
        Err(reason) => {
            tracing::warn!("[browser] ignoring the configured proxy: {reason}");
            None
        }
    }
}

#[cfg(target_os = "macos")]
fn run_on_main<R: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce() -> R + Send + 'static,
) -> Result<R, String> {
    if objc2::MainThreadMarker::new().is_some() {
        return Ok(f());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_schemes_a_webview_can_take() {
        let http = parse_proxy("http://127.0.0.1:7890").unwrap();
        assert_eq!(http.scheme, ProxyScheme::Http);
        assert_eq!(http.host, "127.0.0.1");
        assert_eq!(http.port, 7890);
        assert_eq!(http.to_url_string(), "http://127.0.0.1:7890");

        let socks = parse_proxy("socks5h://proxy.corp:1081").unwrap();
        assert_eq!(socks.scheme, ProxyScheme::Socks5);
        assert_eq!(socks.to_url_string(), "socks5://proxy.corp:1081");

        assert_eq!(parse_proxy("http://proxy.corp").unwrap().port, 80);
        assert_eq!(parse_proxy("socks5://proxy.corp").unwrap().port, 1080);
        assert_eq!(parse_proxy("http://[::1]:8080").unwrap().to_url_string(), "http://[::1]:8080");
    }

    #[test]
    fn refuses_what_the_engines_cannot_express() {
        assert!(parse_proxy("https://proxy.corp:443").unwrap_err().contains("https"));
        assert!(parse_proxy("socks4://proxy.corp:1080").is_err());
        assert!(parse_proxy("http://:8080").is_err());
        assert!(parse_proxy("not a url").is_err());
    }

    #[test]
    fn windows_arguments_are_the_whole_string() {
        assert_eq!(
            windows_browser_args(None),
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --auth-server-allowlist="
        );
        let proxy = parse_proxy("socks5://127.0.0.1:1080").unwrap();
        assert_eq!(
            windows_browser_args(Some(&proxy)),
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --auth-server-allowlist= --proxy-server=socks5://127.0.0.1:1080"
        );
        // Same input, same string: WebView2 rejects a second environment on
        // the same user-data folder with different arguments.
        assert_eq!(windows_browser_args(Some(&proxy)), windows_browser_args(Some(&proxy)));
    }

    #[test]
    fn identifier_is_a_version_5_uuid() {
        // Version nibble 5, RFC 4122 variant — what uuid5() produces, so the
        // constant was not typed by hand.
        assert_eq!(DEFAULT_DATA_STORE_IDENTIFIER[6] >> 4, 5);
        assert_eq!(DEFAULT_DATA_STORE_IDENTIFIER[8] & 0xc0, 0x80);
    }

    #[test]
    fn wire_names() {
        let status = BrowserProxyStatus {
            url: Some("http://127.0.0.1:7890".into()),
            applies: ProxyApplies::NextTab,
            reason: None,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["applies"], "next-tab");
        assert_eq!(json["url"], "http://127.0.0.1:7890");
    }
}
