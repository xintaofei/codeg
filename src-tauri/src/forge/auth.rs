//! Account/token resolution for forge calls.
//!
//! Reads the same `github_accounts` app-metadata blob the settings UI writes
//! (`commands/version_control.rs`) — that reader is desktop-gated, this one is
//! mode-agnostic because the forge paths run in both binaries. Tokens come
//! from `keyring_store` (OS keyring on desktop, hardened `tokens.json` on the
//! server). Every downstream call derives its identity scope from the
//! resolved `(server_host, api_base, account_id)` triple — never from "the
//! current default account" at call time, so a later `is_default` flip cannot
//! silently change which identity a task writes with.

use sea_orm::DatabaseConnection;

use super::{ForgeError, ForgeProvider};
use crate::db::service::app_metadata_service;
use crate::models::{GitHubAccount, GitHubAccountsSettings};

/// Same key as `commands/version_control.rs` — duplicated because that module
/// gates the constant behind the desktop feature. GitLab accounts live in the
/// SAME store: an account has always been "a credential for this host", and a
/// host is one forge or the other, so splitting the store would only create
/// two places to look for the same answer.
const GITHUB_ACCOUNTS_KEY: &str = "github_accounts";

pub struct ResolvedAuth {
    /// Which forge this identity talks to — picks the REST dialect, the auth
    /// header and the endpoint shapes downstream.
    pub provider: ForgeProvider,
    /// Server host (`github.com`, `ghe.corp.com`) — the coordinate system
    /// source keys and git remotes share.
    pub server_host: String,
    /// Derived REST base: `https://api.github.com`, `{server_url}/api/v3`
    /// (GitHub Enterprise) or `{server_url}/api/v4` (GitLab).
    pub api_base: String,
    pub account_id: String,
    /// Login of the pinned account — the username half of the git credential
    /// when this identity pushes. Resolving it here (rather than letting git
    /// pick an account by host) is what makes "the branch is pushed by the
    /// account that triggered the task" true.
    pub username: String,
    pub token: String,
    /// Token scopes as recorded when the account was added, for display only.
    /// Deliberately NOT used as a gate: GitHub's fine-grained tokens report no
    /// scopes at all, so refusing on an empty list would lock out perfectly
    /// good credentials. A token that cannot do the job says so at the API,
    /// and the delivery path turns that into a readable failure.
    pub scopes: Vec<String>,
}

// Hand-written so a stray `{auth:?}` in a log line can never print the token.
impl std::fmt::Debug for ResolvedAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedAuth")
            .field("provider", &self.provider)
            .field("server_host", &self.server_host)
            .field("api_base", &self.api_base)
            .field("account_id", &self.account_id)
            .field("username", &self.username)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Resolve the account (and token) to use against `server_host`.
///
/// `account_id: Some` must exist AND live on that host — a mismatched id is an
/// error, not a fallback, because silently substituting another identity is
/// exactly the drift the plan forbids. `None` prefers the host's `is_default`
/// account, then the first host match.
///
/// Accounts are additionally filtered by forge: one that DECLARES a provider
/// only serves that provider, while an undeclared one (everything stored
/// before GitLab support) serves whatever its host turns out to be. Handing a
/// GitLab token to the GitHub client would spend a real credential on a 401
/// and read as "your token expired".
pub async fn resolve_forge_auth(
    conn: &DatabaseConnection,
    provider: ForgeProvider,
    server_host: &str,
    account_id: Option<&str>,
) -> Result<ResolvedAuth, ForgeError> {
    let host = server_host.trim().to_ascii_lowercase();
    if host.is_empty() {
        return Err(ForgeError::Invalid("server host is empty".into()));
    }
    let settings = load_accounts(conn).await?;
    let on_host: Vec<&GitHubAccount> = settings
        .accounts
        .iter()
        .filter(|a| host_of_server_url(&a.server_url) == host && serves(a, provider))
        .collect();

    let account = match account_id {
        Some(id) => on_host
            .iter()
            .find(|a| a.id == id)
            .copied()
            .ok_or_else(|| {
                ForgeError::Auth(format!("account {id} not found on host {host}"))
            })?,
        // Nothing configured for this host is the ONE auth miss the user can
        // act on, so it gets its own variant (and with it an i18n key + an
        // "add an account" affordance in the workbench).
        None => on_host
            .iter()
            .find(|a| a.is_default)
            .or_else(|| on_host.first())
            .copied()
            .ok_or_else(|| ForgeError::NoAccount {
                provider,
                host: host.clone(),
            })?,
    };

    let token = crate::keyring_store::get_token(&account.id)
        .ok_or_else(|| ForgeError::Auth(format!("no stored token for account {}", account.id)))?;

    Ok(ResolvedAuth {
        api_base: api_base_for(provider, &host, &account.server_url),
        provider,
        server_host: host,
        account_id: account.id.clone(),
        username: account.username.clone(),
        token,
        scopes: account.scopes.clone(),
    })
}

