//! Pure policy decisions for browser tabs. Everything here is a function of
//! its arguments so the tables in the unit tests are the specification.

use tauri::Url;

/// Top-level navigation allow-list. Only real web pages may load in a tab:
/// `http(s)`, the `about:blank` bootstrap page every tab starts from, and
/// `blob:` URLs minted by an http(s) page (Turnstile and friends). Everything
/// else — `file:`, `tauri:`, `javascript:`, `data:` documents, custom schemes —
/// is refused: a tab must never be able to reach the app's own origin or the
/// local filesystem.
pub fn navigation_allowed(url: &Url) -> bool {
    match url.scheme() {
        "http" | "https" => true,
        "about" => url.as_str() == "about:blank",
        "blob" => {
            let inner = url.path();
            inner.starts_with("http://") || inner.starts_with("https://")
        }
        _ => false,
    }
}

/// The initial URL handed to `browser_open_tab` must already be a web page;
/// `about:blank` is accepted so an empty tab can be opened explicitly.
pub fn open_url_allowed(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https") || url.as_str() == "about:blank"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn navigation_allow_list() {
        for ok in [
            "http://localhost:3000/",
            "https://example.com/a?b#c",
            "about:blank",
            "blob:https://example.com/0c8f-4a",
            "blob:http://localhost:3000/x",
        ] {
            assert!(navigation_allowed(&u(ok)), "{ok}");
        }
        for bad in [
            "file:///etc/passwd",
            "tauri://localhost/",
            "javascript:alert(1)",
            "data:text/html,<b>x</b>",
            "about:config",
            "blob:null/abc",
            "blob:file:///x",
            "codeg-doc://grant/index.html",
            "ftp://example.com/",
            "vscode://file/x",
        ] {
            assert!(!navigation_allowed(&u(bad)), "{bad}");
        }
    }

    #[test]
    fn open_url_is_stricter_than_navigation() {
        assert!(open_url_allowed(&u("https://example.com")));
        assert!(open_url_allowed(&u("about:blank")));
        assert!(!open_url_allowed(&u("blob:https://example.com/x")));
        assert!(!open_url_allowed(&u("file:///x")));
    }
}
