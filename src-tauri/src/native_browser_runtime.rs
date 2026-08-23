use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const EMBEDDED_WEBVIEW2_BACKEND_ID: &str = "embedded_webview2";
pub const NATIVE_BRIDGE_PROTOCOL_VERSION: &str = "1";
pub const NATIVE_BRIDGE_MAX_BODY_BYTES: usize = 1024 * 1024;
pub const NATIVE_BRIDGE_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg_attr(
    not(any(test, all(feature = "tauri-runtime", target_os = "windows"))),
    allow(dead_code)
)]
const INITIAL_URL: &str = "about:blank";
const INITIAL_TITLE: &str = "New Tab";
const MAX_CONNECTION_ID_BYTES: usize = 256;
const MAX_TAB_ID_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NativeSurfaceBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl NativeSurfaceBounds {
    fn validate(self) -> Result<Self, NativeBrowserError> {
        let values = [self.x, self.y, self.width, self.height];
        if values.iter().any(|value| !value.is_finite())
            || self.x < 0.0
            || self.y < 0.0
            || self.width < 1.0
            || self.height < 1.0
            || self.width > 100_000.0
            || self.height > 100_000.0
        {
            return Err(NativeBrowserError::InvalidBounds);
        }
        Ok(self)
    }
}

impl Default for NativeSurfaceBounds {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeTabState {
    Ready,
    Loading,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativeBrowserTabSnapshot {
    pub id: String,
    pub generation: u64,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub state: NativeTabState,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NativeBrowserSnapshot {
    pub connection_id: String,
    pub tabs: Vec<NativeBrowserTabSnapshot>,
    pub active_tab_id: Option<String>,
    pub surface_bounds: NativeSurfaceBounds,
    pub surface_visible: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NativeDownloadState {
    InProgress,
    Completed,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativeBrowserDownloadSnapshot {
    pub guid: String,
    pub state: NativeDownloadState,
    pub filename: String,
    pub received_bytes: u64,
    pub total_bytes: u64,
    pub path: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NativeBrowserError {
    #[error("Native Browser is only available in the Windows desktop client")]
    UnsupportedPlatform,
    #[error("Native Browser is not initialized")]
    NotInitialized,
    #[error("Invalid Browser connection identifier")]
    InvalidConnection,
    #[error("Native Browser session was not found")]
    SessionNotFound,
    #[error("Native Browser tab was not found")]
    TabNotFound,
    #[error("Native Browser controller generation is stale")]
    GenerationMismatch,
    #[error("Native Browser surface bounds are invalid")]
    InvalidBounds,
    #[error("Native Browser navigation was blocked")]
    NavigationBlocked,
    #[error("Native Browser controller operation failed")]
    ControllerFailed,
    #[error("Native Browser bridge failed")]
    BridgeFailed,
    #[error("Native Browser bridge request was not authorized")]
    Unauthorized,
    #[error("Native Browser bridge command is not allowed")]
    CommandNotAllowed,
}

impl NativeBrowserError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "UNSUPPORTED_PLATFORM",
            Self::NotInitialized => "NATIVE_RUNTIME_NOT_INITIALIZED",
            Self::InvalidConnection => "INVALID_CONNECTION_ID",
            Self::SessionNotFound => "NATIVE_SESSION_NOT_FOUND",
            Self::TabNotFound => "NATIVE_TAB_NOT_FOUND",
            Self::GenerationMismatch => "NATIVE_GENERATION_MISMATCH",
            Self::InvalidBounds => "INVALID_SURFACE_BOUNDS",
            Self::NavigationBlocked => "NAVIGATION_BLOCKED",
            Self::ControllerFailed => "NATIVE_CONTROLLER_FAILED",
            Self::BridgeFailed => "NATIVE_BRIDGE_FAILED",
            Self::Unauthorized => "NATIVE_BRIDGE_UNAUTHORIZED",
            Self::CommandNotAllowed => "NATIVE_BRIDGE_COMMAND_NOT_ALLOWED",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::GenerationMismatch
                | Self::ControllerFailed
                | Self::BridgeFailed
                | Self::NotInitialized
        )
    }
}

#[derive(Debug, Clone)]
struct NativeTabRecord {
    snapshot: NativeBrowserTabSnapshot,
    webview_label: String,
}

#[derive(Debug, Clone, Default)]
struct NativeSessionRecord {
    tab_order: Vec<String>,
    tabs: HashMap<String, NativeTabRecord>,
    active_tab_id: Option<String>,
    surface_bounds: NativeSurfaceBounds,
    surface_visible: bool,
}

#[derive(Debug, Clone)]
struct PendingNativeTab {
    connection_id: String,
    tab_id: String,
    generation: u64,
    webview_label: String,
    url: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(
    not(all(feature = "tauri-runtime", target_os = "windows")),
    allow(dead_code)
)]
struct ClosedNativeTab {
    webview_label: String,
    was_active: bool,
}

#[derive(Debug, Default)]
struct NativeBrowserRegistry {
    sessions: HashMap<String, NativeSessionRecord>,
    next_generation: u64,
}

#[cfg_attr(
    not(all(feature = "tauri-runtime", target_os = "windows")),
    allow(dead_code)
)]
impl NativeBrowserRegistry {
    fn has_tabs(&self, connection_id: &str) -> bool {
        self.sessions
            .get(connection_id)
            .is_some_and(|session| !session.tab_order.is_empty())
    }