/// Whether a stored account may be used against `provider`. An account with no
/// declared provider matches anything (legacy rows, and the "generic git
/// account" the settings page has always allowed).
fn serves(account: &GitHubAccount, provider: ForgeProvider) -> bool {
    match account.provider.as_deref().map(str::trim) {
        None | Some("") => true,
        Some(declared) => declared.eq_ignore_ascii_case(provider.as_str()),
    }
}

/// What codeg knows about a host, from the accounts configured for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProfile {
    pub provider: ForgeProvider,
    /// The path the instance is mounted under, without surrounding slashes —
    /// empty for the usual root install. GitLab supports a "relative URL
    /// root" (`https://host/gitlab`), and where one is in use EVERY repository
    /// path in a git remote carries that prefix while no API path does.
    pub base_path: String,
}

/// Which forge lives at `host`, and where it is mounted — decided server-side.
///
/// The user's own accounts come first: adding a GitLab credential for
/// `git.corp.com` IS the statement that `git.corp.com` is a GitLab, and typing
/// `https://host/gitlab` as its server URL is the statement about where. Only
/// when nothing is declared does the hostname get a vote, and the last resort
/// is GitHub — the incumbent, and the behaviour every existing install has.
pub async fn host_profile(conn: &DatabaseConnection, server_host: &str) -> HostProfile {
    let accounts = load_accounts(conn).await.unwrap_or_default().accounts;
    host_profile_in(server_host, &accounts)
}

/// Pure half of [`host_profile`].
pub fn host_profile_in(server_host: &str, accounts: &[GitHubAccount]) -> HostProfile {
    let host = server_host.trim().to_ascii_lowercase();
    // A default account speaks for its host before a non-default one does.
    let on_host: Vec<&GitHubAccount> = {
        let (default, rest): (Vec<_>, Vec<_>) = accounts
            .iter()
            .filter(|a| host_of_server_url(&a.server_url) == host)
            .partition(|a| a.is_default);
        default.into_iter().chain(rest).collect()
    };

    // ONE account speaks for the host, and BOTH answers come from it. Taking
    // them from different accounts is how a stray secondary credential at
    // `https://host/gitlab` could strip a root install's `gitlab/team/app`
    // down to `team/app` and send every call to a repository that is not the
    // one on screen. Preference: the first account that declares a provider
    // (that declaration is the most deliberate thing a user can say about a
    // host), else the default one, else the first.
    let authority = on_host
        .iter()
        .find(|a| {
            a.provider
                .as_deref()
                .is_some_and(|p| ForgeProvider::parse(p).is_ok())
        })
        .or_else(|| on_host.first())
        .copied();

    let provider = authority
        .and_then(|a| a.provider.as_deref())
        .and_then(|p| ForgeProvider::parse(p).ok())
        .unwrap_or_else(|| {
            if host == "gitlab.com" {
                ForgeProvider::GitLab
            } else if host == "github.com" {
                ForgeProvider::GitHub
            } else if host.split('.').any(|label| label == "gitlab") {
                // Self-hosted with nothing declared: the name is all there is.
                // `gitlab` as a whole label is a strong enough hint to take;
                // anything else stays GitHub, which is what this codebase did
                // before GitLab existed.
                ForgeProvider::GitLab
            } else {
                ForgeProvider::GitHub
            }
        });

    // An EMPTY path from the authoritative account is an answer, not a miss:
    // it says this instance is mounted at the root.
    let base_path = authority
        .map(|a| path_of_server_url(&a.server_url))
        .unwrap_or_default();

    HostProfile { provider, base_path }
}

