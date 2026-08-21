//! GitLab REST v4 for the workbench and the delivery path.
//!
//! GitLab is not "GitHub with different words" in the four places that matter
//! here, and each of them is a silent-wrong-answer if you assume otherwise:
//!
//! 1. **Issues and merge requests are separate collections.** GitHub serves
//!    both from `/issues` and splits on a `pull_request` key; here each tab is
//!    its own endpoint, and so is each comment target (`/issues/{iid}/notes`
//!    vs `/merge_requests/{iid}/notes`).
//! 2. **The project is a path, not `{owner}/{repo}`.** Subgroups nest
//!    arbitrarily deep, and the whole path goes into ONE percent-encoded path
//!    segment (`group%2Fsub%2Fproj`).
//! 3. **`merged` is a state, not a flag.** `state=closed` excludes merged
//!    merge requests, so the workbench's "closed" tab asks for everything and
//!    filters locally — otherwise a merged merge request would simply vanish
//!    from a list where GitHub shows it.
//! 4. **Numbers are `iid`, not `id`.** `id` is globally unique and appears in
//!    no URL a human ever sees; addressing by it silently reads another
//!    project's work item.
//!
//! Pagination is offset-based (`page` + `per_page`), and the totals are the
//! one place it differs from the GitHub client: `X-Total` / `X-Total-Pages`
//! are OPTIONAL — GitLab omits them past 10,000 rows on purpose — so
//! `X-Next-Page` is the only signal always present. Rate-limit classification
//! is shared with the GitHub client.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use serde::Deserialize;

use super::auth::ResolvedAuth;
use super::deliver::{ForgePr, NewPullRequest};
use super::{
    truncate_chars, urlencode_path, urlencode_query, validate_state_filter, web_origin, ForgeError,
    ForgeIssueList, ForgeIssueRow, ForgeItemKind, ForgeLabel, ForgeLabelList, ForgeSort,
    ListIssuesRequest, BODY_CAP, LABEL_PAGE_SIZE,
};

// ── reads ───────────────────────────────────────────────────────────────────

pub async fn list_issues(
    auth: &ResolvedAuth,
    req: &ListIssuesRequest,
) -> Result<ForgeIssueList, ForgeError> {
    let project = project_ref(&req.owner_repo)?;
    validate_state_filter(&req.state)?;

    let (page, per_page) = req.clamped();

    let collection = match req.tab {
        super::ForgeTab::Issues => "issues",
        super::ForgeTab::Prs => "merge_requests",
    };
    // `with_labels_details=true` turns each entry of `labels` from a bare name
    // into an object carrying the label's colour — the chip the workbench draws
    // is the project's own, as it is on GitHub. Instances that predate the
    // parameter ignore it and keep sending strings, which `RawItemLabel` reads
    // just as happily.
    let mut url = format!(
        "{}/projects/{project}/{collection}?state={}&order_by={}&sort={}\
         &with_labels_details=true&page={page}&per_page={per_page}",
        auth.api_base,
        wire_state(req.tab, &req.state),
        order_by(req.sort),
        req.sort.direction()
    );
    if req.assigned_me {
        // `assignee_username` takes the literal login; there is no `@me`
        // shorthand on the collection endpoints.
        let login = current_login(auth).await?;
        url.push_str(&format!("&assignee_username={}", urlencode_query(&login)));
    }
    if !req.labels.is_empty() {
        // Comma-joined, which is lossless here: GitLab forbids commas in label
        // titles, so no name can be split by this.
        url.push_str(&format!("&labels={}", urlencode_query(&req.labels.join(","))));
    }
    if let Some(text) = req.search.as_deref() {
        // `search` is plain text on both collections, so unlike GitHub's `q`
        // there is no query syntax to strip. `in` is title+description by
        // default and is stated anyway: the box promises that scope, and a
        // promise resting on somebody else's default is one release away from
        // being wrong.
        url.push_str(&format!("&search={}&in=title,description", urlencode_query(text)));
    }

    let response = api_get(auth, &url).await?;
    let total_count = header_i64(response.headers(), "x-total");
    // Empty means "no page after this one" — the one pagination signal GitLab
    // always sends, even where it declines to count.
    let has_next = header_str(response.headers(), "x-next-page")
        .is_some_and(|v| !v.trim().is_empty());
    let raw: Vec<RawItem> = response
        .json()
        .await
        .map_err(|e| ForgeError::Network(format!("bad list payload: {e}")))?;

    let is_pr = req.tab == super::ForgeTab::Prs;
    // Whether this query is narrowed AFTER it arrives — structural, not
    // data-dependent: a page that happens to hold no open rows is still part
    // of a result set whose count includes them elsewhere.
    let filters_locally = wire_state(req.tab, &req.state) == "all" && req.state != "all";
    let rows = raw
        .into_iter()
        .filter(|item| keeps(&req.state, &item.state))
        .map(|item| ForgeIssueRow {
            is_pr,
            number: item.iid,
            title: item.title,
            body: item.description.map(|b| truncate_chars(&b, BODY_CAP)),
            state: display_state(&item.state),
            draft: is_pr && (item.draft || item.work_in_progress),
            labels: item.labels.into_iter().filter_map(RawItemLabel::into_label).collect(),
            author: item.author.map(|a| a.username),
            updated_at: item.updated_at,
            html_url: item.web_url,
            comments: item.user_notes_count,
        })
        .collect();

    Ok(ForgeIssueList {
        rows,
        page,
        per_page,
        // Withheld rather than wrong. Two ways the count stops being true:
        // GitLab omits `X-Total` past 10k rows (documented, for performance),
        // and the closed-merge-request query above asks for `state=all` and
        // filters here — so its total counts open rows the user cannot see.
        // `None` is the signal the UI degrades to previous/next on.
        total_count: total_count.filter(|_| !filters_locally),
        // GitLab paginates the whole collection — no equivalent of GitHub
        // search's first-thousand ceiling, so every match is reachable.
        reachable_count: None,
        has_next,
        // GitLab has no partial-result flag; only GitHub search does.
        incomplete: false,
    })
}

