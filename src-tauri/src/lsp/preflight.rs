use serde::Serialize;
use std::sync::Mutex;

use crate::acp::registry::PlatformBinary;
use crate::lsp::binary_cache;
use crate::lsp::registry::{self, LspDistribution};

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
pub struct LspPreflightResult {
    pub server_id: String,
    pub server_name: String,
    pub passed: bool,
    pub checks: Vec<CheckItem>,
}

pub fn clear_npm_env_cache() {
    *NPM_ENV_CACHE.lock().unwrap() = None;
}

pub async fn run_preflight(server_id: &str) -> LspPreflightResult {
    let meta = match registry::get_lsp_meta(server_id) {
        Some(m) => m,
        None => {
            return LspPreflightResult {
                server_id: server_id.to_string(),
                server_name: server_id.to_string(),
                passed: false,
                checks: vec![CheckItem {
                    check_id: "registry".into(),
                    label: "Registry".into(),
                    status: CheckStatus::Fail,
                    message: format!("Server '{server_id}' not found in registry"),
                    fixes: vec![],
                }],
            };
        }
    };

    let checks = match &meta.distribution {
        LspDistribution::Npm { node_required, .. } => {
            check_npm_environment(*node_required).await
        }
        LspDistribution::Binary {
            version,
            cmd,
            platforms,
            ..
        } => check_binary_environment(server_id, version, cmd, platforms).await,
        LspDistribution::CargoInstall { .. } => check_cargo_environment().await,
        LspDistribution::PipInstall {
            python_required, ..
        } => check_pip_environment(*python_required).await,
    };

    let passed = checks
        .iter()
        .all(|c| !matches!(c.status, CheckStatus::Fail));

    LspPreflightResult {
        server_id: server_id.to_string(),
        server_name: meta.name.to_string(),
        passed,
        checks,
    }
}

async fn check_npm_environment(node_required: Option<&str>) -> Vec<CheckItem> {
    let cached = NPM_ENV_CACHE.lock().unwrap().clone();
    if let Some(cached) = cached {
        let mut checks = cached;
        if let Some(required) = node_required {
            let node_ver = extract_node_version_from_message(&checks[0].message);
            checks.push(build_node_version_check(node_ver.as_deref(), required));
        }
        return checks;
    }

    let node_path = which::which("node").ok();
    let npm_path = which::which("npm").ok();

    let (node_result, npm_result) = tokio::join!(
        async {
            match &node_path {
                Some(p) => crate::process::tokio_command(p).arg("--version").output().await,
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "node not found in PATH",
                )),
            }
        },
        async {
            match &npm_path {
                Some(p) => crate::process::tokio_command(p).arg("--version").output().await,
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "npm not found in PATH",
                )),
            }
        },
    );

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

    let all_passed = checks
        .iter()
        .all(|c| !matches!(c.status, CheckStatus::Fail));
    if all_passed {
        *NPM_ENV_CACHE.lock().unwrap() = Some(checks.clone());
    }

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

async fn check_binary_environment(
    server_id: &str,
    version: &str,
    cmd: &str,
    platforms: &[PlatformBinary],
) -> Vec<CheckItem> {
    let mut checks = Vec::new();

    let current = crate::acp::registry::current_platform();
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

    if platform_supported {
        let cache_check = match binary_cache::find_cached_binary(server_id, version, cmd) {
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
                message: "Binary not cached yet, will be downloaded on install".into(),
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

async fn check_cargo_environment() -> Vec<CheckItem> {
    let cargo_path = which::which("cargo").ok();

    let cargo_check = match &cargo_path {
        Some(p) => {
            match crate::process::tokio_command(p)
                .arg("--version")
                .output()
                .await
            {
                Ok(output) if output.status.success() => {
                    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    CheckItem {
                        check_id: "cargo_available".into(),
                        label: "Cargo".into(),
                        status: CheckStatus::Pass,
                        message: format!("{version} available"),
                        fixes: vec![],
                    }
                }
                _ => CheckItem {
                    check_id: "cargo_available".into(),
                    label: "Cargo".into(),
                    status: CheckStatus::Fail,
                    message: "Cargo is not working correctly".into(),
                    fixes: vec![FixAction {
                        label: "Install Rust".into(),
                        kind: FixActionKind::OpenUrl,
                        payload: "https://rustup.rs/".into(),
                    }],
                },
            }
        }
        None => CheckItem {
            check_id: "cargo_available".into(),
            label: "Cargo".into(),
            status: CheckStatus::Fail,
            message: "Cargo is not installed or not in PATH".into(),
            fixes: vec![FixAction {
                label: "Install Rust".into(),
                kind: FixActionKind::OpenUrl,
                payload: "https://rustup.rs/".into(),
            }],
        },
    };

    vec![cargo_check]
}

async fn check_pip_environment(python_required: Option<&str>) -> Vec<CheckItem> {
    let python_path = which::which("python3")
        .ok()
        .or_else(|| which::which("python").ok());

    let pip_path = which::which("pip3")
        .ok()
        .or_else(|| which::which("pip").ok());

    let mut checks = Vec::new();

    let python_check = match &python_path {
        Some(p) => {
            match crate::process::tokio_command(p)
                .arg("--version")
                .output()
                .await
            {
                Ok(output) if output.status.success() => {
                    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    CheckItem {
                        check_id: "python_available".into(),
                        label: "Python".into(),
                        status: CheckStatus::Pass,
                        message: format!("{version} available"),
                        fixes: vec![],
                    }
                }
                _ => CheckItem {
                    check_id: "python_available".into(),
                    label: "Python".into(),
                    status: CheckStatus::Fail,
                    message: "Python is not working correctly".into(),
                    fixes: vec![FixAction {
                        label: "Install Python".into(),
                        kind: FixActionKind::OpenUrl,
                        payload: "https://www.python.org/downloads/".into(),
                    }],
                },
            }
        }
        None => CheckItem {
            check_id: "python_available".into(),
            label: "Python".into(),
            status: CheckStatus::Fail,
            message: "Python is not installed or not in PATH".into(),
            fixes: vec![FixAction {
                label: "Install Python".into(),
                kind: FixActionKind::OpenUrl,
                payload: "https://www.python.org/downloads/".into(),
            }],
        },
    };
    checks.push(python_check);

    let pip_check = match &pip_path {
        Some(p) => {
            match crate::process::tokio_command(p)
                .arg("--version")
                .output()
                .await
            {
                Ok(output) if output.status.success() => {
                    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    CheckItem {
                        check_id: "pip_available".into(),
                        label: "pip".into(),
                        status: CheckStatus::Pass,
                        message: format!("{version}"),
                        fixes: vec![],
                    }
                }
                _ => CheckItem {
                    check_id: "pip_available".into(),
                    label: "pip".into(),
                    status: CheckStatus::Fail,
                    message: "pip is not working correctly".into(),
                    fixes: vec![],
                },
            }
        }
        None => CheckItem {
            check_id: "pip_available".into(),
            label: "pip".into(),
            status: CheckStatus::Fail,
            message: "pip is not installed or not in PATH".into(),
            fixes: vec![],
        },
    };
    checks.push(pip_check);

    let _ = python_required; // Reserved for future version check

    checks
}

fn parse_node_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches('v');
    let mut parts = v.splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch_str = parts.next()?;
    let patch_digits: String = patch_str
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let patch = patch_digits.parse().ok()?;
    Some((major, minor, patch))
}

fn extract_node_version_from_message(message: &str) -> Option<String> {
    message
        .split_whitespace()
        .find(|s| s.starts_with('v') && s.contains('.'))
        .map(|s| s.to_string())
}

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