    fn plan_tab(
        &mut self,
        connection_id: &str,
        tab_id: Option<String>,
        url: &str,
    ) -> Result<PendingNativeTab, NativeBrowserError> {
        validate_connection_id(connection_id)?;
        let tab_id = tab_id.unwrap_or_else(|| format!("tab-{}", uuid::Uuid::new_v4().simple()));
        validate_tab_id(&tab_id)?;
        if self
            .sessions
            .values()
            .any(|session| session.tabs.contains_key(&tab_id))
        {
            return Err(NativeBrowserError::ControllerFailed);
        }
        self.next_generation = self.next_generation.saturating_add(1).max(1);
        Ok(PendingNativeTab {
            connection_id: connection_id.to_string(),
            webview_label: format!("native-browser-{}", uuid::Uuid::new_v4().simple()),
            tab_id,
            generation: self.next_generation,
            url: url.to_string(),
        })
    }

    fn commit_tab(&mut self, pending: PendingNativeTab) -> NativeBrowserSnapshot {
        let session = self
            .sessions
            .entry(pending.connection_id.clone())
            .or_default();
        let tab_id = pending.tab_id.clone();
        session.tab_order.push(tab_id.clone());
        session.tabs.insert(
            tab_id.clone(),
            NativeTabRecord {
                snapshot: NativeBrowserTabSnapshot {
                    id: tab_id.clone(),
                    generation: pending.generation,
                    url: pending.url,
                    title: INITIAL_TITLE.to_string(),
                    loading: false,
                    can_go_back: false,
                    can_go_forward: false,
                    state: NativeTabState::Ready,
                    error_code: None,
                },
                webview_label: pending.webview_label,
            },
        );
        session.active_tab_id = Some(tab_id);
        self.snapshot(&pending.connection_id)
            .expect("committed session has a snapshot")
    }

    fn snapshot(&self, connection_id: &str) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        validate_connection_id(connection_id)?;
        let session = self
            .sessions
            .get(connection_id)
            .ok_or(NativeBrowserError::SessionNotFound)?;
        let tabs = session
            .tab_order
            .iter()
            .filter_map(|tab_id| session.tabs.get(tab_id))
            .map(|record| record.snapshot.clone())
            .collect();
        Ok(NativeBrowserSnapshot {
            connection_id: connection_id.to_string(),
            tabs,
            active_tab_id: session.active_tab_id.clone(),
            surface_bounds: session.surface_bounds,
            surface_visible: session.surface_visible,
        })
    }

