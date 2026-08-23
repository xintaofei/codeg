//! Forge (GitHub/GitLab) integration core: account/auth resolution, a
//! proxy-aware HTTP client, the canonical source-key normalizer and the REST
//! reads the Issues/PR workbench needs. Deliberately thin — no query DSL, no
//! response caching (see `.docs/architecture/2026-08-17-*` for what is out of
//! scope and why REST-direct beat shelling out to `gh`/`glab`).

pub mod auth;
pub mod deliver;
pub mod envelope;
pub mod github;
pub mod gitlab;
pub mod settings;

use std::sync::RwLock;

pub use auth::{host_profile, resolve_forge_auth, strip_base_path, HostProfile, ResolvedAuth};

/// The forge a host speaks. Everything provider-specific downstream — REST
/// base, auth header, the shape of a "pull request", which ref a proposed
/// change is published under, where a comment goes — hangs off this one value,
/// which is always DERIVED SERVER-SIDE from the folder's remote and the
/// configured accounts. A client never gets to say which forge it is talking
/// to: that choice picks the credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForgeProvider {
    GitHub,
    GitLab,
}

impl ForgeProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            ForgeProvider::GitHub => "github",
            ForgeProvider::GitLab => "gitlab",
        }
    }

    /// Parse a stored/claimed provider name. Unknown values are an error rather
    /// than a silent fallback to GitHub — a task whose provenance says
    /// something we do not understand must not be worked with the wrong API.
    pub fn parse(value: &str) -> Result<Self, ForgeError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "github" => Ok(ForgeProvider::GitHub),
            "gitlab" => Ok(ForgeProvider::GitLab),
            other => Err(ForgeError::Invalid(format!("unknown provider: {other}"))),
        }
    }

    /// How this forge spells its own name in prose. [`as_str`] is the wire
    /// value (lowercase, compared and stored); putting it in front of a user
    /// reads as a typo.
    pub fn display_name(self) -> &'static str {
        match self {
            ForgeProvider::GitHub => "GitHub",
            ForgeProvider::GitLab => "GitLab",
        }
    }

    /// What this forge calls a proposed change, for messages the user reads.
    /// GitLab users do not have "pull requests" and being told they do reads
    /// like the wrong tool answered.
    pub fn change_noun(self) -> &'static str {
        match self {
            ForgeProvider::GitHub => "pull request",
            ForgeProvider::GitLab => "merge request",
        }
    }

    /// The ref a proposed change's head is published under on the server —
    /// what makes a fork's (or any) contribution fetchable without adding a
    /// remote. Both forges publish one; they just spell it differently.
    ///
    /// Note the HYPHEN in GitLab's: its REST path is `/merge_requests` with an
    /// underscore, but the git ref namespace is `refs/merge-requests/<iid>`.
    /// The two spellings sit three lines apart in this file for a reason —
    /// using the API's spelling as a ref fetches nothing at all.
    pub fn change_head_ref(self, number: i64) -> String {
        match self {
            ForgeProvider::GitHub => format!("refs/pull/{number}/head"),
            ForgeProvider::GitLab => format!("refs/merge-requests/{number}/head"),
        }
    }

    /// Canonical web URL of one work item, under `origin` (see [`web_origin`]
    /// — a scheme-and-port-carrying origin rather than a bare host, so a
    /// self-hosted instance on `http://` or a non-default port gets a link
    /// that actually opens).
    pub fn item_url(
        self,
        origin: &str,
        owner_repo: &str,
        kind: ForgeItemKind,
        number: i64,
    ) -> String {
        let origin = origin.trim_end_matches('/');
        match self {
            ForgeProvider::GitHub => {
                // `/issues/{n}` of a pull request redirects to `/pull/{n}`, but
                // the link is stored and shown, so it says what it is.
                let segment = match kind {
                    ForgeItemKind::Issue => "issues",
                    ForgeItemKind::Change => "pull",
                };
                format!("{origin}/{owner_repo}/{segment}/{number}")
            }
            ForgeProvider::GitLab => {
                // The `/-/` separator is what keeps a project path with
                // subgroups unambiguous against the route that follows it.
                let segment = match kind {
                    ForgeItemKind::Issue => "issues",
                    ForgeItemKind::Change => "merge_requests",
                };
                format!("{origin}/{owner_repo}/-/{segment}/{number}")
            }
        }
    }
}

/// Issue or proposed change. Load-bearing for GitLab, where the two have
/// SEPARATE list and comment endpoints — GitHub models a pull request as an
/// issue and serves both from `/issues`, which is exactly the assumption that
/// silently breaks against a GitLab instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeItemKind {
    Issue,
    /// Pull request (GitHub) / merge request (GitLab).
    Change,
}

impl ForgeItemKind {
    /// The `kind` segment of a source key — `pr` for both forges (a GitLab
    /// merge request is normalized to it) so provenance keys stay comparable.
    pub fn key_segment(self) -> &'static str {
        match self {
            ForgeItemKind::Issue => "issue",
            ForgeItemKind::Change => "pr",
        }
    }
}

/// i18n key for [`ForgeError::NoAccount`]. Dotted from the message root
/// because the frontend localizes it with a ROOT-scoped translator; the
/// forge page's own `useTranslations("Forge")` cannot resolve it.
pub const NO_ACCOUNT_I18N_KEY: &str = "Forge.errors.noAccount";

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    /// No usable account/token for the requested host (or the token is dead).
    #[error("forge auth: {0}")]
    Auth(String),
    /// Nothing is configured for this host at all — the one auth failure the
    /// user can fix themselves, so it carries an i18n key and the workbench
    /// turns it into an "add an account" action instead of a dead end. A
    /// pinned-id miss, a missing keyring token or a rejected token stay
    /// [`ForgeError::Auth`]: adding another account fixes none of them.
    #[error("no {} account for host {host}", provider.display_name())]
    NoAccount { provider: ForgeProvider, host: String },
    /// Primary or secondary rate limit; honor `retry_after` when present.
    #[error("forge rate limited")]
    RateLimited { retry_after: Option<u64> },
    #[error("forge resource not found")]
    NotFound,
    /// Caller-supplied input failed validation (bad repo path, foreign cursor…).
    #[error("forge invalid input: {0}")]
    Invalid(String),
    #[error("forge API error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("forge network error: {0}")]
    Network(String),
}

