//! Listener-facing access for the built-in browser's agent tools
//! (`browser_list_tabs` / `browser_snapshot`) carried by codeg-mcp.
//!
//! Nothing here decides whether a page may be read. That decision is
//! `crate::browser::agent`'s, and it is enforced inside
//! `commands::browser::agent_snapshot_core`, which the production impl calls —
//! so an MCP read passes the same grant check, and leaves the same line on the
//! tab's activity strip, as any other read. A tool surface that reimplemented
//! the check would be a second place to get it wrong, and the first place
//! someone forgot to emit the audit line from.
//!
//! Two things this module does own:
//!
//! * **The shape of the answer.** A refusal is a value, not a transport error:
//!   `browser_grant_required` is something the agent can act on (ask the user
//!   to share the tab), so it comes back as an outcome the companion renders
//!   into readable text rather than as a failed tool call that aborts a turn.
//! * **Where there are no tabs at all.** A browser tab is a native webview
//!   this process owns. In server mode there is no such thing — what the user
//!   sees in a "browser tab" is an iframe their own browser renders, which
//!   this process cannot reach — so [`NoBrowserTabs`] answers there, and the
//!   group is not advertised in the first place.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::browser::agent::{AgentTabSummary, PageSnapshot};

/// The tab exists, and this agent may not read it: nobody shared it, or the
/// page left the origin it was shared for.
///
/// The two are one slug on purpose — the distinction is about a page the agent
/// is not allowed to know anything about, and the instruction is the same
/// either way: ask the user to share this tab.
pub const ERROR_GRANT_REQUIRED: &str = "browser_grant_required";

/// No tab by that id. Also the answer to a caller whose token does not check
/// out, so an unauthenticated round trip learns nothing about which tabs
/// exist.
pub const ERROR_NO_SUCH_TAB: &str = "browser_no_such_tab";

/// The grant was in force and the read still did not produce a tree — the page
/// never answered, or answered with something unreadable.
pub const ERROR_READ_FAILED: &str = "browser_read_failed";

/// This build has no built-in browser to read (server mode), or the user has
/// switched the browser tool group off since this agent was launched.
pub const ERROR_UNAVAILABLE: &str = "browser_unavailable";

/// What a `browser_snapshot` asks for when the caller names no cap.
///
/// Not a ceiling: `max_chars` is honoured as given, however large, and the
/// tool says so. It is the default because the caller who names nothing is an
/// LLM with a context window, and a page tree that silently fills it is worse
/// than one that comes back `truncated` with an invitation to ask for more.
pub const DEFAULT_SNAPSHOT_MAX_CHARS: usize = 40_000;

/// What `browser_list_tabs` answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserTabsOutcome {
    pub tabs: Vec<AgentTabSummary>,
    /// Why the list is empty, when it is empty for a reason other than "the
    /// user has no browser tabs open". Without it an agent cannot tell "you
    /// have nothing open" from "this build has no built-in browser".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl BrowserTabsOutcome {
    /// No listing to give, and the reason.
    pub fn unavailable(note: &str) -> Self {
        Self {
            tabs: Vec::new(),
            note: Some(note.to_string()),
        }
    }
}

/// What `browser_snapshot` answers: the page, or why not.
///
/// camelCase on the wire like everything else the browser sends an agent —
/// [`AgentTabSummary`] included, so `tabId` means `tabId` on both sides of a
/// refusal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshotOutcome {
    /// Echoed back so a refusal names the tab it is about — the agent quotes
    /// it to the user when asking them to share it.
    pub tab_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<PageSnapshot>,
    /// One of the `browser_*` slugs above. `None` exactly when `snapshot` is
    /// `Some`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The refusal in words, for the agent to relay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl BrowserSnapshotOutcome {
    pub fn page(tab_id: &str, snapshot: PageSnapshot) -> Self {
        Self {
            tab_id: tab_id.to_string(),
            snapshot: Some(snapshot),
            error: None,
            note: None,
        }
    }

    pub fn refused(tab_id: &str, error: &str, note: impl Into<String>) -> Self {
        Self {
            tab_id: tab_id.to_string(),
            snapshot: None,
            error: Some(error.to_string()),
            note: Some(note.into()),
        }
    }

    /// The refusal the whole grant model exists to produce, in the words the
    /// agent should pass on: name the button, not the mechanism.
    pub fn grant_required(tab_id: &str) -> Self {
        Self::refused(
            tab_id,
            ERROR_GRANT_REQUIRED,
            format!(
                "Browser tab {tab_id} is not shared with agents. Ask the user to open that tab \
                 and press \"Share with agents\" in its toolbar; then try again. Sharing is \
                 theirs to give — there is no way to take it, and no point retrying until they \
                 have."
            ),
        )
    }
}

/// Listener-facing access to the built-in browser's read surface. The
/// production impl (`crate::commands::browser::McpBrowserTools`) exists only in
/// the desktop build; server mode and tests use [`NoBrowserTabs`]. Mirrors
/// [`crate::acp::session_info::SessionInfoAccess`].
#[async_trait]
pub trait BrowserToolAccess: Send + Sync {
    /// Every tab this agent may be told about. Never an error: "no tabs" and
    /// "no browser" are both listings, distinguished by `note`.
    async fn list_tabs(&self) -> BrowserTabsOutcome;