    fn active_identity(&self, connection_id: &str) -> Result<(String, u64), NativeBrowserError> {
        let session = self
            .sessions
            .get(connection_id)
            .ok_or(NativeBrowserError::SessionNotFound)?;
        let tab_id = session
            .active_tab_id
            .as_ref()
            .ok_or(NativeBrowserError::TabNotFound)?;
        let generation = session
            .tabs
            .get(tab_id)
            .ok_or(NativeBrowserError::TabNotFound)?
            .snapshot
            .generation;
        Ok((tab_id.clone(), generation))
    }

    fn validate_identity(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
    ) -> Result<&NativeTabRecord, NativeBrowserError> {
        validate_connection_id(connection_id)?;
        validate_tab_id(tab_id)?;
        let record = self
            .sessions
            .get(connection_id)
            .ok_or(NativeBrowserError::SessionNotFound)?
            .tabs
            .get(tab_id)
            .ok_or(NativeBrowserError::TabNotFound)?;
        if record.snapshot.generation != generation {
            return Err(NativeBrowserError::GenerationMismatch);
        }
        Ok(record)
    }

    fn focus_tab(
        &mut self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        self.validate_identity(connection_id, tab_id, generation)?;
        let session = self
            .sessions
            .get_mut(connection_id)
            .ok_or(NativeBrowserError::SessionNotFound)?;
        session.active_tab_id = Some(tab_id.to_string());
        self.snapshot(connection_id)
    }

    fn close_tab(
        &mut self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
    ) -> Result<ClosedNativeTab, NativeBrowserError> {
        self.validate_identity(connection_id, tab_id, generation)?;
        let session = self
            .sessions
            .get_mut(connection_id)
            .ok_or(NativeBrowserError::SessionNotFound)?;
        let index = session
            .tab_order
            .iter()
            .position(|candidate| candidate == tab_id)
            .ok_or(NativeBrowserError::TabNotFound)?;
        let was_active = session.active_tab_id.as_deref() == Some(tab_id);
        session.tab_order.remove(index);
        let removed = session
            .tabs
            .remove(tab_id)
            .ok_or(NativeBrowserError::TabNotFound)?;
        if was_active {
            session.active_tab_id = session
                .tab_order
                .get(index.min(session.tab_order.len().saturating_sub(1)))
                .cloned();
        }
        if session.tab_order.is_empty() {
            session.active_tab_id = None;
            session.surface_visible = false;
        }
        Ok(ClosedNativeTab {
            webview_label: removed.webview_label,
            was_active,
        })
    }