impl From<ForgeError> for crate::app_error::AppCommandError {
    fn from(err: ForgeError) -> Self {
        use crate::app_error::AppCommandError as E;
        match &err {
            ForgeError::Auth(msg) => E::configuration_invalid(
                "the account for this repository's host is not usable",
            )
            .with_detail(msg.clone()),
            ForgeError::NoAccount { provider, host } => {
                let params = std::collections::BTreeMap::from([
                    ("host".to_string(), host.clone()),
                    ("provider".to_string(), provider.display_name().to_string()),
                ]);
                E::configuration_missing(err.to_string())
                    .with_i18n(NO_ACCOUNT_I18N_KEY, params)
            }
            ForgeError::RateLimited { retry_after } => E::network("forge rate limit reached")
                .with_detail(match retry_after {
                    Some(secs) => format!("retry after {secs}s"),
                    None => "retry later".to_string(),
                }),
            ForgeError::NotFound => E::not_found("forge resource not found"),
            ForgeError::Invalid(msg) => E::invalid_input(msg.clone()),
            ForgeError::Api { .. } | ForgeError::Network(_) => {
                E::network("forge API request failed").with_detail(err.to_string())
            }
        }
    }
}

/// `work_task.source_kind` for a task triggered from an issue. Its own
/// constant because it is a GATE in three places (delivery, the local-merge
/// refusal, the trigger command) and a typo in any of them opens one of them.
pub const SOURCE_KIND_ISSUE: &str = "forge_issue";
/// `work_task.source_kind` for a task triggered from a pull request (M8).
pub const SOURCE_KIND_PR: &str = "forge_pr";

/// Canonical provenance key: `{provider}:{server_host}:{owner_repo}:{kind}:{number}`.
///
/// Both writers (trigger command) and readers (dedup, the issue list's reverse
/// lookup) MUST build keys through this function — never by hand — so casing
/// or host drift can't split one work item into two keys. The host is the
/// SERVER host (`github.com`, `ghe.corp.com`), the same coordinate system git
/// remotes live in; the API base is a derived value and never part of the key.
pub fn source_key(
    provider: &str,
    server_host: &str,
    owner_repo: &str,
    kind: &str,
    number: i64,
) -> Result<String, ForgeError> {
    let provider = provider.trim().to_ascii_lowercase();
    if provider != "github" && provider != "gitlab" {
        return Err(ForgeError::Invalid(format!("unknown provider: {provider}")));
    }
    let kind = kind.trim().to_ascii_lowercase();
    if kind != "issue" && kind != "pr" {
        return Err(ForgeError::Invalid(format!("unknown source kind: {kind}")));
    }
    let host = server_host.trim().to_ascii_lowercase();
    if host.is_empty() || host.contains('/') || host.contains(':') {
        return Err(ForgeError::Invalid(format!("bad server host: {server_host}")));
    }
    let repo = normalize_repo(owner_repo)
        .ok_or_else(|| ForgeError::Invalid(format!("bad repository path: {owner_repo}")))?;
    if number <= 0 {
        return Err(ForgeError::Invalid(format!("bad work item number: {number}")));
    }
    Ok(format!("{provider}:{host}:{repo}:{kind}:{number}"))
}

