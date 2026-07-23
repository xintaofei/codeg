use serde::Serialize;
use std::sync::Mutex;

use crate::acp::binary_cache;
use crate::acp::registry::{self, AgentDistribution};
use crate::models::agent::AgentType;

/// Cache for npm environment check results.
/// Stores `Some(checks)` after a successful (all-pass) run;
/// stays `None` if checks failed so they are retried next time.
static NPM_ENV_CACHE: Mutex<Option<Vec<CheckItem>>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FixActionKind {
    OpenUrl,
    InstallOpencodePlugins,
    InstallUv,
}

#[derive(Debug, Clone, Serialize)]
pub struct FixAction {
    pub label: String,
    pub kind: FixActionKind,
    pub payload: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Fail,
    Warn,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckItem {
    pub check_id: String,
    pub label: String,
    pub status: CheckStatus,
    pub message: String,
    pub fixes: Vec<FixAction>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreflightResult {
    pub agent_type: AgentType,
    pub agent_name: String,
    pub passed: bool,
    pub checks: Vec<CheckItem>,
}

pub fn clear_npm_env_cache() {
    *NPM_ENV_CACHE.lock().unwrap() = None;
}

pub async fn run_preflight(agent_type: AgentType) -> PreflightResult {
    let meta = registry::get_agent_meta(agent_type);
    debug_assert_eq!(meta.agent_type, agent_type);
    let checks = match &meta.distribution {
        AgentDistribution::Npx { node_required, .. } => check_npm_environment(*node_required).await,
        AgentDistribution::Binary {
            version,
            cmd,
            platforms,
            ..
        } => check_binary_environment(agent_type, version, cmd, platforms).await,
        AgentDistribution::Uvx {
            uv_required,
            system_cmd,
            ..
        } => check_uv_environment(*uv_required, *system_cmd).await,
    };

    let passed = checks
        .iter()
        .all(|c| !matches!(c.status, CheckStatus::Fail));

    PreflightResult {
        agent_type,
        agent_name: meta.name.to_string(),
        passed,
        checks,
    }
}

async fn check_npm_environment(node_required: Option<&str>) -> Vec<CheckItem> {
    // Return cached result if a previous check passed.
    // The cache stores only the base checks (node_available + npm_available);
    // the per-agent node_version check is appended separately.
    let cached = NPM_ENV_CACHE.lock().unwrap().clone();
    if let Some(cached) = cached {
        let mut checks = cached;
        if let Some(required) = node_required {
            // Extract node version string from the cached node_available message
            // (format: "Node.js v20.19.0 available")
            let node_ver = extract_node_version_from_message(&checks[0].message);
            checks.push(build_node_version_check(node_ver.as_deref(), required));
        }
        return checks;
    }

    // Resolve absolute paths via `which` crate to avoid GUI PATH issues,
    // then run version checks in parallel.
    let node_path = which::which("node").ok();
    let npm_path = which::which("npm").ok();

    let (node_result, npm_result) = tokio::join!(
        async {
            match &node_path {
                Some(p) => {
                    crate::process::tokio_command(p)
                        .arg("--version")
                        .output()
                        .await
                }
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "node not found in PATH",
                )),
            }
        },
        async {
            match &npm_path {
                Some(p) => {
                    crate::process::tokio_command(p)
                        .arg("--version")
                        .output()
                        .await
                }
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "npm not found in PATH",
                )),
            }
        },
    );

    // Track the raw node version string for reuse in the version check
    let mut node_version_str: Option<String> = None;

    let node_check = match node_result {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            node_version_str = Some(version.clone());
            CheckItem {
                check_id: "node_available".into(),
                label: "Node.js".into(),
                status: CheckStatus::Pass,
                message: format!("Node.js {version} available"),
                fixes: vec![],
            }
        }
        _ => CheckItem {
            check_id: "node_available".into(),
            label: "Node.js".into(),
            status: CheckStatus::Fail,
            message: "Node.js is not installed or not in PATH".into(),
            fixes: vec![FixAction {
                label: "Install Node.js".into(),
                kind: FixActionKind::OpenUrl,
                payload: "https://nodejs.org/".into(),
            }],
        },
    };

    let npm_check = match npm_result {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            CheckItem {
                check_id: "npm_available".into(),
                label: "npm".into(),
                status: CheckStatus::Pass,
                message: format!("npm {version} available"),
                fixes: vec![],
            }
        }
        _ => CheckItem {
            check_id: "npm_available".into(),
            label: "npm".into(),
            status: CheckStatus::Fail,
            message: "npm is not installed or not in PATH".into(),
            fixes: vec![FixAction {
                label: "Install Node.js".into(),
                kind: FixActionKind::OpenUrl,
                payload: "https://nodejs.org/".into(),
            }],
        },
    };

    let mut checks = vec![node_check, npm_check];

    // Cache only if all checks passed — failed results are not cached so
    // the user can retry after installing the missing tools.
    let all_passed = checks
        .iter()
        .all(|c| !matches!(c.status, CheckStatus::Fail));
    if all_passed {
        *NPM_ENV_CACHE.lock().unwrap() = Some(checks.clone());
    }

    // After caching the base checks, append the per-agent Node.js version
    // requirement if specified. Only meaningful when node is available.
    if let Some(required) = node_required {
        if all_passed {
            checks.push(build_node_version_check(
                node_version_str.as_deref(),
                required,
            ));
        }
    }

    checks
}