    fn release_session(&mut self, connection_id: &str) -> Vec<String> {
        self.sessions
            .remove(connection_id)
            .map(|session| {
                session
                    .tabs
                    .into_values()
                    .map(|record| record.webview_label)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn set_surface(
        &mut self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        bounds: NativeSurfaceBounds,
        visible: bool,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        self.validate_identity(connection_id, tab_id, generation)?;
        let bounds = bounds.validate()?;
        if visible {
            for session in self.sessions.values_mut() {
                session.surface_visible = false;
            }
        }
        let session = self
            .sessions
            .get_mut(connection_id)
            .ok_or(NativeBrowserError::SessionNotFound)?;
        session.active_tab_id = Some(tab_id.to_string());
        session.surface_bounds = bounds;
        session.surface_visible = visible;
        self.snapshot(connection_id)
    }

    fn hide_session(&mut self, connection_id: &str) {
        if let Some(session) = self.sessions.get_mut(connection_id) {
            session.surface_visible = false;
        }
    }

    fn hide_all(&mut self) {
        for session in self.sessions.values_mut() {
            session.surface_visible = false;
        }
    }

    fn update_loading(
        &mut self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        url: Option<String>,
        loading: bool,
    ) -> bool {
        let Ok(record) = self.validate_identity(connection_id, tab_id, generation) else {
            return false;
        };
        let current_generation = record.snapshot.generation;
        let Some(record) = self
            .sessions
            .get_mut(connection_id)
            .and_then(|session| session.tabs.get_mut(tab_id))
        else {
            return false;
        };
        if record.snapshot.generation != current_generation {
            return false;
        }
        if let Some(url) = url {
            record.snapshot.url = url;
        }
        record.snapshot.loading = loading;
        record.snapshot.state = if loading {
            NativeTabState::Loading
        } else {
            NativeTabState::Ready
        };
        record.snapshot.error_code = None;
        true
    }

    fn update_url(
        &mut self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        url: String,
    ) -> bool {
        if self
            .validate_identity(connection_id, tab_id, generation)
            .is_err()
        {
            return false;
        }
        let Some(record) = self
            .sessions
            .get_mut(connection_id)
            .and_then(|session| session.tabs.get_mut(tab_id))
        else {
            return false;
        };
        record.snapshot.url = url;
        true
    }

    fn update_title(
        &mut self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        title: String,
    ) -> bool {
        if self
            .validate_identity(connection_id, tab_id, generation)
            .is_err()
        {
            return false;
        }
        let Some(record) = self
            .sessions
            .get_mut(connection_id)
            .and_then(|session| session.tabs.get_mut(tab_id))
        else {
            return false;
        };
        record.snapshot.title = title;
        true
    }

    fn update_history(
        &mut self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        can_go_back: bool,
        can_go_forward: bool,
    ) -> bool {
        if self
            .validate_identity(connection_id, tab_id, generation)
            .is_err()
        {
            return false;
        }
        let Some(record) = self
            .sessions
            .get_mut(connection_id)
            .and_then(|session| session.tabs.get_mut(tab_id))
        else {
            return false;
        };
        record.snapshot.can_go_back = can_go_back;
        record.snapshot.can_go_forward = can_go_forward;
        true
    }

    fn mark_failed(
        &mut self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        error_code: &str,
    ) -> bool {
        if self
            .validate_identity(connection_id, tab_id, generation)
            .is_err()
        {
            return false;
        }
        let Some(record) = self
            .sessions
            .get_mut(connection_id)
            .and_then(|session| session.tabs.get_mut(tab_id))
        else {
            return false;
        };
        record.snapshot.loading = false;
        record.snapshot.state = NativeTabState::Error;
        record.snapshot.error_code = Some(error_code.to_string());
        true
    }

    fn begin_recover(
        &mut self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
    ) -> Result<PendingNativeTab, NativeBrowserError> {
        self.validate_identity(connection_id, tab_id, generation)?;
        self.next_generation = self.next_generation.saturating_add(1).max(1);
        let next_generation = self.next_generation;
        let session = self
            .sessions
            .get_mut(connection_id)
            .ok_or(NativeBrowserError::SessionNotFound)?;
        let record = session
            .tabs
            .get_mut(tab_id)
            .ok_or(NativeBrowserError::TabNotFound)?;
        record.snapshot.generation = next_generation;
        record.snapshot.loading = true;
        record.snapshot.state = NativeTabState::Loading;
        record.snapshot.error_code = None;
        record.webview_label = format!("native-browser-{}", uuid::Uuid::new_v4().simple());
        Ok(PendingNativeTab {
            connection_id: connection_id.to_string(),
            tab_id: tab_id.to_string(),
            generation: next_generation,
            webview_label: record.webview_label.clone(),
            url: record.snapshot.url.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeHistoryDirection {
    Back,
    Forward,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "command",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum NativeBridgeCommand {
    Ensure {
        connection_id: String,
    },
    Snapshot {
        connection_id: String,
    },
    Downloads {
        connection_id: String,
    },
    Release {
        connection_id: String,
    },
    Create {
        connection_id: String,
        url: Option<String>,
    },
    Focus {
        connection_id: String,
        tab_id: String,
        generation: u64,
    },
    Close {
        connection_id: String,
        tab_id: String,
        generation: u64,
    },
    Navigate {
        connection_id: String,
        tab_id: String,
        generation: u64,
        url: String,
    },
    History {
        connection_id: String,
        tab_id: String,
        generation: u64,
        direction: NativeHistoryDirection,
    },
    Reload {
        connection_id: String,
        tab_id: String,
        generation: u64,
    },
    Stop {
        connection_id: String,
        tab_id: String,
        generation: u64,
    },
    Surface {
        connection_id: String,
        tab_id: String,
        generation: u64,
        bounds: NativeSurfaceBounds,
        visible: bool,
    },
    Recover {
        connection_id: String,
        tab_id: String,
        generation: u64,
    },
    Cdp {
        connection_id: String,
        tab_id: String,
        generation: u64,
        method: String,
        #[serde(default)]
        params: serde_json::Value,
    },
    CdpSubscribe {
        connection_id: String,
        tab_id: String,
        generation: u64,
        event: String,
    },
    CdpDrain {
        connection_id: String,
        tab_id: String,
        generation: u64,
        event: String,
        #[serde(default)]
        limit: Option<usize>,
    },
}

pub fn validate_connection_id(connection_id: &str) -> Result<(), NativeBrowserError> {
    if connection_id.is_empty()
        || connection_id.len() > MAX_CONNECTION_ID_BYTES
        || !connection_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    {
        return Err(NativeBrowserError::InvalidConnection);
    }
    Ok(())
}

fn validate_tab_id(tab_id: &str) -> Result<(), NativeBrowserError> {
    if tab_id.is_empty()
        || tab_id.len() > MAX_TAB_ID_BYTES
        || !tab_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    {
        return Err(NativeBrowserError::TabNotFound);
    }
    Ok(())
}

pub fn fixed_time_token_eq(provided: &[u8], expected: &[u8]) -> bool {
    if provided.len() != expected.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in provided.iter().zip(expected) {
        difference |= left ^ right;
    }
    difference == 0
}

pub fn allowed_cdp_methods() -> &'static HashSet<&'static str> {
    static METHODS: std::sync::OnceLock<HashSet<&'static str>> = std::sync::OnceLock::new();
    METHODS.get_or_init(|| {
        [
            "Accessibility.enable",
            "Accessibility.getFullAXTree",
            "Browser.getVersion",
            "DOM.enable",
            "DOM.getDocument",
            "DOM.getOuterHTML",
            "DOM.querySelector",
            "DOM.resolveNode",
            "Input.dispatchKeyEvent",
            "Input.dispatchMouseEvent",
            "Input.insertText",
            "Page.captureScreenshot",
            "Page.enable",
            "Page.getFrameTree",
            "Page.getLayoutMetrics",
            "Page.getNavigationHistory",
            "Page.navigate",
            "Page.navigateToHistoryEntry",
            "Page.reload",
            "Page.stopLoading",
            "Runtime.callFunctionOn",
            "Runtime.enable",
            "Runtime.evaluate",
            "Runtime.releaseObject",
        ]
        .into_iter()
        .collect()
    })
}

pub fn allowed_cdp_events() -> &'static HashSet<&'static str> {
    static EVENTS: std::sync::OnceLock<HashSet<&'static str>> = std::sync::OnceLock::new();
    EVENTS.get_or_init(|| {
        [
            "Browser.downloadProgress",
            "Browser.downloadWillBegin",
            "Fetch.requestPaused",
            "Page.frameNavigated",
            "Page.frameStartedLoading",
            "Page.frameStoppedLoading",
        ]
        .into_iter()
        .collect()
    })
}

#[cfg_attr(
    not(all(feature = "tauri-runtime", target_os = "windows")),
    allow(dead_code)
)]
mod security;

#[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
mod windows_runtime;

#[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
pub use windows_runtime::{NativeBridgeCredentials, NativeBrowserRuntime};

#[cfg(test)]
mod tests {
    use super::*;

    fn commit_tab(
        registry: &mut NativeBrowserRegistry,
        connection_id: &str,
        url: &str,
    ) -> NativeBrowserTabSnapshot {
        let pending = registry.plan_tab(connection_id, None, url).unwrap();
        registry.commit_tab(pending).tabs.last().cloned().unwrap()
    }

    #[test]
    fn registry_owns_tabs_by_connection_and_returns_a_full_snapshot() {
        let mut registry = NativeBrowserRegistry::default();
        let first = commit_tab(&mut registry, "connection-a", INITIAL_URL);
        let second = commit_tab(&mut registry, "connection-a", "https://example.com/");
        let other = commit_tab(&mut registry, "connection-b", INITIAL_URL);

        let snapshot = registry.snapshot("connection-a").unwrap();
        assert_eq!(snapshot.tabs, vec![first, second.clone()]);
        assert_eq!(snapshot.active_tab_id.as_deref(), Some(second.id.as_str()));
        assert!(!snapshot.tabs.iter().any(|tab| tab.id == other.id));
    }

    #[test]
    fn focus_and_close_require_the_current_generation() {
        let mut registry = NativeBrowserRegistry::default();
        let first = commit_tab(&mut registry, "connection-a", INITIAL_URL);
        let second = commit_tab(&mut registry, "connection-a", INITIAL_URL);

        assert_eq!(
            registry.focus_tab("connection-a", &first.id, second.generation),
            Err(NativeBrowserError::GenerationMismatch)
        );
        registry
            .focus_tab("connection-a", &first.id, first.generation)
            .unwrap();
        let closed = registry
            .close_tab("connection-a", &first.id, first.generation)
            .unwrap();
        assert!(closed.was_active);
        assert_eq!(
            registry.snapshot("connection-a").unwrap().active_tab_id,
            Some(second.id)
        );
    }

    #[test]
    fn visible_surface_is_unique_across_sessions() {
        let mut registry = NativeBrowserRegistry::default();
        let first = commit_tab(&mut registry, "connection-a", INITIAL_URL);
        let second = commit_tab(&mut registry, "connection-b", INITIAL_URL);
        let bounds = NativeSurfaceBounds {
            x: 10.0,
            y: 20.0,
            width: 640.0,
            height: 480.0,
        };
        registry
            .set_surface("connection-a", &first.id, first.generation, bounds, true)
            .unwrap();
        registry
            .set_surface("connection-b", &second.id, second.generation, bounds, true)
            .unwrap();

        assert!(!registry.snapshot("connection-a").unwrap().surface_visible);
        assert!(registry.snapshot("connection-b").unwrap().surface_visible);
    }

    #[test]
    fn crash_recovery_invalidates_the_old_controller_generation() {
        let mut registry = NativeBrowserRegistry::default();
        let tab = commit_tab(&mut registry, "connection-a", INITIAL_URL);
        assert!(registry.mark_failed(
            "connection-a",
            &tab.id,
            tab.generation,
            "WEBVIEW2_PROCESS_FAILED"
        ));
        let pending = registry
            .begin_recover("connection-a", &tab.id, tab.generation)
            .unwrap();
        assert!(pending.generation > tab.generation);
        assert!(!registry.update_title("connection-a", &tab.id, tab.generation, "stale".into()));
        assert!(matches!(
            registry.validate_identity("connection-a", &tab.id, tab.generation),
            Err(NativeBrowserError::GenerationMismatch)
        ));
    }

    #[test]
    fn bridge_token_and_cdp_allowlist_fail_closed() {
        assert!(fixed_time_token_eq(b"secret", b"secret"));
        assert!(!fixed_time_token_eq(b"secret", b"secreu"));
        assert!(!fixed_time_token_eq(b"short", b"longer"));
        assert!(allowed_cdp_methods().contains("Runtime.evaluate"));
        assert!(!allowed_cdp_methods().contains("SystemInfo.getProcessInfo"));
        assert!(!allowed_cdp_methods().contains("Browser.setDownloadBehavior"));
        assert!(allowed_cdp_events().contains("Fetch.requestPaused"));
        assert!(!allowed_cdp_events().contains("Runtime.consoleAPICalled"));
    }

    #[test]
    fn bridge_commands_reject_unknown_fields_and_preserve_identity() {
        let valid = serde_json::json!({
            "command": "cdp_subscribe",
            "connectionId": "connection-a",
            "tabId": "tab-a",
            "generation": 7,
            "event": "Page.frameNavigated"
        });
        assert!(matches!(
            serde_json::from_value::<NativeBridgeCommand>(valid).unwrap(),
            NativeBridgeCommand::CdpSubscribe {
                connection_id,
                tab_id,
                generation: 7,
                event,
            } if connection_id == "connection-a"
                && tab_id == "tab-a"
                && event == "Page.frameNavigated"
        ));

        let forged = serde_json::json!({
            "command": "snapshot",
            "connectionId": "connection-a",
            "token": "must-not-be-accepted-in-the-command-body"
        });
        assert!(serde_json::from_value::<NativeBridgeCommand>(forged).is_err());
    }

    #[test]
    fn download_bridge_contract_is_session_scoped_and_camel_case() {
        let command = serde_json::from_value::<NativeBridgeCommand>(serde_json::json!({
            "command": "downloads",
            "connectionId": "connection-a"
        }))
        .unwrap();
        assert!(matches!(
            command,
            NativeBridgeCommand::Downloads { connection_id }
                if connection_id == "connection-a"
        ));

        let value = serde_json::to_value(NativeBrowserDownloadSnapshot {
            guid: "download-a".to_string(),
            state: NativeDownloadState::InProgress,
            filename: "result.txt".to_string(),
            received_bytes: 0,
            total_bytes: 0,
            path: None,
            error_code: None,
        })
        .unwrap();
        assert_eq!(value["state"], "inProgress");
        assert_eq!(value["receivedBytes"], 0);
        assert!(value["path"].is_null());
    }

    #[test]
    fn source_updates_do_not_finish_an_inflight_navigation() {
        let mut registry = NativeBrowserRegistry::default();
        let tab = commit_tab(&mut registry, "connection-a", INITIAL_URL);
        assert!(registry.update_loading(
            "connection-a",
            &tab.id,
            tab.generation,
            Some("https://example.com/".into()),
            true,
        ));
        assert!(registry.update_url(
            "connection-a",
            &tab.id,
            tab.generation,
            "https://example.com/redirected".into(),
        ));

        let snapshot = registry.snapshot("connection-a").unwrap();
        assert!(snapshot.tabs[0].loading);
        assert_eq!(snapshot.tabs[0].state, NativeTabState::Loading);
        assert_eq!(snapshot.tabs[0].url, "https://example.com/redirected");
    }

    #[test]
    fn snapshots_do_not_serialize_webview_labels_or_bridge_secrets() {
        let mut registry = NativeBrowserRegistry::default();
        commit_tab(&mut registry, "connection-a", INITIAL_URL);
        let json = serde_json::to_string(&registry.snapshot("connection-a").unwrap()).unwrap();
        assert!(!json.contains("native-browser-"));
        assert!(!json.contains("token"));
        assert!(!json.contains("endpoint"));
    }

    #[test]
    fn surface_bounds_reject_nan_and_zero_sized_slots() {
        assert_eq!(
            NativeSurfaceBounds {
                width: 0.0,
                ..Default::default()
            }
            .validate(),
            Err(NativeBrowserError::InvalidBounds)
        );
        assert_eq!(
            NativeSurfaceBounds {
                x: f64::NAN,
                ..Default::default()
            }
            .validate(),
            Err(NativeBrowserError::InvalidBounds)
        );
    }
}