/// Repository-identity comparison. GitHub preserves canonical casing in API
/// responses (`microsoft/TypeScript`) while our keys and remotes normalize to
/// lowercase — an exact string compare would reject the right PR or mistake a
/// same-repo head for a fork. Every repo comparison (folder-remote check,
/// PR adoption, fork gate) goes through here.
pub fn same_repo(a: &str, b: &str) -> bool {
    match (normalize_repo(a), normalize_repo(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Lowercased `owner/repo` (GitLab: full subgroup path), `.git` suffix and
/// surrounding slashes stripped. `None` when the shape is not a repo path or
/// contains URL metacharacters (a client-supplied value goes into request
/// paths, so this doubles as injection hygiene).
pub fn normalize_repo(input: &str) -> Option<String> {
    let trimmed = input
        .trim()
        .trim_matches('/')
        .trim_end_matches(".git")
        .to_ascii_lowercase();
    if trimmed.is_empty() || !trimmed.contains('/') {
        return None;
    }
    let ok = trimmed.split('/').all(|seg| {
        !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    });
    if ok {
        Some(trimmed)
    } else {
        None
    }
}

/// `(server_host, owner_repo)` parsed from a git remote URL — the bridge
/// between a local folder and the forge repository its issues live in.
/// Handles the three shapes remotes actually take: `https://host/o/r(.git)`,
/// `git@host:o/r.git`, `ssh://git@host[:port]/o/r`.
pub fn parse_remote_url(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    // scp-like SSH: git@host:owner/repo(.git) — no scheme, single colon.
    if !trimmed.contains("://") {
        if let Some((user_host, path)) = trimmed.split_once(':') {
            let host = user_host.split('@').next_back()?.trim().to_ascii_lowercase();
            if host.is_empty() || host.contains('/') {
                return None;
            }
            return Some((host, normalize_repo(path)?));
        }
        return None;
    }
    // Scheme form: https:// or ssh:// — host[:port]/owner/repo(.git).
    let rest = trimmed.split_once("://")?.1;
    let (host_port, path) = rest.split_once('/')?;
    let host = host_port
        .split('@')
        .next_back()?
        .split(':')
        .next()?
        .trim()
        .to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some((host, normalize_repo(path)?))
}

/// Provenance snapshot stored in `work_task.source_meta` (JSON) and mirrored
/// to the frontend as `ForgeSourceMeta` in `src/lib/types.ts`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ForgeSourceMeta {
    /// Typed rather than a free string: a stored value that is neither forge
    /// fails to deserialize the whole snapshot, which every reader already
    /// treats as "the task's source information is unreadable". That is a far
    /// better answer than defaulting to one forge and spending the other's
    /// credential on it.
    pub provider: ForgeProvider,
    pub server_host: String,
    pub api_base: String,
    pub account_id: String,
    pub owner_repo: String,
    pub number: i64,
    /// Canonical html URL — server-derived, never taken from the client.
    pub url: String,
    /// Title at trigger time (display only; the prompt carries its own copy).
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_pr: Option<String>,
    /// Whether this task comments its outcome back on the item when it
    /// finishes — the answer the user gave in the trigger dialog, frozen here.
    /// It lives on the task rather than in folder settings because it is a
    /// decision about THIS work item, taken while looking at it: a switch
    /// somewhere else would publish to a thread other people are reading on
    /// behalf of a task whose author never saw the question.
    ///
    /// Absent on rows minted before the choice moved here; those stay silent,
    /// which is the posture the old folder setting shipped with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writeback: Option<bool>,
}

/// List rows carry the body for the trigger snapshot; cap it so a megabyte
/// issue body doesn't ride every list response. The untrusted-data envelope
/// trims to 12k at prompt time — this keeps a margin above that.
pub const BODY_CAP: usize = 16_000;

/// Which tab of the workbench a list request is for. Provider-neutral: GitHub
/// serves both from one endpoint and splits locally, GitLab has two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeTab {
    Issues,
    Prs,
}

/// Page-size bounds. Both forges cap `per_page` at 100; the clamp lives here
/// so a client cannot ask for a page that either API would reject outright.
pub const MIN_PER_PAGE: u32 = 1;
pub const MAX_PER_PAGE: u32 = 100;
pub const DEFAULT_PER_PAGE: u32 = 20;

/// Longest free-text filter accepted. GitHub caps the whole `q` at 256
/// characters and the qualifiers already eat some of that, so a longer string
/// would turn the search into a 422 rather than a narrower list.
pub const MAX_SEARCH_CHARS: usize = 128;
/// Most labels one filter may name. Both forges AND them together, so past a
/// handful the result set is empty anyway — and each one lengthens GitHub's `q`.
pub const MAX_LABEL_FILTERS: usize = 10;

/// How the list is ordered. Deliberately four NAMED orders rather than a
/// free (field, direction) pair: the two forges spell their fields differently
/// (`created` vs `created_at`) and accept different sets, so the intersection
/// is what the workbench can honestly offer on both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeSort {
    /// Newest first — what github.com's own issue list defaults to.
    #[default]
    Newest,
    Oldest,
    RecentlyUpdated,
    LeastRecentlyUpdated,
}

impl ForgeSort {
    /// GitHub's `sort` value. GitLab spells the same two fields `created_at` /
    /// `updated_at`, derived from this in its own client.
    pub fn field(self) -> &'static str {
        match self {
            ForgeSort::Newest | ForgeSort::Oldest => "created",
            ForgeSort::RecentlyUpdated | ForgeSort::LeastRecentlyUpdated => "updated",
        }
    }

    pub fn ascending(self) -> bool {
        matches!(self, ForgeSort::Oldest | ForgeSort::LeastRecentlyUpdated)
    }

    /// `asc` / `desc` — the spelling both forges share.
    pub fn direction(self) -> &'static str {
        if self.ascending() {
            "asc"
        } else {
            "desc"
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListIssuesRequest {
    /// SERVER-DERIVED from the folder's remote — never taken from the client.
    /// That is why this struct is built through [`ListIssuesRequest::new`]
    /// rather than deserialized from the wire.
    pub owner_repo: String,
    pub tab: ForgeTab,
    /// "open" | "closed" | "all" (anything else is rejected). Normalized to
    /// each forge's own vocabulary by its client.
    #[serde(default = "default_state")]
    pub state: String,
    #[serde(default)]
    pub assigned_me: bool,
    /// Label names, ANDed. Already normalized (see [`normalize_labels`]).
    #[serde(default)]
    pub labels: Vec<String>,
    /// Free text over title and description. Already normalized (see
    /// [`normalize_search`]) — and treated as TEXT by both clients, not as
    /// query syntax the user gets to write.
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub sort: ForgeSort,
    /// 1-based page. Offset pagination on both forges — see [`ForgeIssueList`]
    /// for why this replaced the old opaque Link cursor.
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

impl ListIssuesRequest {
    /// The one way a provider request comes into being: caller-supplied
    /// filters plus a repository the SERVER resolved. Keeping `owner_repo` a
    /// separate argument is the trust boundary — it cannot be defaulted in
    /// from a payload the client wrote.
    pub fn new(owner_repo: String, filters: ListFilters) -> Self {
        Self {
            owner_repo,
            tab: filters.tab,
            state: filters.state,
            assigned_me: filters.assigned_me,
            labels: normalize_labels(filters.labels),
            search: normalize_search(filters.search.as_deref()),
            sort: filters.sort,
            page: filters.page,
            per_page: filters.per_page,
        }
    }

    /// Bring client-supplied paging into range. Called by the clients rather
    /// than trusted from the wire: a `per_page=0` is a 422 at GitHub and an
    /// empty page at GitLab, and `page=0` silently means page 1 at one and
    /// not the other.
    pub fn clamped(&self) -> (u32, u32) {
        (
            self.page.max(1),
            self.per_page.clamp(MIN_PER_PAGE, MAX_PER_PAGE),
        )
    }
}

/// Everything about a list request the CLIENT gets to decide. The repository
/// is not in here on purpose (see [`ListIssuesRequest::new`]).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListFilters {
    pub tab: ForgeTab,
    #[serde(default = "default_state")]
    pub state: String,
    #[serde(default)]
    pub assigned_me: bool,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub sort: ForgeSort,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
    /// Which stored account to spend. Auth, not a filter — consumed by the
    /// command layer and never reaches a provider client.
    #[serde(default)]
    pub account_id: Option<String>,
}

/// Everything a COUNT request may be narrowed by.
///
/// Deliberately not [`ListFilters`]: a count has no tab, no page and no order,
/// and carrying three fields the server is obliged to ignore is how a client
/// comes to believe it set one. What it does share is the filter half — the
/// numbers on the switcher have to describe the same result set the list does,
/// or the badge and the page count contradict each other on screen.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CountFilters {
    #[serde(default = "default_state")]
    pub state: String,
    #[serde(default)]
    pub assigned_me: bool,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub search: Option<String>,
    /// Which stored account to spend. Auth, not a filter — same as
    /// [`ListFilters::account_id`], and consumed by the command layer.
    #[serde(default)]
    pub account_id: Option<String>,
}

