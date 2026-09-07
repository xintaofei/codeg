//! Callbacks the surfaces install on their webviews. They only touch the
//! registry and emit state; nothing here calls back into the surface, so the
//! webview thread never waits on itself.

use tauri::{AppHandle, Manager, Url};

use super::events;
use super::registry::BrowserRegistry;

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
