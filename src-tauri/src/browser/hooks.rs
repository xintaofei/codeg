//! Callbacks the surfaces install on their webviews. They only touch the
//! registry and emit state; nothing here calls back into the surface, so the
//! webview thread never waits on itself.

use std::time::Duration;

use tauri::{AppHandle, Manager, Url};

use super::events;
use super::registry::BrowserRegistry;
use super::types::{BrowserErrorInfo, BrowserErrorKind, NavigationBlockReason};

pub fn origin_of(url: &Url) -> Option<String> {
    let origin = url.origin();
    if origin.is_tuple() {
        Some(origin.ascii_serialization())
    } else {
        None
    }
}

/// A commit of `about:blank` while the navigation the engine had started was
/// for somewhere else. WebKit refuses some loads without ever reporting a
/// failure — a request to a restricted port (1, 7, 25, … the list every
/// browser keeps) is answered by committing an empty document in place of
/// the page — and this is the only trace it leaves. A page that navigates
/// itself to `about:blank` announces that URL as its provisional start first,
/// so it is not mistaken for one.
pub fn blank_substituted_for(provisional: Option<&str>, committed: &Url) -> bool {
    committed.as_str() == "about:blank"
        && provisional.is_some_and(|started| started != "about:blank")
}

pub fn page_load(app: &AppHandle, tab_id: &str, url: &Url, started: bool) {
    tracing::debug!("[browser] tab {tab_id} page load {}: {url}", if started { "started" } else { "finished" });
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
    let state = registry.update(tab_id, |tab| {
        let failed_address = (started && blank_substituted_for(tab.provisional_url.as_deref(), url))
            .then(|| tab.provisional_url.clone())
            .flatten();
        let substituted = failed_address.is_some();
        if started {
            tab.provisional_url = None;
        }
        let state = &mut tab.state;
        state.url = url.to_string();
        state.origin = origin_of(url);
        if let Some(address) = failed_address {
            // The engine gave up on the page and put nothing in its place:
            // that is a failed load of the address that was asked for, and
            // the empty document that committed is not worth a spinner.
            state.loading = false;
            state.error = Some(BrowserErrorInfo {
                kind: BrowserErrorKind::Failed,
                message: String::new(),
                url: Some(address),
            });
        } else {
            state.loading = started;
            if started {
                state.error = None;
                // The previous document's title must not label the new one;
                // the toolbar falls back to the host until `title_changed`
                // fires.
                state.title.clear();
            }
        }
        if let Some((back, forward)) = history {
            state.can_go_back = back;
            state.can_go_forward = forward;
        }
        (state.clone(), substituted)
    });
    let Some((state, substituted)) = state else {
        return;
    };
    events::emit_state(app, &state);
    if started && !substituted {
        begin_load(app, tab_id);
    }
}

/// The engine started a main-frame navigation (WebKit's
/// `didStartProvisionalNavigation`, before any byte has arrived). This is the
/// earliest the tab knows where it is heading: a link click, a redirect chain
/// or a form post all announce themselves here, so `requested_url` follows
/// the page's own navigations and not only the address bar's. The failed-load
/// watcher is armed from here for page-initiated navigations; the commands
/// arm it themselves as well, and a second arming only retires the first.
pub fn navigation_started(app: &AppHandle, tab_id: &str, url: &Url) {
    let Some(registry) = app.try_state::<BrowserRegistry>() else {
        return;
    };
    let state = registry.update(tab_id, |tab| {
        tab.provisional_url = Some(url.to_string());
        tab.state.requested_url = url.to_string();
        tab.state.loading = true;
        tab.state.error = None;
        tab.state.clone()
    });
    if let Some(state) = state {
        events::emit_state(app, &state);
    }
    begin_load(app, tab_id);
}

/// A navigation the engine reported as failed (a platform delegate callback,
/// where one exists — wry itself never reports failure).
#[derive(Debug, Clone, PartialEq)]
pub struct LoadFailure {
    pub kind: BrowserErrorKind,
    /// The platform's own description, in the system language.
    pub message: String,
    /// The address that failed, when the error names one.
    pub url: Option<String>,
    /// Failed before anything committed (nothing of the new page is showing)
    /// rather than after (the page is up, a later part of the load broke).
    pub provisional: bool,
}

/// Classify a platform load error. `None` means "not a failure of the page":
/// a cancelled navigation (superseded by another, stopped by the user) and a
/// load the host itself redirected to a download or refused by policy both
/// end with an error code that is not the page's fault and must not paint an
/// error page. Domains and codes are Apple's (`NSURLErrorDomain`,
/// `WebKitErrorDomain`); other platforms map their own onto the same kinds.
pub fn classify_load_error(domain: &str, code: i64) -> Option<BrowserErrorKind> {
    match domain {
        "NSURLErrorDomain" => match code {
            // NSURLErrorCancelled
            -999 => None,
            // NSURLErrorCannotFindHost, NSURLErrorDNSLookupFailed
            -1003 | -1006 => Some(BrowserErrorKind::Dns),
            // NSURLErrorSecureConnectionFailed … NSURLErrorClientCertificateRequired
            -1206..=-1200 => Some(BrowserErrorKind::Tls),
            _ => Some(BrowserErrorKind::Failed),
        },
        // WebKitErrorFrameLoadInterruptedByPolicyChange: the policy delegate
        // (our own navigation handler) cancelled the load, or it became a
        // download. Both are handled where they happen.
        "WebKitErrorDomain" if code == 102 => None,
        _ => Some(BrowserErrorKind::Failed),
    }
}