/// GitLab's spelling of the sort field. It is the SAME two fields GitHub
/// offers, suffixed — and the only two both collections (issues and merge
/// requests) accept, which is what makes the workbench's four orders portable.
fn order_by(sort: ForgeSort) -> &'static str {
    match sort.field() {
        "updated" => "updated_at",
        _ => "created_at",
    }
}

/// The project's labels, for the workbench's label filter.
///
/// `with_counts=false` on purpose: the counts are an extra aggregation per
/// label on the server and nothing here shows them.
pub async fn list_labels(
    auth: &ResolvedAuth,
    owner_repo: &str,
) -> Result<ForgeLabelList, ForgeError> {
    let project = project_ref(owner_repo)?;
    let url = format!(
        "{}/projects/{project}/labels?per_page={LABEL_PAGE_SIZE}&with_counts=false",
        auth.api_base
    );
    #[derive(Deserialize)]
    struct RawProjectLabel {
        #[serde(default)]
        name: String,
        /// `#rrggbb` here, unlike GitHub's bare digits.
        #[serde(default)]
        color: Option<String>,
    }
    let raw: Vec<RawProjectLabel> = api_get(auth, &url)
        .await?
        .json()
        .await
        .map_err(|e| ForgeError::Network(format!("bad labels payload: {e}")))?;
    let truncated = raw.len() >= LABEL_PAGE_SIZE;
    Ok(ForgeLabelList {
        labels: raw
            .into_iter()
            .filter_map(|l| ForgeLabel::parse(l.name, l.color.as_deref()))
            .collect(),
        truncated,
    })
}

fn header_str<'h>(headers: &'h reqwest::header::HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name)?.to_str().ok()
}

fn header_i64(headers: &reqwest::header::HeaderMap, name: &str) -> Option<i64> {
    header_str(headers, name)?.trim().parse().ok()
}

/// One merge request by `iid` — what turns "!12" into something checkoutable.
///
/// A merge request opened from a fork reports only the numeric id of its
/// source project, so the fork's path is resolved here (one extra request, and
/// only for forks) — that name goes straight into the refusal the user reads,
/// and "project 4711" would be a worse answer than the truth.
pub async fn get_merge_request(
    auth: &ResolvedAuth,
    owner_repo: &str,
    iid: i64,
) -> Result<ForgePr, ForgeError> {
    let project = project_ref(owner_repo)?;
    if iid <= 0 {
        return Err(ForgeError::Invalid(format!("bad work item number: {iid}")));
    }
    let url = format!("{}/projects/{project}/merge_requests/{iid}", auth.api_base);
    let raw: RawMergeRequest = api_get(auth, &url)
        .await?
        .json()
        .await
        .map_err(|e| ForgeError::Network(format!("bad merge request payload: {e}")))?;
    let foreign_project = raw.source_project_id.filter(|src| Some(*src) != raw.target_project_id);
    let mut pr = map_merge_request(raw, owner_repo);
    if let Some(id) = foreign_project {
        if let Some(path) = project_path(auth, id).await {
            pr.head_repo = path;
        }
    }
    Ok(pr)
}

/// Merge requests whose source branch is `source_branch`, in ANY state — a
/// merged or closed one is exactly what recovery must be able to see.
///
/// Fork sources keep their placeholder head repository here: this list only
/// ever feeds the four-way match, which refuses anything that is not the
/// source project anyway, so resolving each fork's real path would be a
/// request per row spent on rows that cannot be adopted.
pub async fn find_merge_requests(
    auth: &ResolvedAuth,
    owner_repo: &str,
    source_branch: &str,
) -> Result<Vec<ForgePr>, ForgeError> {
    let project = project_ref(owner_repo)?;
    let url = format!(
        "{}/projects/{project}/merge_requests?source_branch={}&state=all&per_page=100",
        auth.api_base,
        urlencode_query(source_branch)
    );
    let raw: Vec<RawMergeRequest> = api_get(auth, &url)
        .await?
        .json()
        .await
        .map_err(|e| ForgeError::Network(format!("bad merge requests payload: {e}")))?;
    Ok(raw
        .into_iter()
        .map(|m| map_merge_request(m, owner_repo))
        .collect())
}

// ── writes ──────────────────────────────────────────────────────────────────

/// `POST /projects/{p}/merge_requests`.
///
/// GitLab has no `draft` parameter: a draft IS a title starting with `Draft:`,
/// which is also how its UI toggles the state. Prefixing is therefore the
/// supported way to open one, not a workaround.
pub async fn create_merge_request(
    auth: &ResolvedAuth,
    owner_repo: &str,
    req: &NewPullRequest<'_>,
) -> Result<ForgePr, ForgeError> {
    let project = project_ref(owner_repo)?;
    let url = format!("{}/projects/{project}/merge_requests", auth.api_base);
    let title = if req.draft && !is_draft_title(req.title) {
        format!("Draft: {}", req.title)
    } else {
        req.title.to_string()
    };
    let body = serde_json::json!({
        "source_branch": req.head,
        "target_branch": req.base,
        "title": title,
        "description": req.body,
    });
    let raw: RawMergeRequest = api_post(auth, &url, &body)
        .await?
        .json()
        .await
        .map_err(|e| ForgeError::Network(format!("bad merge request payload: {e}")))?;
    Ok(map_merge_request(raw, owner_repo))
}