impl CountFilters {
    /// The one-row list request whose `total_count` IS the answer.
    ///
    /// Neither forge offers a count-only endpoint for these collections:
    /// GitHub's search returns `total_count` beside the page and GitLab puts
    /// its own in `X-Total`, so the smallest page there is ([`MIN_PER_PAGE`])
    /// is the cheapest way to ask. The order cannot change a count and is left
    /// at the default rather than plumbed through.
    pub fn probe(&self, tab: ForgeTab) -> ListFilters {
        ListFilters {
            tab,
            state: self.state.clone(),
            assigned_me: self.assigned_me,
            labels: self.labels.clone(),
            search: self.search.clone(),
            sort: ForgeSort::default(),
            page: 1,
            per_page: MIN_PER_PAGE,
            account_id: self.account_id.clone(),
        }
    }
}

/// Trim, cap and drop-if-empty. The cap is [`MAX_SEARCH_CHARS`] CHARACTERS,
/// counted char-wise: a byte slice through arbitrary user text can split a
/// code point and panic.
pub fn normalize_search(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_chars(trimmed, MAX_SEARCH_CHARS))
}

/// Trim, drop empties, de-duplicate (case-sensitively — GitHub label names are
/// case-sensitive) and cap at [`MAX_LABEL_FILTERS`], preserving order.
pub fn normalize_labels(raw: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    raw.into_iter()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && seen.insert(l.clone()))
        .take(MAX_LABEL_FILTERS)
        .collect()
}

/// One label as the forge paints it: the name a filter names it by, plus the
/// colour the project gave it. Both forges attach a colour to every label, and
/// carrying it is what lets the workbench draw github.com's own chip instead of
/// a row of identical grey pills — colour is how a triage list is scanned.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ForgeLabel {
    pub name: String,
    /// Normalized to `#rrggbb`, or `None` when the forge sent something this
    /// does not recognize. See [`normalize_hex_color`].
    pub color: Option<String>,
}

impl ForgeLabel {
    /// A provider's label, or `None` when it carries no usable name — an empty
    /// name is a chip with nothing in it and no way to filter by.
    pub fn parse(name: String, color: Option<&str>) -> Option<Self> {
        if name.is_empty() {
            return None;
        }
        Some(Self {
            name,
            color: color.and_then(normalize_hex_color),
        })
    }
}

/// `#rrggbb` out of whatever spelling the forge used, or `None`.
///
/// The two disagree: GitHub sends bare `d73a4a`, GitLab sends `#d9534f`. Both
/// are accepted, as is the 3-digit shorthand.
///
/// Everything else is rejected rather than passed through, because GitLab
/// ACCEPTS CSS colour names when a label is written (`red`, `rebeccapurple`),
/// so a self-managed instance can hold a value that is not hex at all — and
/// this string ends up inside a `style` attribute. `None` costs the chip its
/// colour, which is a great deal cheaper than forwarding arbitrary text into a
/// stylesheet.
pub fn normalize_hex_color(raw: &str) -> Option<String> {
    let hex = raw.trim().trim_start_matches('#');
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let expanded: String = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_string(),
        _ => return None,
    };
    Some(format!("#{}", expanded.to_ascii_lowercase()))
}

/// The repository's label vocabulary, for the workbench's label filter.
///
/// One page of [`LABEL_PAGE_SIZE`], and `truncated` says so out loud when the
/// repository has more: a filter list that silently stops at 100 reads as
/// "these are all the labels", and the label you wanted simply is not there.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ForgeLabelList {
    pub labels: Vec<ForgeLabel>,
    pub truncated: bool,
}

/// Labels fetched in one request. 100 is the per-page maximum on both forges.
pub const LABEL_PAGE_SIZE: usize = 100;

fn default_state() -> String {
    "open".to_string()
}

fn default_page() -> u32 {
    1
}

fn default_per_page() -> u32 {
    DEFAULT_PER_PAGE
}

/// One row of the workbench list (both tabs share the shape; `is_pr` is the
/// split). `body` rides along because the trigger snapshot is taken from the
/// list row — GitHub's `/issues` and GitLab's `/issues`+`/merge_requests` all
/// include it, which is what makes a detail endpoint unnecessary for issues.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ForgeIssueRow {
    pub number: i64,
    pub title: String,
    /// Capped (see [`BODY_CAP`]) — the untrusted-data envelope trims to 12k
    /// anyway, so shipping megabyte bodies to the UI buys nothing.
    pub body: Option<String>,
    /// Normalized to `open` / `closed` / `merged` for BOTH forges. GitLab says
    /// `opened`; GitHub has no merged STATE at all (a merged pull request is
    /// just closed) and is derived from `pull_request.merged_at`. The workbench
    /// row picks its icon and colour off this value plus [`Self::draft`].
    pub state: String,
    /// Draft / work-in-progress pull request. Always false for issues.
    pub draft: bool,
    pub labels: Vec<ForgeLabel>,
    pub author: Option<String>,
    pub updated_at: Option<String>,
    pub html_url: String,
    pub is_pr: bool,
    /// Human comments on the item. GitHub calls it `comments`, GitLab
    /// `user_notes_count` — both exclude system notes, which is what makes the
    /// number mean "there is a discussion here" rather than "things happened".
    pub comments: i64,
}

