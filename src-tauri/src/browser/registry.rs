//! `tab id → surface + last known state`, shared by the commands, the
//! webview hooks and the window-close cleanup. The mutex is only ever held
//! for map operations; every surface call happens on a clone taken out of it.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::app_error::AppCommandError;

use super::surface::BrowserSurface;
use super::types::{Bounds, BrowserTabState};

/// Recent user gestures reported by the isolated-world helper (untrusted).
/// Consumed by the popup router to tell a gesture-backed `window.open` from
/// an unsolicited one and to match modifier-clicks against navigations.
#[derive(Debug, Clone)]
pub struct GestureRecord {
    pub received: Instant,
    pub payload: Value,
}

/// How many gestures to remember per tab; a click storm never needs more
/// than the last few, and the popup rule only looks one second back.
pub const GESTURE_RING_CAPACITY: usize = 16;

pub struct BrowserTab {
    pub state: BrowserTabState,
    pub surface: BrowserSurface,
    /// Last bounds the frontend asked for; re-applied when the surface is
    /// shown again after being hidden.
    pub last_bounds: Bounds,
    pub visible: bool,
    /// Whether the surface was built with the inspector enabled (a user
    /// preference read at open time). Popups inherit their opener's value.
    pub devtools: bool,
    /// Bumped on every navigation start; a load watcher captures it and
    /// stands down when a newer navigation supersedes its own.
    pub load_seq: u64,
    /// Set to `load_seq` when a navigation turned out to be a download. That
    /// navigation never commits, so its watcher must settle quietly instead
    /// of reporting a page that never arrived — and keying on the GENERATION
    /// rather than on the URL keeps a redirected download working while a
    /// later, genuinely failing navigation still reports itself.
    pub download_seq: Option<u64>,
    pub gestures: VecDeque<GestureRecord>,
}

impl BrowserTab {
    pub fn new(
        state: BrowserTabState,
        surface: BrowserSurface,
        bounds: Bounds,
        visible: bool,
        devtools: bool,
    ) -> Self {
        Self {
            state,
            surface,
            last_bounds: bounds,
            visible,
            devtools,
            load_seq: 0,
            download_seq: None,
            gestures: VecDeque::with_capacity(GESTURE_RING_CAPACITY),
        }
    }
}

#[derive(Default)]
pub struct BrowserRegistry {
    tabs: Mutex<HashMap<String, BrowserTab>>,
}

impl BrowserRegistry {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, BrowserTab>> {
        self.tabs.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn insert(&self, tab: BrowserTab) -> Result<(), AppCommandError> {
        let mut tabs = self.lock();
        let id = tab.state.tab_id.clone();
        if tabs.contains_key(&id) {
            return Err(AppCommandError::already_exists(format!(
                "browser tab {id} is already open"
            )));
        }
        tabs.insert(id, tab);
        Ok(())
    }

    pub fn contains(&self, tab_id: &str) -> bool {
        self.lock().contains_key(tab_id)
    }

    /// A clone of the surface handle, to be used with the lock released.
    pub fn surface(&self, tab_id: &str) -> Option<BrowserSurface> {
        self.lock().get(tab_id).map(|t| t.surface.clone())
    }

    pub fn state(&self, tab_id: &str) -> Option<BrowserTabState> {
        self.lock().get(tab_id).map(|t| t.state.clone())
    }

    pub fn list(&self) -> Vec<BrowserTabState> {
        let mut states: Vec<_> = self.lock().values().map(|t| t.state.clone()).collect();
        states.sort_by(|a, b| a.tab_id.cmp(&b.tab_id));
        states
    }

    pub fn list_for_owner(&self, owner_window: &str) -> Vec<BrowserTabState> {
        self.list()
            .into_iter()
            .filter(|s| s.owner_window == owner_window)
            .collect()
    }

    /// Mutate a tab under the lock and return whatever the closure produced,
    /// or `None` when the tab is gone. Keep the closure free of surface calls.
    pub fn update<R>(&self, tab_id: &str, f: impl FnOnce(&mut BrowserTab) -> R) -> Option<R> {
        self.lock().get_mut(tab_id).map(f)
    }

    /// Update and hand back the resulting state (the usual "mutate then emit"
    /// shape).
    pub fn update_state(
        &self,
        tab_id: &str,
        f: impl FnOnce(&mut BrowserTabState),
    ) -> Option<BrowserTabState> {
        self.update(tab_id, |tab| {
            f(&mut tab.state);
            tab.state.clone()
        })
    }

    pub fn tab_id_for_label(&self, label: &str) -> Option<String> {
        self.lock()
            .values()
            .find(|t| t.surface.label() == label)
            .map(|t| t.state.tab_id.clone())
    }

    pub fn remove(&self, tab_id: &str) -> Option<BrowserTab> {
        self.lock().remove(tab_id)
    }

    /// Detach every tab owned by a window (called when that window is
    /// destroyed); the caller closes the returned surfaces.
    pub fn remove_by_owner(&self, owner_window: &str) -> Vec<BrowserTab> {
        let mut tabs = self.lock();
        let ids: Vec<String> = tabs
            .values()
            .filter(|t| t.state.owner_window == owner_window)
            .map(|t| t.state.tab_id.clone())
            .collect();
        ids.into_iter().filter_map(|id| tabs.remove(&id)).collect()
    }