/// `POST /projects/{p}/{issues|merge_requests}/{iid}/notes`.
///
/// The collection is part of the path here — unlike GitHub, where a pull
/// request is an issue and one endpoint serves both. Posting an issue note to
/// the merge-request collection (or the reverse) is a 404 against a number
/// that may well exist in the other collection.
pub async fn create_note(
    auth: &ResolvedAuth,
    owner_repo: &str,
    kind: ForgeItemKind,
    iid: i64,
    body: &str,
) -> Result<String, ForgeError> {
    let repo = super::normalize_repo(owner_repo)
        .ok_or_else(|| ForgeError::Invalid(format!("bad repository path: {owner_repo}")))?;
    let project = project_ref(owner_repo)?;
    if iid <= 0 {
        return Err(ForgeError::Invalid(format!("bad work item number: {iid}")));
    }
    let collection = collection_of(kind);
    let url = format!("{}/projects/{project}/{collection}/{iid}/notes", auth.api_base);
    #[derive(Deserialize)]
    struct RawNote {
        #[serde(default)]
        id: i64,
    }
    let created: RawNote = api_post(auth, &url, &serde_json::json!({ "body": body }))
        .await?
        .json()
        .await
        .map_err(|e| ForgeError::Network(format!("bad note payload: {e}")))?;
    // Notes carry no web URL of their own; the anchor on the item's page is
    // the link a human can actually follow.
    Ok(format!(
        "{}/{repo}/-/{collection}/{iid}#note_{}",
        web_origin(auth),
        created.id
    ))
}

// ── plumbing ────────────────────────────────────────────────────────────────

/// `GET {api_base}/user` → username, cached per `(api_base, account)`.
static LOGIN_CACHE: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

async fn current_login(auth: &ResolvedAuth) -> Result<String, ForgeError> {
    let cache_key = format!("{}\n{}", auth.api_base, auth.account_id);
    if let Some(hit) = LOGIN_CACHE.read().ok().and_then(|c| c.get(&cache_key).cloned()) {
        return Ok(hit);
    }
    #[derive(Deserialize)]
    struct User {
        username: String,
    }
    let user: User = api_get(auth, &format!("{}/user", auth.api_base))
        .await?
        .json()
        .await
        .map_err(|e| ForgeError::Network(format!("bad /user payload: {e}")))?;
    if let Ok(mut cache) = LOGIN_CACHE.write() {
        cache.insert(cache_key, user.username.clone());
    }
    Ok(user.username)
}

/// `path_with_namespace` of a project id. Best-effort: it only ever improves
/// the wording of a refusal that has already been decided.
async fn project_path(auth: &ResolvedAuth, project_id: i64) -> Option<String> {
    #[derive(Deserialize)]
    struct RawProject {
        #[serde(default)]
        path_with_namespace: String,
    }
    let url = format!("{}/projects/{project_id}", auth.api_base);
    let project: RawProject = api_get(auth, &url).await.ok()?.json().await.ok()?;
    Some(project.path_with_namespace).filter(|p| !p.is_empty())
}

pub(crate) async fn api_get(
    auth: &ResolvedAuth,
    url: &str,
) -> Result<reqwest::Response, ForgeError> {
    let response = super::http_client()?
        .get(url)
        .header("PRIVATE-TOKEN", &auth.token)
        .header("User-Agent", "codeg")
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| ForgeError::Network(e.to_string()))?;
    finish(response).await
}

pub(crate) async fn api_post(
    auth: &ResolvedAuth,
    url: &str,
    body: &serde_json::Value,
) -> Result<reqwest::Response, ForgeError> {
    let response = super::http_client()?
        .post(url)
        .header("PRIVATE-TOKEN", &auth.token)
        .header("User-Agent", "codeg")
        .header("Accept", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| ForgeError::Network(e.to_string()))?;
    finish(response).await
}

/// Success through, everything else classified. GitLab signals its rate limit
/// with 429 (and `Retry-After`), never with the 403-plus-quota-header shape
/// GitHub uses — so a 403 here really is about the credential.
async fn finish(response: reqwest::Response) -> Result<reqwest::Response, ForgeError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    Err(match status {
        429 => ForgeError::RateLimited { retry_after },
        401 | 403 => ForgeError::Auth(format!("GitLab returned {status}")),
        404 => ForgeError::NotFound,
        _ => {
            let message = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect();
            ForgeError::Api { status, message }
        }
    })
}

/// The project as ONE percent-encoded path segment. `normalize_repo` runs
/// first, so what gets encoded has already been checked for URL
/// metacharacters — the encoding is for the slashes of a subgroup path, not a
/// substitute for validation.
fn project_ref(owner_repo: &str) -> Result<String, ForgeError> {
    let repo = super::normalize_repo(owner_repo)
        .ok_or_else(|| ForgeError::Invalid(format!("bad repository path: {owner_repo}")))?;
    Ok(urlencode_path(&repo))
}

fn collection_of(kind: ForgeItemKind) -> &'static str {
    match kind {
        ForgeItemKind::Issue => "issues",
        ForgeItemKind::Change => "merge_requests",
    }
}

/// Our state filter in GitLab's vocabulary. "closed" asks for everything on
/// the merge-request collection because GitLab's own `closed` excludes merged
/// ones — see [`keeps`] for the other half.
fn wire_state(tab: super::ForgeTab, state: &str) -> &'static str {
    match (tab, state) {
        (_, "open") => "opened",
        (super::ForgeTab::Prs, "closed") => "all",
        (_, "closed") => "closed",
        _ => "all",
    }
}