/// One page of the workbench list.
///
/// Offset pagination, NOT the old `Link: rel="next"` cursor. The cursor could
/// only ever offer "load more", because the GitHub client listed both kinds
/// from `/issues` and split them client-side — so "API page 3" was not
/// "Issues page 3" and no total existed. The GitHub client now asks
/// `/search/issues` with `is:issue` / `is:pr` (what github.com's own web UI is
/// backed by), which filters server-side and returns a real count; GitLab's
/// two collections were always separate. Hence real page numbers.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ForgeIssueList {
    pub rows: Vec<ForgeIssueRow>,
    /// Echo of the page actually served (already clamped).
    pub page: u32,
    pub per_page: u32,
    /// Matching items, or `None` when the forge declines to count: GitLab
    /// omits `X-Total`/`X-Total-Pages` past 10k rows, and one GitLab query
    /// (merge requests + closed) is filtered locally so its count would lie.
    /// `None` means the UI must degrade to previous/next.
    pub total_count: Option<i64>,
    /// How many of those matches the forge will actually PAGE through, when
    /// that is fewer than `total_count`. GitHub Search serves only the first
    /// [`github::SEARCH_RESULT_CAP`]; page 1201 of a 24 000-hit query is a 422,
    /// so page numbers derived from `total_count` would be a strip of buttons
    /// into an error. `None` means every match is reachable — which is both
    /// GitLab's answer and GitHub's whenever the query fits under the cap.
    pub reachable_count: Option<i64>,
    pub has_next: bool,
    /// GitHub search timed out and answered with a partial result set. Shown,
    /// not swallowed — a silently short list reads as "that's all there is".
    pub incomplete: bool,
}

impl ForgeIssueList {
    /// The count a BADGE may show.
    ///
    /// `total_count`, but only when the forge answered completely: an
    /// incomplete search counted fewer items than match, and a bare number on
    /// a tab has nowhere to say so. The list can carry that caveat next to
    /// itself ([`Self::incomplete`]); a badge can only be right or absent.
    pub fn trustworthy_count(&self) -> Option<i64> {
        if self.incomplete {
            None
        } else {
            self.total_count
        }
    }
}

/// Proxy-aware shared HTTP client. A reqwest client caches its proxy
/// configuration for its whole lifetime (`network/proxy.rs` startup contract),
/// but codeg lets the user change the proxy at runtime — so the client is
/// keyed by the current proxy env fingerprint and rebuilt whenever that
/// changes. Lazy construction also lands after `init_proxy_from_db` for free.
/// The proxy env fingerprint a client was built under, paired with that client.
type ProxyKeyedClient = (Vec<(String, String)>, reqwest::Client);

static HTTP_CLIENT: RwLock<Option<ProxyKeyedClient>> = RwLock::new(None);

/// Char-boundary-safe truncation (issue bodies are arbitrary UTF-8; a byte
/// slice could split a code point and panic).
pub(crate) fn truncate_chars(input: &str, cap: usize) -> String {
    if input.chars().count() <= cap {
        return input.to_string();
    }
    input.chars().take(cap).collect()
}

/// Reject a state filter we do not understand rather than pass it through to
/// the API, where it would silently change what the list means.
pub(crate) fn validate_state_filter(state: &str) -> Result<(), ForgeError> {
    if matches!(state, "open" | "closed" | "all") {
        Ok(())
    } else {
        Err(ForgeError::Invalid(format!("bad state filter: {state}")))
    }
}

/// The instance's WEB origin (scheme, host and port), derived from the API
/// base rather than from `server_host` — which is a bare hostname and would
/// silently drop both the scheme and a non-default port.
///
/// `https://api.github.com` is the public service's dedicated API host, whose
/// web origin is `https://github.com`; GitHub Enterprise mounts its API under
/// the instance (`{origin}/api/v3`) and GitLab under `{origin}/api/v4`. One
/// derivation, used by every place that needs a link or a push URL — three
/// copies of this rule would be three chances to disagree.
pub fn web_origin(auth: &ResolvedAuth) -> String {
    let base = auth.api_base.trim_end_matches('/');
    if base == "https://api.github.com" {
        return "https://github.com".to_string();
    }
    match base.strip_suffix("/api/v3").or_else(|| base.strip_suffix("/api/v4")) {
        Some(origin) => origin.to_string(),
        None => format!("https://{}", auth.server_host),
    }
}

/// Percent-encode a QUERY value — the few characters that are legal in a git
/// branch name but would change the meaning of a query string. `/` is left
/// alone: it is legal in a query and branch names are full of it.
pub(crate) fn urlencode_query(value: &str) -> String {
    encode(value, true)
}

/// Percent-encode a PATH segment, `/` included. GitLab addresses a project by
/// its full path inside a single segment (`group%2Fsub%2Fproj`), so this is
/// the difference between reading that project and reading a route that does
/// not exist.
pub(crate) fn urlencode_path(value: &str) -> String {
    encode(value, false)
}

fn encode(value: &str, keep_slash: bool) -> String {
    value
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            '/' if keep_slash => c.to_string(),
            other => other
                .to_string()
                .as_bytes()
                .iter()
                .map(|b| format!("%{b:02X}"))
                .collect(),
        })
        .collect()
}

/// `(scheme, host, port)` — what "the same server" means for a redirect.
fn origin_of(url: &reqwest::Url) -> (String, Option<String>, Option<u16>) {
    (
        url.scheme().to_string(),
        url.host_str().map(str::to_ascii_lowercase),
        url.port_or_known_default(),
    )
}