    /// Read one shared page. `max_chars` is the caller's own cap; `None` means
    /// [`DEFAULT_SNAPSHOT_MAX_CHARS`].
    async fn snapshot(&self, tab_id: &str, max_chars: Option<usize>) -> BrowserSnapshotOutcome;
}

/// The answer where there is no built-in browser: server mode, and the stub in
/// every test that does not care about one.
pub struct NoBrowserTabs;

/// Said to an agent in a runtime that has no native tabs, and to one whose
/// user has switched the group off. Both are "not here", and neither is worth
/// retrying.
pub const NO_BROWSER_NOTE: &str =
    "The built-in browser is not available in this session, so there are no tabs to read.";

#[async_trait]
impl BrowserToolAccess for NoBrowserTabs {
    async fn list_tabs(&self) -> BrowserTabsOutcome {
        BrowserTabsOutcome::unavailable(NO_BROWSER_NOTE)
    }

    async fn snapshot(&self, tab_id: &str, _max_chars: Option<usize>) -> BrowserSnapshotOutcome {
        BrowserSnapshotOutcome::refused(tab_id, ERROR_UNAVAILABLE, NO_BROWSER_NOTE)
    }
}

/// The hot-swappable feature config read at MCP injection time, and again at
/// call time.
///
/// Re-read at call time — unlike the other read-only groups, like the
/// chat-authoring writers — because this one is a window onto pages the user is
/// looking at. Switching it off should stop the agent that is already running,
/// not only the next one launched; a user reaching for that switch is reaching
/// for it *now*.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowserToolsConfig {
    pub enabled: bool,
}

/// Shared, hot-swappable handle to [`BrowserToolsConfig`]. Cloned into
/// `DelegationInjection` (read at injection), into the access impl (read at
/// call time), and into `AppState` (updated on save).
#[derive(Clone, Default)]
pub struct BrowserToolsRuntimeConfig {
    inner: Arc<RwLock<BrowserToolsConfig>>,
}

impl BrowserToolsRuntimeConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn snapshot(&self) -> BrowserToolsConfig {
        self.inner.read().await.clone()
    }

    pub async fn set(&self, cfg: BrowserToolsConfig) {
        *self.inner.write().await = cfg;
    }

    pub async fn is_enabled(&self) -> bool {
        self.inner.read().await.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refusal has to name the tab and carry a slug the agent can branch on
    /// — the prose is for the user, the slug is for the model.
    #[test]
    fn a_refusal_names_the_tab_and_the_button_that_lifts_it() {
        let out = BrowserSnapshotOutcome::grant_required("t7");
        assert_eq!(out.tab_id, "t7");
        assert_eq!(out.error.as_deref(), Some(ERROR_GRANT_REQUIRED));
        let note = out.note.expect("a refusal explains itself");
        assert!(note.contains("t7"));
        assert!(note.contains("Share with agents"));
        assert!(out.snapshot.is_none());
    }

    /// `snapshot` and `error` are exclusive, and the absent one is absent from
    /// the wire rather than present-and-null: the companion branches on which
    /// key is there.
    #[test]
    fn the_wire_carries_exactly_one_of_the_page_and_the_refusal() {
        let refused = serde_json::to_value(BrowserSnapshotOutcome::grant_required("t1"))
            .expect("serialises");
        assert_eq!(refused["error"], ERROR_GRANT_REQUIRED);
        // camelCase, the same spelling the listing uses.
        assert_eq!(refused["tabId"], "t1");
        assert!(refused.get("tab_id").is_none());
        assert!(refused.get("snapshot").is_none());

        let page = serde_json::to_value(BrowserSnapshotOutcome::page(
            "t1",
            PageSnapshot {
                generation: "3.0".into(),
                url: "https://example.com/".into(),
                title: "Example".into(),
                viewport: crate::browser::agent::SnapshotViewport {
                    width: 1280.0,
                    height: 800.0,
                    dpr: 2.0,
                },
                tree: "- heading \"Example\"".into(),
                refs_count: 1,
                truncated: false,
            },
        ))
        .expect("serialises");
        assert_eq!(page["snapshot"]["title"], "Example");
        assert!(page.get("error").is_none());
        assert!(page.get("note").is_none());
    }

    /// An empty listing is not an error, and the two kinds of empty are told
    /// apart by the note.
    #[tokio::test]
    async fn a_runtime_without_tabs_says_so_rather_than_looking_idle() {
        let out = NoBrowserTabs.list_tabs().await;
        assert!(out.tabs.is_empty());
        assert_eq!(out.note.as_deref(), Some(NO_BROWSER_NOTE));

        let quiet = BrowserTabsOutcome::default();
        assert!(quiet.tabs.is_empty());
        assert_eq!(quiet.note, None);

        let refused = NoBrowserTabs.snapshot("t1", None).await;
        assert_eq!(refused.error.as_deref(), Some(ERROR_UNAVAILABLE));
    }

    #[tokio::test]
    async fn runtime_config_round_trips() {
        let cfg = BrowserToolsRuntimeConfig::new();
        assert!(!cfg.is_enabled().await);
        cfg.set(BrowserToolsConfig { enabled: true }).await;
        assert!(cfg.is_enabled().await);
        assert_eq!(cfg.snapshot().await, BrowserToolsConfig { enabled: true });
    }
}