/// The path component of a stored `server_url`, without surrounding slashes.
fn path_of_server_url(server_url: &str) -> String {
    let trimmed = server_url.trim();
    let no_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let after_host = match no_scheme.split_once('/') {
        Some((_, rest)) => rest,
        None => return String::new(),
    };
    after_host
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_matches('/')
        .to_ascii_lowercase()
}

/// Drop the instance's mount path from a repository path parsed out of a git
/// remote. A no-op for the usual root install, and for anything that does not
/// actually start with that prefix — a wrong guess here would rename the
/// repository, so it only ever removes a prefix it can see.
pub fn strip_base_path(owner_repo: &str, base_path: &str) -> String {
    if base_path.is_empty() {
        return owner_repo.to_string();
    }
    let lowered = owner_repo.to_ascii_lowercase();
    let prefix = format!("{base_path}/");
    let Some(rest) = lowered.strip_prefix(&prefix) else {
        return owner_repo.to_string();
    };
    // What is left still has to look like a repository path; a project always
    // lives in a namespace, so a bare name means the prefix was not a mount
    // path at all and the original stands.
    if rest.contains('/') && !rest.starts_with('/') {
        rest.to_string()
    } else {
        owner_repo.to_string()
    }
}

async fn load_accounts(conn: &DatabaseConnection) -> Result<GitHubAccountsSettings, ForgeError> {
    let raw = app_metadata_service::get_value(conn, GITHUB_ACCOUNTS_KEY)
        .await
        .map_err(|e| ForgeError::Network(format!("failed to read accounts: {e}")))?;
    match raw {
        Some(raw) => serde_json::from_str::<GitHubAccountsSettings>(&raw)
            .map_err(|e| ForgeError::Auth(format!("stored accounts unreadable: {e}"))),
        None => Ok(GitHubAccountsSettings::default()),
    }
}

