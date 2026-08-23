//! GitHub REST reads for the workbench. Four hard-won rules are load-bearing
//! here (see `.docs/architecture/2026-08-17-Orca-GitHub-GitLab集成分析.md`):
//!
//! 1. The list comes from `/search/issues`, NOT `/repos/{o}/{r}/issues`. This
//!    is what github.com's own issue list is backed by, and the reason is
//!    pagination: `/issues` serves issues and pull requests from one feed with
//!    no way to ask for one kind, so splitting them client-side made every
//!    page sparse ("API page 3" was not "Issues page 3") and no total existed.
//!    `is:issue` / `is:pr` filters server-side and `total_count` comes back,
//!    which is what makes real page numbers possible.
//! 2. `/pulls` is still never an option for the PR tab: it silently ignores
//!    `assignee`/`labels` filters (HTTP 200, unfiltered rows).
//! 3. Search costs more than it looks: a SEPARATE 30-requests-per-minute quota
//!    (core is 5000/hour), and only the first [`SEARCH_RESULT_CAP`] results are
//!    reachable — the same ceiling github.com shows. `incomplete_results` means
//!    the query timed out and the page is partial; it is surfaced, not hidden.
//! 4. `assignee=@me` is a 422 on `/issues`; the literal login is resolved via
//!    `GET /user` (cached per api_base+account) and reused for the `assignee:`
//!    qualifier here.
//!
//! One limit is deliberately NOT worked around: `q` may not exceed 256
//! characters. Ten long label names plus a full-length text filter can pass it,
//! and GitHub answers 422 with a message that says exactly that — which is
//! surfaced. Silently dropping the last few qualifiers would instead show a
//! list that quietly ignores part of what the user asked for.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use serde::Deserialize;

use super::auth::ResolvedAuth;
use super::{
    truncate_chars, urlencode_query, validate_state_filter, ForgeError, ForgeIssueList,
    ForgeIssueRow, ForgeLabel, ForgeLabelList, ForgeTab, ListIssuesRequest, BODY_CAP,
    LABEL_PAGE_SIZE,
};

/// How deep search pagination goes, per GitHub's documented limit. Beyond this
/// the API errors instead of paging, so `has_next` has to stop here — the same
/// ceiling the github.com issue list has.
pub(crate) const SEARCH_RESULT_CAP: i64 = 1000;

#[derive(Debug, Deserialize)]
struct SearchPage {
    #[serde(default)]
    total_count: i64,
    #[serde(default)]
    incomplete_results: bool,
    #[serde(default)]
    items: Vec<RawIssue>,
}

#[derive(Debug, Deserialize)]
struct RawIssue {
    number: i64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    draft: bool,
    /// Human comments; excludes the timeline's system events.
    #[serde(default)]
    comments: i64,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    user: Option<RawUser>,
    #[serde(default)]
    labels: Vec<RawLabel>,
    /// Presence of this key is what makes a search hit a pull request.
    #[serde(default)]
    pull_request: Option<RawPullRequestRef>,
}