/// Apply a reported failure. A provisional failure replaces the page with the
/// error page for the address that was asked for; a failure after commit
/// only stops the spinner — the document that committed stays, as in a
/// browser. Either way the load watcher, seeing `loading: false`, stands
/// down.
pub fn navigation_failed(app: &AppHandle, tab_id: &str, failure: LoadFailure) {
    let Some(registry) = app.try_state::<BrowserRegistry>() else {
        return;
    };
    let state = registry.update(tab_id, |tab| {
        if failure.provisional {
            tab.provisional_url = None;
        }
        let state = &mut tab.state;
        state.loading = false;
        if failure.provisional {
            let url = failure
                .url
                .clone()
                .filter(|u| !u.is_empty())
                .or_else(|| (!state.requested_url.is_empty()).then(|| state.requested_url.clone()))
                .or_else(|| (!state.url.is_empty()).then(|| state.url.clone()));
            state.error = Some(BrowserErrorInfo {
                kind: failure.kind,
                message: failure.message.clone(),
                url,
            });
        }
        state.clone()
    });
    if let Some(state) = state {
        events::emit_state(app, &state);
    }
}

/// A top-level navigation was refused (scheme not allowed, or a site rule).
/// Nothing changes in the tab; the status layer shows why the click did
/// nothing.
pub fn navigation_blocked(app: &AppHandle, tab_id: &str, url: &str, reason: NavigationBlockReason) {
    tracing::info!("[browser] tab {tab_id} blocked navigation to {url} ({reason:?})");
    events::emit_navigation_blocked(app, tab_id, url, reason);
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
///
/// Known limit: the engine does not say which navigation a download came
/// from, and for a redirected download the reported URL is the FINAL one
/// (verified on macOS: navigating to a URL that 302s to a file reports the
/// file's URL), so the address cannot be used to correlate either. A download
/// callback that arrives after a later navigation has started therefore marks
/// that navigation's generation, and if it then fails its error is not
/// reported. The alternative — matching on the URL — would put an error page
/// over a perfectly good page for every redirected download, which is the
/// common case.
///
/// Owned windows have no load watcher at all (`is_loading` has no handle to
/// answer from), so nothing would ever consume the mark: they settle here and
/// now, or the tab would spin for ever.
pub fn navigation_became_download(app: &AppHandle, tab_id: &str) {
    let Some(registry) = app.try_state::<BrowserRegistry>() else {
        return;
    };
    let embedded = registry
        .surface(tab_id)
        .map(|surface| surface.is_embedded())
        .unwrap_or(false);
    if embedded {
        registry.update(tab_id, |tab| tab.download_seq = Some(tab.load_seq));
        return;
    }
    let state = registry.update(tab_id, |tab| {
        tab.download_seq = None;
        settle_after_download(&mut tab.state);
        tab.state.clone()
    });
    if let Some(state) = state {
        events::emit_state(app, &state);
    }
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

    /// An empty document committed in place of the page that was started is
    /// a refused load; a page that heads for `about:blank` itself is not.
    #[test]
    fn a_blank_commit_counts_as_failure_only_when_something_else_was_started() {
        let blank = Url::parse("about:blank").unwrap();
        let page = Url::parse("http://127.0.0.1:1/").unwrap();
        assert!(blank_substituted_for(Some("http://127.0.0.1:1/"), &blank));
        assert!(!blank_substituted_for(Some("about:blank"), &blank));
        assert!(!blank_substituted_for(None, &blank));
        assert!(!blank_substituted_for(Some("http://127.0.0.1:1/"), &page));
    }

    /// Apple's codes, by kind — and the two that are NOT page failures.
    #[test]
    fn load_errors_classify_by_domain_and_code() {
        use BrowserErrorKind::*;
        assert_eq!(classify_load_error("NSURLErrorDomain", -1003), Some(Dns));
        assert_eq!(classify_load_error("NSURLErrorDomain", -1006), Some(Dns));
        assert_eq!(classify_load_error("NSURLErrorDomain", -1200), Some(Tls));
        assert_eq!(classify_load_error("NSURLErrorDomain", -1202), Some(Tls));
        assert_eq!(classify_load_error("NSURLErrorDomain", -1206), Some(Tls));
        assert_eq!(classify_load_error("NSURLErrorDomain", -1004), Some(Failed));
        assert_eq!(classify_load_error("NSURLErrorDomain", -1001), Some(Failed));
        assert_eq!(classify_load_error("NSURLErrorDomain", -1009), Some(Failed));
        assert_eq!(classify_load_error("NSURLErrorDomain", -1199), Some(Failed));
        assert_eq!(classify_load_error("NSURLErrorDomain", -1207), Some(Failed));
        // Superseded / stopped: not an error of the page.
        assert_eq!(classify_load_error("NSURLErrorDomain", -999), None);
        // Cancelled by our own policy handler, or became a download.
        assert_eq!(classify_load_error("WebKitErrorDomain", 102), None);
        assert_eq!(classify_load_error("WebKitErrorDomain", 101), Some(Failed));
        assert_eq!(classify_load_error("WKErrorDomain", 2), Some(Failed));
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