/// Whether a row survives the filter the API could not fully express. Mirrors
/// [`display_state`]: `locked` DISPLAYS as open, so the closed tab must drop it
/// too — otherwise the merged-inclusive query leaks a row the icon then draws
/// as open into a list of closed ones.
fn keeps(requested: &str, actual: &str) -> bool {
    match requested {
        "closed" => !matches!(actual, "opened" | "locked"),
        _ => true,
    }
}

/// GitLab's `opened`/`locked`/`merged`/`closed` in the three words the rest of
/// codeg (and the workbench row's icon) understands. `merged` survives as
/// itself: it is a real state here, unlike GitHub where it has to be inferred.
fn display_state(state: &str) -> String {
    match state {
        "opened" | "locked" => "open".to_string(),
        "merged" => "merged".to_string(),
        _ => "closed".to_string(),
    }
}

/// Whether a title already declares itself a draft, so it is not prefixed
/// twice. `get(..6)` rather than `[..6]`: a title is arbitrary user text, and
/// slicing bytes through the middle of a code point PANICS — six bytes lands
/// mid-character on plenty of real titles.
fn is_draft_title(title: &str) -> bool {
    title
        .trim_start()
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("draft:"))
}

fn map_merge_request(raw: RawMergeRequest, owner_repo: &str) -> ForgePr {
    let same_project = match (raw.source_project_id, raw.target_project_id) {
        (Some(src), Some(dst)) => src == dst,
        // A payload that does not say cannot be shown to be the source
        // project, and "unknown" must not read as "ours".
        _ => false,
    };
    ForgePr {
        number: raw.iid,
        html_url: raw.web_url,
        state: display_state(&raw.state),
        merged: raw.state == "merged",
        // `diff_refs` is the precise head of the diff under review and is
        // present on the single-merge-request payload; the list payload only
        // carries `sha`, which is the same commit.
        head_sha: raw
            .diff_refs
            .and_then(|d| d.head_sha)
            .filter(|s| !s.is_empty())
            .or(raw.sha)
            .unwrap_or_default(),
        head_ref: raw.source_branch,
        head_repo: if same_project {
            owner_repo.to_string()
        } else {
            // Deliberately not a repository path: `same_repo` rejects it, so
            // every gate reads this as "somewhere else" without having to
            // spend a request to learn the fork's name.
            format!("project-{}", raw.source_project_id.unwrap_or_default())
        },
        base_ref: raw.target_branch,
    }
}

#[derive(Debug, Deserialize)]
struct RawItem {
    iid: i64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    state: String,
    /// Merge requests only. GitLab renamed `work_in_progress` to `draft`; old
    /// self-managed instances still send only the former, so both are read.
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    work_in_progress: bool,
    #[serde(default)]
    labels: Vec<RawItemLabel>,
    #[serde(default)]
    author: Option<RawUser>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    web_url: String,
    /// Human comments. GitLab's own name for it: `notes` also covers the
    /// system events ("changed the milestone"), which nobody wants counted.
    #[serde(default)]
    user_notes_count: i64,
}

/// A label on a collection item, in either shape GitLab may send it.
///
/// With `with_labels_details=true` (which the list URL asks for) each entry is
/// an object carrying the colour; without it — and on any instance old enough
/// to ignore the parameter — it is the bare name. Reading both is what keeps a
/// self-managed GitLab listing at all: a hard-coded object shape would turn
/// every row on such an instance into a deserialization failure.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawItemLabel {
    Name(String),
    Detail {
        #[serde(default)]
        name: String,
        #[serde(default)]
        color: Option<String>,
    },
}

