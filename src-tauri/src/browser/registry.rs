//! `tab id → surface + last known state`, shared by the commands, the
//! webview hooks and the window-close cleanup. The mutex is only ever held
//! for map operations; every surface call happens on a clone taken out of it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
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
    /// Bumped by every `set_visible` request. A hide that first captures a
    /// freeze frame is asynchronous; when it comes back it applies only if no
    /// newer request has been made meanwhile — otherwise a quick close of the
    /// overlay would be followed by a stale hide.
    pub visible_seq: u64,
    /// Which incarnation of this tab id this is (a tab id is reused when a
    /// released tab is brought back). An operation that spans an await
    /// captures it and stands down if the id now names a later incarnation.
    pub generation: u64,
    /// How many navigations the host has learned of in this incarnation —
    /// documents from the engine, same-document route changes from the
    /// helper's poll. Together with `generation` this is the epoch a page
    /// snapshot is stamped with, so that a ref read from one page is not
    /// answered against another; see `agent::epoch` for what the host can and
    /// cannot see here.
    pub nav_epoch: u64,
    /// URL of the main-frame navigation the engine reported as started and
    /// has neither committed nor failed yet (platforms with a navigation
    /// delegate only). Lets a commit of `about:blank` in its place be
    /// recognised for what it is: a load the engine refused silently.
    pub provisional_url: Option<String>,
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
            visible_seq: 0,
            generation: 0,
            nav_epoch: 0,
            provisional_url: None,
            gestures: VecDeque::with_capacity(GESTURE_RING_CAPACITY),
        }
    }
}

/// The right to open a tab under an id, held from before its surface is
/// built until the tab is registered (`insert_reserved`) or the attempt is
/// abandoned (drop). Two opens of one id would otherwise both build a
/// surface and the loser, closing "its" surface, would close the winner's.
pub struct OpenReservation<'a> {
    registry: &'a BrowserRegistry,
    tab_id: String,
    consumed: bool,
}

impl Drop for OpenReservation<'_> {
    fn drop(&mut self) {
        if !self.consumed {
            self.registry.release_opening(&self.tab_id);
        }
    }
}

#[derive(Default)]
pub struct BrowserRegistry {
    tabs: Mutex<HashMap<String, BrowserTab>>,
    /// Ids whose open is under way (reserved, not yet inserted).
    opening: Mutex<HashSet<String>>,
    /// One async lock per tab for `set_visible`: a hide that first captures
    /// a freeze frame spans an await, and the request behind it must not
    /// apply in between (tokio's mutex hands the lock out in arrival order).
    visibility: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Source of `BrowserTab::generation`, never reused within a process.
    generations: AtomicU64,
}

impl BrowserRegistry {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, BrowserTab>> {
        self.tabs.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The visibility lock of a tab (created on first use, dropped with the
    /// tab). The std mutex guarding the map is released before the caller
    /// awaits on the returned lock.
    pub fn visibility_lock(&self, tab_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .visibility
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks
            .entry(tab_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    fn release_opening(&self, tab_id: &str) {
        self.opening
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(tab_id);
    }

    /// Claim `tab_id` for an open that is about to build a surface. Fails
    /// when a tab with that id exists or another open of it is under way.
    /// Lock order: tabs, then opening.
    pub fn reserve(&self, tab_id: &str) -> Result<OpenReservation<'_>, AppCommandError> {
        let tabs = self.lock();
        let mut opening = self
            .opening
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if tabs.contains_key(tab_id) || !opening.insert(tab_id.to_string()) {
            return Err(AppCommandError::already_exists(format!(
                "browser tab {tab_id} is already open"
            )));
        }
        Ok(OpenReservation {
            registry: self,
            tab_id: tab_id.to_string(),
            consumed: false,
        })
    }

    /// Register a tab under the id its reservation holds.
    pub fn insert_reserved(
        &self,
        tab: BrowserTab,
        mut reservation: OpenReservation<'_>,
    ) -> Result<(), AppCommandError> {
        debug_assert_eq!(reservation.tab_id, tab.state.tab_id);
        reservation.consumed = true;
        let id = reservation.tab_id.clone();
        let result = self.insert(tab);
        self.release_opening(&id);
        result
    }

    /// Register a tab whose id was not reserved (a popup adopted on the main
    /// thread, whose id was minted there and checked against this map).
    pub fn insert(&self, mut tab: BrowserTab) -> Result<(), AppCommandError> {
        tab.generation = self.generations.fetch_add(1, Ordering::Relaxed) + 1;
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

    /// Read several things about one tab under a single lock. For callers
    /// that need a surface AND what was true of the tab when they took it —
    /// two separate lookups can straddle a close and reopen, and answer about
    /// two different tabs that happen to share an id. Keep the closure free
    /// of surface calls, like `update`.
    pub fn read<R>(&self, tab_id: &str, f: impl FnOnce(&BrowserTab) -> R) -> Option<R> {
        self.lock().get(tab_id).map(f)
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
        self.remove_if(tab_id, |_| true)
    }

    /// Detach the tab under `tab_id` only if `matches` says so about the tab
    /// that is there NOW — a tab id is reused across incarnations, and a
    /// caller that decided on an earlier snapshot must not remove a tab it
    /// never looked at.
    pub fn remove_if(&self, tab_id: &str, matches: impl FnOnce(&BrowserTab) -> bool) -> Option<BrowserTab> {
        // Lock order everywhere: tabs, then visibility (never the reverse),
        // and the lock entry goes while the tabs lock is still held — a tab
        // inserted under the same id in between would otherwise lose its
        // own, freshly created lock.
        let mut tabs = self.lock();
        if !tabs.get(tab_id).is_some_and(matches) {
            return None;
        }
        let removed = tabs.remove(tab_id);
        if removed.is_some() {
            self.visibility
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(tab_id);
        }
        removed
    }

    /// Ids of the tabs living in `profile`, at this moment.
    pub fn tabs_in_profile(&self, profile: &str) -> Vec<String> {
        self.lock()
            .values()
            .filter(|t| t.state.profile.as_deref() == Some(profile))
            .map(|t| t.state.tab_id.clone())
            .collect()
    }

    /// Any one surface of a tab living in `profile`, taken under the one
    /// lock (a separate lookup by id could name a tab that has meanwhile
    /// been closed and reopened in another profile).
    pub fn surface_in_profile(&self, profile: &str) -> Option<BrowserSurface> {
        self.lock()
            .values()
            .find(|t| t.state.profile.as_deref() == Some(profile))
            .map(|t| t.surface.clone())
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
        let removed: Vec<BrowserTab> = ids.into_iter().filter_map(|id| tabs.remove(&id)).collect();
        // Same order and same reason as `remove`: still under the tabs lock.
        let mut locks = self
            .visibility
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for tab in &removed {
            locks.remove(&tab.state.tab_id);
        }
        removed
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

    /// One open at a time per id: the second reservation fails while the
    /// first is held, and a dropped reservation frees the id again.
    #[test]
    fn open_reservations_are_exclusive_and_released_on_drop() {
        let registry = BrowserRegistry::default();
        let first = registry.reserve("t1").expect("first reservation");
        assert!(registry.reserve("t1").is_err(), "second open of the same id must fail");
        assert!(registry.reserve("t2").is_ok(), "another id is unaffected");
        drop(first);
        assert!(registry.reserve("t1").is_ok(), "released on drop");
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
