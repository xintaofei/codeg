use sea_orm::DatabaseConnection;

use crate::app_error::AppCommandError;
use crate::models::SystemProxySettings;

const PROXY_ENV_KEYS: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

/// Canonicalize a user-entered proxy address into a URL that carries an
/// explicit scheme.
///
/// `reqwest` accepts a scheme-less `host:port` because it silently retries the
/// parse as `http://{input}` — but it keeps that repair to itself, so a bare
/// `127.0.0.1:7890` used to survive validation and land verbatim in
/// `HTTP_PROXY`. Every in-process reqwest call then worked (it repairs the env
/// value the same way) while every spawned child died: npm parses the value
/// with WHATWG `new URL()`, where a scheme may not start with a digit, and
/// aborts with a bare `ERR_INVALID_URL` before touching the network. Doing the
/// repair here — once, at the boundary — keeps both sides reading the same
/// address.
///
/// A value that already names a scheme is returned untouched, so `socks5://`
/// and `https://` proxies are never rewritten to `http://`.
pub(crate) fn normalize_proxy_url(raw: &str) -> Result<String, AppCommandError> {
    let trimmed = raw.trim();
    let normalized = if needs_http_prefix(trimmed) {
        format!("http://{trimmed}")
    } else {
        trimmed.to_string()
    };

    let parsed = reqwest::Url::parse(&normalized).map_err(|e| {
        AppCommandError::configuration_invalid("Invalid proxy URL").with_detail(e.to_string())
    })?;
    if !names_a_host(&parsed) {
        return Err(AppCommandError::configuration_invalid("Invalid proxy URL")
            .with_detail("a proxy address must include a host, e.g. http://127.0.0.1:7890"));
    }
    reqwest::Proxy::all(&normalized).map_err(|e| {
        AppCommandError::configuration_invalid("Invalid proxy URL").with_detail(e.to_string())
    })?;

    Ok(normalized)
}

/// Whether the URL actually names a host. `has_host()` is not enough: for a
/// non-special scheme the `url` crate reports the empty authority in `socks5://`
/// as a host, so it answers `true` for an address that can never be dialled.
fn names_a_host(url: &reqwest::Url) -> bool {
    url.host_str().is_some_and(|host| !host.is_empty())
}

/// Whether `value` is an abbreviated `host:port` that needs `http://` to become
/// a usable proxy URL. The single source of truth for the repair decision, so
/// what [`normalize_proxy_url`] rewrites and what
/// [`proxy_env_vars_missing_scheme`] reports can never disagree.
///
/// Naming a host — not "did it parse" — is the discriminator: `localhost:7890`
/// and `proxy.corp.com:8080` parse just fine, as URLs whose *scheme* is
/// `localhost` / `proxy.corp.com` and whose path is the port. Only a real scheme
/// leaves a host behind.
fn needs_http_prefix(trimmed: &str) -> bool {
    // Something that already spells out a scheme separator is malformed rather
    // than abbreviated (`http://` with no host); prefixing it would launder
    // nonsense into a URL that parses (`http://http://`).
    if trimmed.contains("://") {
        return false;
    }
    !reqwest::Url::parse(trimmed).is_ok_and(|url| names_a_host(&url))
}

/// The value these settings should put in the proxy env vars: `None` when the
/// proxy is disabled (meaning "clear them"), else the normalized URL.
///
/// Split out from [`apply_system_proxy_settings`] so the decision can be tested
/// without mutating process env — an env write would race every other test in
/// the binary.
pub(crate) fn proxy_env_value(
    settings: &SystemProxySettings,
) -> Result<Option<String>, AppCommandError> {
    if !settings.enabled {
        return Ok(None);
    }

    let proxy_url = settings
        .proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppCommandError::configuration_missing("Proxy URL is required when proxy is enabled")
        })?;

    normalize_proxy_url(proxy_url).map(Some)
}

pub fn apply_system_proxy_settings(settings: &SystemProxySettings) -> Result<(), AppCommandError> {
    // Normalize here as well as at the save path: this is the single choke
    // point for env writes, so no caller can leak an un-prefixed address into a
    // child process even if it bypassed `normalize_proxy_settings`.
    match proxy_env_value(settings)? {
        Some(proxy_url) => {
            for key in PROXY_ENV_KEYS {
                unsafe {
                    std::env::set_var(key, &proxy_url);
                }
            }
        }
        None => clear_proxy_env(),
    }

    Ok(())
}

pub fn clear_proxy_env() {
    for key in PROXY_ENV_KEYS {
        unsafe {
            std::env::remove_var(key);
        }
    }
}

/// Load persisted proxy settings from the DB and apply them to process env.
/// Must run before the first reqwest client is built — otherwise that client
/// caches the proxy-less config and ignores the user's choice for its lifetime.
/// Errors are logged and dropped: a misconfigured proxy must not block startup.
///
/// Only writes env vars when the DB explicitly stores `enabled=true`. A fresh
/// install or an explicit disable in the UI leaves externally-set HTTP_PROXY
/// alone, so docker `-e` and systemd `Environment=` keep working. Runtime
/// disable through `update_system_proxy_settings` still clears env — that path
/// is the user's explicit intent, not a default.
pub async fn init_proxy_from_db(conn: &DatabaseConnection) {
    match crate::commands::system_settings::load_system_proxy_settings(conn).await {
        Ok(settings) if settings.enabled => {
            if let Err(err) = apply_system_proxy_settings(&settings) {
                tracing::error!("[Settings] failed to apply system proxy settings: {err}");
            }
        }
        Ok(_) => {}
        Err(err) => {
            tracing::error!("[Settings] failed to load system proxy settings: {err}");
        }
    }
}

/// Names of the proxy env vars currently holding a scheme-less address, e.g.
/// `HTTPS_PROXY=127.0.0.1:7890`.
///
/// codeg's own settings can no longer produce one (they are normalized before
/// export), but the startup contract deliberately leaves externally-provided
/// values alone — a docker `-e` or a shell export can still carry a bare
/// `host:port`. Node-based tooling rejects those outright, so callers use this
/// to turn an opaque `Invalid URL` into an actionable message.
pub(crate) fn proxy_env_vars_missing_scheme() -> Vec<String> {
    current_proxy_env_vars()
        .into_iter()
        .filter(|(_, value)| needs_http_prefix(value))
        .map(|(key, _)| key)
        .collect()
}

pub fn current_proxy_env_vars() -> Vec<(String, String)> {
    PROXY_ENV_KEYS
        .iter()
        .filter_map(|key| {
            std::env::var(key).ok().and_then(|value| {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(((*key).to_string(), trimmed.to_string()))
                }
            })
        })
        .collect()
}