impl RawItemLabel {
    fn into_label(self) -> Option<ForgeLabel> {
        match self {
            Self::Name(name) => ForgeLabel::parse(name, None),
            Self::Detail { name, color } => ForgeLabel::parse(name, color.as_deref()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawUser {
    #[serde(default)]
    username: String,
}

#[derive(Debug, Deserialize)]
struct RawMergeRequest {
    iid: i64,
    #[serde(default)]
    web_url: String,
    /// `opened` | `locked` | `merged` | `closed`.
    #[serde(default)]
    state: String,
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    diff_refs: Option<RawDiffRefs>,
    #[serde(default)]
    source_branch: String,
    #[serde(default)]
    target_branch: String,
    #[serde(default)]
    source_project_id: Option<i64>,
    #[serde(default)]
    target_project_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RawDiffRefs {
    #[serde(default)]
    head_sha: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::ForgeProvider;
    use axum::extract::Query;
    use axum::routing::{get, post};
    use axum::Json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn auth_for(api_base: String) -> ResolvedAuth {
        ResolvedAuth {
            provider: ForgeProvider::GitLab,
            server_host: "gitlab.test".into(),
            api_base,
            account_id: "acc-test".into(),
            username: "alice".into(),
            token: "tok-test".into(),
            scopes: vec!["api".into()],
        }
    }

    fn item_json(iid: i64, state: &str) -> serde_json::Value {
        serde_json::json!({
            "iid": iid,
            "title": format!("item {iid}"),
            "description": format!("body {iid}"),
            "state": state,
            // What `with_labels_details=true` returns — plus one bare name, the
            // shape an instance that ignores the parameter still sends.
            "labels": [{ "name": "bug", "color": "#D9534F" }, "legacy", { "name": "" }],
            "author": { "username": "alice" },
            "updated_at": "2026-08-18T00:00:00Z",
            "web_url": format!("https://gitlab.test/group/sub/proj/-/issues/{iid}"),
            "user_notes_count": iid,
        })
    }

    fn mr_json(iid: i64, state: &str, source_project: i64) -> serde_json::Value {
        serde_json::json!({
            "iid": iid,
            "web_url": format!("https://gitlab.test/group/sub/proj/-/merge_requests/{iid}"),
            "state": state,
            "sha": "abc123",
            "source_branch": "feature",
            "target_branch": "main",
            "source_project_id": source_project,
            "target_project_id": 1,
        })
    }

    /// `(api_base, MR-create bodies, note bodies, /user hits, last list query)`.
    /// The last element is what the collection endpoint was actually asked for,
    /// so a test can assert the URL rather than infer it.
    #[allow(clippy::type_complexity)]
    async fn mock_api() -> (
        String,
        Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>,
        Arc<AtomicUsize>,
        Arc<RwLock<HashMap<String, String>>>,
    ) {
        let creates: Arc<std::sync::Mutex<Vec<serde_json::Value>>> = Default::default();
        let notes: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>> = Default::default();
        let user_hits = Arc::new(AtomicUsize::new(0));
        let last_query: Arc<RwLock<HashMap<String, String>>> = Default::default();
        let seen = creates.clone();
        let issue_notes = notes.clone();
        let mr_notes = notes.clone();
        let hits = user_hits.clone();
        let issue_query = last_query.clone();
        let mr_query = last_query.clone();
        // The project path arrives percent-encoded, so it is ONE segment.
        let app = axum::Router::new()
            .route(
                "/projects/group%2Fsub%2Fproj/issues",
                get(move |Query(q): Query<HashMap<String, String>>| async move {
                    if let Ok(mut slot) = issue_query.write() {
                        *slot = q.clone();
                    }
                    let page: u32 =
                        q.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
                    let mut headers = axum::http::HeaderMap::new();
                    // 3 matches over 2 pages — the shape GitLab sends when it
                    // is willing to count.
                    headers.insert("x-total", "3".parse().unwrap());
                    headers.insert("x-total-pages", "2".parse().unwrap());
                    headers.insert(
                        "x-next-page",
                        if page < 2 { "2" } else { "" }.parse().unwrap(),
                    );
                    let rows = if q.get("assignee_username").map(String::as_str) == Some("alice") {
                        vec![item_json(9, "opened")]
                    } else if page >= 2 {
                        vec![item_json(3, "opened")]
                    } else {
                        vec![item_json(1, "opened")]
                    };
                    (headers, Json(serde_json::Value::Array(rows)))
                }),
            )
            .route(
                "/projects/group%2Fsub%2Fproj/merge_requests",
                get(move |Query(q): Query<HashMap<String, String>>| async move {
                    if let Ok(mut slot) = mr_query.write() {
                        *slot = q.clone();
                    }
                    let mut headers = axum::http::HeaderMap::new();
                    // A count IS sent — the client must still refuse to trust
                    // it for the locally-filtered "closed" query.
                    headers.insert("x-total", "3".parse().unwrap());
                    headers.insert("x-next-page", "".parse().unwrap());
                    let rows = match q.get("source_branch").map(String::as_str) {
                        Some("feature") => vec![mr_json(4, "opened", 1)],
                        Some(_) => vec![],
                        // The tab listing: `state=all` is what the "closed"
                        // filter asks for, and it comes back merged + open.
                        // A draft rides along so the row mapping is covered.
                        None => {
                            let mut draft = mr_json(6, "opened", 1);
                            draft["draft"] = serde_json::json!(true);
                            vec![
                                mr_json(5, "merged", 1),
                                draft,
                                mr_json(7, "closed", 1),
                                mr_json(11, "locked", 1),
                            ]
                        }
                    };
                    (headers, Json(serde_json::Value::Array(rows)))
                })
                .post(move |Json(body): Json<serde_json::Value>| {
                    seen.lock().unwrap().push(body);
                    async { Json(mr_json(8, "opened", 1)) }
                }),
            )
            .route(
                "/projects/group%2Fsub%2Fproj/merge_requests/4",
                get(|| async {
                    let mut mr = mr_json(4, "opened", 1);
                    mr["diff_refs"] = serde_json::json!({ "head_sha": "deadbee" });
                    Json(mr)
                }),
            )
            .route(
                "/projects/group%2Fsub%2Fproj/merge_requests/9",
                get(|| async { Json(mr_json(9, "opened", 42)) }),
            )
            .route(
                "/projects/42",
                get(|| async {
                    Json(serde_json::json!({ "path_with_namespace": "contributor/proj" }))
                }),
            )
            .route(
                "/projects/group%2Fsub%2Fproj/issues/7/notes",
                post(move |Json(body): Json<serde_json::Value>| {
                    issue_notes.lock().unwrap().push(("issues".to_string(), body));
                    async { Json(serde_json::json!({ "id": 55 })) }
                }),
            )
            .route(
                "/projects/group%2Fsub%2Fproj/merge_requests/7/notes",
                post(move |Json(body): Json<serde_json::Value>| {
                    mr_notes.lock().unwrap().push(("merge_requests".to_string(), body));
                    async { Json(serde_json::json!({ "id": 66 })) }
                }),
            )
            .route(
                "/user",
                get(move || {
                    hits.fetch_add(1, Ordering::SeqCst);
                    async { Json(serde_json::json!({ "username": "alice" })) }
                }),
            )
            .route(
                "/projects/group%2Fsub%2Fproj/labels",
                get(|Query(q): Query<HashMap<String, String>>| async move {
                    // Counts are an extra per-label aggregation server-side and
                    // nothing here shows them.
                    assert_eq!(q.get("with_counts").map(String::as_str), Some("false"));
                    Json(serde_json::json!([
                        { "name": "bug", "color": "#D9534F" },
                        // GitLab accepts CSS colour names when a label is
                        // written, so a stored colour need not be hex at all.
                        { "name": "help wanted", "color": "rebeccapurple" },
                        { "name": "" },
                    ]))
                }),
            )
            .route(
                "/limited/projects/group%2Fsub%2Fproj/issues",
                get(|| async {
                    let mut headers = axum::http::HeaderMap::new();
                    headers.insert("retry-after", "17".parse().unwrap());
                    (
                        axum::http::StatusCode::TOO_MANY_REQUESTS,
                        headers,
                        Json(serde_json::json!({ "message": "slow down" })),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), creates, notes, user_hits, last_query)
    }

    fn req(tab: super::super::ForgeTab, state: &str) -> ListIssuesRequest {
        ListIssuesRequest {
            owner_repo: "Group/Sub/Proj".into(),
            tab,
            state: state.into(),
            assigned_me: false,
            labels: vec![],
            search: None,
            sort: ForgeSort::default(),
            page: 1,
            per_page: 20,
        }
    }

    /// A subgroup path is ONE percent-encoded segment; anything else addresses
    /// a route that does not exist. The row shape (iid, description, GitLab's
    /// `opened`) is normalized to what the workbench renders.
    #[tokio::test]
    async fn issues_come_from_the_encoded_project_path() {
        let (api_base, _, _, _, _) = mock_api().await;
        let auth = auth_for(api_base.clone());
        let list = list_issues(&auth, &req(super::super::ForgeTab::Issues, "open"))
            .await
            .expect("list");
        assert_eq!(list.rows.len(), 1);
        let row = &list.rows[0];
        assert_eq!((row.number, row.state.as_str()), (1, "open"));
        assert_eq!(row.body.as_deref(), Some("body 1"));
        // The empty label is dropped; a detailed one keeps its colour and a
        // bare one still lists, just without a swatch.
        assert_eq!(
            row.labels,
            vec![
                ForgeLabel { name: "bug".into(), color: Some("#d9534f".into()) },
                ForgeLabel { name: "legacy".into(), color: None },
            ]
        );
        assert_eq!(row.author.as_deref(), Some("alice"));
        assert!(!row.is_pr);

        // Offset pagination: `X-Total` is the count and `X-Next-Page` is what
        // says whether another page exists.
        assert_eq!((list.page, list.per_page), (1, 20));
        assert_eq!(list.total_count, Some(3));
        assert!(list.has_next);

        let page2 = list_issues(
            &auth,
            &ListIssuesRequest { page: 2, ..req(super::super::ForgeTab::Issues, "open") },
        )
        .await
        .expect("page 2");
        assert_eq!(page2.rows[0].number, 3);
        // Empty `X-Next-Page` is GitLab's end-of-list, not a missing header.
        assert!(!page2.has_next);
    }

    /// GitLab's `state=closed` excludes merged merge requests. The workbench's
    /// closed tab has to show them (GitHub's does), so the query asks for
    /// everything and the open ones are dropped here.
    ///
    /// That local narrowing is exactly why this query must report NO total: the
    /// `X-Total` the server sends counts the open rows the user cannot see, so
    /// page numbers built from it would be wrong. `locked` displays as open, so
    /// it is dropped alongside `opened` — otherwise the closed tab would show a
    /// row the icon then draws green.
    #[tokio::test]
    async fn the_closed_tab_shows_merged_and_withholds_the_untrustworthy_total() {
        let (api_base, _, _, _, _) = mock_api().await;
        let auth = auth_for(api_base);
        let list = list_issues(&auth, &req(super::super::ForgeTab::Prs, "closed"))
            .await
            .expect("list");
        assert_eq!(
            list.rows.iter().map(|r| r.number).collect::<Vec<_>>(),
            vec![5, 7],
            "merged and closed, but neither the open nor the locked one"
        );
        assert!(list.rows.iter().all(|r| r.is_pr));
        // `merged` survives as itself now — the icon draws it differently.
        assert_eq!(list.rows[0].state, "merged");
        assert_eq!(list.rows[1].state, "closed");
        assert_eq!(
            list.total_count, None,
            "a count that includes rows we filtered out must not be shown"
        );
        assert_eq!(wire_state(super::super::ForgeTab::Prs, "closed"), "all");
        // Issues have no merged state, so theirs stays a plain closed query —
        // nothing is filtered locally, so its count IS trustworthy.
        assert_eq!(wire_state(super::super::ForgeTab::Issues, "closed"), "closed");
        let issues = list_issues(&auth, &req(super::super::ForgeTab::Issues, "closed"))
            .await
            .expect("list");
        assert_eq!(issues.total_count, Some(3));
    }

    /// The open tab is a plain `state=opened` query, so its count is honest —
    /// and a draft merge request has to arrive marked as one.
    #[tokio::test]
    async fn the_open_tab_counts_and_carries_the_draft_flag() {
        let (api_base, _, _, _, _) = mock_api().await;
        let auth = auth_for(api_base);
        let list = list_issues(&auth, &req(super::super::ForgeTab::Prs, "open"))
            .await
            .expect("list");
        assert_eq!(list.total_count, Some(3), "no local filtering on this one");
        // GitLab paginates the whole collection: nothing is out of reach, so
        // the footer's page numbers come straight from the total. Only GitHub
        // search has a ceiling to declare.
        assert_eq!(list.reachable_count, None);
        assert_eq!(list.trustworthy_count(), Some(3));
        let draft = list.rows.iter().find(|r| r.number == 6).expect("draft row");
        assert!(draft.draft && draft.state == "open");
        assert!(
            list.rows.iter().filter(|r| r.number != 6).all(|r| !r.draft),
            "draft must not spill onto the other rows"
        );
    }

    /// Search text, labels and the sort order all reach the collection URL.
    /// Unlike GitHub there is no query syntax to strip — `search` is plain text
    /// over title and description — but the params still have to be there.
    #[tokio::test]
    async fn text_label_and_sort_filters_reach_the_url() {
        let (api_base, _, _, _, last) = mock_api().await;
        let auth = auth_for(api_base);
        list_issues(
            &auth,
            &ListIssuesRequest {
                labels: vec!["bug".into(), "help wanted".into()],
                search: Some("login timeout".into()),
                sort: ForgeSort::LeastRecentlyUpdated,
                ..req(super::super::ForgeTab::Issues, "open")
            },
        )
        .await
        .expect("list");
        let sent = last.read().unwrap().clone();
        // Comma-joined: GitLab forbids commas in label titles, so nothing splits.
        assert_eq!(sent.get("labels").map(String::as_str), Some("bug,help wanted"));
        assert_eq!(sent.get("search").map(String::as_str), Some("login timeout"));
        // The scope the box promises, stated rather than inherited: it happens
        // to be GitLab's default too, and a promise resting on somebody else's
        // default is one release away from being wrong.
        assert_eq!(sent.get("in").map(String::as_str), Some("title,description"));
        assert_eq!(sent.get("order_by").map(String::as_str), Some("updated_at"));
        assert_eq!(sent.get("sort").map(String::as_str), Some("asc"));
        // Without this the rows come back with bare label NAMES and every chip
        // in the workbench is grey.
        assert_eq!(sent.get("with_labels_details").map(String::as_str), Some("true"));

        // The default order, and GitLab's spelling of GitHub's `created`.
        list_issues(&auth, &req(super::super::ForgeTab::Issues, "open"))
            .await
            .expect("list");
        let plain = last.read().unwrap().clone();
        assert_eq!(plain.get("order_by").map(String::as_str), Some("created_at"));
        assert_eq!(plain.get("sort").map(String::as_str), Some("desc"));
        // Absent filters are absent params, not empty ones — `labels=` matches
        // items with no labels at all on some GitLab versions. `in` belongs to
        // the text and goes with it.
        assert!(
            !plain.contains_key("labels")
                && !plain.contains_key("search")
                && !plain.contains_key("in")
        );
    }

    /// `user_notes_count`, not `notes`: the latter counts the system events
    /// ("changed the milestone"), which is not what a discussion badge means.
    #[tokio::test]
    async fn rows_carry_the_human_comment_count() {
        let (api_base, _, _, _, _) = mock_api().await;
        let auth = auth_for(api_base);
        let list = list_issues(&auth, &req(super::super::ForgeTab::Issues, "open"))
            .await
            .expect("list");
        assert_eq!(list.rows[0].comments, 1);
    }

    /// The label vocabulary for the filter, from the project's own labels.
    #[tokio::test]
    async fn project_labels_are_listed() {
        let (api_base, _, _, _, _) = mock_api().await;
        let auth = auth_for(api_base);
        let list = list_labels(&auth, "Group/Sub/Proj").await.expect("labels");
        assert_eq!(
            list.labels,
            vec![
                ForgeLabel { name: "bug".into(), color: Some("#d9534f".into()) },
                // Unrecognized colour → no colour, not a dropped label.
                ForgeLabel { name: "help wanted".into(), color: None },
            ],
            "empty name dropped"
        );
        assert!(!list.truncated, "three of a hundred is not a full page");
        assert!(list_labels(&auth, "no-slash").await.is_err());
    }

    #[tokio::test]
    async fn assigned_me_resolves_the_login_once() {
        let (api_base, _, _, user_hits, _) = mock_api().await;
        let auth = auth_for(api_base);
        let request = ListIssuesRequest {
            assigned_me: true,
            ..req(super::super::ForgeTab::Issues, "open")
        };
        assert_eq!(
            list_issues(&auth, &request).await.unwrap().rows[0].number,
            9
        );
        assert_eq!(list_issues(&auth, &request).await.unwrap().rows.len(), 1);
        assert_eq!(user_hits.load(Ordering::SeqCst), 1, "login cached");
    }

    /// The single-merge-request payload is what a task is checked out from:
    /// `diff_refs.head_sha` wins over `sha`, and a fork's real path is
    /// resolved so the refusal can name it.
    #[tokio::test]
    async fn a_merge_request_is_looked_up_by_iid() {
        let (api_base, _, _, _, _) = mock_api().await;
        let auth = auth_for(api_base);
        let mr = get_merge_request(&auth, "Group/Sub/Proj", 4).await.expect("mr");
        assert_eq!((mr.number, mr.head_ref.as_str(), mr.base_ref.as_str()), (4, "feature", "main"));
        assert_eq!(mr.head_sha, "deadbee", "diff_refs wins over sha");
        assert_eq!(mr.state, "open");
        assert!(!mr.merged);
        // Same project: the head repository IS this repository, which is what
        // every same_repo gate downstream asks.
        assert!(super::super::same_repo(&mr.head_repo, "group/sub/proj"));

        let fork = get_merge_request(&auth, "group/sub/proj", 9).await.expect("mr");
        assert_eq!(fork.head_repo, "contributor/proj", "fork path resolved for the message");
        assert!(!super::super::same_repo(&fork.head_repo, "group/sub/proj"));

        assert!(get_merge_request(&auth, "group/sub/proj", 0).await.is_err());
        assert!(get_merge_request(&auth, "not-a-path", 4).await.is_err());
    }

    /// The delivery's lookup: by source branch, in any state, mapped into the
    /// same shape the four-way match already knows.
    #[tokio::test]
    async fn merge_requests_are_found_by_source_branch() {
        let (api_base, _, _, _, _) = mock_api().await;
        let auth = auth_for(api_base);
        let found = find_merge_requests(&auth, "group/sub/proj", "feature")
            .await
            .expect("find");
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].number, found[0].head_sha.as_str()), (4, "abc123"));
        assert!(find_merge_requests(&auth, "group/sub/proj", "other")
            .await
            .unwrap()
            .is_empty());
    }

    /// GitLab has no `draft` parameter — the title carries it, which is also
    /// how its own UI models a draft.
    #[tokio::test]
    async fn a_draft_merge_request_is_a_prefixed_title() {
        let (api_base, creates, _, _, _) = mock_api().await;
        let auth = auth_for(api_base);
        let made = create_merge_request(
            &auth,
            "group/sub/proj",
            &NewPullRequest {
                title: "Fix #7",
                head: "task/7",
                base: "main",
                body: "Closes #7",
                draft: true,
            },
        )
        .await
        .expect("create");
        assert_eq!(made.number, 8);
        let sent = creates.lock().unwrap().first().cloned().unwrap();
        assert_eq!(sent["source_branch"], "task/7");
        assert_eq!(sent["target_branch"], "main");
        assert_eq!(sent["title"], "Draft: Fix #7");
        assert_eq!(sent["description"], "Closes #7");

        // Not a draft, and an already-prefixed title is not prefixed twice.
        create_merge_request(
            &auth,
            "group/sub/proj",
            &NewPullRequest {
                title: "Draft: Fix #7",
                head: "task/7",
                base: "main",
                body: "",
                draft: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(creates.lock().unwrap()[1]["title"], "Draft: Fix #7");
        assert!(is_draft_title("  draft: x") && !is_draft_title("drafts"));
        // Titles are arbitrary user text. Byte six lands in the middle of a
        // character here, which a byte slice would panic on.
        assert!(!is_draft_title("修a复登录"));
        assert!(!is_draft_title("修"));
        assert!(!is_draft_title(""));
    }

    /// Issue notes and merge-request notes are DIFFERENT endpoints; sending
    /// one to the other is a 404 against a number that exists in the other
    /// collection.
    #[tokio::test]
    async fn notes_go_to_the_collection_the_item_belongs_to() {
        let (api_base, _, notes, _, _) = mock_api().await;
        let auth = auth_for(api_base);
        let issue_url = create_note(&auth, "Group/Sub/Proj", ForgeItemKind::Issue, 7, "done")
            .await
            .expect("issue note");
        let mr_url = create_note(&auth, "group/sub/proj", ForgeItemKind::Change, 7, "done")
            .await
            .expect("mr note");
        let sent = notes.lock().unwrap().clone();
        assert_eq!(sent[0].0, "issues");
        assert_eq!(sent[0].1["body"], "done");
        assert_eq!(sent[1].0, "merge_requests");
        // A note has no URL of its own; the anchor on the item's page does.
        assert!(issue_url.ends_with("/group/sub/proj/-/issues/7#note_55"), "{issue_url}");
        assert!(mr_url.ends_with("/group/sub/proj/-/merge_requests/7#note_66"), "{mr_url}");

        assert!(create_note(&auth, "not-a-path", ForgeItemKind::Issue, 7, "x").await.is_err());
        assert!(create_note(&auth, "group/sub/proj", ForgeItemKind::Issue, 0, "x").await.is_err());
    }

    #[tokio::test]
    async fn rate_limits_are_told_apart_from_auth_failures() {
        let (api_base, _, _, _, _) = mock_api().await;
        let auth = auth_for(format!("{api_base}/limited"));
        match list_issues(&auth, &req(super::super::ForgeTab::Issues, "open")).await {
            Err(ForgeError::RateLimited { retry_after }) => assert_eq!(retry_after, Some(17)),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    /// Every request URL is now built here from the resolved api_base, so the
    /// only thing the client still supplies is coordinates — and a project path
    /// it made up must be refused rather than turned into a URL.
    #[tokio::test]
    async fn a_crafted_project_path_is_rejected() {
        let (api_base, _, _, _, _) = mock_api().await;
        let auth = auth_for(api_base);
        for bad in ["no-slash", "acme/app?x=1", "http://169.254.169.254/latest"] {
            let hostile = ListIssuesRequest {
                owner_repo: bad.into(),
                ..req(super::super::ForgeTab::Issues, "open")
            };
            assert!(
                matches!(list_issues(&auth, &hostile).await, Err(ForgeError::Invalid(_))),
                "{bad} should be refused"
            );
        }
    }

    #[test]
    fn project_paths_become_one_encoded_segment() {
        assert_eq!(project_ref("Group/Sub/Proj").unwrap(), "group%2Fsub%2Fproj");
        assert_eq!(project_ref("acme/app.git").unwrap(), "acme%2Fapp");
        assert!(project_ref("no-slash").is_err());
        assert!(project_ref("acme/app?x=1").is_err());
    }

    /// A payload that does not say which project the source lives in cannot be
    /// shown to be ours — "unknown" must not read as "same repository".
    #[test]
    fn an_unstated_source_project_is_not_this_repository() {
        let raw: RawMergeRequest =
            serde_json::from_value(mr_json(3, "opened", 1)).expect("parse");
        assert!(super::super::same_repo(
            &map_merge_request(raw, "group/proj").head_repo,
            "group/proj"
        ));
        let mut bare = mr_json(3, "opened", 1);
        bare["source_project_id"] = serde_json::Value::Null;
        let raw: RawMergeRequest = serde_json::from_value(bare).expect("parse");
        let mapped = map_merge_request(raw, "group/proj");
        assert!(!super::super::same_repo(&mapped.head_repo, "group/proj"));
    }

}
