use serde::Serialize;

use super::conversation::DbConversationSummary;

/// Which field produced this search hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMatchKind {
    Title,
    Content,
    Both,
}

/// One conversation-level search result. Snippet fields are raw text windows
/// around the first match and are safe to render as text.
#[derive(Clone, Debug, Serialize)]
pub struct DbConversationSearchResult {
    pub summary: DbConversationSummary,
    pub match_kind: SearchMatchKind,
    pub snippet_prefix: Option<String>,
    pub snippet_match: Option<String>,
    pub snippet_suffix: Option<String>,
    pub content_match_count: u32,
}

/// Read-only view of the content index lifecycle for the search dialog.
#[derive(Clone, Debug, Serialize)]
pub struct SearchIndexStatus {
    pub mode: String,
    pub user_enabled: bool,
    pub user_mode: String,
    pub indexed_conversation_count: i32,
    pub visible_conversation_count: i64,
    pub building: bool,
    pub progress: f64,
}
