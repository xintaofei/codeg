//! `tab id → surface + last known state`, shared by the commands, the
//! webview hooks and the window-close cleanup. The mutex is only ever held
//! for map operations; every surface call happens on a clone taken out of it.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use crate::app_error::AppCommandError;

use super::surface::BrowserSurface;
use super::types::{Bounds, BrowserTabState};

pub struct BrowserTab {
    pub state: BrowserTabState,
    pub surface: BrowserSurface,
    /// Last bounds the frontend asked for; re-applied when the surface is
    /// shown again after being hidden.
    pub last_bounds: Bounds,
    pub visible: bool,
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

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }
}