/// Parse a Node.js version string like "v20.19.0" or "20.19.0" into (major, minor, patch).
/// Handles pre-release suffixes such as "v22.0.0-nightly" by stripping non-numeric tails.
fn parse_node_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches('v');
    let mut parts = v.splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch_str = parts.next()?;
    // Strip pre-release/build suffixes: "0-nightly" → "0", "3+build" → "3"
    let patch_digits: String = patch_str
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let patch = patch_digits.parse().ok()?;
    Some((major, minor, patch))
}

/// Extract the node version string from a cached node_available message.
/// Expected format: "Node.js v20.19.0 available" → Some("v20.19.0")
fn extract_node_version_from_message(message: &str) -> Option<String> {
    message
        .split_whitespace()
        .find(|s| s.starts_with('v') && s.contains('.'))
        .map(|s| s.to_string())
}

/// Build a `CheckItem` for the Node.js version requirement check.
/// `current_version` is the raw output from `node --version` (e.g. "v20.19.0").
fn build_node_version_check(current_version: Option<&str>, required: &str) -> CheckItem {
    let current_version = match current_version {
        Some(v) => v,
        None => {
            return CheckItem {
                check_id: "node_version".into(),
                label: "Node.js version".into(),
                status: CheckStatus::Fail,
                message: "Cannot determine Node.js version".into(),
                fixes: vec![],
            };
        }
    };

    let current = parse_node_version(current_version);
    let required_parsed = parse_node_version(required);

    match (current, required_parsed) {
        (Some(cur), Some(req)) if cur >= req => CheckItem {
            check_id: "node_version".into(),
            label: "Node.js version".into(),
            status: CheckStatus::Pass,
            message: format!(
                "Node.js {current_version} meets the minimum requirement (>={required})"
            ),
            fixes: vec![],
        },
        (Some(_), Some(_)) => CheckItem {
            check_id: "node_version".into(),
            label: "Node.js version".into(),
            status: CheckStatus::Fail,
            message: format!(
                "Node.js {current_version} is too old — this package requires Node.js >={required}"
            ),
            fixes: vec![FixAction {
                label: "Update Node.js".into(),
                kind: FixActionKind::OpenUrl,
                payload: "https://nodejs.org/".into(),
            }],
        },
        _ => CheckItem {
            check_id: "node_version".into(),
            label: "Node.js version".into(),
            status: CheckStatus::Warn,
            message: format!("Cannot parse Node.js version; required >={required}"),
            fixes: vec![],
        },
    }
}

/// Preflight for `Uvx` agents (Python ACP agents launched via `uvx`, e.g.
/// Hermes). Passes when either the `uv` tool runner is resolvable, or — as a
/// fallback — the agent's own CLI is already installed on PATH.
async fn check_uv_environment(
    uv_required: Option<&str>,
    system_cmd: Option<(&str, &[&str])>,
) -> Vec<CheckItem> {
    // Primary: the `uv` tool runner (uvx) fetches + launches the agent package.
    if let Some(uvx_path) = crate::commands::acp::resolve_uvx_command() {
        let version = run_uv_version(&uvx_path).await;
        let mut checks = vec![CheckItem {
            check_id: "uv_available".into(),
            label: "uv".into(),
            status: CheckStatus::Pass,
            message: match &version {
                Some(v) => format!("uv {v} available"),
                None => "uv available".into(),
            },
            fixes: vec![],
        }];
        if let Some(required) = uv_required {
            checks.push(build_uv_version_check(version.as_deref(), required));
        }
        return checks;
    }

    // Fallback: the agent's own CLI is already installed on PATH (e.g. a user
    // who ran the official installer has `hermes` available). The agent is
    // launchable as-is, but installing uv unlocks codeg's managed install /
    // upgrade flow, so offer it as a non-blocking action.
    if let Some((cmd, _)) = system_cmd {
        if crate::commands::acp::resolve_command_on_path(cmd).is_some() {
            return vec![CheckItem {
                check_id: "uv_available".into(),
                label: "uv".into(),
                status: CheckStatus::Warn,
                message: format!(
                    "uv not found; will launch via the system `{cmd}` command on PATH. Install uv to enable managed install/upgrade."
                ),
                fixes: vec![FixAction {
                    label: "Install uv".into(),
                    kind: FixActionKind::InstallUv,
                    payload: String::new(),
                }],
            }];
        }
    }

    // uv is required and not installed: a hard failure with an actionable
    // installer. Installing uv is a separate step from installing the agent.
    vec![CheckItem {
        check_id: "uv_available".into(),
        label: "uv".into(),
        status: CheckStatus::Fail,
        message: "uv (the Python tool runner) is not installed. Click Install uv to set it up."
            .into(),
        fixes: vec![FixAction {
            label: "Install uv".into(),
            kind: FixActionKind::InstallUv,
            payload: String::new(),
        }],
    }]
}