#[derive(Debug, Deserialize)]
struct RawPullRequestRef {
    /// Set only once the pull request is merged. GitHub has no `merged` STATE
    /// — a merged pull request reports `state: "closed"` like any other — so
    /// this timestamp is the only thing that tells the two apart.
    #[serde(default)]
    merged_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct RawLabel {
    #[serde(default)]
    name: String,
    /// Six hex digits with NO leading `#` — GitHub's own spelling, which is
    /// why the normalizer accepts the hash rather than requiring it.
    #[serde(default)]
    color: Option<String>,
}

/// A label name as a quoted `label:` qualifier. Always quoted (names carry
/// spaces — `help wanted` — and an unquoted one would split into two terms),
/// with the two characters that end a quoted string escaped.
fn label_qualifier(name: &str) -> String {
    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
    format!("label:\"{escaped}\"")
}

/// The user's free text, stripped of query SYNTAX.
///
/// The box is labelled "search title and description", so that is what it has
/// to do: typing `is:closed` must search for those words, not silently widen
/// the list to closed items. `:` and `"` are what form a qualifier and a
/// phrase, and a leading `-` is search's negation — all three go, and runs of
/// whitespace collapse so the result is a clean list of terms (ANDed, which is
/// what a filter box is expected to do).
fn search_terms(raw: &str) -> String {
    raw.split_whitespace()
        .map(|word| {
            word.trim_start_matches(['-', '+'])
                .replace([':', '"'], "")
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `q` value for one tab+filter combination.
///
/// Exactly ONE `repo:` qualifier, which is what keeps this immune to the
/// advanced-search change of 2025-09-04 (a space between MULTIPLE `repo:` /
/// `org:` / `user:` qualifiers flipped from OR to AND).
fn search_query(
    repo: &str,
    tab: ForgeTab,
    state: &str,
    assignee: Option<&str>,
    labels: &[String],
    search: Option<&str>,
) -> String {
    let mut q = format!(
        "repo:{repo} is:{}",
        match tab {
            ForgeTab::Issues => "issue",
            ForgeTab::Prs => "pr",
        }
    );
    // "all" is the absence of a state qualifier, not a value search understands.
    if state == "open" || state == "closed" {
        q.push_str(&format!(" state:{state}"));
    }
    if let Some(login) = assignee {
        q.push_str(&format!(" assignee:{login}"));
    }
    for label in labels {
        q.push(' ');
        q.push_str(&label_qualifier(label));
    }
    if let Some(raw) = search {
        let terms = search_terms(raw);
        if !terms.is_empty() {
            // Scope said out loud. Search's DEFAULT for free text is title,
            // body AND comments, while the box promises "title and
            // description" — without this qualifier a hit whose only match
            // lives in a comment appears with nothing in it to see, and
            // inflates the count and the page numbers with it. GitLab's
            // `search` already defaults to title+description; this is what
            // makes GitHub agree with it.
            q.push_str(" in:title,body ");
            q.push_str(&terms);
        }
    }
    q
}

pub async fn list_issues(
    conn_auth: &ResolvedAuth,
    req: &ListIssuesRequest,
) -> Result<ForgeIssueList, ForgeError> {
    let repo = super::normalize_repo(&req.owner_repo)
        .ok_or_else(|| ForgeError::Invalid(format!("bad repository path: {}", req.owner_repo)))?;
    validate_state_filter(&req.state)?;
    let (page, per_page) = req.clamped();

    let assignee = if req.assigned_me {
        Some(current_login(conn_auth).await?)
    } else {
        None
    };
    let q = search_query(
        &repo,
        req.tab,
        &req.state,
        assignee.as_deref(),
        &req.labels,
        req.search.as_deref(),
    );
    let url = format!(
        "{}/search/issues?q={}&advanced_search=true&sort={}&order={}&page={page}&per_page={per_page}",
        conn_auth.api_base,
        urlencode_query(&q),
        req.sort.field(),
        req.sort.direction(),
    );

    let page_data: SearchPage = api_get(conn_auth, &url)
        .await?
        .json()
        .await
        .map_err(|e| ForgeError::Network(format!("bad search payload: {e}")))?;

    let is_pr = req.tab == ForgeTab::Prs;
    let rows = page_data
        .items
        .into_iter()
        .map(|r| {
            let merged = r.pull_request.as_ref().is_some_and(|p| p.merged_at.is_some());
            ForgeIssueRow {
                is_pr,
                number: r.number,
                title: r.title,
                body: r.body.map(|b| truncate_chars(&b, BODY_CAP)),
                state: if merged { "merged".to_string() } else { r.state },
                draft: is_pr && r.draft,
                labels: r
                    .labels
                    .into_iter()
                    .filter_map(|l| ForgeLabel::parse(l.name, l.color.as_deref()))
                    .collect(),
                author: r.user.map(|u| u.login),
                updated_at: r.updated_at,
                html_url: r.html_url,
                comments: r.comments,
            }
        })
        .collect();

    // Reachable rows, not matching rows: paging past the cap is an error at the
    // API, so a "next" that leads there would be a button into a failure — and
    // so would the numbered page that `total_count` alone implies, which is why
    // the ceiling travels with the page instead of staying local to `has_next`.
    let reachable = page_data.total_count.min(SEARCH_RESULT_CAP);
    Ok(ForgeIssueList {
        rows,
        page,
        per_page,
        total_count: Some(page_data.total_count),
        reachable_count: (page_data.total_count > SEARCH_RESULT_CAP).then_some(SEARCH_RESULT_CAP),
        has_next: i64::from(page) * i64::from(per_page) < reachable,
        incomplete: page_data.incomplete_results,
    })
}

/// The repository's labels, for the workbench's label filter.
///
/// `/repos/{o}/{r}/labels`, NOT a search endpoint: this runs on the core quota
/// (5000/hour), so opening the filter costs nothing against the 30-per-minute
/// budget the list itself lives on.
pub async fn list_labels(
    auth: &ResolvedAuth,
    owner_repo: &str,
) -> Result<ForgeLabelList, ForgeError> {
    let repo = super::normalize_repo(owner_repo)
        .ok_or_else(|| ForgeError::Invalid(format!("bad repository path: {owner_repo}")))?;
    let url = format!("{}/repos/{repo}/labels?per_page={LABEL_PAGE_SIZE}", auth.api_base);
    let raw: Vec<RawLabel> = api_get(auth, &url)
        .await?
        .json()
        .await
        .map_err(|e| ForgeError::Network(format!("bad labels payload: {e}")))?;
    // A full page means there may be more. Said out loud rather than silently
    // truncated: a filter list that stops at 100 otherwise reads as complete.
    let truncated = raw.len() >= LABEL_PAGE_SIZE;
    Ok(ForgeLabelList {
        labels: raw
            .into_iter()
            .filter_map(|l| ForgeLabel::parse(l.name, l.color.as_deref()))
            .collect(),
        truncated,
    })
}

/// `GET {api_base}/user` → login, cached per `(api_base, account)` — resolving
/// it on every "assigned to me" page would waste a rate-limit point per click.
static LOGIN_CACHE: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

async fn current_login(auth: &ResolvedAuth) -> Result<String, ForgeError> {
    let cache_key = format!("{}\n{}", auth.api_base, auth.account_id);
    if let Some(hit) = LOGIN_CACHE.read().ok().and_then(|c| c.get(&cache_key).cloned()) {
        return Ok(hit);
    }
    #[derive(Deserialize)]
    struct User {
        login: String,
    }
    let user: User = api_get(auth, &format!("{}/user", auth.api_base))
        .await?
        .json()
        .await
        .map_err(|e| ForgeError::Network(format!("bad /user payload: {e}")))?;
    if let Ok(mut cache) = LOGIN_CACHE.write() {
        cache.insert(cache_key, user.login.clone());
    }
    Ok(user.login)
}

/// Authenticated GET with the standard GitHub headers; non-2xx classified into
/// `ForgeError` (rate limits distinguished from plain auth failures).
pub(crate) async fn api_get(
    auth: &ResolvedAuth,
    url: &str,
) -> Result<reqwest::Response, ForgeError> {
    let response = super::http_client()?
        .get(url)
        .header("Authorization", format!("Bearer {}", auth.token))
        .header("User-Agent", "codeg")
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| ForgeError::Network(e.to_string()))?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(classify_failure(status.as_u16(), response).await)
}

/// Authenticated POST with the same headers and the same failure taxonomy as
/// [`api_get`]. Writes are never retried here: a retried create could open a
/// duplicate pull request, so the caller decides.
pub(crate) async fn api_post(
    auth: &ResolvedAuth,
    url: &str,
    body: &serde_json::Value,
) -> Result<reqwest::Response, ForgeError> {
    let response = super::http_client()?
        .post(url)
        .header("Authorization", format!("Bearer {}", auth.token))
        .header("User-Agent", "codeg")
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(body)
        .send()
        .await
        .map_err(|e| ForgeError::Network(e.to_string()))?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(classify_failure(status.as_u16(), response).await)
}

async fn classify_failure(status: u16, response: reqwest::Response) -> ForgeError {
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let primary_exhausted = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim() == "0");
    match status {
        429 => ForgeError::RateLimited { retry_after },
        403 if retry_after.is_some() || primary_exhausted => {
            ForgeError::RateLimited { retry_after }
        }
        401 | 403 => ForgeError::Auth(format!("GitHub returned {status}")),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Query;
    use axum::http::HeaderMap;
    use axum::routing::get;
    use axum::Json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn auth_for(api_base: String) -> ResolvedAuth {
        ResolvedAuth {
            provider: super::super::ForgeProvider::GitHub,
            server_host: "github.test".into(),
            api_base,
            account_id: "acc-test".into(),
            username: "alice".into(),
            token: "tok-test".into(),
            scopes: vec!["repo".into()],
        }
    }

    /// A search hit. `merged_at` only ever appears inside `pull_request`, which
    /// is also the key that marks the hit as a pull request at all.
    fn hit(number: i64, kind: Kind) -> serde_json::Value {
        let mut v = serde_json::json!({
            "number": number,
            "title": format!("item {number}"),
            "body": format!("body {number}"),
            "state": "open",
            "comments": number,
            "updated_at": "2026-08-17T00:00:00Z",
            "html_url": format!("https://github.test/acme/app/issues/{number}"),
            "user": { "login": "alice" },
            // Bare six-digit hex, GitHub's own spelling — no leading `#`.
            "labels": [ { "name": "bug", "color": "D73A4A" }, { "name": "" } ],
        });
        match kind {
            Kind::Issue => {}
            Kind::OpenPr => v["pull_request"] = serde_json::json!({ "merged_at": null }),
            Kind::DraftPr => {
                v["draft"] = serde_json::json!(true);
                v["pull_request"] = serde_json::json!({ "merged_at": null });
            }
            Kind::MergedPr => {
                v["state"] = serde_json::json!("closed");
                v["pull_request"] =
                    serde_json::json!({ "merged_at": "2026-08-16T00:00:00Z" });
            }
            Kind::ClosedPr => {
                v["state"] = serde_json::json!("closed");
                v["pull_request"] = serde_json::json!({ "merged_at": null });
            }
        }
        v
    }

    #[derive(Clone, Copy)]
    enum Kind {
        Issue,
        OpenPr,
        DraftPr,
        MergedPr,
        ClosedPr,
    }

    /// The last `q` / `page` / `per_page` the mock was asked for, so a test can
    /// assert what actually went over the wire rather than infer it.
    #[derive(Default)]
    struct LastQuery {
        q: String,
        page: String,
        per_page: String,
        advanced_search: String,
        sort: String,
        order: String,
    }

    /// One mock GitHub search API on an OS-assigned port. Returns the api_base,
    /// the `/user` hit counter and the recorded query params.
    async fn mock_api() -> (String, Arc<AtomicUsize>, Arc<RwLock<LastQuery>>) {
        let user_hits = Arc::new(AtomicUsize::new(0));
        let hits = user_hits.clone();
        let seen = Arc::new(RwLock::new(LastQuery::default()));
        let recorder = seen.clone();
        let app = axum::Router::new()
            .route(
                "/search/issues",
                get(move |Query(params): Query<HashMap<String, String>>| {
                    let recorder = recorder.clone();
                    async move {
                        let q = params.get("q").cloned().unwrap_or_default();
                        if let Ok(mut slot) = recorder.write() {
                            *slot = LastQuery {
                                q: q.clone(),
                                page: params.get("page").cloned().unwrap_or_default(),
                                per_page: params.get("per_page").cloned().unwrap_or_default(),
                                advanced_search: params
                                    .get("advanced_search")
                                    .cloned()
                                    .unwrap_or_default(),
                                sort: params.get("sort").cloned().unwrap_or_default(),
                                order: params.get("order").cloned().unwrap_or_default(),
                            };
                        }
                        // The mock answers the QUERY, the way search does — no
                        // client-side splitting is involved any more.
                        let items = if q.contains("assignee:alice") {
                            vec![hit(9, Kind::Issue)]
                        } else if q.contains("is:pr") {
                            vec![
                                hit(2, Kind::OpenPr),
                                hit(4, Kind::DraftPr),
                                hit(6, Kind::MergedPr),
                                hit(8, Kind::ClosedPr),
                            ]
                        } else {
                            vec![hit(1, Kind::Issue), hit(3, Kind::Issue)]
                        };
                        Json(serde_json::json!({
                            "total_count": 57,
                            "incomplete_results": false,
                            "items": items,
                        }))
                    }
                }),
            )
            .route(
                "/user",
                get(move || {
                    hits.fetch_add(1, Ordering::SeqCst);
                    async { Json(serde_json::json!({ "login": "alice" })) }
                }),
            )
            .route(
                // The repository path goes into the PATH here, lowercased by
                // `normalize_repo` — an `Acme/App` request must land on this.
                "/repos/acme/app/labels",
                get(|| async {
                    Json(serde_json::json!([
                        { "name": "bug", "color": "d73a4a" },
                        // No colour GitHub could have sent — a label written
                        // through some other tool. The name still filters.
                        { "name": "help wanted", "color": "rebeccapurple" },
                        { "name": "" },
                    ]))
                }),
            )
            .route(
                "/huge/repos/acme/app/labels",
                get(|| async {
                    let full: Vec<serde_json::Value> = (0..LABEL_PAGE_SIZE)
                        .map(|i| serde_json::json!({ "name": format!("label-{i}") }))
                        .collect();
                    Json(serde_json::Value::Array(full))
                }),
            )
            .route(
                // A repository with more matches than search will paginate.
                "/huge/search/issues",
                get(|| async {
                    Json(serde_json::json!({
                        "total_count": 24_000,
                        "incomplete_results": true,
                        "items": [],
                    }))
                }),
            )
            .route(
                "/limited/search/issues",
                get(|| async {
                    let mut headers = HeaderMap::new();
                    headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
                    headers.insert("retry-after", "31".parse().unwrap());
                    (
                        axum::http::StatusCode::FORBIDDEN,
                        headers,
                        Json(serde_json::json!({ "message": "rate limited" })),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), user_hits, seen)
    }

    fn req(tab: ForgeTab) -> ListIssuesRequest {
        ListIssuesRequest {
            owner_repo: "Acme/App".into(),
            tab,
            state: "open".into(),
            assigned_me: false,
            labels: vec![],
            search: None,
            sort: super::super::ForgeSort::default(),
            page: 1,
            per_page: 20,
        }
    }

    /// The query string IS the feature: `is:issue` / `is:pr` is what moved the
    /// tab split from the client to the server, and one `repo:` qualifier is
    /// what keeps advanced-search's OR→AND change irrelevant here.
    #[test]
    fn query_names_one_repo_and_the_kind() {
        assert_eq!(
            search_query("acme/app", ForgeTab::Issues, "open", None, &[], None),
            "repo:acme/app is:issue state:open"
        );
        assert_eq!(
            search_query("acme/app", ForgeTab::Prs, "closed", Some("alice"), &[], None),
            "repo:acme/app is:pr state:closed assignee:alice"
        );
        // "all" is the ABSENCE of a state qualifier — `state:all` matches
        // nothing, so passing it through would empty the list.
        assert_eq!(
            search_query("acme/app", ForgeTab::Prs, "all", None, &[], None),
            "repo:acme/app is:pr"
        );
        assert_eq!(
            search_query("acme/app", ForgeTab::Issues, "open", None, &[], None)
                .matches("repo:")
                .count(),
            1
        );
    }

    /// Labels are ANDed as quoted qualifiers — unquoted, `help wanted` would
    /// split into a label named "help" plus a stray search term.
    #[test]
    fn label_filters_are_quoted_qualifiers() {
        assert_eq!(
            search_query(
                "acme/app",
                ForgeTab::Issues,
                "open",
                None,
                &["bug".to_string(), "help wanted".to_string()],
                None
            ),
            "repo:acme/app is:issue state:open label:\"bug\" label:\"help wanted\""
        );
        // A name carrying the two characters that END a quoted string.
        assert_eq!(label_qualifier(r#"say "hi"\ok"#), r#"label:"say \"hi\"\\ok""#);
    }

    /// The box says "search title and description", so it must not double as a
    /// query box: typing `is:closed` searches for those words and does NOT
    /// widen the list, and a leading `-` does not negate a term.
    #[test]
    fn free_text_is_stripped_of_query_syntax() {
        assert_eq!(search_terms("  login   timeout "), "login timeout");
        assert_eq!(search_terms("is:closed -bug \"exact\""), "isclosed bug exact");
        // Nothing but syntax is nothing at all — an empty tail must not leave a
        // trailing space on `q`.
        assert_eq!(search_terms(" : \" "), "");
        assert_eq!(
            search_query("acme/app", ForgeTab::Issues, "open", None, &[], Some(" : ")),
            "repo:acme/app is:issue state:open"
        );
        assert_eq!(
            search_query(
                "acme/app",
                ForgeTab::Issues,
                "open",
                None,
                &[],
                Some("login timeout")
            ),
            "repo:acme/app is:issue state:open in:title,body login timeout"
        );
    }

    /// …and the scope has to be SAID. GitHub's default for free text is title,
    /// body AND comments, so without `in:` a match living only in a comment
    /// joins the list, the count and the page numbers — under a box that
    /// promises "title and description". GitLab's `search` already means
    /// title+description, so this is what makes the two forges agree.
    #[test]
    fn free_text_searches_only_the_title_and_the_body() {
        let q = search_query("acme/app", ForgeTab::Issues, "open", None, &[], Some("boom"));
        assert!(q.contains("in:title,body"), "{q}");
        // The qualifier belongs to the TEXT: with nothing to search it would be
        // a lone qualifier narrowing a query that has no keywords to narrow.
        assert!(
            !search_query("acme/app", ForgeTab::Issues, "open", None, &[], None)
                .contains("in:"),
            "no text, no scope qualifier"
        );
        assert!(
            !search_query("acme/app", ForgeTab::Issues, "open", None, &[], Some(" : "))
                .contains("in:"),
            "text that was all syntax leaves no text either"
        );
    }

    /// Server-side filtering plus a real total: the page number and size go out
    /// verbatim and `total_count` comes back, which is what page numbers need.
    #[tokio::test]
    async fn list_paginates_by_page_number_with_a_total() {
        let (api_base, _, seen) = mock_api().await;
        let auth = auth_for(api_base);

        let issues = list_issues(
            &auth,
            &ListIssuesRequest { page: 2, per_page: 20, ..req(ForgeTab::Issues) },
        )
        .await
        .unwrap();
        assert_eq!(issues.rows.iter().map(|r| r.number).collect::<Vec<_>>(), vec![1, 3]);
        assert!(issues.rows.iter().all(|r| !r.is_pr));
        // Empty label dropped; the colour rides along, hashed and lowercased.
        assert_eq!(
            issues.rows[0].labels,
            vec![ForgeLabel { name: "bug".into(), color: Some("#d73a4a".into()) }]
        );
        assert_eq!(issues.rows[0].author.as_deref(), Some("alice"));
        assert_eq!((issues.page, issues.per_page), (2, 20));
        assert_eq!(issues.total_count, Some(57));
        // Well under the cap, so nothing is out of reach and the footer builds
        // its page numbers from the total itself.
        assert_eq!(issues.reachable_count, None);
        assert!(issues.has_next, "40 of 57 shown");
        assert!(!issues.incomplete);
        assert_eq!(issues.trustworthy_count(), Some(57), "a badge may show this");

        {
            let sent = seen.read().unwrap();
            assert_eq!(sent.q, "repo:acme/app is:issue state:open");
            assert_eq!((sent.page.as_str(), sent.per_page.as_str()), ("2", "20"));
            assert_eq!(sent.advanced_search, "true");
            // The default order, and the one github.com's own list opens on.
            assert_eq!((sent.sort.as_str(), sent.order.as_str()), ("created", "desc"));
        }
        // The comment count rides along on the same payload.
        assert_eq!(issues.rows[0].comments, 1);

        // Last page: 57 matches, 20 per page → page 3 ends the list.
        let last = list_issues(
            &auth,
            &ListIssuesRequest { page: 3, per_page: 20, ..req(ForgeTab::Issues) },
        )
        .await
        .unwrap();
        assert!(!last.has_next);
    }

    /// Out-of-range paging never reaches the API: `per_page=0` is a 422 there
    /// and `page=0` means different things at the two forges.
    #[tokio::test]
    async fn paging_is_clamped_before_it_is_sent() {
        let (api_base, _, seen) = mock_api().await;
        let auth = auth_for(api_base);

        let list = list_issues(
            &auth,
            &ListIssuesRequest { page: 0, per_page: 9_999, ..req(ForgeTab::Issues) },
        )
        .await
        .unwrap();
        assert_eq!((list.page, list.per_page), (1, 100));
        let sent = seen.read().unwrap();
        assert_eq!((sent.page.as_str(), sent.per_page.as_str()), ("1", "100"));
    }

    /// A pull request's four display states all come off ONE payload: `draft`
    /// on the hit, and `merged_at` inside `pull_request` — GitHub reports a
    /// merged pull request as plain `closed`, so without that timestamp the
    /// merged and the abandoned ones would look identical.
    #[tokio::test]
    async fn pull_request_rows_carry_draft_and_merged() {
        let (api_base, _, _) = mock_api().await;
        let auth = auth_for(api_base);

        let prs = list_issues(&auth, &req(ForgeTab::Prs)).await.unwrap();
        let by_number = |n: i64| prs.rows.iter().find(|r| r.number == n).expect("row");
        assert!(prs.rows.iter().all(|r| r.is_pr));
        assert_eq!((by_number(2).state.as_str(), by_number(2).draft), ("open", false));
        assert_eq!((by_number(4).state.as_str(), by_number(4).draft), ("open", true));
        assert_eq!((by_number(6).state.as_str(), by_number(6).draft), ("merged", false));
        assert_eq!((by_number(8).state.as_str(), by_number(8).draft), ("closed", false));

        // `draft` on an ISSUE hit is meaningless; it must never leak through.
        let issues = list_issues(&auth, &req(ForgeTab::Issues)).await.unwrap();
        assert!(issues.rows.iter().all(|r| !r.draft));
    }

    /// Search only paginates the first [`SEARCH_RESULT_CAP`] results — past
    /// that the API errors, so "next" must stop rather than offer a button
    /// into a failure. A timed-out query is reported, not hidden.
    #[tokio::test]
    async fn paging_stops_at_the_search_cap_and_reports_partial_results() {
        let (api_base, _, _) = mock_api().await;
        let auth = auth_for(format!("{api_base}/huge"));

        let mid = list_issues(
            &auth,
            &ListIssuesRequest { page: 9, per_page: 100, ..req(ForgeTab::Issues) },
        )
        .await
        .unwrap();
        assert_eq!(mid.total_count, Some(24_000), "the true count is still told");
        // …and the ceiling is told SEPARATELY, because page numbers are built
        // from it. Without this the footer would offer page 1200 of a
        // 20-per-page list, and clicking it is a 422.
        assert_eq!(mid.reachable_count, Some(SEARCH_RESULT_CAP));
        assert!(mid.has_next, "900 of a reachable 1000");
        assert!(mid.incomplete);
        // An incomplete search counted FEWER items than match, so the number is
        // not one a bare badge may show.
        assert_eq!(mid.trustworthy_count(), None);

        // Page 10 × 100 lands exactly on the cap: there is no reachable page 11.
        let at_cap = list_issues(
            &auth,
            &ListIssuesRequest { page: 10, per_page: 100, ..req(ForgeTab::Issues) },
        )
        .await
        .unwrap();
        assert!(!at_cap.has_next);
    }

    /// `assignee=@me` would be a 422 — the login is resolved through `/user`
    /// exactly once and reused from the cache afterwards.
    #[tokio::test]
    async fn assigned_me_resolves_login_once() {
        let (api_base, user_hits, seen) = mock_api().await;
        let auth = auth_for(api_base);
        let request = ListIssuesRequest { assigned_me: true, ..req(ForgeTab::Issues) };

        let first = list_issues(&auth, &request).await.unwrap();
        assert_eq!(first.rows.iter().map(|r| r.number).collect::<Vec<_>>(), vec![9]);
        assert!(seen.read().unwrap().q.contains("assignee:alice"));
        let second = list_issues(&auth, &request).await.unwrap();
        assert_eq!(second.rows.len(), 1);
        assert_eq!(user_hits.load(Ordering::SeqCst), 1, "login cached after first use");
    }

    /// 403 + exhausted quota is a rate limit (with its retry hint), NOT an
    /// auth failure — the UI shows a cooldown, not "re-enter your token".
    /// Search has its own, much smaller quota, so this path matters more now.
    #[tokio::test]
    async fn exhausted_quota_classifies_as_rate_limit() {
        let (api_base, _, _) = mock_api().await;
        let auth = auth_for(format!("{api_base}/limited"));
        match list_issues(&auth, &req(ForgeTab::Issues)).await {
            Err(ForgeError::RateLimited { retry_after }) => assert_eq!(retry_after, Some(31)),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    /// Each named order reaches the API as its own `sort`+`order` pair. Without
    /// both halves "oldest" and "newest" would be the same request.
    #[tokio::test]
    async fn every_sort_order_reaches_the_api() {
        use super::super::ForgeSort;
        let (api_base, _, seen) = mock_api().await;
        let auth = auth_for(api_base);
        for (sort, field, direction) in [
            (ForgeSort::Newest, "created", "desc"),
            (ForgeSort::Oldest, "created", "asc"),
            (ForgeSort::RecentlyUpdated, "updated", "desc"),
            (ForgeSort::LeastRecentlyUpdated, "updated", "asc"),
        ] {
            list_issues(&auth, &ListIssuesRequest { sort, ..req(ForgeTab::Issues) })
                .await
                .unwrap();
            let sent = seen.read().unwrap();
            assert_eq!(
                (sent.sort.as_str(), sent.order.as_str()),
                (field, direction),
                "{sort:?}"
            );
        }
    }

    /// Labels and free text both go out on `q`, together with everything else —
    /// the filters compose rather than replacing each other.
    #[tokio::test]
    async fn label_and_text_filters_go_out_together() {
        let (api_base, _, seen) = mock_api().await;
        let auth = auth_for(api_base);
        list_issues(
            &auth,
            &ListIssuesRequest {
                labels: vec!["bug".into(), "help wanted".into()],
                search: Some("login is:open".into()),
                ..req(ForgeTab::Issues)
            },
        )
        .await
        .unwrap();
        assert_eq!(
            seen.read().unwrap().q,
            "repo:acme/app is:issue state:open label:\"bug\" label:\"help wanted\" \
             in:title,body login isopen"
        );
    }

    /// The label vocabulary comes from the repository, on the CORE quota — and
    /// a full page is reported as such, because a filter list that silently
    /// stops at 100 reads as "these are all the labels this repo has".
    #[tokio::test]
    async fn labels_come_from_the_repository_and_admit_truncation() {
        let (api_base, _, _) = mock_api().await;
        let auth = auth_for(api_base.clone());
        let list = list_labels(&auth, "Acme/App").await.expect("labels");
        assert_eq!(
            list.labels,
            vec![
                ForgeLabel { name: "bug".into(), color: Some("#d73a4a".into()) },
                // Unrecognized colour → no colour, NOT a dropped label: the
                // name is what the filter needs, the swatch is decoration.
                ForgeLabel { name: "help wanted".into(), color: None },
            ],
            "empty name dropped"
        );
        assert!(!list.truncated);

        let huge = auth_for(format!("{api_base}/huge"));
        let full = list_labels(&huge, "acme/app").await.expect("labels");
        assert_eq!(full.labels.len(), LABEL_PAGE_SIZE);
        assert!(full.truncated);

        assert!(list_labels(&auth, "no-slash").await.is_err());
    }

    /// A repository path the client made up must not become a search that
    /// silently reads a DIFFERENT repository.
    #[tokio::test]
    async fn a_bad_repository_path_is_rejected() {
        let (api_base, _, _) = mock_api().await;
        let auth = auth_for(api_base);
        let hostile = ListIssuesRequest {
            owner_repo: "no-slash".into(),
            ..req(ForgeTab::Issues)
        };
        assert!(matches!(
            list_issues(&auth, &hostile).await,
            Err(ForgeError::Invalid(_))
        ));
    }
}
