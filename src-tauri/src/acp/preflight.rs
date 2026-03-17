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

    // Run node and npm checks in parallel
    let (node_result, npm_result) = tokio::join!(
        crate::process::tokio_command("node")
            .arg("--version")
            .output(),
        crate::process::tokio_command("npm")
            .arg("--version")
            .output(),
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

    // Check binary cache
    if platform_supported {
        let cache_check = match binary_cache::find_cached_binary_for_agent(agent_type, version, cmd)
        {
            Ok(Some(_)) => CheckItem {
                check_id: "binary_cached".into(),
                label: "Binary cache".into(),
                status: CheckStatus::Pass,
                message: "Binary is cached locally".into(),
                fixes: vec![],
            },
            Ok(None) => CheckItem {
                check_id: "binary_cached".into(),
                label: "Binary cache".into(),
                status: CheckStatus::Warn,
                message: "Binary not cached yet, will be downloaded on first connection".into(),
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

    checks
}