/// Host of a stored account `server_url`. Empty/blank means github.com — the
/// settings UI leaves the field empty for the public service.
pub fn host_of_server_url(server_url: &str) -> String {
    let trimmed = server_url.trim();
    if trimmed.is_empty() {
        return "github.com".to_string();
    }
    let no_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let host_port = no_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let host = host_port.split('@').next_back().unwrap_or("");
    host.split(':').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// REST base for a host. github.com uses the dedicated API host; GitHub
/// Enterprise mounts its API under the instance (`{origin}/api/v3`, the exact
/// derivation `validate_github_token` already uses); GitLab — public and
/// self-hosted alike — always mounts `{origin}/api/v4`.
fn api_base_for(provider: ForgeProvider, host: &str, server_url: &str) -> String {
    if provider == ForgeProvider::GitHub && host == "github.com" {
        return "https://api.github.com".to_string();
    }
    let origin = server_origin(host, server_url);
    match provider {
        ForgeProvider::GitHub => format!("{origin}/api/v3"),
        ForgeProvider::GitLab => format!("{origin}/api/v4"),
    }
}

/// `https://host[:port]` for a stored account. A stored `server_url` is
/// whatever the user typed, which is sometimes a bare hostname — pasting that
/// straight into a request URL would produce a relative path, so the scheme is
/// supplied when it is missing.
fn server_origin(host: &str, server_url: &str) -> String {
    let trimmed = server_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return format!("https://{host}");
    }
    if trimmed.contains("://") {
        return trimmed.to_string();
    }
    format!("https://{trimmed}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_extraction_covers_the_stored_shapes() {
        assert_eq!(host_of_server_url(""), "github.com");
        assert_eq!(host_of_server_url("   "), "github.com");
        assert_eq!(host_of_server_url("https://github.com"), "github.com");
        assert_eq!(host_of_server_url("https://GitHub.com/"), "github.com");
        assert_eq!(host_of_server_url("https://ghe.corp.com"), "ghe.corp.com");
        assert_eq!(host_of_server_url("http://ghe.corp.com:8443/x"), "ghe.corp.com");
        assert_eq!(host_of_server_url("ghe.corp.com"), "ghe.corp.com");
    }

    /// Build a stored account; every test that needs a variant tweaks the
    /// field it is about.
    fn account(id: &str, server_url: &str, provider: Option<&str>) -> GitHubAccount {
        GitHubAccount {
            id: id.into(),
            server_url: server_url.into(),
            username: "alice".into(),
            scopes: vec![],
            avatar_url: None,
            is_default: false,
            created_at: "2026-01-01".into(),
            provider: provider.map(str::to_string),
        }
    }

    #[test]
    fn api_base_matches_validate_github_token_derivation() {
        let gh = ForgeProvider::GitHub;
        assert_eq!(api_base_for(gh, "github.com", ""), "https://api.github.com");
        assert_eq!(
            api_base_for(gh, "github.com", "https://github.com"),
            "https://api.github.com"
        );
        assert_eq!(
            api_base_for(gh, "ghe.corp.com", "https://ghe.corp.com/"),
            "https://ghe.corp.com/api/v3"
        );
        assert_eq!(api_base_for(gh, "ghe.corp.com", ""), "https://ghe.corp.com/api/v3");
        // A bare hostname is what users actually type; without a scheme the
        // result would be a relative path rather than a request URL.
        assert_eq!(
            api_base_for(gh, "ghe.corp.com", "ghe.corp.com"),
            "https://ghe.corp.com/api/v3"
        );
    }

    /// GitLab mounts its API under the instance itself — public and
    /// self-hosted alike, there is no `api.gitlab.com`.
    #[test]
    fn gitlab_api_base_is_always_under_the_instance() {
        let gl = ForgeProvider::GitLab;
        assert_eq!(api_base_for(gl, "gitlab.com", ""), "https://gitlab.com/api/v4");
        assert_eq!(
            api_base_for(gl, "gitlab.com", "https://gitlab.com"),
            "https://gitlab.com/api/v4"
        );
        // The port has to survive: it is part of the origin the pushes use too.
        assert_eq!(
            api_base_for(gl, "gitlab.corp.com", "https://gitlab.corp.com:8443/"),
            "https://gitlab.corp.com:8443/api/v4"
        );
    }

    /// The user's own accounts decide what a host is; the hostname only gets a
    /// vote when nothing is declared, and GitHub stays the last resort so
    /// every existing install keeps behaving exactly as it did.
    #[test]
    fn a_hosts_forge_comes_from_its_accounts_first() {
        assert_eq!(host_profile_in("github.com", &[]).provider, ForgeProvider::GitHub);
        assert_eq!(host_profile_in("gitlab.com", &[]).provider, ForgeProvider::GitLab);
        assert_eq!(host_profile_in("gitlab.corp.com", &[]).provider, ForgeProvider::GitLab);
        // "gitlab" as a whole label only — `mygitlabhost.com` says nothing.
        assert_eq!(host_profile_in("mygitlabhost.com", &[]).provider, ForgeProvider::GitHub);
        assert_eq!(host_profile_in("ghe.corp.com", &[]).provider, ForgeProvider::GitHub);

        // A declared account overrides the hostname guess in both directions.
        let declared = [account("a", "https://git.corp.com", Some("gitlab"))];
        assert_eq!(host_profile_in("git.corp.com", &declared).provider, ForgeProvider::GitLab);
        let declared = [account("a", "https://gitlab.corp.com", Some("github"))];
        assert_eq!(
            host_profile_in("gitlab.corp.com", &declared).provider,
            ForgeProvider::GitHub
        );
        // Accounts on OTHER hosts have no say.
        let elsewhere = [account("a", "https://gitlab.com", Some("gitlab"))];
        assert_eq!(host_profile_in("ghe.corp.com", &elsewhere).provider, ForgeProvider::GitHub);
        // Undeclared accounts (everything stored before GitLab support) do not
        // vote either — they are credentials for a host, not a statement.
        let legacy = [account("a", "https://gitlab.corp.com", None)];
        assert_eq!(host_profile_in("gitlab.corp.com", &legacy).provider, ForgeProvider::GitLab);

        // Two declarations on one host: the default account speaks for it.
        let mut github_default = account("gh", "https://git.corp.com", Some("github"));
        github_default.is_default = true;
        let mixed = [account("gl", "https://git.corp.com", Some("gitlab")), github_default];
        assert_eq!(host_profile_in("git.corp.com", &mixed).provider, ForgeProvider::GitHub);
    }

    /// A GitLab mounted under a relative URL root (`https://host/gitlab`) puts
    /// that prefix in front of every repository path a git remote carries,
    /// while no API path has it. Without stripping, the project addressed over
    /// REST would be `gitlab%2Fgroup%2Fproj` — a project that does not exist —
    /// and the push URL would repeat the prefix twice.
    #[test]
    fn an_instance_mount_path_is_learned_from_its_account_and_stripped() {
        let mounted = [account("a", "https://host.example/gitlab", Some("gitlab"))];
        let profile = host_profile_in("host.example", &mounted);
        assert_eq!(profile.base_path, "gitlab");
        assert_eq!(profile.provider, ForgeProvider::GitLab);
        assert_eq!(strip_base_path("gitlab/group/proj", &profile.base_path), "group/proj");
        // Deeper mounts and trailing slashes are the same statement.
        let deep = [account("a", "https://host.example/tools/gitlab/", Some("gitlab"))];
        assert_eq!(host_profile_in("host.example", &deep).base_path, "tools/gitlab");

        // The ordinary root install: nothing to learn, nothing to strip.
        let root = [account("a", "https://gitlab.com", Some("gitlab"))];
        assert_eq!(host_profile_in("gitlab.com", &root).base_path, "");
        assert_eq!(strip_base_path("group/proj", ""), "group/proj");

        // BOTH answers come from ONE account. A stray second credential that
        // happens to name a path must not make a root install look mounted —
        // that would strip a real namespace off every repository on the host.
        let mut authoritative = account("root", "https://host.example", Some("gitlab"));
        authoritative.is_default = true;
        let mixed = [
            account("stray", "https://host.example/gitlab", None),
            authoritative,
        ];
        let profile = host_profile_in("host.example", &mixed);
        assert_eq!(profile.base_path, "", "the declaring account speaks, path and all");
        assert_eq!(profile.provider, ForgeProvider::GitLab);
        assert_eq!(strip_base_path("gitlab/team/app", &profile.base_path), "gitlab/team/app");

        // Stripping only ever removes a prefix it can actually see, and never
        // leaves something that is no longer a repository path — a project
        // always lives in a namespace.
        assert_eq!(strip_base_path("group/proj", "gitlab"), "group/proj");
        assert_eq!(strip_base_path("gitlab/proj", "gitlab"), "gitlab/proj");
        assert_eq!(strip_base_path("GitLab/Group/Proj", "gitlab"), "group/proj");
    }

    /// A GitLab token handed to the GitHub client buys a 401 that reads like
    /// an expired credential. Declared accounts serve only their own forge;
    /// undeclared ones serve whatever the host is.
    #[test]
    fn accounts_only_serve_the_forge_they_declare() {
        assert!(serves(&account("a", "", Some("gitlab")), ForgeProvider::GitLab));
        assert!(!serves(&account("a", "", Some("gitlab")), ForgeProvider::GitHub));
        assert!(serves(&account("a", "", Some("GitHub")), ForgeProvider::GitHub));
        assert!(serves(&account("a", "", None), ForgeProvider::GitLab));
        assert!(serves(&account("a", "", Some("  ")), ForgeProvider::GitHub));
    }

    /// Error paths never touch the token store, so they are safe to assert in
    /// BOTH modes (the happy path needs a token and is covered below in the
    /// server-mode-only test — the desktop build would write the real OS
    /// keyring, which a unit test must never do).
    #[tokio::test]
    async fn resolution_rejects_wrong_host_and_missing_accounts() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let mut only = account("acc-x", "https://github.com", None);
        only.is_default = true;
        let settings = GitHubAccountsSettings {
            accounts: vec![only],
        };
        app_metadata_service::upsert_value(
            &db.conn,
            GITHUB_ACCOUNTS_KEY,
            &serde_json::to_string(&settings).unwrap(),
        )
        .await
        .unwrap();
        let gh = ForgeProvider::GitHub;
        // A pinned id on the WRONG host is an error, not a silent substitute.
        assert!(matches!(
            resolve_forge_auth(&db.conn, gh, "ghe.corp.com", Some("acc-x")).await,
            Err(ForgeError::Auth(_))
        ));
        // Unknown host: no account at all — its OWN variant, because that is
        // the miss the workbench turns into "add an account".
        assert!(matches!(
            resolve_forge_auth(&db.conn, gh, "nowhere.example", None).await,
            Err(ForgeError::NoAccount { .. })
        ));
        // Blank host is invalid input, not an auth miss.
        assert!(matches!(
            resolve_forge_auth(&db.conn, gh, "  ", None).await,
            Err(ForgeError::Invalid(_))
        ));
    }

    /// Server mode only: the file-backed token store can be pointed at a temp
    /// dir. Desktop would hit the real OS keyring — never in a unit test.
    #[cfg(not(feature = "tauri-runtime"))]
    #[tokio::test]
    async fn resolution_prefers_pinned_id_then_default_and_redacts_debug() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let db_conn = db.conn.clone();
        let dir = tmp.path().to_string_lossy().to_string();
        temp_env::async_with_vars([("CODEG_DATA_DIR", Some(dir.as_str()))], async move {
            let db = db_conn;
            resolution_happy_path(&db).await;
        })
        .await;
    }

    #[cfg(not(feature = "tauri-runtime"))]
    async fn resolution_happy_path(conn: &sea_orm::DatabaseConnection) {
        let mut default = account("acc-default", "https://github.com", None);
        default.is_default = true;
        default.scopes = vec!["repo".into()];
        let mut alt = account("acc-alt", "", None);
        alt.username = "bob".into();
        let mut ghe = account("acc-ghe", "https://ghe.corp.com", None);
        ghe.username = "carol".into();
        let mut gitlab = account("acc-gitlab", "https://gitlab.com", Some("gitlab"));
        gitlab.username = "dan".into();
        let settings = GitHubAccountsSettings {
            accounts: vec![default, alt, ghe, gitlab],
        };
        app_metadata_service::upsert_value(
            conn,
            GITHUB_ACCOUNTS_KEY,
            &serde_json::to_string(&settings).unwrap(),
        )
        .await
        .unwrap();
        crate::keyring_store::set_token("acc-alt", "tok-alt").unwrap();
        crate::keyring_store::set_token("acc-default", "tok-default").unwrap();
        crate::keyring_store::set_token("acc-gitlab", "tok-gitlab").unwrap();
        let gh = ForgeProvider::GitHub;

        // Pinned id wins over the default account.
        let pinned = resolve_forge_auth(conn, gh, "github.com", Some("acc-alt"))
            .await
            .unwrap();
        assert_eq!(pinned.account_id, "acc-alt");
        // The pinned account's own login rides along — the push identity.
        assert_eq!(pinned.username, "bob");
        assert_eq!(pinned.api_base, "https://api.github.com");
        let shown = format!("{pinned:?}");
        assert!(shown.contains("<redacted>") && !shown.contains("tok-alt"));

        // No pin → the host's default.
        let by_default = resolve_forge_auth(conn, gh, "github.com", None).await.unwrap();
        assert_eq!(by_default.account_id, "acc-default");

        // A pinned id on the WRONG host is an error, not a silent substitute.
        assert!(matches!(
            resolve_forge_auth(conn, gh, "ghe.corp.com", Some("acc-alt")).await,
            Err(ForgeError::Auth(_))
        ));
        // GHE host derives the /api/v3 base — but without a stored token the
        // resolution must fail rather than hand back an unusable identity.
        // Still plain `Auth`: an account IS configured here, so "add an
        // account" would be the wrong advice.
        assert!(matches!(
            resolve_forge_auth(conn, gh, "ghe.corp.com", None).await,
            Err(ForgeError::Auth(_))
        ));
        // Unknown host: no account at all — the actionable variant.
        assert!(matches!(
            resolve_forge_auth(conn, gh, "nowhere.example", None).await,
            Err(ForgeError::NoAccount { .. })
        ));

        // The GitLab account resolves for GitLab, with GitLab's API base…
        let gl = resolve_forge_auth(conn, ForgeProvider::GitLab, "gitlab.com", None)
            .await
            .unwrap();
        assert_eq!((gl.account_id.as_str(), gl.api_base.as_str()), ("acc-gitlab", "https://gitlab.com/api/v4"));
        // …and is invisible to the GitHub client, which would only spend it on
        // a 401 that reads like an expired token.
        assert!(matches!(
            resolve_forge_auth(conn, gh, "gitlab.com", Some("acc-gitlab")).await,
            Err(ForgeError::Auth(_))
        ));
    }
}