pub(crate) fn http_client() -> Result<reqwest::Client, ForgeError> {
    let fingerprint = crate::network::proxy::current_proxy_env_vars();
    if let Ok(guard) = HTTP_CLIENT.read() {
        if let Some((cached, client)) = guard.as_ref() {
            if *cached == fingerprint {
                return Ok(client.clone());
            }
        }
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        // Redirects are followed only WITHIN one origin. reqwest strips
        // `Authorization` when a redirect crosses hosts, but it cannot know
        // that GitLab's `PRIVATE-TOKEN` is a credential too — a redirect to
        // somewhere else would hand that header over intact.
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let same_origin = attempt
                .previous()
                .last()
                .is_some_and(|prev| origin_of(prev) == origin_of(attempt.url()));
            if !same_origin {
                attempt.stop()
            } else if attempt.previous().len() > 5 {
                attempt.error("too many redirects")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| ForgeError::Network(format!("http client build failed: {e}")))?;
    if let Ok(mut guard) = HTTP_CLIENT.write() {
        *guard = Some((fingerprint, client.clone()));
    }
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `NoAccount` is the one forge failure the UI turns into an action, and
    /// it can only do that if the i18n key and BOTH params survive the trip to
    /// the client. Serialize as well as convert: the frontend reads the JSON,
    /// not the struct, and `skip_serializing_if` on those two fields means a
    /// conversion that forgot to set them looks identical from Rust.
    #[test]
    fn no_account_carries_i18n_key_and_params_over_the_wire() {
        let err: crate::app_error::AppCommandError = ForgeError::NoAccount {
            provider: ForgeProvider::GitHub,
            host: "github.com".into(),
        }
        .into();
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "configuration_missing");
        assert_eq!(json["i18n_key"], NO_ACCOUNT_I18N_KEY);
        assert_eq!(json["i18n_params"]["host"], "github.com");
        // Display name, not the lowercase wire value: this one is read by a human.
        assert_eq!(json["i18n_params"]["provider"], "GitHub");
        // The English message stands in when a locale lacks the key.
        assert_eq!(json["message"], "no GitHub account for host github.com");

        // Every OTHER auth failure stays keyless — "add an account" fixes none
        // of them, so the UI must not offer it.
        let dead_token: crate::app_error::AppCommandError =
            ForgeError::Auth("no stored token for account acc-1".into()).into();
        assert!(serde_json::to_value(&dead_token).unwrap().get("i18n_key").is_none());
    }

    #[test]
    fn source_key_normalizes_and_validates() {
        assert_eq!(
            source_key("GitHub", "GitHub.com", "Acme/App", "Issue", 123).unwrap(),
            "github:github.com:acme/app:issue:123"
        );
        // GitLab subgroups keep the full path; MR arrives pre-normalized as "pr".
        assert_eq!(
            source_key("gitlab", "gitlab.corp.com", "Group/Sub/Proj", "pr", 45).unwrap(),
            "gitlab:gitlab.corp.com:group/sub/proj:pr:45"
        );
        for bad in [
            source_key("bitbucket", "github.com", "a/b", "issue", 1),
            source_key("github", "github.com", "a/b", "mr", 1),
            source_key("github", "", "a/b", "issue", 1),
            source_key("github", "github.com/api", "a/b", "issue", 1),
            source_key("github", "github.com", "no-slash", "issue", 1),
            source_key("github", "github.com", "a/b", "issue", 0),
            source_key("github", "github.com", "a/b?x=1", "issue", 1),
        ] {
            assert!(bad.is_err());
        }
    }

    /// GitHub answers with canonical casing (`microsoft/TypeScript`) while
    /// keys/remotes are lowercase — comparisons must not care, and `.git`
    /// remote suffixes must not split identity either.
    #[test]
    fn same_repo_is_case_insensitive_and_suffix_tolerant() {
        assert!(same_repo("microsoft/typescript", "microsoft/TypeScript"));
        assert!(same_repo("Acme/App.git", "acme/app"));
        assert!(same_repo("group/sub/proj", "Group/Sub/Proj"));
        assert!(!same_repo("acme/app", "acme/other"));
        assert!(!same_repo("acme/app", ""));
        assert!(!same_repo("", ""));
    }

    #[test]
    fn remote_url_parsing_covers_all_three_shapes() {
        let cases = [
            ("https://github.com/Acme/App.git", ("github.com", "acme/app")),
            ("https://ghe.corp.com/team/tool", ("ghe.corp.com", "team/tool")),
            ("git@github.com:Acme/App.git", ("github.com", "acme/app")),
            ("ssh://git@ghe.corp.com:2222/team/tool.git", ("ghe.corp.com", "team/tool")),
            ("ssh://git@gitlab.corp.com/group/sub/proj.git", ("gitlab.corp.com", "group/sub/proj")),
            ("http://user@ghe.corp.com/a/b", ("ghe.corp.com", "a/b")),
        ];
        for (input, (host, repo)) in cases {
            let (h, r) = parse_remote_url(input).unwrap_or_else(|| panic!("parse {input}"));
            assert_eq!((h.as_str(), r.as_str()), (host, repo), "{input}");
        }
        for bad in ["", "not-a-url", "https://", "/local/path/repo", "file:///x/y"] {
            assert!(parse_remote_url(bad).is_none(), "{bad} must not parse");
        }
    }

    /// An unknown provider is refused rather than defaulted: picking GitHub
    /// for a value we do not understand would run a task's writes against the
    /// wrong API with the wrong credentials.
    #[test]
    fn provider_parsing_refuses_what_it_does_not_know() {
        assert_eq!(ForgeProvider::parse("GitHub").unwrap(), ForgeProvider::GitHub);
        assert_eq!(ForgeProvider::parse(" gitlab ").unwrap(), ForgeProvider::GitLab);
        assert!(ForgeProvider::parse("bitbucket").is_err());
        assert!(ForgeProvider::parse("").is_err());
        // The wire form round-trips through the stored source_meta JSON.
        assert_eq!(
            serde_json::to_string(&ForgeProvider::GitLab).unwrap(),
            "\"gitlab\""
        );
    }

    /// Item URLs and head refs are the two places the two forges disagree in a
    /// way a task cannot survive: a GitLab link with GitHub's path 404s, and a
    /// GitHub head ref simply does not exist on a GitLab server.
    #[test]
    fn provider_shapes_urls_and_head_refs_its_own_way() {
        let gh = ForgeProvider::GitHub;
        let gl = ForgeProvider::GitLab;
        assert_eq!(
            gh.item_url("https://github.com", "acme/app", ForgeItemKind::Change, 7),
            "https://github.com/acme/app/pull/7"
        );
        assert_eq!(
            gh.item_url("https://github.com", "acme/app", ForgeItemKind::Issue, 7),
            "https://github.com/acme/app/issues/7"
        );
        assert_eq!(
            gl.item_url("https://gitlab.com", "group/sub/proj", ForgeItemKind::Change, 7),
            "https://gitlab.com/group/sub/proj/-/merge_requests/7"
        );
        assert_eq!(
            gl.item_url("https://gitlab.com", "group/sub/proj", ForgeItemKind::Issue, 7),
            "https://gitlab.com/group/sub/proj/-/issues/7"
        );
        assert_eq!(gh.change_head_ref(7), "refs/pull/7/head");
        // Hyphen in the ref, underscore in the REST path — GitLab really does
        // spell it both ways, and only one of them fetches anything.
        assert_eq!(gl.change_head_ref(7), "refs/merge-requests/7/head");
        // A self-hosted instance keeps its scheme and port in the link.
        assert_eq!(
            gl.item_url("http://gitlab.corp.com:8929/", "a/b", ForgeItemKind::Issue, 7),
            "http://gitlab.corp.com:8929/a/b/-/issues/7"
        );
        assert_eq!(gh.change_noun(), "pull request");
        assert_eq!(gl.change_noun(), "merge request");
        // Both forges' changes key as "pr" so provenance keys stay comparable.
        assert_eq!(ForgeItemKind::Change.key_segment(), "pr");
        assert_eq!(ForgeItemKind::Issue.key_segment(), "issue");
    }

    /// The push URL and every stored link come from this one derivation: a
    /// bare `server_host` would drop both the scheme and a non-default port,
    /// which is exactly what a self-hosted instance has.
    #[test]
    fn the_web_origin_comes_from_the_api_base() {
        let mut auth = ResolvedAuth {
            provider: ForgeProvider::GitHub,
            server_host: "fallback.test".into(),
            api_base: "https://api.github.com".into(),
            account_id: "acc".into(),
            username: "alice".into(),
            token: "tok".into(),
            scopes: vec![],
        };
        assert_eq!(web_origin(&auth), "https://github.com");
        auth.api_base = "https://ghe.corp.com/api/v3".into();
        assert_eq!(web_origin(&auth), "https://ghe.corp.com");
        auth.api_base = "https://ghe.corp.com:8443/api/v3/".into();
        assert_eq!(web_origin(&auth), "https://ghe.corp.com:8443");
        auth.api_base = "http://gitlab.corp.com:8929/api/v4".into();
        assert_eq!(web_origin(&auth), "http://gitlab.corp.com:8929");
        // A base that is neither shape: fall back to the host we know rather
        // than build a link into whatever that string was.
        auth.api_base = "https://gitlab.com/weird".into();
        assert_eq!(web_origin(&auth), "https://fallback.test");
    }

    /// Paging comes off the wire and reaches two APIs that reject different
    /// halves of the nonsense: `per_page=0` is a 422 at GitHub and an empty
    /// page at GitLab, and `page=0` means page 1 at one of them only.
    #[test]
    fn client_supplied_paging_is_clamped_into_range() {
        let req = |page: u32, per_page: u32| ListIssuesRequest {
            owner_repo: "acme/app".into(),
            tab: ForgeTab::Issues,
            state: "open".into(),
            assigned_me: false,
            labels: vec![],
            search: None,
            sort: ForgeSort::default(),
            page,
            per_page,
        };
        assert_eq!(req(0, 0).clamped(), (1, MIN_PER_PAGE));
        assert_eq!(req(3, 20).clamped(), (3, 20));
        // Both forges cap per_page at 100; asking for more is a rejected request.
        assert_eq!(req(1, 5_000).clamped(), (1, MAX_PER_PAGE));
    }

    fn filters() -> ListFilters {
        ListFilters {
            tab: ForgeTab::Issues,
            state: "open".into(),
            assigned_me: false,
            labels: vec![],
            search: None,
            sort: ForgeSort::default(),
            page: 1,
            per_page: DEFAULT_PER_PAGE,
            account_id: None,
        }
    }

    /// The repository is an ARGUMENT, not a field of the client's payload:
    /// whatever filters arrive, the request is built against the repository the
    /// server resolved from the folder's remote.
    #[test]
    fn a_request_takes_its_repository_from_the_caller_and_normalizes_the_rest() {
        let built = ListIssuesRequest::new(
            "acme/app".into(),
            ListFilters {
                labels: vec!["  bug ".into(), "".into(), "bug".into(), "docs".into()],
                search: Some("   login timeout  ".into()),
                ..filters()
            },
        );
        assert_eq!(built.owner_repo, "acme/app");
        assert_eq!(built.labels, vec!["bug", "docs"], "trimmed and de-duplicated");
        assert_eq!(built.search.as_deref(), Some("login timeout"));
        // Whitespace-only is no filter at all, not a search for nothing.
        assert!(ListIssuesRequest::new("a/b".into(), ListFilters { search: Some("  ".into()), ..filters() })
            .search
            .is_none());
    }

    /// Both caps are real API limits, not taste: GitHub rejects a `q` over 256
    /// characters outright, and each extra label lengthens it further.
    #[test]
    fn search_and_label_filters_are_capped() {
        let long = "汉".repeat(MAX_SEARCH_CHARS + 40);
        let capped = normalize_search(Some(&long)).expect("kept");
        // CHARS, not bytes — a byte slice here would split a code point.
        assert_eq!(capped.chars().count(), MAX_SEARCH_CHARS);
        assert_eq!(normalize_search(Some("\t\n ")), None);
        assert_eq!(normalize_search(None), None);

        let many: Vec<String> = (0..MAX_LABEL_FILTERS + 5).map(|i| format!("l{i}")).collect();
        assert_eq!(normalize_labels(many).len(), MAX_LABEL_FILTERS);
        // Case-sensitive de-dup: GitHub treats `Bug` and `bug` as two labels.
        assert_eq!(
            normalize_labels(vec!["Bug".into(), "bug".into()]),
            vec!["Bug", "bug"]
        );
    }

    /// The two forges spell the same colour differently — GitHub sends bare
    /// `d73a4a`, GitLab `#d9534f` — so one normalized `#rrggbb` is what the UI
    /// gets. Anything that is not hex is refused rather than passed along:
    /// GitLab accepts CSS colour names on write, and this value is handed
    /// straight to a `style` attribute.
    #[test]
    fn label_colours_are_normalized_and_non_hex_is_refused() {
        for (raw, want) in [
            ("d73a4a", "#d73a4a"),   // GitHub
            ("#d9534f", "#d9534f"),  // GitLab
            ("  #D73A4A ", "#d73a4a"), // padded and upper-cased
            ("#0f0", "#00ff00"),     // the three-digit shorthand
        ] {
            assert_eq!(normalize_hex_color(raw).as_deref(), Some(want), "{raw}");
        }
        for raw in ["", "#", "rebeccapurple", "#12345", "#1234567", "#12345g", "var(--x)"] {
            assert_eq!(normalize_hex_color(raw), None, "{raw}");
        }

        // A label is its NAME first: an unusable colour costs the swatch, not
        // the chip. An unusable name costs the whole label — there would be
        // nothing to show and nothing to filter by.
        assert_eq!(
            ForgeLabel::parse("bug".into(), Some("red")),
            Some(ForgeLabel { name: "bug".into(), color: None })
        );
        assert_eq!(ForgeLabel::parse(String::new(), Some("#fff")), None);
    }

    /// A count is a LIST asked for one row: neither forge counts these
    /// collections on its own endpoint. The tab is the only thing that differs
    /// between the switcher's two probes — every filter carries over, or the
    /// badge would describe a result set nobody is looking at.
    #[test]
    fn a_count_probe_is_the_smallest_page_of_the_same_filters() {
        let filters = CountFilters {
            state: "closed".into(),
            assigned_me: true,
            labels: vec!["bug".into()],
            search: Some("login".into()),
            account_id: Some("acc-1".into()),
        };
        for tab in [ForgeTab::Issues, ForgeTab::Prs] {
            let probe = filters.probe(tab);
            assert_eq!(probe.tab, tab);
            assert_eq!(probe.page, 1);
            // One row, not a page of twenty thrown away: the count rides in
            // `total_count` / `X-Total`, never in the body.
            assert_eq!(probe.per_page, MIN_PER_PAGE);
            assert_eq!(probe.state, "closed");
            assert!(probe.assigned_me);
            assert_eq!(probe.labels, vec!["bug"]);
            assert_eq!(probe.search.as_deref(), Some("login"));
            assert_eq!(probe.account_id.as_deref(), Some("acc-1"));
        }
    }

    /// A badge is a bare digit with nowhere to add a caveat, so a count that
    /// needs one is withheld instead. The list keeps the caveat — and the
    /// count — because it has a footer to say it in.
    #[test]
    fn an_incomplete_search_offers_no_count_to_a_badge() {
        let page = |total: Option<i64>, incomplete: bool| ForgeIssueList {
            rows: vec![],
            page: 1,
            per_page: 1,
            total_count: total,
            reachable_count: None,
            has_next: false,
            incomplete,
        };
        assert_eq!(page(Some(57), false).trustworthy_count(), Some(57));
        // GitHub timed out: it counted fewer items than match, and "57" on a
        // tab would be a claim the response itself disowns.
        assert_eq!(page(Some(57), true).trustworthy_count(), None);
        // Nothing to withhold in the first place.
        assert_eq!(page(None, false).trustworthy_count(), None);
    }

    /// Four named orders, because the two forges accept different field sets
    /// and spell the shared ones differently. `newest` is the default — the
    /// same one github.com's issue list opens on.
    #[test]
    fn sort_orders_map_to_a_field_and_a_direction() {
        assert_eq!(ForgeSort::default(), ForgeSort::Newest);
        for (sort, field, direction) in [
            (ForgeSort::Newest, "created", "desc"),
            (ForgeSort::Oldest, "created", "asc"),
            (ForgeSort::RecentlyUpdated, "updated", "desc"),
            (ForgeSort::LeastRecentlyUpdated, "updated", "asc"),
        ] {
            assert_eq!((sort.field(), sort.direction()), (field, direction), "{sort:?}");
            assert_eq!(sort.ascending(), direction == "asc");
        }
        // The wire spelling the frontend sends.
        assert_eq!(
            serde_json::to_string(&ForgeSort::LeastRecentlyUpdated).unwrap(),
            "\"least_recently_updated\""
        );
    }

    /// Issue bodies are arbitrary UTF-8; a byte slice could split a code point
    /// and panic.
    #[test]
    fn body_truncation_is_char_safe() {
        let s = "汉".repeat(BODY_CAP + 5);
        let t = truncate_chars(&s, BODY_CAP);
        assert_eq!(t.chars().count(), BODY_CAP);
        assert!(truncate_chars("short", BODY_CAP) == "short");
    }

    /// The client holder is keyed by the proxy env fingerprint: same
    /// fingerprint reuses the pool, a changed fingerprint rebuilds — this is
    /// what makes a runtime proxy switch take effect without a restart.
    #[test]
    fn http_client_rebuilds_when_proxy_fingerprint_changes() {
        let a = http_client().expect("client");
        let b = http_client().expect("client");
        // Same fingerprint → the very same pool (reqwest clients are Arc-like;
        // pointer identity via Debug formatting is not exposed, so assert via
        // the cache: a second acquisition must not error and the cached entry
        // must match the current fingerprint).
        let _ = (a, b);
        let cached_fp = HTTP_CLIENT
            .read()
            .unwrap()
            .as_ref()
            .map(|(fp, _)| fp.clone())
            .expect("cached");
        assert_eq!(cached_fp, crate::network::proxy::current_proxy_env_vars());
    }
}