/// Run `<uvx> --version` and extract the version token (output looks like
/// "uvx 0.8.10 (hash date)").
async fn run_uv_version(uvx_path: &std::path::Path) -> Option<String> {
    let output = crate::process::tokio_command(uvx_path)
        .arg("--version")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace().nth(1).map(|s| s.to_string())
}

/// Build a `CheckItem` for the `uv` minimum-version requirement. Too-old is a
/// `Warn` (not `Fail`): recent uv releases are backward compatible for the
/// `uvx --from <pkg>==<ver>` invocation, so an old uv should not hard-block.
fn build_uv_version_check(current: Option<&str>, required: &str) -> CheckItem {
    match (current.and_then(parse_node_version), parse_node_version(required)) {
        (Some(cur), Some(req)) if cur >= req => CheckItem {
            check_id: "uv_version".into(),
            label: "uv version".into(),
            status: CheckStatus::Pass,
            message: format!("uv {} meets the minimum requirement (>={required})", current.unwrap_or("")),
            fixes: vec![],
        },
        (Some(_), Some(_)) => CheckItem {
            check_id: "uv_version".into(),
            label: "uv version".into(),
            status: CheckStatus::Warn,
            message: format!(
                "uv {} is older than the recommended >={required}; consider `uv self update`",
                current.unwrap_or("")
            ),
            fixes: vec![],
        },
        _ => CheckItem {
            check_id: "uv_version".into(),
            label: "uv version".into(),
            status: CheckStatus::Warn,
            message: format!("Cannot parse uv version; recommended >={required}"),
            fixes: vec![],
        },
    }
}

/// The registry `dir_entry` for a binary agent (None for single-file agents
/// and non-binary distributions).
fn binary_dir_entry(agent_type: AgentType) -> Option<registry::BinaryDirEntry> {
    match registry::get_agent_meta(agent_type).distribution {
        AgentDistribution::Binary { dir_entry, .. } => dir_entry,
        _ => None,
    }
}

