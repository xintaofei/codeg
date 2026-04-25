use crate::app_error::AppCommandError;
use crate::models::SystemProxySettings;
use crate::network::http_client;

const PROXY_ENV_KEYS: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

/// Apply proxy settings.
///
/// Two effects:
/// 1. Pushes the snapshot into the [`http_client`] factory so all reqwest
///    calls inside this process pick up the new proxy on next build.
/// 2. Mirrors into the process environment so child processes (`git`, ACP
///    binaries, MCP subprocesses) inherit it. The env mutation runs only
///    on settings changes, which under Tauri/Axum happens from the command
///    handler thread; that's still racy in theory but matches prior
///    behavior — the alternative (drop env vars entirely) would silently
///    break every spawned subprocess that relies on proxy.
pub fn apply_system_proxy_settings(settings: &SystemProxySettings) -> Result<(), AppCommandError> {
    if settings.enabled {
        let proxy_url = settings
            .proxy_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppCommandError::configuration_missing(
                    "Proxy URL is required when proxy is enabled",
                )
            })?;

        for key in PROXY_ENV_KEYS {
            // SAFETY: see module-level comment. Called from a command handler
            // before child processes are spawned for the new request.
            unsafe {
                std::env::set_var(key, proxy_url);
            }
        }
    } else {
        clear_proxy_env();
    }

    http_client::set_proxy_settings(settings.clone());
    Ok(())
}

pub fn clear_proxy_env() {
    for key in PROXY_ENV_KEYS {
        // SAFETY: see apply_system_proxy_settings.
        unsafe {
            std::env::remove_var(key);
        }
    }
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
