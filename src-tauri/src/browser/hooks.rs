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

/// The tab's state once a navigation became a download: not loading, no
/// error, and back on the document it is showing (what it "asked for" is no
/// longer the file being fetched).
fn settle_after_download(state: &mut crate::browser::types::BrowserTabState) {
    state.loading = false;
    state.error = None;
    state.requested_url = state.url.clone();
}

const LOAD_POLL: Duration = Duration::from_millis(500);
const LOAD_FINISH_GRACE: Duration = Duration::from_millis(300);

/// wry reports navigation start and finish but never failure, so a DNS,
/// connection or TLS error would leave `loading: true` forever. Poll the
/// engine's own flag until it clears; if our state is still loading a moment
/// later, the load ended without the requested page — surface it as an error
/// (a failed reload of the page already showing just stops the spinner). A
/// newer navigation (higher `load_seq`) retires the watcher.
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
            let next = registry.update(&tab_id, |tab| {
                // This very navigation turned into a download: it was never
                // going to commit, so there is nothing to report.
                if tab.download_seq == Some(seq) {
                    tab.download_seq = None;
                    settle_after_download(&mut tab.state);
                    return tab.state.clone();
                }
                let state = &mut tab.state;
                state.loading = false;
                // The load ended without the requested page: either nothing
                // ever committed, or an older document is still showing while
                // the address the user asked for never arrived (a reload that
                // failed keeps its page and just stops the spinner). The error
                // page names the requested address; its wording is the status
                // layer's, in the user's language.
                let requested_arrived = has_document
                    && (state.requested_url.is_empty() || state.requested_url == state.url);
                if !requested_arrived {
                    let url = if state.requested_url.is_empty() {
                        state.url.clone()
                    } else {
                        state.requested_url.clone()
                    };
                    state.error = Some(BrowserErrorInfo {
                        kind: BrowserErrorKind::Failed,
                        message: String::new(),
                        url: Some(url),
                    });
                }
                state.clone()
            });
            if let Some(next) = next {
                events::emit_state(&app, &next);
            }
            return;
        }
    });
}

/// A navigation turned into a download. Nothing will ever commit for it, so
/// the watcher armed for it must not report "the requested address never
/// arrived" and paint an error page over the document the tab is still
/// perfectly happily showing.
///
/// This only MARKS the generation; the watcher settles when it concludes.
/// Retiring the watcher here instead would be wrong whenever the download's
/// callback arrives late: by then the tab may be loading something else, and
/// clearing that navigation's state would both stop its spinner early and
/// swallow its real failure.
pub fn navigation_became_download(app: &AppHandle, tab_id: &str) {
    let Some(registry) = app.try_state::<BrowserRegistry>() else {
        return;
    };
    registry.update(tab_id, |tab| tab.download_seq = Some(tab.load_seq));
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
    use crate::browser::types::{BrowserTabState, ChannelKind, SurfaceKind};

    fn state(url: &str, requested: &str) -> BrowserTabState {
        BrowserTabState {
            tab_id: "t1".into(),
            owner_window: "main".into(),
            surface: SurfaceKind::Child,
            channel: ChannelKind::Native,
            url: url.into(),
            requested_url: requested.into(),
            title: "Listing".into(),
            favicon: None,
            loading: true,
            can_go_back: false,
            can_go_forward: false,
            origin: None,
            zoom: 1.0,
            error: Some(BrowserErrorInfo {
                kind: BrowserErrorKind::Failed,
                message: String::new(),
                url: Some(requested.into()),
            }),
            remote_host: None,
            opener_tab_id: None,
        }
    }

    /// Clicking a link that downloads leaves the navigation for ever
    /// uncommitted. Without this the load watcher's "the requested address
    /// never arrived" rule paints an error page over a perfectly good page.
    #[test]
    fn a_download_leaves_the_tab_on_the_page_it_is_showing() {
        let mut s = state("http://127.0.0.1:8790/", "http://127.0.0.1:8790/a.bin");
        settle_after_download(&mut s);
        assert!(!s.loading);
        assert!(s.error.is_none());
        assert_eq!(s.requested_url, "http://127.0.0.1:8790/");
        assert_eq!(s.url, "http://127.0.0.1:8790/");
        assert_eq!(s.title, "Listing");
    }

    /// A tab opened straight on a download URL has no document at all; it
    /// stays empty rather than claiming a failure.
    #[test]
    fn a_download_into_a_fresh_tab_settles_empty() {
        let mut s = state("", "http://127.0.0.1:8790/a.bin");
        settle_after_download(&mut s);
        assert!(!s.loading);
        assert!(s.error.is_none());
        assert_eq!(s.requested_url, "");
    }

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
