//! Opt-in, release-based agent updates. There is deliberately no model catalog
//! here: a fresh ACP session remains authoritative for models and controls.
//!
//! npm/uv receive exact versions, so their caches retain old installations for
//! running processes. Binary releases use the existing versioned archive cache.
//! Failed checks/preparation keep the previous prepared launch or the ordinary
//! user-managed installation. Credentials never enter the persisted plan.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use super::{binary_cache, registry, remote_registry};
use crate::models::agent::AgentType;

pub const CHANNEL_ENV: &str = "CODEG_ADAPTER_CHANNEL";
pub const AUTOMATIC: &str = "automatic";
const CHECK_INTERVAL: u64 = 60 * 60;
const RETRY_INTERVAL: u64 = 5 * 60;
const BOOTSTRAP: &str = include_str!("managed-runtime.mjs");

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Launch {
    Npx { program: PathBuf, args: Vec<String> },
    Binary { path: PathBuf, version: String },
    Uvx { package: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Prepared {
    version: String,
    launch: Launch,
}

#[derive(Default, Deserialize, Serialize)]
struct Cache {
    checked_at: u64,
    failed: bool,
    prepared: Option<Prepared>,
}

type UpdateLocks = Mutex<HashMap<String, Arc<Mutex<()>>>>;
static LOCKS: OnceLock<UpdateLocks> = OnceLock::new();

pub fn enabled(env: &BTreeMap<String, String>) -> bool {
    env.get(CHANNEL_ENV).is_some_and(|v| v.trim() == AUTOMATIC)
}

fn runtime_overridden(env: &BTreeMap<String, String>, key: &str) -> bool {
    env.get(key).is_some_and(|v| !v.trim().is_empty())
        || std::env::var(key).is_ok_and(|v| !v.trim().is_empty())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn due(cache: &Cache, time: u64) -> bool {
    cache.checked_at == 0
        || time < cache.checked_at
        || time - cache.checked_at
            >= if cache.failed {
                RETRY_INTERVAL
            } else {
                CHECK_INTERVAL
            }
}

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

/// Package identity, not a version, URL, path or npm alias. The same allowlist
/// keeps registry lookups and npm's option parser on the declared package.
fn npm_name(spec: &str) -> Option<&str> {
    let end = spec.rfind('@').filter(|i| *i > 0).unwrap_or(spec.len());
    if end < spec.len() && spec[end + 1..].bytes().any(|b| b"/:\\".contains(&b)) {
        return None;
    }
    let name = &spec[..end];
    let valid_part = |s: &str| {
        !s.is_empty()
            && !s.starts_with('.')
            && !s.starts_with('-')
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    if let Some(scoped) = name.strip_prefix('@') {
        let (scope, package) = scoped.split_once('/')?;
        (valid_part(scope) && valid_part(package)).then_some(name)
    } else {
        valid_part(name).then_some(name)
    }
}

fn safe_version(version: &str) -> bool {
    version.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && version.len() <= 100
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

/// These are supported runtime override contracts, not model names. Match the
/// distribution package so extra accounts/custom entries get the same behavior.
fn runtime_for(package: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match package {
        "@agentclientprotocol/claude-agent-acp" => Some((
            "@anthropic-ai/claude-code",
            "claude",
            "CLAUDE_CODE_EXECUTABLE",
        )),
        "@agentclientprotocol/codex-acp" => Some(("@openai/codex", "codex", "CODEX_PATH")),
        "pi-acp" => Some(("@earendil-works/pi-coding-agent", "pi", "PI_ACP_PI_COMMAND")),
        _ => None,
    }
}

async fn json(url: &str) -> Result<serde_json::Value, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| "cannot create release client")?
        .get(url)
        .send()
        .await
        .map_err(|_| "release lookup unavailable")?
        .error_for_status()
        .map_err(|_| "release lookup rejected")?
        .json()
        .await
        .map_err(|_| "invalid release metadata".into())
}

async fn npm_latest(package: &str) -> Result<String, String> {
    let encoded = package.replace('/', "%2f");
    let value = json(&format!("https://registry.npmjs.org/{encoded}/latest")).await?;
    let version = value["version"]
        .as_str()
        .filter(|v| safe_version(v))
        .ok_or("missing package version")?;
    Ok(version.to_owned())
}

fn python_package(spec: &str) -> Option<&str> {
    let name = spec.split(['=', '<', '>', '!', '~']).next()?.trim();
    // Extras are retained in the launch spec but omitted from the PyPI URL.
    let base = name.split('[').next()?;
    if base.is_empty()
        || !base
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return None;
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_.[],".contains(&b))
    {
        return None;
    }
    Some(name)
}

async fn run(program: &str, args: &[String], cwd: &Path) -> Result<(), String> {
    let mut command = crate::process::tokio_command(program);
    command
        .args(args)
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Only installation metadata goes into args. Never pass session env/auth to
    // package managers or print their output into a session or public report.
    let status = tokio::time::timeout(Duration::from_secs(180), command.status())
        .await
        .map_err(|_| "agent update timed out")?
        .map_err(|_| "agent updater could not start")?;
    if status.success() {
        Ok(())
    } else {
        Err("agent preparation failed".into())
    }
}

fn bootstrap_path(root: &Path) -> Result<PathBuf, String> {
    let path = root.join(format!("runtime-{}.mjs", &digest(BOOTSTRAP)[..16]));
    if !path.is_file() {
        std::fs::write(&path, BOOTSTRAP).map_err(|_| "cannot write runtime launcher")?;
    }
    Ok(path)
}

async fn prepare(
    agent_type: AgentType,
    env: &BTreeMap<String, String>,
    root: &Path,
) -> Result<Prepared, String> {
    match registry::get_agent_meta(agent_type).distribution {
        registry::AgentDistribution::Npx { package, cmd, .. } => {
            let name = npm_name(package).ok_or("this package source is explicitly pinned")?;
            let version = npm_latest(name).await?;
            let mut packages = vec![format!("{name}@{version}")];
            let runtime = runtime_for(name).filter(|(_, _, key)| !runtime_overridden(env, key));
            let mut runtime_version = String::new();
            if let Some((package, _, _)) = runtime {
                runtime_version = npm_latest(package).await?;
                packages.push(format!("{package}@{runtime_version}"));
            }
            let mut args = vec![
                "--yes".into(),
                "--registry=https://registry.npmjs.org".into(),
                "--include=optional".into(),
                "--ignore-scripts=false".into(),
                format!("--prefix={}", root.display()),
            ];
            args.extend(packages.iter().map(|spec| format!("--package={spec}")));
            args.extend([
                "--".into(),
                "node".into(),
                bootstrap_path(root)?.to_string_lossy().into_owned(),
                name.into(),
                cmd.into(),
                version.clone(),
            ]);
            let (runtime_package, runtime_cmd, runtime_key) = runtime.unwrap_or(("", "", ""));
            args.extend([
                runtime_package.into(),
                runtime_cmd.into(),
                runtime_key.into(),
                runtime_version,
            ]);
            let mut check = args.clone();
            check.push("--codeg-check".into());
            let program = crate::commands::acp::resolve_npx_command("npx")
                .await
                .ok_or("npx unavailable")?;
            run(&program.to_string_lossy(), &check, root).await?;
            // Preparation used the network; actual launches use the exact warm
            // cache. An offline launch cannot resolve a different version.
            args.insert(1, "--offline".into());
            Ok(Prepared {
                version,
                launch: Launch::Npx { program, args },
            })
        }
        registry::AgentDistribution::Binary { cmd, .. } => {
            let release = tokio::time::timeout(
                Duration::from_secs(8),
                remote_registry::fetch_binary_release(agent_type, registry::current_platform()),
            )
            .await
            .map_err(|_| "registry lookup timed out")?
            .map_err(|_| "registry lookup unavailable")?
            .ok_or("no published release for this agent/platform")?;
            if !safe_version(&release.version) || !release.archive_url.starts_with("https://") {
                return Err("invalid binary release".into());
            }
            let path = binary_cache::ensure_binary_for_agent_with_progress(
                agent_type,
                &release.version,
                &release.archive_url,
                cmd,
                release.sha256.as_deref(),
                |_| {},
            )
            .await
            .map_err(|_| "binary preparation failed")?;
            Ok(Prepared {
                version: release.version.clone(),
                launch: Launch::Binary {
                    path,
                    version: release.version,
                },
            })
        }
        registry::AgentDistribution::Uvx {
            package,
            cmd,
            python,
            ..
        } => {
            let name = python_package(package).ok_or("this Python source is explicitly pinned")?;
            let base = name.split('[').next().unwrap_or(name);
            let value = json(&format!("https://pypi.org/pypi/{base}/json")).await?;
            let version = value["info"]["version"]
                .as_str()
                .filter(|v| safe_version(v))
                .ok_or("missing Python package version")?
                .to_owned();
            let package = format!("{name}=={version}");
            let mut args = crate::commands::acp::uvx_python_args(python);
            args.extend([
                "--from".into(),
                package.clone(),
                cmd.into(),
                "--version".into(),
            ]);
            let uvx = crate::commands::acp::resolve_uvx_command().ok_or("uvx unavailable")?;
            run(&uvx.to_string_lossy(), &args, root).await?;
            Ok(Prepared {
                version,
                launch: Launch::Uvx { package },
            })
        }
    }
}

fn effective_env(
    agent_type: AgentType,
    env: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let base = match registry::get_agent_meta(agent_type).distribution {
        registry::AgentDistribution::Npx { env, .. }
        | registry::AgentDistribution::Binary { env, .. }
        | registry::AgentDistribution::Uvx { env, .. } => env,
    };
    let mut merged: BTreeMap<String, String> = base
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect();
    merged.extend(env.clone());
    merged
}

fn cache_key(agent_type: AgentType, env: &BTreeMap<String, String>) -> String {
    let distribution = registry::get_agent_meta(agent_type).distribution;
    // Include the recipe and explicit runtime override *presence*, never its
    // value. An account-specific path/key must not be persisted in this cache.
    let identity = match &distribution {
        registry::AgentDistribution::Npx { package, cmd, .. } => {
            let overridden = npm_name(package)
                .and_then(runtime_for)
                .is_some_and(|(_, _, key)| runtime_overridden(env, key));
            format!("npm:{package}:{cmd}:{overridden}")
        }
        registry::AgentDistribution::Binary { cmd, .. } => {
            format!("binary:{}:{cmd}", registry::registry_id_for(agent_type))
        }
        registry::AgentDistribution::Uvx {
            package,
            cmd,
            python,
            ..
        } => format!("uv:{package}:{cmd}:{python:?}"),
    };
    digest(&format!(
        "{}:{identity}:{}",
        registry::current_platform(),
        digest(BOOTSTRAP)
    ))
}

/// Read only the prepared version for settings. No release lookup or installer
/// runs while listing agents, and no manifest/path is sent to the frontend.
pub fn prepared_version(agent_type: AgentType, env: &BTreeMap<String, String>) -> Option<String> {
    let env = effective_env(agent_type, env);
    if !enabled(&env) {
        return None;
    }
    let path = binary_cache::cache_dir()
        .ok()?
        .join("automatic")
        .join(format!("{}.json", cache_key(agent_type, &env)));
    let cache: Cache = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    cache.prepared.map(|prepared| prepared.version)
}

/// Called only when creating an agent process, never on a prompt or a live
/// connection. All entry points (desktop, web, delegation and probes) share it.
pub async fn resolve(agent_type: AgentType, env: &BTreeMap<String, String>) -> Option<Launch> {
    let env = effective_env(agent_type, env);
    if !enabled(&env) {
        return None;
    }
    let key = cache_key(agent_type, &env);
    let root = binary_cache::cache_dir().ok()?.join("automatic");
    std::fs::create_dir_all(&root).ok()?;
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let lock = locks.lock().await.entry(key.clone()).or_default().clone();
    let _guard = lock.lock().await;
    let path = root.join(format!("{key}.json"));
    let mut cache: Cache = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if due(&cache, now()) {
        match prepare(agent_type, &env, &root).await {
            Ok(prepared) => {
                cache.prepared = Some(prepared);
                cache.failed = false;
            }
            Err(reason) => {
                cache.failed = true;
                tracing::warn!(agent = %agent_type, "automatic agent update unavailable: {reason}; keeping prepared/installed runtime");
            }
        }
        cache.checked_at = now();
        if let Ok(bytes) = serde_json::to_vec(&cache) {
            let staging = root.join(format!("{key}.{}.tmp", uuid::Uuid::new_v4()));
            if std::fs::write(&staging, bytes).is_ok() && std::fs::rename(&staging, &path).is_err()
            {
                let _ = std::fs::remove_file(staging);
            }
        }
    }
    cache.prepared.map(|p| p.launch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_is_explicit_and_keeps_manual_channels_unchanged() {
        for value in ["", "latest", "pinned", "true", "Automatic"] {
            assert!(!enabled(&BTreeMap::from([(
                CHANNEL_ENV.into(),
                value.into()
            )])));
        }
        assert!(enabled(&BTreeMap::from([(
            CHANNEL_ENV.into(),
            " automatic ".into()
        )])));
    }

    #[test]
    fn source_parsing_cannot_become_a_path_url_or_npm_option() {
        assert_eq!(npm_name("@scope/adapter@1.2.3"), Some("@scope/adapter"));
        assert_eq!(npm_name("agent@latest"), Some("agent"));
        for spec in [
            "../agent",
            "--prefix=/tmp",
            "https://example.com/a",
            "@scope/a/b",
            "git+ssh:repo",
        ] {
            assert_eq!(npm_name(spec), None, "{spec}");
        }
        assert_eq!(
            python_package("agent[tools,cli]==1.2.3"),
            Some("agent[tools,cli]")
        );
        assert_eq!(python_package("agent @ https://example.com/a.whl"), None);
        assert!(!safe_version("../../escape"));
        assert!(!safe_version("--latest"));
        assert!(safe_version("2026.9.22-build1"));
    }

    #[test]
    fn wrapper_runtime_contracts_follow_packages_including_extra_accounts() {
        assert_eq!(
            runtime_for("@agentclientprotocol/claude-agent-acp")
                .unwrap()
                .2,
            "CLAUDE_CODE_EXECUTABLE"
        );
        assert_eq!(
            runtime_for("@agentclientprotocol/codex-acp").unwrap().2,
            "CODEX_PATH"
        );
        assert!(runtime_for("@google/gemini-cli").is_none());
    }

    #[test]
    fn successful_checks_are_throttled_and_failures_retry_without_discarding_plan() {
        let mut cache = Cache {
            checked_at: 100,
            failed: false,
            prepared: None,
        };
        assert!(!due(&cache, 101));
        assert!(due(&cache, 100 + CHECK_INTERVAL));
        cache.failed = true;
        assert!(!due(&cache, 100 + RETRY_INTERVAL - 1));
        assert!(due(&cache, 100 + RETRY_INTERVAL));
        assert!(due(&cache, 99));
    }
}
