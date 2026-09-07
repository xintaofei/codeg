//! Callbacks the surfaces install on their webviews. They only touch the
//! registry and emit state; nothing here calls back into the surface, so the
//! webview thread never waits on itself.

use std::time::Duration;

use tauri::{AppHandle, Manager, Url};

use super::events;
use super::registry::BrowserRegistry;
use super::types::{BrowserErrorInfo, BrowserErrorKind};

pub fn origin_of(url: &Url) -> Option<String> {
    let origin = url.origin();
    if origin.is_tuple() {
        Some(origin.ascii_serialization())
    } else {
        None
    }
}

pub fn page_load(app: &AppHandle, tab_id: &str, url: &Url, started: bool) {
    let Some(registry) = app.try_state::<BrowserRegistry>() else {
        return;
    };
    // History flags are only trustworthy once the navigation committed; the
    // surface call runs inline here (main thread) and never takes the
    // registry lock itself.
    let history = if started {
        None
    } else {
        registry
            .surface(tab_id)
            .map(|surface| {
                (
                    surface.can_go_back().unwrap_or(false),
                    surface.can_go_forward().unwrap_or(false),
                )
            })
    };
    let state = registry.update_state(tab_id, |state| {
        state.url = url.to_string();
        state.loading = started;
        state.origin = origin_of(url);
        if started {
            state.error = None;
            // The previous document's title must not label the new one; the
            // toolbar falls back to the host until `title_changed` fires.
            state.title.clear();
        }
        if let Some((back, forward)) = history {
            state.can_go_back = back;
            state.can_go_forward = forward;
        }
    });
    if let Some(state) = state {
        events::emit_state(app, &state);
    }
    if started {
        begin_load(app, tab_id);
    }
}

/// Arm the failed-load watcher for a navigation that is starting now. Called
/// where a load is kicked off (open, address bar, adopted popup) as well as
/// from `page_load`: wry's "started" is WebKit's `didCommitNavigation`, so a
/// navigation that never commits (DNS failure) never reports starting at all.
pub fn begin_load(app: &AppHandle, tab_id: &str) {
    let Some(registry) = app.try_state::<BrowserRegistry>() else {
        return;
    };
    if let Some(seq) = registry.update(tab_id, |tab| {
        tab.load_seq += 1;
        tab.load_seq
    }) {
        watch_load(app.clone(), tab_id.to_string(), seq);
    }
}

const LOAD_POLL: Duration = Duration::from_millis(500);
const LOAD_FINISH_GRACE: Duration = Duration::from_millis(300);

/// wry reports navigation start and finish but never failure, so a DNS,
/// connection or TLS error would leave `loading: true` forever. Poll the
/// engine's own flag until it clears; if our state is still loading a moment
/// later, the load ended without a document — surface it as an error, or,
/// when an older document is still showing, just stop the spinner. A newer
/// navigation (higher `load_seq`) retires the watcher.
fn watch_load(app: AppHandle, tab_id: String, seq: u64) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(LOAD_POLL).await;
            let Some(registry) = app.try_state::<BrowserRegistry>() else {
                return;
            };
            let Some((surface, loading, current)) = registry.update(&tab_id, |tab| {
                (tab.surface.clone(), tab.state.loading, tab.load_seq)
            }) else {
                return;
            };
            if current != seq || !loading {
                return;
            }
            match surface.is_loading() {
                Ok(true) => continue,
                Ok(false) => {}
                // No load state on this surface (owned windows for now).
                Err(_) => return,
            }
            // `didFinish` may still be on its way.
            tokio::time::sleep(LOAD_FINISH_GRACE).await;
            let Some((loading, current)) =
                registry.update(&tab_id, |tab| (tab.state.loading, tab.load_seq))
            else {
                return;
            };
            if current != seq || !loading {
                return;
            }
            let has_document = surface.url().is_ok();
            let next = registry.update_state(&tab_id, |state| {
                state.loading = false;
                if !has_document {
                    // Nothing committed, so `url` is still empty: the error
                    // page needs the address the user asked for. The wording
                    // is the status layer's, in the user's language.
                    let url = if state.url.is_empty() {
                        state.requested_url.clone()
                    } else {
                        state.url.clone()
                    };
                    state.error = Some(BrowserErrorInfo {
                        kind: BrowserErrorKind::Failed,
                        message: String::new(),
                        url: Some(url),
                    });
                }
            });
            if let Some(next) = next {
                events::emit_state(&app, &next);
            }
            return;
        }
    });
}

pub fn title_changed(app: &AppHandle, tab_id: &str, title: String) {
    let Some(registry) = app.try_state::<BrowserRegistry>() else {
        return;
    };
    let state = registry.update_state(tab_id, |state| state.title = title);
    if let Some(state) = state {
        events::emit_state(app, &state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_is_none_for_opaque_urls() {
        assert_eq!(
            origin_of(&Url::parse("https://example.com:8443/a").unwrap()).as_deref(),
            Some("https://example.com:8443")
        );
        assert_eq!(origin_of(&Url::parse("about:blank").unwrap()), None);
        assert_eq!(origin_of(&Url::parse("blob:null/x").unwrap()), None);
    }
}