async fn check_binary_environment(
    agent_type: AgentType,
    version: &str,
    cmd: &str,
    platforms: &[registry::PlatformBinary],
) -> Vec<CheckItem> {
    let mut checks = Vec::new();

    // Check platform support
    let current = registry::current_platform();
    let platform_supported = platforms.iter().any(|p| p.platform == current);

    let platform_check = if platform_supported {
        CheckItem {
            check_id: "platform_supported".into(),
            label: "Platform".into(),
            status: CheckStatus::Pass,
            message: format!("Platform {current} is supported"),
            fixes: vec![],
        }
    } else {
        CheckItem {
            check_id: "platform_supported".into(),
            label: "Platform".into(),
            status: CheckStatus::Fail,
            message: format!("Platform {current} is not supported"),
            fixes: vec![],
        }
    };
    checks.push(platform_check);

    // Check binary cache.
    //
    // Pass as long as *any* cached version is present — the session-page
    // connect path uses the best cached version via
    // `find_best_cached_binary_for_agent`, so an older-but-working cache
    // should still be considered "ready". If the cached version differs
    // from the registry's recommended version, we note it in the message
    // but still pass — the Settings page's version-badge flow is the
    // canonical place to surface "upgrade available".
    if platform_supported {
        let cache_check = match binary_cache::find_best_cached_binary_for_agent(agent_type, cmd) {
            Ok(Some((_, cached_version))) => {
                let message = if cached_version == version {
                    "Binary is cached locally".to_string()
                } else {
                    format!("Binary {cached_version} is cached locally (recommended: {version})")
                };
                CheckItem {
                    check_id: "binary_cached".into(),
                    label: "Binary cache".into(),
                    status: CheckStatus::Pass,
                    message,
                    fixes: vec![],
                }
            }
            // Dir-tree agents (Cursor): a user-installed CLI on PATH /
            // ~/.local/bin is launchable as-is — the connect path falls back
            // to it — so report ready instead of a misleading warn.
            Ok(None)
                if binary_dir_entry(agent_type).is_some()
                    && crate::commands::acp::resolve_system_agent_binary(cmd).is_some() =>
            {
                CheckItem {
                    check_id: "binary_cached".into(),
                    label: "Binary cache".into(),
                    status: CheckStatus::Pass,
                    message: format!(
                        "Using the system-installed {cmd} (codeg-managed download also available)"
                    ),
                    fixes: vec![],
                }
            }
            Ok(None) => CheckItem {
                check_id: "binary_cached".into(),
                label: "Binary cache".into(),
                status: CheckStatus::Warn,
                message:
                    "Binary is not installed. Download it from Agent Settings before connecting."
                        .into(),
                fixes: vec![],
            },
            Err(_) => CheckItem {
                check_id: "binary_cached".into(),
                label: "Binary cache".into(),
                status: CheckStatus::Warn,
                message: "Cannot determine binary cache path".into(),
                fixes: vec![],
            },
        };
        checks.push(cache_check);
    }

    // OpenCode plugin checks
    if agent_type == AgentType::OpenCode {
        use crate::acp::opencode_plugins::{self, spec_has_floating_version, PluginStatus};
        match opencode_plugins::check_opencode_plugins(None) {
            Ok(summary) => {
                let missing: Vec<_> = summary
                    .plugins
                    .iter()
                    .filter(|p| p.status == PluginStatus::Missing)
                    .collect();

                if summary.plugins.is_empty() {
                    checks.push(CheckItem {
                        check_id: "opencode_plugins".into(),
                        label: "OpenCode plugins".into(),
                        status: CheckStatus::Pass,
                        message: "No plugins declared".into(),
                        fixes: vec![],
                    });
                } else if missing.is_empty() {
                    checks.push(CheckItem {
                        check_id: "opencode_plugins".into(),
                        label: "OpenCode plugins".into(),
                        status: CheckStatus::Pass,
                        message: format!("{} plugin(s) installed", summary.plugins.len()),
                        fixes: vec![],
                    });
                } else {
                    let names: Vec<&str> = missing.iter().map(|p| p.name.as_str()).collect();
                    checks.push(CheckItem {
                        check_id: "opencode_plugins".into(),
                        label: "OpenCode plugins".into(),
                        status: CheckStatus::Fail,
                        message: format!(
                            "{} plugin(s) not installed: {}",
                            missing.len(),
                            names.join(", ")
                        ),
                        fixes: vec![FixAction {
                            label: "Install Plugins".into(),
                            kind: FixActionKind::InstallOpencodePlugins,
                            payload: String::new(),
                        }],
                    });
                }

                // Warn about @latest specs that cause slow startup
                let floating: Vec<&str> = summary
                    .plugins
                    .iter()
                    .filter(|p| spec_has_floating_version(&p.declared_spec))
                    .map(|p| p.name.as_str())
                    .collect();
                if !floating.is_empty() {
                    checks.push(CheckItem {
                        check_id: "opencode_plugins_floating".into(),
                        label: "Plugin versions".into(),
                        status: CheckStatus::Warn,
                        message: format!(
                            "{} plugin(s) use @latest which forces a network check on every startup: {}. \
                             Install via the plugin manager to auto-pin versions.",
                            floating.len(),
                            floating.join(", ")
                        ),
                        fixes: vec![FixAction {
                            label: "Install Plugins".into(),
                            kind: FixActionKind::InstallOpencodePlugins,
                            payload: String::new(),
                        }],
                    });
                }

                // Project-level config hint
                if summary.has_project_config_hint {
                    checks.push(CheckItem {
                        check_id: "opencode_project_config_hint".into(),
                        label: "Project config".into(),
                        status: CheckStatus::Warn,
                        message:
                            "Project-level opencode config detected; its plugins are not checked. \
                             Expect slower first connect if it declares plugins."
                                .into(),
                        fixes: vec![],
                    });
                }
            }
            Err(e) => {
                checks.push(CheckItem {
                    check_id: "opencode_plugins".into(),
                    label: "OpenCode plugins".into(),
                    status: CheckStatus::Warn,
                    message: format!("Failed to parse opencode.json: {e}"),
                    fixes: vec![],
                });
            }
        }
    }

    checks
}