    pub fn push_gesture(&self, tab_id: &str, payload: Value) {
        self.update(tab_id, |tab| {
            if tab.gestures.len() == GESTURE_RING_CAPACITY {
                tab.gestures.pop_front();
            }
            tab.gestures.push_back(GestureRecord {
                received: Instant::now(),
                payload,
            });
        });
    }

    /// Consume a recent (`within`) modifier-click on a plain anchor whose
    /// resolved href is `url`: the page let the engine navigate in place, and
    /// the host turns that into a background tab instead. Only a `click`
    /// with button 0, the platform's primary modifier (⌘ on macOS, Ctrl
    /// elsewhere), no `target`, and no `download` qualifies; the record is
    /// removed so a single gesture cannot spawn two tabs.
    pub fn take_modifier_click(&self, tab_id: &str, url: &str, within: Duration) -> bool {
        let wanted = normalize_for_match(url);
        let mut tabs = self.lock();
        let Some(tab) = tabs.get_mut(tab_id) else {
            return false;
        };
        let idx = tab.gestures.iter().rposition(|g| {
            g.received.elapsed() <= within && gesture_is_modifier_click(&g.payload, &wanted)
        });
        match idx {
            Some(i) => {
                tab.gestures.remove(i);
                true
            }
            None => false,
        }
    }

    /// Newest first.
    pub fn recent_gestures(&self, tab_id: &str) -> Vec<GestureRecord> {
        self.lock()
            .get(tab_id)
            .map(|tab| tab.gestures.iter().rev().cloned().collect())
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }
}

fn normalize_for_match(url: &str) -> String {
    // Compare without the fragment: an anchor's `href` and the navigation it
    // produces agree up to the fragment, which the engine may drop.
    match tauri::Url::parse(url) {
        Ok(mut u) => {
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => url.to_string(),
    }
}

fn gesture_is_modifier_click(payload: &Value, wanted: &str) -> bool {
    if payload.get("type").and_then(Value::as_str) != Some("click") {
        return false;
    }
    if payload.get("button").and_then(Value::as_i64) != Some(0) {
        return false;
    }
    let modifiers = payload.get("modifiers");
    let flag = |k: &str| {
        modifiers
            .and_then(|m| m.get(k))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let primary = if cfg!(target_os = "macos") { flag("meta") } else { flag("ctrl") };
    if !primary {
        return false;
    }
    let Some(anchor) = payload.get("anchor").filter(|a| !a.is_null()) else {
        return false;
    };
    if anchor.get("download").and_then(Value::as_bool).unwrap_or(false) {
        return false;
    }
    let target = anchor.get("target").and_then(Value::as_str).unwrap_or("");
    if !(target.is_empty() || target.eq_ignore_ascii_case("_self")) {
        return false;
    }
    anchor
        .get("href")
        .and_then(Value::as_str)
        .map(|h| normalize_for_match(h) == wanted)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn click(href: &str, meta: bool, ctrl: bool, extra: Value) -> Value {
        let mut v = json!({
            "type": "click", "button": 0,
            "modifiers": { "meta": meta, "ctrl": ctrl, "shift": false, "alt": false },
            "anchor": { "href": href, "target": "", "download": false }
        });
        if let (Some(obj), Some(more)) = (v.as_object_mut(), extra.as_object()) {
            for (k, val) in more {
                if k == "anchor" {
                    if let Some(a) = obj.get_mut("anchor").and_then(Value::as_object_mut) {
                        for (ak, av) in val.as_object().unwrap() {
                            a.insert(ak.clone(), av.clone());
                        }
                    }
                } else {
                    obj.insert(k.clone(), val.clone());
                }
            }
        }
        v
    }

    #[test]
    fn modifier_click_matching_rules() {
        let primary = cfg!(target_os = "macos");
        let (meta, ctrl) = (primary, !primary);
        let wanted = normalize_for_match("https://example.com/a?b=1#frag");
        assert!(gesture_is_modifier_click(&click("https://example.com/a?b=1", meta, ctrl, json!({})), &wanted));
        // Wrong modifier, plain click, middle click, _blank, download, other href: no match.
        assert!(!gesture_is_modifier_click(&click("https://example.com/a?b=1", !meta, !ctrl, json!({})), &wanted));
        assert!(!gesture_is_modifier_click(&click("https://example.com/a?b=1", false, false, json!({})), &wanted));
        assert!(!gesture_is_modifier_click(&click("https://example.com/a?b=1", meta, ctrl, json!({"button": 1})), &wanted));
        assert!(!gesture_is_modifier_click(&click("https://example.com/a?b=1", meta, ctrl, json!({"anchor": {"target": "_blank"}})), &wanted));
        assert!(!gesture_is_modifier_click(&click("https://example.com/a?b=1", meta, ctrl, json!({"anchor": {"download": true}})), &wanted));
        assert!(!gesture_is_modifier_click(&click("https://example.com/other", meta, ctrl, json!({})), &wanted));
        assert!(!gesture_is_modifier_click(&json!({"type": "keydown"}), &wanted));
    }
}
