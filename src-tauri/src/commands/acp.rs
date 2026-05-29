use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
#[cfg(feature = "tauri-runtime")]
use tauri::{Manager, State};

use crate::acp::binary_cache;
use crate::acp::error::AcpError;
use crate::acp::manager::ConnectionManager;
use crate::acp::opencode_plugins::{self, PluginCheckSummary};
use crate::acp::preflight::{self, PreflightResult};
use crate::acp::registry;
use crate::acp::types::{
    AcpAgentInfo, AgentSkillContent, AgentSkillItem, AgentSkillLayout, AgentSkillLocation,
    AgentSkillScope, AgentSkillsListResult,
};
#[cfg(feature = "tauri-runtime")]
use crate::acp::types::{ConnectionInfo, ForkResultInfo, PromptInputBlock};
use crate::db::service::agent_setting_service;
use crate::db::service::model_provider_service;
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::web::event_bridge::EventEmitter;

const ACP_AGENTS_UPDATED_EVENT: &str = "app://acp-agents-updated";
const NPM_PREFIX_TIMEOUT: Duration = Duration::from_millis(1500);

static NPM_GLOBAL_PREFIX_CACHE: tokio::sync::OnceCell<PathBuf> = tokio::sync::OnceCell::const_new();

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
struct AcpAgentsUpdatedEventPayload {
    reason: &'static str,
    agent_type: Option<AgentType>,
}

fn emit_acp_agents_updated(
    emitter: &EventEmitter,
    reason: &'static str,
    agent_type: Option<AgentType>,
) {
    crate::web::event_bridge::emit_event(
        emitter,
        ACP_AGENTS_UPDATED_EVENT,
        AcpAgentsUpdatedEventPayload { reason, agent_type },
    );
}

const AGENT_INSTALL_EVENT: &str = "app://agent-install";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentInstallEventKind {
    Started,
    Log,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AgentInstallEvent {
    pub task_id: String,
    pub kind: AgentInstallEventKind,
    pub payload: String,
}

fn emit_agent_install_event(
    emitter: &EventEmitter,
    task_id: &str,
    kind: AgentInstallEventKind,
    payload: impl Into<String>,
) {
    crate::web::event_bridge::emit_event(
        emitter,
        AGENT_INSTALL_EVENT,
        AgentInstallEvent {
            task_id: task_id.to_string(),
            kind,
            payload: payload.into(),
        },
    );
}

fn is_version_like(value: &str) -> bool {
    value.chars().any(|c| c.is_ascii_digit()) && value.contains('.')
}

fn normalize_version_candidate(value: &str) -> Option<String> {
    let normalized = value.trim().trim_start_matches('v');
    if is_version_like(normalized) {
        Some(normalized.to_string())
    } else {
        None
    }
}

fn version_from_package_spec(package: &str) -> Option<String> {
    let (_, maybe_version) = package.rsplit_once('@')?;
    let version = maybe_version.trim();
    if version.is_empty() || version.eq_ignore_ascii_case("latest") {
        return None;
    }
    normalize_version_candidate(version)
}

fn package_name_from_spec(package: &str) -> String {
    let normalized = package.trim();
    if normalized.is_empty() {
        return String::new();
    }

    if let Some(index) = normalized.rfind('@') {
        if index > 0 {
            let version_part = normalized[index + 1..].trim();
            if !version_part.is_empty() {
                return normalized[..index].to_string();
            }
        }
    }

    normalized.to_string()
}

/// Validate and normalize a user-supplied custom version for install.
///
/// Stricter than [`normalize_version_candidate`]: tolerates a leading `v`/`V`,
/// then requires the first character to be a digit and the rest to be drawn from
/// `[0-9A-Za-z.-+]` (covers semver pre-release/build metadata and calendar
/// versions like `2026.5.20`). This rejects npm dist-tags (`latest`, `next`) and
/// anything containing whitespace, `@`, or path separators, so the result is
/// safe to interpolate into an npm package spec (`name@<v>`) and to substitute
/// into a binary download URL. Returns the version without the leading `v`.
fn sanitize_custom_version(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let normalized = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    let mut chars = normalized.chars();
    if !chars.next()?.is_ascii_digit() {
        return None;
    }
    // Require a dotted version (e.g. `1.2.3`) so the validator agrees with the
    // detection fallback `version_from_package_spec`, which needs a `.` — and so
    // a "custom version" is a concrete version rather than an npm range (`2`).
    if !normalized.contains('.') {
        return None;
    }
    let all_allowed = normalized
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    all_allowed.then(|| normalized.to_string())
}

/// Build the `npm install -g` spec for an agent.
///
/// `version_override` of `None` or all-whitespace yields the registry-pinned
/// `package` spec unchanged (current behavior). A non-empty override is
/// validated via [`sanitize_custom_version`] and combined with the registry
/// package *name* (its pinned version is dropped) to form `name@<version>`. An
/// override that fails validation is rejected with an error.
fn build_npm_install_spec(package: &str, version_override: Option<&str>) -> Result<String, AcpError> {
    match version_override {
        Some(raw) if !raw.trim().is_empty() => {
            let version = sanitize_custom_version(raw)
                .ok_or_else(|| AcpError::protocol(format!("invalid custom version: {}", raw.trim())))?;
            Ok(format!("{}@{version}", package_name_from_spec(package)))
        }
        _ => Ok(package.to_string()),
    }
}

/// Substitute a custom version into a registry binary download URL by replacing
/// every occurrence of the registry version string. The registry version is
/// embedded in the GitHub release URL (the path tag, and for some agents the
/// asset filename), so a plain replace yields the URL for the requested version
/// — assuming the upstream release reuses the same asset-naming convention.
fn apply_custom_version_to_url(url: &str, registry_version: &str, custom_version: &str) -> String {
    url.replace(registry_version, custom_version)
}

/// Check whether an NPX agent command is spawnable.
/// Uses PATH first, then falls back to the current npm global prefix to handle
/// GUI environments that don't inherit the user's shell PATH.
pub(crate) async fn is_cmd_available(cmd: &str) -> bool {
    resolve_npx_command(cmd).await.is_some()
}

fn resolve_command_on_path(cmd: &str) -> Option<PathBuf> {
    which::which(cmd).ok()
}

pub(crate) async fn resolve_npx_command(cmd: &str) -> Option<PathBuf> {
    if let Some(path) = resolve_command_on_path(cmd) {
        return Some(path);
    }
    resolve_npx_command_from_current_npm_prefix(cmd).await
}

#[derive(Default)]
struct NpxCommandResolver {
    per_cmd_cache: HashMap<String, Option<PathBuf>>,
    request_npm_prefix: Option<Option<PathBuf>>,
}

impl NpxCommandResolver {
    async fn resolve_for_list(&mut self, cmd: &str) -> Option<PathBuf> {
        if let Some(cached) = self.per_cmd_cache.get(cmd) {
            return cached.clone();
        }

        let resolved = if let Some(path) = resolve_command_on_path(cmd) {
            Some(path)
        } else {
            let prefix = if let Some(prefix) = &self.request_npm_prefix {
                prefix.clone()
            } else {
                let resolved_prefix = cached_npm_global_prefix().await;
                self.request_npm_prefix = Some(resolved_prefix.clone());
                resolved_prefix
            };
            prefix.and_then(|p| resolve_npx_command_from_npm_prefix(cmd, &p))
        };

        self.per_cmd_cache.insert(cmd.to_string(), resolved.clone());
        resolved
    }
}

async fn resolve_npx_command_from_current_npm_prefix(cmd: &str) -> Option<PathBuf> {
    let prefix = cached_npm_global_prefix().await?;
    resolve_npx_command_from_npm_prefix(cmd, &prefix)
}

async fn cached_npm_global_prefix() -> Option<PathBuf> {
    cached_npm_global_prefix_with(&NPM_GLOBAL_PREFIX_CACHE, resolve_current_npm_global_prefix).await
}

async fn cached_npm_global_prefix_with<F, Fut>(
    cache: &tokio::sync::OnceCell<PathBuf>,
    resolve: F,
) -> Option<PathBuf>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Option<PathBuf>>,
{
    if let Some(prefix) = cache.get() {
        return Some(prefix.clone());
    }

    let resolved = resolve().await?;
    match cache.set(resolved.clone()) {
        Ok(()) => Some(resolved),
        Err(_) => cache.get().cloned(),
    }
}

async fn resolve_current_npm_global_prefix() -> Option<PathBuf> {
    let npm_path = which::which("npm").ok()?;
    let mut cmd = crate::process::tokio_command(npm_path);
    cmd.arg("prefix").arg("-g").kill_on_drop(true);
    let output = tokio::time::timeout(NPM_PREFIX_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    npm_global_prefix_from_stdout(&output.stdout)
}

fn npm_global_prefix_from_stdout(stdout: &[u8]) -> Option<PathBuf> {
    let stdout_text = String::from_utf8_lossy(stdout);
    let prefix = stdout_text.lines().next()?.trim();
    if prefix.is_empty() {
        return None;
    }
    Some(PathBuf::from(prefix))
}

fn npm_prefix_bin_dir(prefix: &Path) -> PathBuf {
    if cfg!(windows) {
        prefix.to_path_buf()
    } else {
        prefix.join("bin")
    }
}

fn resolve_npx_command_from_npm_prefix(cmd: &str, prefix: &Path) -> Option<PathBuf> {
    let bin_dir = npm_prefix_bin_dir(prefix);

    #[cfg(windows)]
    let candidates = [
        bin_dir.join(format!("{cmd}.cmd")),
        bin_dir.join(format!("{cmd}.exe")),
        bin_dir.join(cmd),
    ];

    #[cfg(not(windows))]
    let candidates = [bin_dir.join(cmd)];

    candidates
        .into_iter()
        .find(|path| is_npm_command_candidate(path))
}

#[cfg(windows)]
fn is_npm_command_candidate(path: &Path) -> bool {
    path.is_file()
}

#[cfg(not(windows))]
fn is_npm_command_candidate(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.is_file()
        && path
            .metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

/// Verify that the agent SDK / binary is installed and usable.
///
/// This is the pre-spawn guard used by the session-page connect path:
/// the session page must NEVER trigger a download or install, so if the
/// agent isn't ready we return `AcpError::SdkNotInstalled` immediately
/// and let the frontend prompt the user to install from Agent Settings.
///
/// For NPX agents: checks the command is spawnable in this process environment.
/// For Binary agents: checks platform support and that the binary is
/// already cached locally.
pub(crate) async fn verify_agent_installed(agent_type: AgentType) -> Result<(), AcpError> {
    let meta = registry::get_agent_meta(agent_type);
    match meta.distribution {
        registry::AgentDistribution::Npx { cmd, .. } => {
            if !is_cmd_available(cmd).await {
                // INVARIANT: the substring "is not installed" is matched
                // verbatim by the frontend catch block in
                // `src/contexts/acp-connections-context.tsx` to surface a
                // localized install prompt. Do not change the wording.
                return Err(AcpError::SdkNotInstalled(format!(
                    "{} is not installed. Please install it in Agent Settings.",
                    meta.name
                )));
            }
            Ok(())
        }
        registry::AgentDistribution::Binary { cmd, platforms, .. } => {
            let platform = registry::current_platform();
            if !platforms.iter().any(|p| p.platform == platform) {
                return Err(AcpError::PlatformNotSupported(format!(
                    "{} is not available on {platform}",
                    meta.name
                )));
            }
            // Accept any cached version — the Settings page will still
            // surface "upgrade available" for stale caches via its own
            // version-badge flow.
            if binary_cache::find_best_cached_binary_for_agent(agent_type, cmd)?.is_none() {
                // INVARIANT: see note above — "is not installed" is a
                // stable substring the frontend matches against.
                return Err(AcpError::SdkNotInstalled(format!(
                    "{} is not installed. Please install it in Agent Settings.",
                    meta.name
                )));
            }
            Ok(())
        }
    }
}

/// Detect the actual installed version of an npm global package by running
/// `npm list -g <package_name> --json` and parsing the JSON output.
///
/// Checks both the system global prefix and the user-local prefix
/// (`~/.codeg/npm-global/`) so packages installed via the EACCES fallback are
/// found as well.
async fn detect_npm_global_version(package_name: &str) -> Option<String> {
    let npm_path = which::which("npm").ok()?;

    // Try the default global prefix first.
    if let Some(v) = npm_list_version(&npm_path, package_name, None).await {
        return Some(v);
    }

    // Fallback: check the user-local prefix.
    if let Some(prefix) = crate::process::user_npm_prefix() {
        if prefix.exists() {
            return npm_list_version(&npm_path, package_name, Some(&prefix)).await;
        }
    }

    None
}

/// Run `npm list -g <package_name> --json [--prefix=<p>]` and extract the
/// installed version string.
async fn npm_list_version(
    npm_path: &std::path::Path,
    package_name: &str,
    prefix: Option<&std::path::Path>,
) -> Option<String> {
    let mut cmd = crate::process::tokio_command(npm_path);
    cmd.arg("list")
        .arg("-g")
        .arg(package_name)
        .arg("--json")
        .arg("--depth=0");
    if let Some(p) = prefix {
        cmd.arg(format!("--prefix={}", p.display()));
    }
    let output = cmd.output().await.ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).ok()?;
    let version = json
        .get("dependencies")?
        .get(package_name)?
        .get("version")?
        .as_str()?;
    normalize_version_candidate(version)
}

async fn detect_local_version(agent_type: AgentType) -> Option<String> {
    let meta = registry::get_agent_meta(agent_type);
    match meta.distribution {
        registry::AgentDistribution::Npx { cmd, package, .. } => {
            if !is_cmd_available(cmd).await {
                return None;
            }
            // Try `npm list -g <package_name> --json` to get the real installed version.
            let pkg_name = package_name_from_spec(package);
            detect_npm_global_version(&pkg_name).await
        }
        registry::AgentDistribution::Binary { cmd, .. } => {
            binary_cache::detect_installed_version(agent_type, cmd)
                .ok()
                .flatten()
        }
    }
}

/// Official npm registry URL – used to bypass local mirror configurations that
/// may not have synced niche packages like `@agentclientprotocol/*`.
const NPM_OFFICIAL_REGISTRY: &str = "https://registry.npmjs.org";

/// Run an npm command with piped stdout/stderr, streaming each line as a log event.
/// Returns (success: bool, collected_stderr: String) so callers can inspect errors.
async fn run_npm_streaming(
    args: &[&str],
    task_id: &str,
    emitter: &EventEmitter,
) -> Result<(bool, String), AcpError> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut cmd = crate::process::tokio_command("npm");
    for arg in args {
        cmd.arg(arg);
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| AcpError::protocol(format!("failed to spawn npm: {e}")))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let emitter_clone = emitter.clone();
    let task_id_owned = task_id.to_string();

    let stdout_handle = tokio::spawn({
        let emitter = emitter_clone.clone();
        let task_id = task_id_owned.clone();
        async move {
            if let Some(out) = stdout {
                let reader = BufReader::new(out);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    emit_agent_install_event(&emitter, &task_id, AgentInstallEventKind::Log, &line);
                }
            }
        }
    });

    let stderr_handle = tokio::spawn({
        let emitter = emitter_clone;
        let task_id = task_id_owned;
        async move {
            let mut collected = String::new();
            if let Some(err) = stderr {
                let reader = BufReader::new(err);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    emit_agent_install_event(&emitter, &task_id, AgentInstallEventKind::Log, &line);
                    if !collected.is_empty() {
                        collected.push('\n');
                    }
                    collected.push_str(&line);
                }
            }
            collected
        }
    });

    let (_, stderr_result) = tokio::join!(stdout_handle, stderr_handle);
    let collected_stderr = stderr_result.unwrap_or_default();

    let status = child
        .wait()
        .await
        .map_err(|e| AcpError::protocol(format!("failed to wait for npm process: {e}")))?;

    Ok((status.success(), collected_stderr))
}

async fn install_npm_global_package_streaming(
    package: &str,
    task_id: &str,
    emitter: &EventEmitter,
) -> Result<(), AcpError> {
    let registry_arg = format!("--registry={NPM_OFFICIAL_REGISTRY}");

    emit_agent_install_event(
        emitter,
        task_id,
        AgentInstallEventKind::Log,
        format!("$ npm install -g {package}"),
    );

    let (success, stderr) =
        run_npm_streaming(&["install", "-g", &registry_arg, package], task_id, emitter).await?;

    if !success {
        // EACCES: permission denied — retry with a user-local --prefix so
        // we don't require root/sudo on macOS / Linux.
        if stderr.contains("EACCES") {
            emit_agent_install_event(
                emitter,
                task_id,
                AgentInstallEventKind::Log,
                "Permission denied, retrying with user prefix...",
            );
            return install_npm_to_user_prefix_streaming(package, &registry_arg, task_id, emitter)
                .await;
        }

        // EEXIST: file conflict — retry with --force to overwrite
        if stderr.contains("EEXIST") {
            emit_agent_install_event(
                emitter,
                task_id,
                AgentInstallEventKind::Log,
                "File conflict, retrying with --force...",
            );
            let (retry_success, retry_stderr) = run_npm_streaming(
                &["install", "-g", "--force", &registry_arg, package],
                task_id,
                emitter,
            )
            .await?;
            if !retry_success {
                if retry_stderr.contains("EACCES") {
                    emit_agent_install_event(
                        emitter,
                        task_id,
                        AgentInstallEventKind::Log,
                        "Permission denied on --force retry, falling back to user prefix...",
                    );
                    return install_npm_to_user_prefix_streaming(
                        package,
                        &registry_arg,
                        task_id,
                        emitter,
                    )
                    .await;
                }
                let err = retry_stderr.trim().to_string();
                let msg = if err.is_empty() {
                    "failed to install npm package globally (with --force)".to_string()
                } else {
                    format!("failed to install npm package globally (with --force): {err}")
                };
                return Err(AcpError::protocol(msg));
            }
            return Ok(());
        }

        let err = stderr.trim().to_string();
        let msg = if err.is_empty() {
            "failed to install npm package globally".to_string()
        } else {
            format!("failed to install npm package globally: {err}")
        };
        return Err(AcpError::protocol(msg));
    }

    Ok(())
}

/// Fallback: install an npm package into a user-local prefix (`~/.codeg/npm-global/`)
/// when the system global prefix is not writable (EACCES).
async fn install_npm_to_user_prefix_streaming(
    package: &str,
    registry_arg: &str,
    task_id: &str,
    emitter: &EventEmitter,
) -> Result<(), AcpError> {
    let prefix = crate::process::user_npm_prefix().ok_or_else(|| {
        AcpError::protocol(
            "npm install -g failed with EACCES and could not determine home directory for fallback"
                .to_string(),
        )
    })?;

    // Ensure the prefix directory exists.
    tokio::fs::create_dir_all(&prefix).await.map_err(|e| {
        AcpError::protocol(format!(
            "failed to create user npm prefix {}: {e}",
            prefix.display()
        ))
    })?;

    let prefix_arg = format!("--prefix={}", prefix.display());

    emit_agent_install_event(
        emitter,
        task_id,
        AgentInstallEventKind::Log,
        format!("$ npm install -g --prefix={} {package}", prefix.display()),
    );

    let (success, stderr) = run_npm_streaming(
        &["install", "-g", &prefix_arg, registry_arg, package],
        task_id,
        emitter,
    )
    .await?;

    if !success {
        // EEXIST in the user prefix: retry with --force to overwrite stale files
        // from a previous installation.
        if stderr.contains("EEXIST") {
            emit_agent_install_event(
                emitter,
                task_id,
                AgentInstallEventKind::Log,
                "File conflict in user prefix, retrying with --force...",
            );
            let (force_success, force_stderr) = run_npm_streaming(
                &[
                    "install",
                    "-g",
                    "--force",
                    &prefix_arg,
                    registry_arg,
                    package,
                ],
                task_id,
                emitter,
            )
            .await?;
            if !force_success {
                let err = force_stderr.trim().to_string();
                let msg = if err.is_empty() {
                    format!(
                        "failed to install npm package (user prefix {}, --force)",
                        prefix.display()
                    )
                } else {
                    format!(
                        "failed to install npm package (user prefix {}, --force): {err}",
                        prefix.display()
                    )
                };
                return Err(AcpError::protocol(msg));
            }
            // --force succeeded, fall through to PATH setup below.
        } else {
            let err = stderr.trim().to_string();
            let msg = if err.is_empty() {
                format!(
                    "failed to install npm package globally (user prefix {})",
                    prefix.display()
                )
            } else {
                format!(
                    "failed to install npm package globally (user prefix {}): {err}",
                    prefix.display()
                )
            };
            return Err(AcpError::protocol(msg));
        }
    }

    // Make sure the user prefix bin dir is in PATH for subsequent `which` lookups.
    crate::process::ensure_user_npm_prefix_in_path();

    Ok(())
}

async fn uninstall_npm_global_package(package: &str) -> Result<(), AcpError> {
    let package_name = package_name_from_spec(package);

    if !package_name.is_empty() {
        // Try uninstalling from the default global prefix.
        let output = crate::process::tokio_command("npm")
            .arg("uninstall")
            .arg("-g")
            .arg(&package_name)
            .output()
            .await
            .map_err(|e| AcpError::protocol(format!("failed to run npm uninstall -g: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // EACCES: the package may have been installed to the user-local
            // prefix via the EACCES fallback — try uninstalling from there.
            if stderr.contains("EACCES") {
                return uninstall_npm_from_user_prefix(&package_name).await;
            }
            let err = stderr.trim().to_string();
            let msg = if err.is_empty() {
                "failed to uninstall npm package globally".to_string()
            } else {
                format!("failed to uninstall npm package globally: {err}")
            };
            return Err(AcpError::protocol(msg));
        }

        // Also try removing from the user prefix (best-effort) in case the
        // package was installed in both locations.
        let _ = uninstall_npm_from_user_prefix(&package_name).await;
    }

    Ok(())
}

/// Uninstall an npm package from the user-local prefix (`~/.codeg/npm-global/`).
async fn uninstall_npm_from_user_prefix(package_name: &str) -> Result<(), AcpError> {
    let prefix = match crate::process::user_npm_prefix() {
        Some(p) if p.exists() => p,
        _ => return Ok(()),
    };

    let prefix_arg = format!("--prefix={}", prefix.display());
    let output = crate::process::tokio_command("npm")
        .arg("uninstall")
        .arg("-g")
        .arg(&prefix_arg)
        .arg(package_name)
        .output()
        .await
        .map_err(|e| {
            AcpError::protocol(format!(
                "failed to run npm uninstall -g with user prefix: {e}"
            ))
        })?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if err.is_empty() {
            format!(
                "failed to uninstall npm package from user prefix (exit code {})",
                output.status.code().unwrap_or(-1)
            )
        } else {
            format!("failed to uninstall npm package from user prefix: {err}")
        };
        return Err(AcpError::protocol(msg));
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillStorageKind {
    SkillDirectoryOnly,
    SkillDirectoryOrMarkdownFile,
}

#[derive(Debug, Clone)]
pub(crate) struct SkillStorageSpec {
    pub kind: SkillStorageKind,
    pub global_dirs: Vec<PathBuf>,
    pub project_rel_dirs: Vec<&'static str>,
}

fn home_dir_or_default() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn codex_home_dir() -> PathBuf {
    let configured = std::env::var("CODEX_HOME").ok().and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    match configured {
        Some(value) => {
            if value == "~" {
                home_dir_or_default()
            } else if let Some(remain) = value.strip_prefix("~/") {
                home_dir_or_default().join(remain)
            } else {
                PathBuf::from(value)
            }
        }
        None => home_dir_or_default().join(".codex"),
    }
}

fn codex_config_toml_path() -> PathBuf {
    codex_home_dir().join("config.toml")
}

fn codex_auth_json_path() -> PathBuf {
    codex_home_dir().join("auth.json")
}

fn opencode_primary_config_path() -> PathBuf {
    home_dir_or_default()
        .join(".config")
        .join("opencode")
        .join("opencode.json")
}

fn opencode_legacy_config_path() -> PathBuf {
    home_dir_or_default()
        .join(".config")
        .join("opencode")
        .join("config.json")
}

fn resolve_opencode_config_path() -> PathBuf {
    let primary = opencode_primary_config_path();
    if primary.exists() {
        return primary;
    }

    let legacy = opencode_legacy_config_path();
    if legacy.exists() {
        return legacy;
    }

    primary
}

fn opencode_auth_json_path() -> PathBuf {
    home_dir_or_default()
        .join(".local")
        .join("share")
        .join("opencode")
        .join("auth.json")
}

fn load_opencode_auth_json_raw() -> Option<String> {
    fs::read_to_string(opencode_auth_json_path()).ok()
}

// ---------------------------------------------------------------------------
// Cline config helpers
// ---------------------------------------------------------------------------

fn cline_data_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("CLINE_DIR") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    home_dir_or_default().join(".cline").join("data")
}

fn cline_global_state_path() -> PathBuf {
    cline_data_dir().join("globalState.json")
}

fn cline_secrets_path() -> PathBuf {
    cline_data_dir().join("secrets.json")
}

fn load_cline_secrets_json_raw() -> Option<String> {
    fs::read_to_string(cline_secrets_path()).ok()
}

/// Cline provider → secrets.json field name for the API key.
fn cline_api_key_field_for_provider(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "apiKey",
        "openrouter" => "openRouterApiKey",
        "openai-native" => "openAiNativeApiKey",
        "openai" => "openAiApiKey",
        "gemini" => "geminiApiKey",
        "deepseek" => "deepSeekApiKey",
        "mistral" => "mistralApiKey",
        "xai" => "xaiApiKey",
        _ => "openAiApiKey",
    }
}

/// Cline provider → globalState model ID key suffix.
/// Providers in ProviderKeyMap use `actMode{Suffix}` / `planMode{Suffix}`,
/// others use `actModeApiModelId` / `planModeApiModelId`.
fn cline_model_id_keys_for_provider(provider: &str) -> (&'static str, &'static str) {
    match provider {
        "openrouter" | "cline" => ("actModeOpenRouterModelId", "planModeOpenRouterModelId"),
        "openai" => ("actModeOpenAiModelId", "planModeOpenAiModelId"),
        "ollama" => ("actModeOllamaModelId", "planModeOllamaModelId"),
        "lmstudio" => ("actModeLmStudioModelId", "planModeLmStudioModelId"),
        "litellm" => ("actModeLiteLlmModelId", "planModeLiteLlmModelId"),
        "requesty" => ("actModeRequestyModelId", "planModeRequestyModelId"),
        "groq" => ("actModeGroqModelId", "planModeGroqModelId"),
        _ => ("actModeApiModelId", "planModeApiModelId"),
    }
}

/// Read globalState.json + secrets.json and merge into a unified config JSON
/// with keys: apiProvider, model, apiKey, apiBaseUrl.
fn load_cline_local_config_json() -> Option<String> {
    let mut merged = serde_json::Map::new();

    if let Ok(raw) = fs::read_to_string(cline_global_state_path()) {
        if let Ok(state) = serde_json::from_str::<serde_json::Value>(&raw) {
            // Cline uses actModeApiProvider / planModeApiProvider (prefer actMode)
            let provider = state
                .get("actModeApiProvider")
                .or_else(|| state.get("planModeApiProvider"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .unwrap_or("anthropic")
                .to_string();

            merged.insert(
                "apiProvider".to_string(),
                serde_json::Value::String(provider.clone()),
            );

            // Read model from provider-specific key
            let (act_key, _plan_key) = cline_model_id_keys_for_provider(&provider);
            if let Some(model_id) = state
                .get(act_key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                merged.insert(
                    "model".to_string(),
                    serde_json::Value::String(model_id.to_string()),
                );
            }

            // Read provider-specific baseUrl key
            let base_url_key = match provider.as_str() {
                "anthropic" => "anthropicBaseUrl",
                "gemini" => "geminiBaseUrl",
                "ollama" => "ollamaBaseUrl",
                "lmstudio" => "lmStudioBaseUrl",
                "litellm" => "liteLlmBaseUrl",
                "requesty" => "requestyBaseUrl",
                _ => "openAiBaseUrl",
            };
            if let Some(base_url) = state
                .get(base_url_key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                merged.insert(
                    "apiBaseUrl".to_string(),
                    serde_json::Value::String(base_url.to_string()),
                );
            }
        }
    }

    // Read API key from secrets.json based on provider
    if let Ok(raw) = fs::read_to_string(cline_secrets_path()) {
        if let Ok(secrets) = serde_json::from_str::<serde_json::Value>(&raw) {
            let provider = merged
                .get("apiProvider")
                .and_then(|v| v.as_str())
                .unwrap_or("anthropic");
            let key_field = cline_api_key_field_for_provider(provider);
            if let Some(api_key) = secrets
                .get(key_field)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                merged.insert(
                    "apiKey".to_string(),
                    serde_json::Value::String(api_key.to_string()),
                );
            }
        }
    }

    if merged.is_empty() {
        return None;
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(merged)).ok()
}

/// Split merged config back into globalState.json + secrets.json.
/// Writes `actModeApiProvider`, `planModeApiProvider`, provider-specific model keys,
/// `openAiBaseUrl`, and `welcomeViewCompleted` to globalState.json,
/// and the provider-specific API key to secrets.json.
fn persist_cline_local_config(config_patch_json: Option<&str>) -> Result<(), AcpError> {
    let Some(raw_patch) = config_patch_json else {
        return Ok(());
    };
    let runtime = serde_json::from_str::<AgentRuntimeConfig>(raw_patch)
        .map_err(|e| AcpError::protocol(format!("invalid config_json: {e}")))?;
    let patch = serde_json::from_str::<serde_json::Value>(raw_patch)
        .map_err(|e| AcpError::protocol(format!("invalid config_json: {e}")))?;

    let provider = patch
        .get("apiProvider")
        .and_then(|v| v.as_str())
        .unwrap_or("anthropic")
        .to_string();

    // --- Update globalState.json (merge) ---
    let gs_path = cline_global_state_path();
    let mut gs = if gs_path.exists() {
        match fs::read_to_string(&gs_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        {
            Some(existing) if existing.is_object() => existing,
            _ => serde_json::json!({}),
        }
    } else {
        serde_json::json!({})
    };
    let gs_obj = gs
        .as_object_mut()
        .ok_or_else(|| AcpError::protocol("globalState root must be object"))?;

    // Cline checks welcomeViewCompleted first in isAuthConfigured()
    gs_obj.insert(
        "welcomeViewCompleted".to_string(),
        serde_json::Value::Bool(true),
    );

    // Set both act/plan mode providers
    gs_obj.insert(
        "actModeApiProvider".to_string(),
        serde_json::Value::String(provider.clone()),
    );
    gs_obj.insert(
        "planModeApiProvider".to_string(),
        serde_json::Value::String(provider.clone()),
    );

    // Set provider-specific model ID keys
    let (act_model_key, plan_model_key) = cline_model_id_keys_for_provider(&provider);
    match trim_non_empty(runtime.model) {
        Some(model) => {
            gs_obj.insert(
                act_model_key.to_string(),
                serde_json::Value::String(model.clone()),
            );
            gs_obj.insert(plan_model_key.to_string(), serde_json::Value::String(model));
        }
        None => {
            gs_obj.remove(act_model_key);
            gs_obj.remove(plan_model_key);
        }
    }

    // Each provider uses its own baseUrl key in globalState
    let base_url_key = match provider.as_str() {
        "anthropic" => "anthropicBaseUrl",
        "gemini" => "geminiBaseUrl",
        "ollama" => "ollamaBaseUrl",
        "lmstudio" => "lmStudioBaseUrl",
        "litellm" => "liteLlmBaseUrl",
        "requesty" => "requestyBaseUrl",
        _ => "openAiBaseUrl",
    };
    match trim_non_empty(runtime.api_base_url) {
        Some(base_url) => {
            gs_obj.insert(
                base_url_key.to_string(),
                serde_json::Value::String(base_url),
            );
        }
        None => {
            gs_obj.remove(base_url_key);
        }
    }

    if let Some(parent) = gs_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| AcpError::protocol(format!("create cline data directory failed: {e}")))?;
    }
    let serialized_gs = serde_json::to_string_pretty(&gs)
        .map_err(|e| AcpError::protocol(format!("serialize cline globalState failed: {e}")))?;
    fs::write(&gs_path, format!("{serialized_gs}\n"))
        .map_err(|e| AcpError::protocol(format!("write cline globalState failed: {e}")))?;

    // --- Update secrets.json ---
    let secrets_path = cline_secrets_path();
    let mut secrets = if secrets_path.exists() {
        match fs::read_to_string(&secrets_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        {
            Some(existing) if existing.is_object() => existing,
            _ => serde_json::json!({}),
        }
    } else {
        serde_json::json!({})
    };
    let secrets_obj = secrets
        .as_object_mut()
        .ok_or_else(|| AcpError::protocol("secrets root must be object"))?;

    let key_field = cline_api_key_field_for_provider(&provider);
    match trim_non_empty(runtime.api_key) {
        Some(api_key) => {
            secrets_obj.insert(key_field.to_string(), serde_json::Value::String(api_key));
        }
        None => {
            secrets_obj.remove(key_field);
        }
    }

    if let Some(parent) = secrets_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| AcpError::protocol(format!("create cline data directory failed: {e}")))?;
    }
    let serialized_secrets = serde_json::to_string_pretty(&secrets)
        .map_err(|e| AcpError::protocol(format!("serialize cline secrets failed: {e}")))?;
    fs::write(&secrets_path, format!("{serialized_secrets}\n"))
        .map_err(|e| AcpError::protocol(format!("write cline secrets failed: {e}")))?;

    Ok(())
}

fn load_codex_auth_json_raw() -> Option<String> {
    fs::read_to_string(codex_auth_json_path()).ok()
}

fn load_codex_config_toml_raw() -> Option<String> {
    fs::read_to_string(codex_config_toml_path()).ok()
}

fn load_codex_local_config_json() -> Option<String> {
    let mut merged = serde_json::Map::new();

    if let Ok(raw_toml) = fs::read_to_string(codex_config_toml_path()) {
        if let Ok(value) = raw_toml.parse::<toml::Value>() {
            if let Some(model) = value
                .get("model")
                .and_then(|item| item.as_str())
                .map(str::trim)
                .filter(|item| !item.is_empty())
            {
                merged.insert(
                    "model".to_string(),
                    serde_json::Value::String(model.to_string()),
                );
            }

            let model_provider = value
                .get("model_provider")
                .and_then(|item| item.as_str())
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string);

            let mut api_base_url: Option<String> = None;
            if let Some(provider) = model_provider {
                api_base_url = value
                    .get("model_providers")
                    .and_then(|table| table.get(provider.as_str()))
                    .and_then(|table| table.get("base_url"))
                    .and_then(|item| item.as_str())
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_string);
            }
            if api_base_url.is_none() {
                api_base_url = value
                    .get("model_providers")
                    .and_then(|table| table.as_table())
                    .and_then(|providers| {
                        providers.values().find_map(|item| {
                            item.get("base_url")
                                .and_then(|base| base.as_str())
                                .map(str::trim)
                                .filter(|base| !base.is_empty())
                                .map(str::to_string)
                        })
                    });
            }
            if let Some(base_url) = api_base_url {
                merged.insert(
                    "apiBaseUrl".to_string(),
                    serde_json::Value::String(base_url),
                );
            }

            if let Some(env) = value.get("env").and_then(|item| item.as_table()) {
                let mut env_map = serde_json::Map::new();
                for (key, item) in env {
                    let Some(raw) = item.as_str() else {
                        continue;
                    };
                    let trimmed = raw.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    env_map.insert(
                        key.to_string(),
                        serde_json::Value::String(trimmed.to_string()),
                    );
                }
                if !env_map.is_empty() {
                    merged.insert("env".to_string(), serde_json::Value::Object(env_map));
                }
            }
        }
    }

    if let Ok(raw_auth) = fs::read_to_string(codex_auth_json_path()) {
        if let Ok(auth) = serde_json::from_str::<serde_json::Value>(&raw_auth) {
            if let Some(api_key) = auth
                .get("OPENAI_API_KEY")
                .and_then(|item| item.as_str())
                .map(str::trim)
                .filter(|item| !item.is_empty())
            {
                merged.insert(
                    "apiKey".to_string(),
                    serde_json::Value::String(api_key.to_string()),
                );
            }
        }
    }

    if merged.is_empty() {
        return None;
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(merged)).ok()
}

fn persist_codex_local_config(config_patch_json: Option<&str>) -> Result<(), AcpError> {
    let Some(raw_patch) = config_patch_json else {
        return Ok(());
    };
    let runtime = serde_json::from_str::<AgentRuntimeConfig>(raw_patch)
        .map_err(|e| AcpError::protocol(format!("invalid config_json: {e}")))?;
    let AgentRuntimeConfig {
        api_base_url,
        api_key,
        model,
        env,
    } = runtime;

    let config_path = codex_config_toml_path();
    let mut toml_value = if config_path.exists() {
        match fs::read_to_string(&config_path)
            .ok()
            .and_then(|raw| raw.parse::<toml::Value>().ok())
        {
            Some(existing) if existing.is_table() => existing,
            _ => toml::Value::Table(toml::map::Map::new()),
        }
    } else {
        toml::Value::Table(toml::map::Map::new())
    };

    let table = toml_value
        .as_table_mut()
        .ok_or_else(|| AcpError::protocol("codex config root must be a TOML table"))?;

    match trim_non_empty(model) {
        Some(model) => {
            table.insert("model".to_string(), toml::Value::String(model));
        }
        None => {
            table.remove("model");
        }
    }

    let provider_name = table
        .get("model_provider")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "codeg".to_string());
    table.insert(
        "model_provider".to_string(),
        toml::Value::String(provider_name.clone()),
    );

    let providers_item = table
        .entry("model_providers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if !providers_item.is_table() {
        *providers_item = toml::Value::Table(toml::map::Map::new());
    }
    let providers = providers_item
        .as_table_mut()
        .ok_or_else(|| AcpError::protocol("invalid model_providers table"))?;
    let provider_item = providers
        .entry(provider_name.clone())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if !provider_item.is_table() {
        *provider_item = toml::Value::Table(toml::map::Map::new());
    }
    let provider_table = provider_item
        .as_table_mut()
        .ok_or_else(|| AcpError::protocol("invalid model provider table"))?;
    match trim_non_empty(api_base_url) {
        Some(base_url) => {
            provider_table.insert("base_url".to_string(), toml::Value::String(base_url));
        }
        None => {
            provider_table.remove("base_url");
        }
    }
    if provider_name == "codeg" {
        provider_table.insert("name".to_string(), toml::Value::String("codeg".to_string()));
        provider_table.insert(
            "wire_api".to_string(),
            toml::Value::String("responses".to_string()),
        );
        provider_table.insert(
            "requires_openai_auth".to_string(),
            toml::Value::Boolean(true),
        );
    }

    if env.is_empty() {
        table.remove("env");
    } else {
        let mut env_table = toml::map::Map::new();
        for (key, value) in env {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                continue;
            }
            env_table.insert(key, toml::Value::String(trimmed.to_string()));
        }
        if env_table.is_empty() {
            table.remove("env");
        } else {
            table.insert("env".to_string(), toml::Value::Table(env_table));
        }
    }

    let serialized_toml = toml::to_string_pretty(&toml_value)
        .map_err(|e| AcpError::protocol(format!("serialize codex toml failed: {e}")))?;
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            AcpError::protocol(format!("create codex config directory failed: {e}"))
        })?;
    }
    fs::write(&config_path, format!("{serialized_toml}\n"))
        .map_err(|e| AcpError::protocol(format!("write codex config failed: {e}")))?;

    let auth_path = codex_auth_json_path();
    let mut auth_value = if auth_path.exists() {
        match fs::read_to_string(&auth_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        {
            Some(existing) if existing.is_object() => existing,
            _ => serde_json::json!({}),
        }
    } else {
        serde_json::json!({})
    };
    let auth_obj = auth_value
        .as_object_mut()
        .ok_or_else(|| AcpError::protocol("codex auth root must be object"))?;
    match trim_non_empty(api_key) {
        Some(api_key) => {
            auth_obj.insert(
                "OPENAI_API_KEY".to_string(),
                serde_json::Value::String(api_key),
            );
        }
        None => {
            auth_obj.remove("OPENAI_API_KEY");
        }
    }
    let serialized_auth = serde_json::to_string_pretty(&auth_value)
        .map_err(|e| AcpError::protocol(format!("serialize codex auth failed: {e}")))?;
    if let Some(parent) = auth_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| AcpError::protocol(format!("create codex auth directory failed: {e}")))?;
    }
    fs::write(&auth_path, format!("{serialized_auth}\n"))
        .map_err(|e| AcpError::protocol(format!("write codex auth failed: {e}")))?;

    Ok(())
}

fn persist_codex_native_config_files(
    codex_auth_json: Option<&str>,
    codex_config_toml: Option<&str>,
) -> Result<(), AcpError> {
    if let Some(raw_toml) = codex_config_toml {
        toml::from_str::<toml::Table>(raw_toml)
            .map_err(|e| AcpError::protocol(format!("invalid codex config.toml: {e}")))?;
        let path = codex_config_toml_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AcpError::protocol(format!("create codex directory failed: {e}")))?;
        }
        fs::write(&path, raw_toml)
            .map_err(|e| AcpError::protocol(format!("write codex config.toml failed: {e}")))?;
    }

    if let Some(raw_auth) = codex_auth_json {
        let parsed = serde_json::from_str::<serde_json::Value>(raw_auth)
            .map_err(|e| AcpError::protocol(format!("invalid codex auth.json: {e}")))?;
        if !parsed.is_object() {
            return Err(AcpError::protocol(
                "invalid codex auth.json: root must be a JSON object",
            ));
        }
        let path = codex_auth_json_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AcpError::protocol(format!("create codex directory failed: {e}")))?;
        }
        fs::write(&path, raw_auth)
            .map_err(|e| AcpError::protocol(format!("write codex auth.json failed: {e}")))?;
    }

    Ok(())
}

fn persist_opencode_auth_json(raw_auth: &str) -> Result<(), AcpError> {
    let parsed = serde_json::from_str::<serde_json::Value>(raw_auth)
        .map_err(|e| AcpError::protocol(format!("invalid opencode auth.json: {e}")))?;
    if !parsed.is_object() {
        return Err(AcpError::protocol(
            "invalid opencode auth.json: root must be a JSON object",
        ));
    }
    let path = opencode_auth_json_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| AcpError::protocol(format!("create opencode directory failed: {e}")))?;
    }
    fs::write(&path, format!("{raw_auth}\n"))
        .map_err(|e| AcpError::protocol(format!("write opencode auth.json failed: {e}")))?;
    Ok(())
}

fn agent_local_config_path(agent_type: AgentType) -> Option<PathBuf> {
    match agent_type {
        AgentType::ClaudeCode => Some(home_dir_or_default().join(".claude").join("settings.json")),
        AgentType::Gemini => Some(home_dir_or_default().join(".gemini").join("settings.json")),
        AgentType::OpenCode => Some(resolve_opencode_config_path()),
        AgentType::Cline => Some(cline_global_state_path()),
        _ => None,
    }
}

pub(crate) fn load_agent_local_config_json(agent_type: AgentType) -> Option<String> {
    if agent_type == AgentType::Codex {
        return load_codex_local_config_json();
    }
    if agent_type == AgentType::Cline {
        return load_cline_local_config_json();
    }

    let path = agent_local_config_path(agent_type)?;
    if !path.exists() {
        return None;
    }

    let raw = fs::read_to_string(path).ok()?;
    let parsed = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    if !parsed.is_object() {
        return None;
    }
    serde_json::to_string_pretty(&parsed).ok()
}

fn merge_json_values(base: &mut serde_json::Value, patch: &serde_json::Value) {
    if let (Some(base_obj), Some(patch_obj)) = (base.as_object_mut(), patch.as_object()) {
        for (key, patch_value) in patch_obj {
            if patch_value.is_null() {
                // null in patch means "remove this key"
                base_obj.remove(key);
                continue;
            }
            match base_obj.get_mut(key) {
                Some(base_value) => merge_json_values(base_value, patch_value),
                None => {
                    base_obj.insert(key.clone(), patch_value.clone());
                }
            }
        }
        return;
    }

    *base = patch.clone();
}

fn persist_agent_local_config_json(
    agent_type: AgentType,
    config_patch_json: Option<&str>,
) -> Result<(), AcpError> {
    if agent_type == AgentType::Codex {
        return persist_codex_local_config(config_patch_json);
    }
    if agent_type == AgentType::Cline {
        return persist_cline_local_config(config_patch_json);
    }

    let Some(path) = agent_local_config_path(agent_type) else {
        return Ok(());
    };
    let Some(raw_patch) = config_patch_json else {
        return Ok(());
    };

    let patch = serde_json::from_str::<serde_json::Value>(raw_patch)
        .map_err(|e| AcpError::protocol(format!("invalid config_json: {e}")))?;
    if !patch.is_object() {
        return Err(AcpError::protocol(
            "invalid config_json: root must be a JSON object",
        ));
    }

    if agent_type == AgentType::OpenCode {
        let serialized = serde_json::to_string_pretty(&patch)
            .map_err(|e| AcpError::protocol(format!("serialize config_json failed: {e}")))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AcpError::protocol(format!("create config directory failed: {e}")))?;
        }
        fs::write(&path, format!("{serialized}\n"))
            .map_err(|e| AcpError::protocol(format!("write local config failed: {e}")))?;
        return Ok(());
    }

    let mut base = if path.exists() {
        match fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        {
            Some(existing) if existing.is_object() => existing,
            _ => serde_json::json!({}),
        }
    } else {
        serde_json::json!({})
    };

    merge_json_values(&mut base, &patch);
    let serialized = serde_json::to_string_pretty(&base)
        .map_err(|e| AcpError::protocol(format!("serialize config_json failed: {e}")))?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| AcpError::protocol(format!("create config directory failed: {e}")))?;
    }
    fs::write(&path, format!("{serialized}\n"))
        .map_err(|e| AcpError::protocol(format!("write local config failed: {e}")))?;

    Ok(())
}

pub(crate) fn skill_storage_spec(agent_type: AgentType) -> Option<SkillStorageSpec> {
    match agent_type {
        AgentType::ClaudeCode => Some(SkillStorageSpec {
            kind: SkillStorageKind::SkillDirectoryOnly,
            global_dirs: vec![home_dir_or_default().join(".claude").join("skills")],
            project_rel_dirs: vec![".claude/skills"],
        }),
        AgentType::Codex => Some(SkillStorageSpec {
            kind: SkillStorageKind::SkillDirectoryOrMarkdownFile,
            global_dirs: vec![
                codex_home_dir().join("skills"),
                // `.system` is where Codex CLI stores its own bundled
                // skills (imagegen, skill-creator, etc.). The directory
                // name is a Codex convention, not a stable contract —
                // if Codex renames it we'll silently stop listing them.
                // `is_read_only_skill_path` mirrors this path to prevent
                // edit/delete from clobbering CLI assets.
                codex_home_dir().join("skills").join(".system"),
                home_dir_or_default().join(".agents").join("skills"),
            ],
            project_rel_dirs: vec![".codex/skills", ".agents/skills"],
        }),
        AgentType::OpenCode => Some(SkillStorageSpec {
            kind: SkillStorageKind::SkillDirectoryOnly,
            global_dirs: vec![home_dir_or_default()
                .join(".config")
                .join("opencode")
                .join("skills")],
            project_rel_dirs: vec![".opencode/skills"],
        }),
        AgentType::Gemini => Some(SkillStorageSpec {
            kind: SkillStorageKind::SkillDirectoryOnly,
            global_dirs: vec![
                home_dir_or_default().join(".gemini").join("skills"),
                home_dir_or_default().join(".agents").join("skills"),
            ],
            project_rel_dirs: vec![".gemini/skills", ".agents/skills"],
        }),
        AgentType::OpenClaw => Some(SkillStorageSpec {
            kind: SkillStorageKind::SkillDirectoryOnly,
            global_dirs: vec![home_dir_or_default().join(".openclaw").join("skills")],
            project_rel_dirs: vec!["skills"],
        }),
        AgentType::Cline => Some(SkillStorageSpec {
            kind: SkillStorageKind::SkillDirectoryOnly,
            global_dirs: vec![
                home_dir_or_default().join(".agents").join("skills"),
                home_dir_or_default().join(".cline").join("skills"),
            ],
            project_rel_dirs: vec![
                ".agents/skills",
                ".cline/skills",
                ".clinerules/skills",
                ".claude/skills",
            ],
        }),
    }
}

fn scope_rank(scope: AgentSkillScope) -> u8 {
    match scope {
        AgentSkillScope::Global => 0,
        AgentSkillScope::Project => 1,
    }
}

pub(crate) fn validate_skill_id(raw: &str) -> Result<String, AcpError> {
    let id = raw.trim();
    if id.is_empty() {
        return Err(AcpError::protocol("skill id cannot be empty"));
    }
    if id.starts_with('.') {
        return Err(AcpError::protocol("skill id cannot start with a dot (.)"));
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(AcpError::protocol(
            "skill id cannot contain path separators",
        ));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(AcpError::protocol(
            "skill id can only include letters, numbers, '-', '_' and '.'",
        ));
    }
    Ok(id.to_string())
}

pub(crate) fn scoped_skill_dirs(
    agent_type: AgentType,
    scope: AgentSkillScope,
    workspace_path: Option<&str>,
) -> Result<Vec<PathBuf>, AcpError> {
    let spec = skill_storage_spec(agent_type).ok_or_else(|| {
        AcpError::protocol(format!(
            "{agent_type} skills are not supported in Settings yet"
        ))
    })?;

    match scope {
        AgentSkillScope::Global => Ok(spec.global_dirs),
        AgentSkillScope::Project => {
            let workspace = workspace_path
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .ok_or_else(|| {
                    AcpError::protocol("workspace_path is required for project scoped skills")
                })?;
            Ok(spec
                .project_rel_dirs
                .iter()
                .map(|relative| PathBuf::from(workspace).join(relative))
                .collect())
        }
    }
}

pub(crate) fn preferred_scope_skill_dir(
    agent_type: AgentType,
    scope: AgentSkillScope,
    workspace_path: Option<&str>,
) -> Result<PathBuf, AcpError> {
    let dirs = scoped_skill_dirs(agent_type, scope, workspace_path)?;
    dirs.into_iter()
        .next()
        .ok_or_else(|| AcpError::protocol("no skill directory resolved for this agent"))
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

fn skill_name_from_id(id: &str) -> String {
    id.to_string()
}

/// Best-effort extraction of a one-line skill description from a markdown
/// file's YAML frontmatter. Prefers `short-description` (commonly nested under
/// a `metadata:` block) and falls back to a top-level `description`. Only the
/// first 4 KiB is read; frontmatter always fits, and skill bodies can be large.
fn read_skill_description(content_path: &Path) -> Option<String> {
    use std::io::Read;
    let mut file = fs::File::open(content_path).ok()?;
    let mut buf = [0u8; 4096];
    let n = file.read(&mut buf).ok()?;
    let head = std::str::from_utf8(&buf[..n]).ok()?;

    let mut lines = head.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }

    let mut short: Option<String> = None;
    let mut long: Option<String> = None;
    for line in lines {
        let trimmed_end = line.trim_end();
        if trimmed_end == "---" || trimmed_end == "..." {
            break;
        }
        let is_top_level = !line.starts_with(|c: char| c.is_whitespace());
        let stripped = line.trim();

        // `short-description` is allowed at any indent so it resolves when
        // nested under `metadata:` (Codex's `.system` skills follow this).
        if short.is_none() {
            if let Some(rest) = stripped.strip_prefix("short-description:") {
                if let Some(val) = parse_frontmatter_scalar(rest) {
                    short = Some(val);
                    break;
                }
            }
        }
        // `description` is only honored at the top level to avoid colliding
        // with unrelated nested `description:` keys.
        if is_top_level && long.is_none() {
            if let Some(rest) = line.strip_prefix("description:") {
                if let Some(val) = parse_frontmatter_scalar(rest) {
                    long = Some(val);
                }
            }
        }
    }
    short.or(long)
}

/// Read a single-line YAML scalar (with optional matching quotes). Returns
/// `None` for empty values or block-scalar markers (`|` / `>`) we can't span.
fn parse_frontmatter_scalar(rest: &str) -> Option<String> {
    let val = rest.trim();
    if val.starts_with('|') || val.starts_with('>') {
        return None;
    }
    let unquoted = val
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| val.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(val)
        .trim();
    if unquoted.is_empty() {
        None
    } else {
        Some(unquoted.to_string())
    }
}

fn build_skill_item(
    id: String,
    scope: AgentSkillScope,
    layout: AgentSkillLayout,
    path: PathBuf,
) -> AgentSkillItem {
    let description = read_skill_description(&skill_content_path(layout, &path));
    AgentSkillItem {
        name: skill_name_from_id(&id),
        id,
        scope,
        layout,
        path: path.to_string_lossy().to_string(),
        description,
        read_only: false,
    }
}

/// Codex ships a handful of built-in skills under `~/.codex/skills/.system/`
/// (imagegen, skill-creator, etc.). We scan that directory so users see
/// these in the `$` autocomplete and the Skills settings list — but any
/// write to those files would clobber the CLI's own assets.
fn is_read_only_skill_path(agent_type: AgentType, skill_path: &Path) -> bool {
    if agent_type != AgentType::Codex {
        return false;
    }
    let ro_root = codex_home_dir().join("skills").join(".system");
    skill_path.starts_with(&ro_root)
}

fn skill_content_path(layout: AgentSkillLayout, skill_path: &Path) -> PathBuf {
    match layout {
        AgentSkillLayout::SkillDirectory => skill_path.join("SKILL.md"),
        AgentSkillLayout::MarkdownFile => skill_path.to_path_buf(),
    }
}

/// Symlink-safe removal: if `path` is a symlink (to a file or directory),
/// only the link itself is removed. Otherwise directories are removed
/// recursively and files are unlinked. This prevents `remove_dir_all` from
/// accidentally wiping the contents of a symlink target — which is critical
/// for the Experts feature where agent skill dirs may contain symlinks into
/// the central `~/.codeg/skills/` store.
pub(crate) fn remove_skill_entry(path: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    let file_type = meta.file_type();

    #[cfg(windows)]
    let is_reparse_point = {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    };

    if file_type.is_symlink() {
        #[cfg(windows)]
        {
            // Directory symlinks on Windows require remove_dir.
            return match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                    fs::remove_dir(path)
                }
                Err(err) => Err(err),
            };
        }

        #[cfg(not(windows))]
        {
            return fs::remove_file(path);
        }
    }

    if file_type.is_dir() {
        #[cfg(windows)]
        {
            // Junctions are directory reparse points; remove only the link.
            if is_reparse_point {
                return fs::remove_dir(path);
            }
        }
        return fs::remove_dir_all(path);
    }

    fs::remove_file(path)
}

fn list_skills_from_dir(
    scope: AgentSkillScope,
    dir: &Path,
    kind: SkillStorageKind,
) -> Result<Vec<AgentSkillItem>, AcpError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(dir)
        .map_err(|e| AcpError::protocol(format!("failed to read skills directory: {e}")))?;

    let mut by_id: BTreeMap<String, AgentSkillItem> = BTreeMap::new();
    for entry in entries {
        let entry = match entry {
            Ok(value) => value,
            Err(_) => continue,
        };
        let path = entry.path();
        let file_name = entry.file_name();
        let id = file_name.to_string_lossy().to_string();

        if path.is_dir()
            && matches!(
                kind,
                SkillStorageKind::SkillDirectoryOnly
                    | SkillStorageKind::SkillDirectoryOrMarkdownFile
            )
        {
            let skill_doc = path.join("SKILL.md");
            if !skill_doc.is_file() {
                continue;
            }
            by_id.insert(
                id.clone(),
                build_skill_item(id, scope, AgentSkillLayout::SkillDirectory, path),
            );
            continue;
        }

        if path.is_file()
            && matches!(kind, SkillStorageKind::SkillDirectoryOrMarkdownFile)
            && is_markdown_file(&path)
        {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
                .unwrap_or_else(|| id.clone());
            if by_id.contains_key(&stem) {
                continue;
            }
            by_id.insert(
                stem.clone(),
                build_skill_item(stem, scope, AgentSkillLayout::MarkdownFile, path),
            );
        }
    }

    Ok(by_id.into_values().collect())
}

fn locate_existing_skill(
    dir: &Path,
    kind: SkillStorageKind,
    skill_id: &str,
    scope: AgentSkillScope,
) -> Option<AgentSkillItem> {
    if matches!(
        kind,
        SkillStorageKind::SkillDirectoryOnly | SkillStorageKind::SkillDirectoryOrMarkdownFile
    ) {
        let skill_dir = dir.join(skill_id);
        if skill_dir.is_dir() && skill_dir.join("SKILL.md").is_file() {
            return Some(build_skill_item(
                skill_id.to_string(),
                scope,
                AgentSkillLayout::SkillDirectory,
                skill_dir,
            ));
        }
    }

    if matches!(kind, SkillStorageKind::SkillDirectoryOrMarkdownFile) {
        let file_path = dir.join(format!("{skill_id}.md"));
        if file_path.is_file() {
            return Some(build_skill_item(
                skill_id.to_string(),
                scope,
                AgentSkillLayout::MarkdownFile,
                file_path,
            ));
        }
    }

    None
}

fn locate_existing_skill_across_dirs(
    dirs: &[PathBuf],
    kind: SkillStorageKind,
    skill_id: &str,
    scope: AgentSkillScope,
) -> Option<AgentSkillItem> {
    for dir in dirs {
        if let Some(found) = locate_existing_skill(dir, kind, skill_id, scope) {
            return Some(found);
        }
    }
    None
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRuntimeConfig {
    #[serde(default, alias = "api_base_url")]
    api_base_url: Option<String>,
    #[serde(default, alias = "api_key")]
    api_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

fn trim_non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Primary env var keys for each agent type: (api_base_url, api_key, model).
/// Shared by runtime env resolution, model-provider cascade, and config patching.
fn agent_env_keys(agent_type: AgentType) -> (&'static str, &'static str, &'static str) {
    match agent_type {
        AgentType::ClaudeCode => (
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_MODEL",
        ),
        AgentType::Gemini => ("GOOGLE_GEMINI_BASE_URL", "GEMINI_API_KEY", "GEMINI_MODEL"),
        _ => ("OPENAI_BASE_URL", "OPENAI_API_KEY", "OPENAI_MODEL"),
    }
}

/// Serialize a BTreeMap into env_json for database storage.
/// Returns `None` when the map is empty.
fn serialize_env_map(env: &BTreeMap<String, String>) -> Result<Option<String>, AcpError> {
    if env.is_empty() {
        Ok(None)
    } else {
        serde_json::to_string(env)
            .map(Some)
            .map_err(|e| AcpError::protocol(e.to_string()))
    }
}

pub(crate) fn build_runtime_env_from_setting(
    agent_type: AgentType,
    setting: Option<&crate::db::entities::agent_setting::Model>,
    local_config_json: Option<&str>,
) -> BTreeMap<String, String> {
    let mut merged = setting
        .and_then(|model| model.env_json.as_deref())
        .and_then(|raw| serde_json::from_str::<BTreeMap<String, String>>(raw).ok())
        .unwrap_or_default();

    let Some(raw_config_json) = local_config_json else {
        return merged;
    };
    let Ok(config) = serde_json::from_str::<AgentRuntimeConfig>(raw_config_json) else {
        return merged;
    };

    for (key, value) in config.env {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        merged.insert(key, trimmed.to_string());
    }

    let (api_base_url_key, api_key_key, model_key) = agent_env_keys(agent_type);
    if let Some(value) = trim_non_empty(config.api_base_url) {
        merged.insert(api_base_url_key.to_string(), value);
    }
    if let Some(value) = trim_non_empty(config.api_key) {
        merged.insert(api_key_key.to_string(), value);
    }
    if agent_type != AgentType::ClaudeCode {
        if let Some(value) = trim_non_empty(config.model) {
            merged.insert(model_key.to_string(), value);
        }
    }

    merged
}

/// Resolve model provider credentials into runtime env vars if `model_provider_id` is set.
pub(crate) async fn apply_model_provider_env(
    agent_type: AgentType,
    setting: Option<&crate::db::entities::agent_setting::Model>,
    runtime_env: &mut BTreeMap<String, String>,
    conn: &sea_orm::DatabaseConnection,
) {
    let provider_id = match setting.and_then(|s| s.model_provider_id) {
        Some(id) => id,
        None => return,
    };
    let provider = match model_provider_service::get_by_id(conn, provider_id).await {
        Ok(Some(p)) => p,
        _ => return,
    };
    let (url_key, key_key, _) = agent_env_keys(agent_type);
    if !provider.api_url.trim().is_empty() {
        runtime_env.insert(url_key.to_string(), provider.api_url.clone());
    }
    if !provider.api_key.trim().is_empty() {
        runtime_env.insert(key_key.to_string(), provider.api_key.clone());
    }
}

/// Claude Code provider-model JSON keys → ANTHROPIC_*_MODEL env var names.
const CLAUDE_MODEL_KEY_MAP: &[(&str, &str)] = &[
    ("main", "ANTHROPIC_MODEL"),
    ("reasoning", "ANTHROPIC_REASONING_MODEL"),
    ("haiku", "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
    ("sonnet", "ANTHROPIC_DEFAULT_SONNET_MODEL"),
    ("opus", "ANTHROPIC_DEFAULT_OPUS_MODEL"),
];

/// Parse the model field stored on a model_provider into the env-var actions to
/// apply on the dependent agent's `env_json` / local config file.
///
/// The provider's model field is authoritative: every env key relevant to the
/// agent type is returned, with `Some(value)` meaning "set" and `None` meaning
/// "clear". This lets the caller overwrite even when the provider's value is
/// empty.
///
/// - Claude: returns 5 entries (one per ANTHROPIC_*_MODEL). Each entry is `None`
///   when the provider's JSON omits that key or has an empty value.
/// - Gemini: returns `GEMINI_MODEL`.
/// - Codex: returns `OPENAI_MODEL` so the provider can override env_json (the
///   root `model` in `config.toml` is handled separately by
///   `provider_codex_model_action`).
/// - Others: returns `OPENAI_MODEL`.
pub(crate) fn parse_provider_model(
    agent_type: AgentType,
    raw: Option<&str>,
) -> BTreeMap<String, Option<String>> {
    let mut out: BTreeMap<String, Option<String>> = BTreeMap::new();
    let trimmed_raw = raw.map(str::trim).filter(|s| !s.is_empty());
    match agent_type {
        AgentType::ClaudeCode => {
            let parsed = trimmed_raw
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                .and_then(|v| v.as_object().cloned());
            for (key, env_name) in CLAUDE_MODEL_KEY_MAP {
                let value = parsed
                    .as_ref()
                    .and_then(|obj| obj.get(*key))
                    .and_then(|x| x.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                out.insert((*env_name).to_string(), value);
            }
        }
        AgentType::Gemini => {
            out.insert("GEMINI_MODEL".to_string(), trimmed_raw.map(str::to_string));
        }
        _ => {
            out.insert("OPENAI_MODEL".to_string(), trimmed_raw.map(str::to_string));
        }
    }
    out
}

/// Action to apply to the Codex `config.toml` root `model` key.
pub(crate) enum CodexModelAction {
    /// Not a Codex agent — leave the toml untouched.
    NoOp,
    /// Set the `model` key to this value.
    Set(String),
    /// Remove the `model` key.
    Clear,
}

pub(crate) fn provider_codex_model_action(
    agent_type: AgentType,
    raw: Option<&str>,
) -> CodexModelAction {
    if agent_type != AgentType::Codex {
        return CodexModelAction::NoOp;
    }
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => CodexModelAction::Set(v.to_string()),
        None => CodexModelAction::Clear,
    }
}

/// Update on-disk config files for a single agent when model provider credentials change.
/// Uses `agent_env_keys` to determine the correct env var names per agent type.
///
/// For `model_env`: entries with `Some(value)` are written; entries with `None`
/// are explicitly cleared (overwritten with empty string in the env-patch, so
/// `persist_agent_local_config_json` removes them).
fn cascade_update_agent_config(
    agent_type: AgentType,
    api_url: &str,
    api_key: &str,
    model_env: &BTreeMap<String, Option<String>>,
    codex_model: &CodexModelAction,
) -> Result<(), AcpError> {
    let (url_key, key_key, _) = agent_env_keys(agent_type);
    match agent_type {
        AgentType::ClaudeCode | AgentType::Gemini => {
            // Write into config.env (not root-level). For model entries, use
            // JSON-null for "clear" — `merge_json_values` interprets null as
            // "remove this key".
            let mut env = serde_json::Map::new();
            env.insert(
                url_key.to_string(),
                serde_json::Value::String(api_url.to_string()),
            );
            env.insert(
                key_key.to_string(),
                serde_json::Value::String(api_key.to_string()),
            );
            for (k, v) in model_env {
                let value = match v {
                    Some(s) => serde_json::Value::String(s.clone()),
                    None => serde_json::Value::Null,
                };
                env.insert(k.clone(), value);
            }
            let patch = serde_json::json!({ "env": env });
            let patch_str =
                serde_json::to_string(&patch).map_err(|e| AcpError::protocol(e.to_string()))?;
            persist_agent_local_config_json(agent_type, Some(&patch_str))?;
        }
        AgentType::OpenClaw => {
            // agent_local_config_path returns None for OpenClaw — no-op
        }
        AgentType::Codex => {
            let auth_path = codex_auth_json_path();
            let mut auth_obj = if auth_path.exists() {
                fs::read_to_string(&auth_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                    .filter(|v| v.is_object())
                    .unwrap_or_else(|| serde_json::json!({}))
            } else {
                serde_json::json!({})
            };
            if !api_key.trim().is_empty() {
                auth_obj[key_key] = serde_json::Value::String(api_key.to_string());
            }
            let auth_str = serde_json::to_string_pretty(&auth_obj)
                .map_err(|e| AcpError::protocol(e.to_string()))?;

            let config_path = codex_config_toml_path();
            let mut toml_value = if config_path.exists() {
                fs::read_to_string(&config_path)
                    .ok()
                    .and_then(|raw| raw.parse::<toml::Value>().ok())
                    .filter(|v| v.is_table())
                    .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()))
            } else {
                toml::Value::Table(toml::map::Map::new())
            };
            let table = toml_value
                .as_table_mut()
                .ok_or_else(|| AcpError::protocol("codex config root must be a TOML table"))?;
            table.remove("api_base_url");

            let provider_name = table
                .get("model_provider")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| "codeg".to_string());
            table.insert(
                "model_provider".to_string(),
                toml::Value::String(provider_name.clone()),
            );

            let providers_item = table
                .entry("model_providers".to_string())
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            if !providers_item.is_table() {
                *providers_item = toml::Value::Table(toml::map::Map::new());
            }
            let providers = providers_item
                .as_table_mut()
                .ok_or_else(|| AcpError::protocol("invalid model_providers table"))?;
            let provider_item = providers
                .entry(provider_name.clone())
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            if !provider_item.is_table() {
                *provider_item = toml::Value::Table(toml::map::Map::new());
            }
            let provider_table = provider_item
                .as_table_mut()
                .ok_or_else(|| AcpError::protocol("invalid model provider table"))?;
            if api_url.trim().is_empty() {
                provider_table.remove("base_url");
            } else {
                provider_table.insert(
                    "base_url".to_string(),
                    toml::Value::String(api_url.to_string()),
                );
            }
            if provider_name == "codeg" {
                provider_table.insert("name".to_string(), toml::Value::String("codeg".to_string()));
                provider_table.insert(
                    "wire_api".to_string(),
                    toml::Value::String("responses".to_string()),
                );
                provider_table.insert(
                    "requires_openai_auth".to_string(),
                    toml::Value::Boolean(true),
                );
            }
            match codex_model {
                CodexModelAction::Set(model) => {
                    table.insert("model".to_string(), toml::Value::String(model.to_string()));
                }
                CodexModelAction::Clear => {
                    table.remove("model");
                }
                CodexModelAction::NoOp => {}
            }
            let toml_str = toml::to_string_pretty(&toml_value)
                .map_err(|e| AcpError::protocol(e.to_string()))?;

            persist_codex_native_config_files(Some(&auth_str), Some(&toml_str))?;
        }
        AgentType::OpenCode => {
            let auth_path = opencode_auth_json_path();
            let mut auth_obj = if auth_path.exists() {
                fs::read_to_string(&auth_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                    .filter(|v| v.is_object())
                    .unwrap_or_else(|| serde_json::json!({}))
            } else {
                serde_json::json!({})
            };
            if !api_key.trim().is_empty() {
                auth_obj["api_key"] = serde_json::Value::String(api_key.to_string());
            }
            let auth_str = serde_json::to_string_pretty(&auth_obj)
                .map_err(|e| AcpError::protocol(e.to_string()))?;
            persist_opencode_auth_json(&auth_str)?;

            let patch = serde_json::json!({ "apiBaseUrl": api_url });
            let patch_str =
                serde_json::to_string(&patch).map_err(|e| AcpError::protocol(e.to_string()))?;
            persist_agent_local_config_json(agent_type, Some(&patch_str))?;
        }
        AgentType::Cline => {}
    }
    Ok(())
}

/// Cascade model provider changes (credentials + model) to all dependent agent settings
/// and config files.
pub(crate) async fn cascade_update_model_provider(
    db: &AppDatabase,
    provider_id: i32,
    new_api_url: &str,
    new_api_key: &str,
    new_model: Option<&str>,
    emitter: &EventEmitter,
) -> Result<(), AcpError> {
    let dependents = agent_setting_service::find_by_model_provider_id(&db.conn, provider_id)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    for setting in &dependents {
        let agent_type: AgentType = match serde_json::from_str(&setting.agent_type) {
            Ok(at) => at,
            Err(_) => continue,
        };

        // 1. Update env_json in database (uses agent_env_keys for consistent key names)
        let (url_key, key_key, _) = agent_env_keys(agent_type);
        let mut env_map: BTreeMap<String, String> = setting
            .env_json
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();

        if !new_api_url.trim().is_empty() {
            env_map.insert(url_key.to_string(), new_api_url.to_string());
        }
        if !new_api_key.trim().is_empty() {
            env_map.insert(key_key.to_string(), new_api_key.to_string());
        }

        let model_env = parse_provider_model(agent_type, new_model);
        for (k, v) in &model_env {
            match v {
                Some(value) => {
                    env_map.insert(k.clone(), value.clone());
                }
                None => {
                    env_map.remove(k);
                }
            }
        }

        let patch = agent_setting_service::AgentSettingsUpdate {
            enabled: setting.enabled,
            env_json: serialize_env_map(&env_map)?,
            model_provider_id: setting.model_provider_id,
        };
        agent_setting_service::update(&db.conn, agent_type, patch)
            .await
            .map_err(|e| AcpError::protocol(e.to_string()))?;

        // 2. Update on-disk config files
        let codex_action = provider_codex_model_action(agent_type, new_model);
        if let Err(e) = cascade_update_agent_config(
            agent_type,
            new_api_url,
            new_api_key,
            &model_env,
            &codex_action,
        ) {
            eprintln!(
                "[ModelProvider] cascade_update_agent_config({agent_type}) failed: {e}, skipping config update"
            );
        }

        emit_acp_agents_updated(emitter, "env_updated", Some(agent_type));
    }
    Ok(())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_preflight(
    agent_type: AgentType,
    force_refresh: Option<bool>,
) -> Result<PreflightResult, AcpError> {
    if force_refresh.unwrap_or(false) {
        preflight::clear_npm_env_cache();
    }
    Ok(preflight::run_preflight(agent_type).await)
}

/// Resolve the full runtime env every ACP spawn should receive — settings
/// override, model provider credentials, git credential helper, OpenClaw
/// reset flag. Returns `AcpError::protocol("...disabled in settings")` when
/// the user has disabled the agent.
///
/// This is the **single source of truth** for "what env does an agent
/// process see". Three call sites depend on it:
///
///   1. `acp_connect` — the user-initiated session entry point.
///   2. `ConnectionManagerSpawner::spawn` — used by the delegation broker
///      to spawn subagents. Before this helper existed, delegation passed
///      `BTreeMap::new()`, silently bypassing settings/credentials and
///      letting disabled agents still be invoked through delegation.
///   3. `probe_agent_options` — the live settings-page probe. Must match
///      delegation's env exactly so what the user sees in the panel is
///      what `delegate_to_agent` will actually receive.
///
/// Diverging any of these from the others reintroduces the
/// "[UI shows options] != [delegation gets options]" inconsistency that
/// the multi-agent settings panel was designed to prevent.
pub(crate) async fn build_session_runtime_env(
    db: &AppDatabase,
    agent_type: AgentType,
    session_id: Option<&str>,
    data_dir: &Path,
) -> Result<BTreeMap<String, String>, AcpError> {
    let setting = agent_setting_service::get_by_agent_type(&db.conn, agent_type)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    let disabled = setting
        .as_ref()
        .map(|model| !model.enabled)
        .unwrap_or(false);
    if disabled {
        return Err(AcpError::protocol(format!(
            "{agent_type} is disabled in settings"
        )));
    }

    let local_config_json = load_agent_local_config_json(agent_type);
    let mut runtime_env =
        build_runtime_env_from_setting(agent_type, setting.as_ref(), local_config_json.as_deref());
    apply_model_provider_env(agent_type, setting.as_ref(), &mut runtime_env, &db.conn).await;

    if let Some(cred_env) = crate::commands::terminal::prepare_credential_env(data_dir) {
        for (key, value) in cred_env {
            runtime_env.insert(key, value);
        }
    }

    if agent_type == AgentType::OpenClaw && session_id.is_none() {
        runtime_env.insert("OPENCLAW_RESET_SESSION".into(), "1".into());
    }

    Ok(runtime_env)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
#[allow(clippy::too_many_arguments)]
pub async fn acp_connect(
    agent_type: AgentType,
    working_dir: Option<String>,
    session_id: Option<String>,
    preferred_mode_id: Option<String>,
    preferred_config_values: Option<BTreeMap<String, String>>,
    manager: State<'_, ConnectionManager>,
    db: State<'_, AppDatabase>,
    app_handle: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<String, AcpError> {
    // Resolve through the effective data dir so a custom `CODEG_DATA_DIR`
    // reaches the credential helper script the agent's git subprocess
    // will execute. `acp_connect` may be called before the app data dir
    // exists on disk (first launch); fall back to a sentinel that the
    // credential helper treats as "no credentials configured".
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map(|p| crate::paths::resolve_effective_data_dir(&p))
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let runtime_env =
        build_session_runtime_env(&db, agent_type, session_id.as_deref(), &app_data_dir).await?;

    // Guard: the session page must never trigger a download or install.
    // If the agent isn't ready, return SdkNotInstalled here so the frontend
    // can prompt the user to install it from Agent Settings.
    verify_agent_installed(agent_type).await?;

    let emitter = EventEmitter::Tauri(app_handle);
    manager
        .spawn_agent(
            agent_type,
            working_dir,
            session_id,
            runtime_env,
            window.label().to_string(),
            emitter,
            preferred_mode_id,
            preferred_config_values.unwrap_or_default(),
        )
        .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_prompt(
    connection_id: String,
    blocks: Vec<PromptInputBlock>,
    folder_id: Option<i32>,
    conversation_id: Option<i32>,
    db: State<'_, crate::db::AppDatabase>,
    manager: State<'_, ConnectionManager>,
) -> Result<(), AcpError> {
    manager
        .send_prompt_linked(
            &db,
            &connection_id,
            blocks,
            folder_id,
            conversation_id,
            None,
        )
        .await
        .map(|_| ())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_set_mode(
    connection_id: String,
    mode_id: String,
    manager: State<'_, ConnectionManager>,
) -> Result<(), AcpError> {
    manager.set_mode(&connection_id, mode_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_set_config_option(
    connection_id: String,
    config_id: String,
    value_id: String,
    manager: State<'_, ConnectionManager>,
) -> Result<(), AcpError> {
    manager
        .set_config_option(&connection_id, config_id, value_id)
        .await
}

/// Spawn a transient ACP connection for `agent_type` with a silent emitter,
/// read whatever `SessionConfigOptions` / `SessionModes` the agent advertises,
/// and tear it down. The returned snapshot drives the delegation-settings UI
/// so the user picks from the exact option set the agent will accept when
/// codeg-mcp later spawns a subagent.
///
/// Does NOT touch the chat-side `selectorsCache`, `localStorage` preferences,
/// or any active connection state — see `ConnectionManager::probe_agent_options`
/// for the isolation guarantees.
pub async fn acp_describe_agent_options_core(
    manager: &ConnectionManager,
    db: &AppDatabase,
    data_dir: &Path,
    agent_type: AgentType,
    working_dir: Option<String>,
) -> Result<crate::acp::types::AgentOptionsSnapshot, AcpError> {
    verify_agent_installed(agent_type).await?;
    // Build the same runtime env delegation/acp_connect would build so
    // probe sees exactly what `delegate_to_agent` will see at runtime.
    // Without this, the settings UI could show options that the agent
    // never advertises in production (settings override an API URL,
    // model_provider injects a different model list, etc.).
    let runtime_env = build_session_runtime_env(db, agent_type, None, data_dir).await?;
    manager
        .probe_agent_options(agent_type, working_dir, runtime_env)
        .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_describe_agent_options(
    agent_type: AgentType,
    working_dir: Option<String>,
    manager: State<'_, ConnectionManager>,
    db: State<'_, AppDatabase>,
    app_handle: tauri::AppHandle,
) -> Result<crate::acp::types::AgentOptionsSnapshot, AcpError> {
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map(|p| crate::paths::resolve_effective_data_dir(&p))
        .unwrap_or_else(|_| PathBuf::from("."));
    acp_describe_agent_options_core(&manager, &db, &app_data_dir, agent_type, working_dir).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_cancel(
    connection_id: String,
    db: State<'_, AppDatabase>,
    manager: State<'_, ConnectionManager>,
) -> Result<(), AcpError> {
    manager.cancel(&db.conn, &connection_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_fork(
    connection_id: String,
    db: State<'_, AppDatabase>,
    manager: State<'_, ConnectionManager>,
) -> Result<ForkResultInfo, AcpError> {
    manager.fork_session(&db, &connection_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_respond_permission(
    connection_id: String,
    request_id: String,
    option_id: String,
    manager: State<'_, ConnectionManager>,
) -> Result<(), AcpError> {
    manager
        .respond_permission(&connection_id, &request_id, &option_id)
        .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_disconnect(
    connection_id: String,
    manager: State<'_, ConnectionManager>,
) -> Result<(), AcpError> {
    manager.disconnect(&connection_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_touch_connection(
    connection_id: String,
    manager: State<'_, ConnectionManager>,
) -> Result<bool, AcpError> {
    Ok(manager.touch(&connection_id).await)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_list_connections(
    manager: State<'_, ConnectionManager>,
) -> Result<Vec<ConnectionInfo>, AcpError> {
    Ok(manager.list_connections().await)
}

pub(crate) async fn acp_get_session_snapshot_core(
    manager: &ConnectionManager,
    connection_id: &str,
) -> Result<Option<crate::acp::LiveSessionSnapshot>, AcpError> {
    let Some(state) = manager.get_state(connection_id).await else {
        return Ok(None);
    };
    let snap = state.read().await.to_snapshot();
    Ok(Some(snap))
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_get_session_snapshot(
    connection_id: String,
    manager: State<'_, ConnectionManager>,
) -> Result<Option<crate::acp::LiveSessionSnapshot>, AcpError> {
    acp_get_session_snapshot_core(&manager, &connection_id).await
}

pub(crate) async fn acp_get_session_snapshot_by_conversation_core(
    manager: &ConnectionManager,
    conversation_id: i32,
) -> Result<Option<crate::acp::LiveSessionSnapshot>, AcpError> {
    let Some(conn_id) = manager
        .find_connection_by_conversation_id(conversation_id)
        .await
    else {
        return Ok(None);
    };
    acp_get_session_snapshot_core(manager, &conn_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_get_session_snapshot_by_conversation(
    conversation_id: i32,
    manager: State<'_, ConnectionManager>,
) -> Result<Option<crate::acp::LiveSessionSnapshot>, AcpError> {
    acp_get_session_snapshot_by_conversation_core(&manager, conversation_id).await
}

pub(crate) async fn acp_get_agent_status_core(
    agent_type: AgentType,
    db: &AppDatabase,
) -> Result<crate::acp::types::AcpAgentStatus, AcpError> {
    let platform = registry::current_platform();
    let meta = registry::get_agent_meta(agent_type);
    let setting = agent_setting_service::get_by_agent_type(&db.conn, agent_type)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    let (available, installed_version) = match &meta.distribution {
        registry::AgentDistribution::Npx { cmd, .. } => (
            true,
            resolve_npx_command(cmd)
                .await
                .and_then(|_| setting.as_ref().and_then(|m| m.installed_version.clone())),
        ),
        registry::AgentDistribution::Binary { platforms, cmd, .. } => {
            let detected = binary_cache::detect_installed_version(agent_type, cmd)
                .ok()
                .flatten();
            (platforms.iter().any(|p| p.platform == platform), detected)
        }
    };

    Ok(crate::acp::types::AcpAgentStatus {
        agent_type,
        available,
        enabled: setting.map(|m| m.enabled).unwrap_or(true),
        installed_version,
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_get_agent_status(
    agent_type: AgentType,
    db: tauri::State<'_, AppDatabase>,
) -> Result<crate::acp::types::AcpAgentStatus, AcpError> {
    acp_get_agent_status_core(agent_type, &db).await
}

pub(crate) async fn acp_list_agents_core(db: &AppDatabase) -> Result<Vec<AcpAgentInfo>, AcpError> {
    let platform = registry::current_platform();
    let agent_types = registry::all_acp_agents();

    let defaults = agent_types
        .iter()
        .enumerate()
        .map(
            |(idx, agent_type)| agent_setting_service::AgentDefaultInput {
                agent_type: *agent_type,
                registry_id: registry::registry_id_for(*agent_type).to_string(),
                default_sort_order: idx as i32,
            },
        )
        .collect::<Vec<_>>();

    agent_setting_service::ensure_defaults(&db.conn, &defaults)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    let settings_map = agent_setting_service::list_map_by_agent_type(&db.conn)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    let mut agents = Vec::new();
    let mut npx_resolver = NpxCommandResolver::default();
    for (idx, agent_type) in agent_types.into_iter().enumerate() {
        let setting = settings_map.get(&agent_type);
        let meta = registry::get_agent_meta(agent_type);
        let (available, dist_type, local_installed_version) = match &meta.distribution {
            registry::AgentDistribution::Npx { cmd, .. } => {
                // Keep the list path bounded: each list request probes npm
                // global prefix at most once, then reuses the result across
                // all NPX agents in the loop.
                let cached = npx_resolver
                    .resolve_for_list(cmd)
                    .await
                    .and_then(|_| setting.and_then(|m| m.installed_version.clone()));
                (true, "npx", cached)
            }
            registry::AgentDistribution::Binary { platforms, cmd, .. } => {
                let detected = binary_cache::detect_installed_version(agent_type, cmd)
                    .ok()
                    .flatten();
                (
                    platforms.iter().any(|p| p.platform == platform),
                    "binary",
                    detected,
                )
            }
        };

        let mut env = setting
            .and_then(|m| m.env_json.as_deref())
            .and_then(|s| serde_json::from_str::<BTreeMap<String, String>>(s).ok())
            .unwrap_or_default();
        let local_config_json = load_agent_local_config_json(agent_type);
        if let Some(raw_local_config) = local_config_json.as_deref() {
            if let Ok(local_cfg) = serde_json::from_str::<AgentRuntimeConfig>(raw_local_config) {
                for (key, value) in local_cfg.env {
                    let trimmed = value.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    env.insert(key, trimmed.to_string());
                }
                let (api_base_url_key, api_key_key, model_key) = agent_env_keys(agent_type);
                if let Some(value) = trim_non_empty(local_cfg.api_base_url) {
                    env.insert(api_base_url_key.to_string(), value);
                }
                if let Some(value) = trim_non_empty(local_cfg.api_key) {
                    env.insert(api_key_key.to_string(), value);
                }
                if agent_type != AgentType::ClaudeCode {
                    if let Some(value) = trim_non_empty(local_cfg.model) {
                        env.insert(model_key.to_string(), value);
                    }
                }
            }
        }
        let sort_order = setting.map(|m| m.sort_order).unwrap_or(idx as i32);
        // Persist detected version to DB for binary agents (npx written during install/upgrade)
        if dist_type == "binary" {
            let _ = agent_setting_service::set_installed_version(
                &db.conn,
                agent_type,
                local_installed_version.clone(),
            )
            .await;
        }
        let codex_auth_json = if agent_type == AgentType::Codex {
            load_codex_auth_json_raw()
        } else {
            None
        };
        let opencode_auth_json = if agent_type == AgentType::OpenCode {
            load_opencode_auth_json_raw()
        } else {
            None
        };
        let codex_config_toml = if agent_type == AgentType::Codex {
            load_codex_config_toml_raw()
        } else {
            None
        };
        let cline_secrets_json = if agent_type == AgentType::Cline {
            load_cline_secrets_json_raw()
        } else {
            None
        };

        agents.push(AcpAgentInfo {
            agent_type,
            registry_id: registry::registry_id_for(agent_type).to_string(),
            registry_version: meta.registry_version().map(ToString::to_string),
            name: meta.name.to_string(),
            description: meta.description.to_string(),
            available,
            distribution_type: dist_type.to_string(),
            enabled: setting.map(|m| m.enabled).unwrap_or(true),
            sort_order,
            installed_version: local_installed_version,
            env,
            config_json: local_config_json,
            config_file_path: agent_local_config_path(agent_type)
                .map(|path| path.display().to_string()),
            opencode_auth_json,
            codex_auth_json,
            codex_config_toml,
            cline_secrets_json,
            model_provider_id: setting.and_then(|m| m.model_provider_id),
        });
    }

    agents.sort_by(|a, b| {
        a.sort_order
            .cmp(&b.sort_order)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(agents)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_list_agents(
    db: tauri::State<'_, AppDatabase>,
) -> Result<Vec<AcpAgentInfo>, AcpError> {
    acp_list_agents_core(&db).await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_clear_binary_cache(agent_type: AgentType) -> Result<(), AcpError> {
    let meta = registry::get_agent_meta(agent_type);
    if matches!(
        meta.distribution,
        registry::AgentDistribution::Binary { .. }
    ) {
        binary_cache::clear_agent_cache(agent_type)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn acp_update_agent_preferences_core(
    agent_type: AgentType,
    enabled: bool,
    env: BTreeMap<String, String>,
    config_json: Option<String>,
    opencode_auth_json: Option<String>,
    codex_auth_json: Option<String>,
    codex_config_toml: Option<String>,
    db: &AppDatabase,
    emitter: &EventEmitter,
) -> Result<(), AcpError> {
    let default = agent_setting_service::AgentDefaultInput {
        agent_type,
        registry_id: registry::registry_id_for(agent_type).to_string(),
        default_sort_order: i32::MAX / 2,
    };

    agent_setting_service::ensure_defaults(&db.conn, &[default])
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    let env_json = serialize_env_map(&env)?;
    let config_json = config_json.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    let opencode_auth_json = opencode_auth_json.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    if let Some(raw) = config_json.as_deref() {
        let parsed = serde_json::from_str::<serde_json::Value>(raw)
            .map_err(|e| AcpError::protocol(format!("invalid config_json: {e}")))?;
        if !parsed.is_object() {
            return Err(AcpError::protocol(
                "invalid config_json: root must be a JSON object",
            ));
        }
    }

    let patch = agent_setting_service::AgentSettingsUpdate {
        enabled,
        env_json,
        model_provider_id: None,
    };
    agent_setting_service::update(&db.conn, agent_type, patch)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    if agent_type == AgentType::Codex {
        if codex_auth_json.is_some() || codex_config_toml.is_some() {
            persist_codex_native_config_files(
                codex_auth_json.as_deref(),
                codex_config_toml.as_deref(),
            )?;
        }
        emit_acp_agents_updated(emitter, "preferences_updated", Some(agent_type));
        return Ok(());
    }

    if agent_type == AgentType::OpenCode {
        if let Some(raw_auth) = opencode_auth_json.as_deref() {
            persist_opencode_auth_json(raw_auth)?;
        }
        if let Some(raw) = config_json.as_deref() {
            persist_agent_local_config_json(agent_type, Some(raw))?;
        }
        emit_acp_agents_updated(emitter, "preferences_updated", Some(agent_type));
        return Ok(());
    }

    if agent_type == AgentType::Cline {
        if let Some(raw) = config_json.as_deref() {
            persist_cline_local_config(Some(raw))?;
        }
        emit_acp_agents_updated(emitter, "preferences_updated", Some(agent_type));
        return Ok(());
    }

    let mut local_patch_value = config_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if !env.is_empty() {
        let env_json_value =
            serde_json::to_value(&env).map_err(|e| AcpError::protocol(e.to_string()))?;
        if let Some(obj) = local_patch_value.as_object_mut() {
            obj.insert("env".to_string(), env_json_value);
        }
    }
    let local_patch_json = serde_json::to_string(&local_patch_value)
        .map_err(|e| AcpError::protocol(format!("serialize local patch failed: {e}")))?;
    persist_agent_local_config_json(agent_type, Some(local_patch_json.as_str()))?;
    emit_acp_agents_updated(emitter, "preferences_updated", Some(agent_type));
    Ok(())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
#[allow(clippy::too_many_arguments)]
pub async fn acp_update_agent_preferences(
    agent_type: AgentType,
    enabled: bool,
    env: BTreeMap<String, String>,
    config_json: Option<String>,
    opencode_auth_json: Option<String>,
    codex_auth_json: Option<String>,
    codex_config_toml: Option<String>,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<(), AcpError> {
    let emitter = EventEmitter::Tauri(app);
    acp_update_agent_preferences_core(
        agent_type,
        enabled,
        env,
        config_json,
        opencode_auth_json,
        codex_auth_json,
        codex_config_toml,
        &db,
        &emitter,
    )
    .await
}

pub(crate) async fn acp_update_agent_env_core(
    agent_type: AgentType,
    enabled: bool,
    env: BTreeMap<String, String>,
    model_provider_id: Option<i32>,
    db: &AppDatabase,
    emitter: &EventEmitter,
) -> Result<(), AcpError> {
    let default = agent_setting_service::AgentDefaultInput {
        agent_type,
        registry_id: registry::registry_id_for(agent_type).to_string(),
        default_sort_order: i32::MAX / 2,
    };

    agent_setting_service::ensure_defaults(&db.conn, &[default])
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    // If a provider is selected, the provider's model field is authoritative:
    // each relevant env key is set when the provider has a value and cleared
    // (removed) when empty. Codex's root `model` in config.toml is handled the
    // same way.
    let mut merged_env = env;
    let mut codex_action = CodexModelAction::NoOp;
    if let Some(pid) = model_provider_id {
        let provider = crate::db::service::model_provider_service::get_by_id(&db.conn, pid)
            .await
            .map_err(|e| AcpError::protocol(e.to_string()))?
            .ok_or_else(|| AcpError::protocol(format!("model provider not found: {pid}")))?;

        // Reject cross-type binding: provider.model is formatted for its declared
        // agent_type (Claude = JSON, Codex/Gemini/others = plain string). Binding
        // a mismatched provider would parse the model under the wrong format and
        // silently write invalid env / config.toml entries.
        let provider_agent_type: AgentType =
            serde_json::from_value(serde_json::Value::String(provider.agent_type.clone()))
                .map_err(|_| {
                    AcpError::protocol(format!(
                        "model provider {pid} has invalid agent_type: {}",
                        provider.agent_type
                    ))
                })?;
        if provider_agent_type != agent_type {
            return Err(AcpError::protocol(format!(
                "model provider {pid} is for {provider_agent_type}, cannot be bound to {agent_type}"
            )));
        }

        let model_env = parse_provider_model(agent_type, provider.model.as_deref());
        for (k, v) in model_env {
            match v {
                Some(value) => {
                    merged_env.insert(k, value);
                }
                None => {
                    merged_env.remove(&k);
                }
            }
        }
        codex_action = provider_codex_model_action(agent_type, provider.model.as_deref());
    }

    let patch = agent_setting_service::AgentSettingsUpdate {
        enabled,
        env_json: serialize_env_map(&merged_env)?,
        model_provider_id,
    };
    agent_setting_service::update(&db.conn, agent_type, patch)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    if let Err(e) = apply_codex_root_model_action(&codex_action) {
        eprintln!("[acp_update_agent_env] apply_codex_root_model_action failed: {e}");
    }

    emit_acp_agents_updated(emitter, "env_updated", Some(agent_type));
    Ok(())
}

/// Apply a `CodexModelAction` to the `model` field at the root of
/// `~/.codex/config.toml`, preserving everything else.
fn apply_codex_root_model_action(action: &CodexModelAction) -> Result<(), AcpError> {
    if matches!(action, CodexModelAction::NoOp) {
        return Ok(());
    }
    let config_path = codex_config_toml_path();
    let mut toml_value = if config_path.exists() {
        fs::read_to_string(&config_path)
            .ok()
            .and_then(|raw| raw.parse::<toml::Value>().ok())
            .filter(|v| v.is_table())
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()))
    } else {
        toml::Value::Table(toml::map::Map::new())
    };
    let table = toml_value
        .as_table_mut()
        .ok_or_else(|| AcpError::protocol("codex config root must be a TOML table"))?;
    match action {
        CodexModelAction::Set(model) => {
            table.insert("model".to_string(), toml::Value::String(model.clone()));
        }
        CodexModelAction::Clear => {
            table.remove("model");
        }
        CodexModelAction::NoOp => unreachable!(),
    }
    let toml_str =
        toml::to_string_pretty(&toml_value).map_err(|e| AcpError::protocol(e.to_string()))?;
    persist_codex_native_config_files(None, Some(&toml_str))?;
    Ok(())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_update_agent_env(
    agent_type: AgentType,
    enabled: bool,
    env: BTreeMap<String, String>,
    model_provider_id: Option<i32>,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<(), AcpError> {
    let emitter = EventEmitter::Tauri(app);
    acp_update_agent_env_core(agent_type, enabled, env, model_provider_id, &db, &emitter).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn acp_update_agent_config_core(
    agent_type: AgentType,
    config_json: Option<String>,
    opencode_auth_json: Option<String>,
    codex_auth_json: Option<String>,
    codex_config_toml: Option<String>,
    emitter: &EventEmitter,
) -> Result<(), AcpError> {
    let config_json = config_json.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    let opencode_auth_json = opencode_auth_json.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    if let Some(raw) = config_json.as_deref() {
        let parsed = serde_json::from_str::<serde_json::Value>(raw)
            .map_err(|e| AcpError::protocol(format!("invalid config_json: {e}")))?;
        if !parsed.is_object() {
            return Err(AcpError::protocol(
                "invalid config_json: root must be a JSON object",
            ));
        }
    }

    if agent_type == AgentType::Codex {
        if codex_auth_json.is_some() || codex_config_toml.is_some() {
            persist_codex_native_config_files(
                codex_auth_json.as_deref(),
                codex_config_toml.as_deref(),
            )?;
        }
        emit_acp_agents_updated(emitter, "config_updated", Some(agent_type));
        return Ok(());
    }

    if agent_type == AgentType::OpenCode {
        if let Some(raw_auth) = opencode_auth_json.as_deref() {
            persist_opencode_auth_json(raw_auth)?;
        }
        if let Some(raw) = config_json.as_deref() {
            persist_agent_local_config_json(agent_type, Some(raw))?;
        }
        emit_acp_agents_updated(emitter, "config_updated", Some(agent_type));
        return Ok(());
    }

    if agent_type == AgentType::Cline {
        if let Some(raw) = config_json.as_deref() {
            persist_cline_local_config(Some(raw))?;
        }
        emit_acp_agents_updated(emitter, "config_updated", Some(agent_type));
        return Ok(());
    }

    // Claude Code, Gemini, OpenClaw — write config JSON to local file without merging env
    let local_patch_value = config_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    let local_patch_json = serde_json::to_string(&local_patch_value)
        .map_err(|e| AcpError::protocol(format!("serialize local patch failed: {e}")))?;
    persist_agent_local_config_json(agent_type, Some(local_patch_json.as_str()))?;
    emit_acp_agents_updated(emitter, "config_updated", Some(agent_type));
    Ok(())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_update_agent_config(
    agent_type: AgentType,
    config_json: Option<String>,
    opencode_auth_json: Option<String>,
    codex_auth_json: Option<String>,
    codex_config_toml: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), AcpError> {
    let emitter = EventEmitter::Tauri(app);
    acp_update_agent_config_core(
        agent_type,
        config_json,
        opencode_auth_json,
        codex_auth_json,
        codex_config_toml,
        &emitter,
    )
    .await
}

pub(crate) async fn acp_download_agent_binary_core(
    agent_type: AgentType,
    version_override: Option<String>,
    task_id: String,
    emitter: &EventEmitter,
) -> Result<(), AcpError> {
    emit_agent_install_event(emitter, &task_id, AgentInstallEventKind::Started, "");

    let meta = registry::get_agent_meta(agent_type);
    let result = match meta.distribution {
        registry::AgentDistribution::Binary {
            version,
            cmd,
            platforms,
            ..
        } => {
            // A custom version substitutes into the pinned download URL and the
            // cache key; `None`/empty keeps the registry-pinned version.
            let custom = match version_override.as_deref() {
                Some(raw) if !raw.trim().is_empty() => Some(sanitize_custom_version(raw)
                    .ok_or_else(|| {
                        AcpError::protocol(format!("invalid custom version: {}", raw.trim()))
                    })?),
                _ => None,
            };

            let platform = registry::current_platform();
            let fallback = platforms
                .iter()
                .find(|p| p.platform == platform)
                .ok_or_else(|| {
                    AcpError::PlatformNotSupported(format!(
                        "{} is not available on {platform}",
                        meta.name
                    ))
                })?;

            let effective_version = custom.as_deref().unwrap_or(version);
            let archive_url = match &custom {
                Some(c) => apply_custom_version_to_url(fallback.url, version, c),
                None => fallback.url.to_string(),
            };

            emit_agent_install_event(
                emitter,
                &task_id,
                AgentInstallEventKind::Log,
                format!("Downloading {} v{effective_version} for {platform}", meta.name),
            );

            let emitter_clone = emitter.clone();
            let task_id_clone = task_id.clone();
            let _ = binary_cache::ensure_binary_for_agent_with_progress(
                agent_type,
                effective_version,
                &archive_url,
                cmd,
                move |msg| {
                    emit_agent_install_event(
                        &emitter_clone,
                        &task_id_clone,
                        AgentInstallEventKind::Log,
                        msg,
                    );
                },
            )
            .await?;
            emit_acp_agents_updated(emitter, "binary_downloaded", Some(agent_type));
            Ok(())
        }
        registry::AgentDistribution::Npx { .. } => Err(AcpError::protocol(
            "download is only supported for binary agents",
        )),
    };

    match &result {
        Ok(()) => {
            emit_agent_install_event(
                emitter,
                &task_id,
                AgentInstallEventKind::Completed,
                format!("{} installed successfully", meta.name),
            );
        }
        Err(e) => {
            emit_agent_install_event(
                emitter,
                &task_id,
                AgentInstallEventKind::Failed,
                e.to_string(),
            );
        }
    }
    result
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_download_agent_binary(
    agent_type: AgentType,
    version: Option<String>,
    task_id: String,
    app: tauri::AppHandle,
) -> Result<(), AcpError> {
    let emitter = EventEmitter::Tauri(app);
    acp_download_agent_binary_core(agent_type, version, task_id, &emitter).await
}

pub(crate) async fn acp_detect_agent_local_version_core(
    agent_type: AgentType,
    conn: &sea_orm::DatabaseConnection,
) -> Result<Option<String>, AcpError> {
    let detected = detect_local_version(agent_type).await;
    if let Some(version) = detected.clone() {
        let _ =
            agent_setting_service::set_installed_version(conn, agent_type, Some(version.clone()))
                .await;
        return Ok(Some(version));
    }

    // Binary agents detect their version purely from the on-disk cache, so a
    // `None` here means the binary is genuinely absent (cleared cache, or a
    // failed custom/upgrade install). Return `None` authoritatively rather than
    // falling back to the DB, which would resurrect a removed version as a
    // phantom that can no longer be launched. The returned value does NOT depend
    // on the mirror write below, so a swallowed write cannot reintroduce the
    // phantom. (NPX detection runs `npm list`, which can fail transiently, so
    // for npx we keep the DB value as a best-effort fallback.)
    if matches!(
        registry::get_agent_meta(agent_type).distribution,
        registry::AgentDistribution::Binary { .. }
    ) {
        let _ = agent_setting_service::set_installed_version(conn, agent_type, None).await;
        return Ok(None);
    }

    let fallback = agent_setting_service::get_by_agent_type(conn, agent_type)
        .await
        .ok()
        .flatten()
        .and_then(|m| m.installed_version);
    Ok(fallback)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_detect_agent_local_version(
    agent_type: AgentType,
    db: State<'_, AppDatabase>,
) -> Result<Option<String>, AcpError> {
    acp_detect_agent_local_version_core(agent_type, &db.conn).await
}

pub(crate) async fn acp_prepare_npx_agent_core(
    agent_type: AgentType,
    registry_version: Option<String>,
    version_override: Option<String>,
    clean_first: bool,
    task_id: String,
    db: &AppDatabase,
    emitter: &EventEmitter,
) -> Result<String, AcpError> {
    emit_agent_install_event(emitter, &task_id, AgentInstallEventKind::Started, "");

    let meta = registry::get_agent_meta(agent_type);
    let result = match meta.distribution {
        registry::AgentDistribution::Npx { package, .. } => {
            // `version_override` of None/empty keeps the registry-pinned spec;
            // a custom version installs `<name>@<version>` instead.
            let install_spec = build_npm_install_spec(package, version_override.as_deref())?;

            let default = agent_setting_service::AgentDefaultInput {
                agent_type,
                registry_id: registry::registry_id_for(agent_type).to_string(),
                default_sort_order: i32::MAX / 2,
            };
            agent_setting_service::ensure_defaults(&db.conn, &[default])
                .await
                .map_err(|e| AcpError::protocol(e.to_string()))?;

            let existing = agent_setting_service::get_by_agent_type(&db.conn, agent_type)
                .await
                .ok()
                .flatten()
                .and_then(|m| m.installed_version);

            // Best-effort uninstall before reinstall. Forces npm to re-resolve
            // the dependency graph from scratch, which is required for
            // platform-specific optionalDependencies (e.g. native CLI binaries
            // shipped as `<pkg>-darwin-x64`) to be picked up after an upgrade.
            // Failures here are logged and swallowed so we still attempt the
            // install — for example when nothing is currently installed.
            if clean_first {
                let package_name = package_name_from_spec(package);
                emit_agent_install_event(
                    emitter,
                    &task_id,
                    AgentInstallEventKind::Log,
                    format!("$ npm uninstall -g {package_name} (clean reinstall)"),
                );
                if let Err(e) = uninstall_npm_global_package(package).await {
                    emit_agent_install_event(
                        emitter,
                        &task_id,
                        AgentInstallEventKind::Log,
                        format!("(warning) uninstall step failed, continuing: {e}"),
                    );
                }
            }

            emit_agent_install_event(
                emitter,
                &task_id,
                AgentInstallEventKind::Log,
                format!("Installing {} ({install_spec})", meta.name),
            );
            install_npm_global_package_streaming(&install_spec, &task_id, emitter).await?;

            emit_agent_install_event(
                emitter,
                &task_id,
                AgentInstallEventKind::Log,
                "Detecting installed version...",
            );
            let resolved = detect_local_version(agent_type)
                .await
                .or_else(|| version_from_package_spec(&install_spec))
                .or_else(|| {
                    registry_version
                        .as_deref()
                        .and_then(normalize_version_candidate)
                })
                .or(existing)
                .ok_or_else(|| {
                    AcpError::protocol(
                        "npm global install succeeded but failed to determine local version",
                    )
                })?;

            agent_setting_service::set_installed_version(
                &db.conn,
                agent_type,
                Some(resolved.clone()),
            )
            .await
            .map_err(|e| AcpError::protocol(e.to_string()))?;
            emit_acp_agents_updated(emitter, "npx_prepared", Some(agent_type));
            Ok(resolved)
        }
        registry::AgentDistribution::Binary { .. } => Err(AcpError::protocol(
            "prepare is only supported for npx agents",
        )),
    };

    match &result {
        Ok(version) => {
            emit_agent_install_event(
                emitter,
                &task_id,
                AgentInstallEventKind::Completed,
                format!("{} v{version} installed successfully", meta.name),
            );
        }
        Err(e) => {
            // When clean_first was true the uninstall step may already have
            // succeeded by the time install failed, leaving the DB pointing at
            // a version that no longer exists on disk. Resync the DB to the
            // actual filesystem state so the UI doesn't mislead the user into
            // thinking they can connect.
            if clean_first {
                let detected = detect_local_version(agent_type).await;
                if let Err(sync_err) =
                    agent_setting_service::set_installed_version(&db.conn, agent_type, detected)
                        .await
                {
                    eprintln!(
                        "[acp] failed to resync installed_version after clean upgrade failure: {sync_err}"
                    );
                }
                emit_acp_agents_updated(emitter, "npx_prepare_failed", Some(agent_type));
            }
            emit_agent_install_event(
                emitter,
                &task_id,
                AgentInstallEventKind::Failed,
                e.to_string(),
            );
        }
    }
    result
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_prepare_npx_agent(
    agent_type: AgentType,
    registry_version: Option<String>,
    version: Option<String>,
    clean_first: Option<bool>,
    task_id: String,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<String, AcpError> {
    let emitter = EventEmitter::Tauri(app);
    acp_prepare_npx_agent_core(
        agent_type,
        registry_version,
        version,
        clean_first.unwrap_or(false),
        task_id,
        &db,
        &emitter,
    )
    .await
}

pub(crate) async fn acp_uninstall_agent_core(
    agent_type: AgentType,
    task_id: String,
    db: &AppDatabase,
    emitter: &EventEmitter,
) -> Result<(), AcpError> {
    emit_agent_install_event(emitter, &task_id, AgentInstallEventKind::Started, "");

    let meta = registry::get_agent_meta(agent_type);
    emit_agent_install_event(
        emitter,
        &task_id,
        AgentInstallEventKind::Log,
        format!("Uninstalling {}...", meta.name),
    );

    let result: Result<(), AcpError> = async {
        match meta.distribution {
            registry::AgentDistribution::Binary { .. } => {
                binary_cache::clear_agent_cache(agent_type)?;
            }
            registry::AgentDistribution::Npx { package, .. } => {
                uninstall_npm_global_package(package).await?;
            }
        }

        agent_setting_service::set_installed_version(&db.conn, agent_type, None)
            .await
            .map_err(|e| AcpError::protocol(e.to_string()))?;
        emit_acp_agents_updated(emitter, "agent_uninstalled", Some(agent_type));
        Ok(())
    }
    .await;

    match &result {
        Ok(()) => {
            emit_agent_install_event(
                emitter,
                &task_id,
                AgentInstallEventKind::Completed,
                format!("{} uninstalled successfully", meta.name),
            );
        }
        Err(e) => {
            emit_agent_install_event(
                emitter,
                &task_id,
                AgentInstallEventKind::Failed,
                e.to_string(),
            );
        }
    }
    result
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_uninstall_agent(
    agent_type: AgentType,
    task_id: String,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<(), AcpError> {
    let emitter = EventEmitter::Tauri(app);
    acp_uninstall_agent_core(agent_type, task_id, &db, &emitter).await
}

pub(crate) async fn acp_reorder_agents_core(
    agent_types: &[AgentType],
    db: &AppDatabase,
    emitter: &EventEmitter,
) -> Result<(), AcpError> {
    if agent_types.is_empty() {
        return Ok(());
    }
    agent_setting_service::reorder(&db.conn, agent_types)
        .await
        .map_err(|e| {
            let message = e.to_string();
            if message.contains("database or disk is full") || message.contains("(code: 13)") {
                AcpError::protocol("无法保存排序：数据库可写空间不足。请释放磁盘空间后重试。")
            } else {
                AcpError::protocol(message)
            }
        })?;
    emit_acp_agents_updated(emitter, "agent_reordered", None);
    Ok(())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_reorder_agents(
    agent_types: Vec<AgentType>,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<(), AcpError> {
    let emitter = EventEmitter::Tauri(app);
    acp_reorder_agents_core(&agent_types, &db, &emitter).await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_list_agent_skills(
    agent_type: AgentType,
    workspace_path: Option<String>,
) -> Result<AgentSkillsListResult, AcpError> {
    let Some(spec) = skill_storage_spec(agent_type) else {
        return Ok(AgentSkillsListResult {
            supported: false,
            message: Some(format!("{agent_type} 暂不支持在设置页管理 Skills")),
            locations: Vec::new(),
            skills: Vec::new(),
        });
    };

    let mut locations = Vec::new();
    let mut skills_by_key: BTreeMap<String, AgentSkillItem> = BTreeMap::new();

    for dir in &spec.global_dirs {
        locations.push(AgentSkillLocation {
            scope: AgentSkillScope::Global,
            path: dir.to_string_lossy().to_string(),
            exists: dir.exists(),
        });
        let listed = list_skills_from_dir(AgentSkillScope::Global, dir, spec.kind)?;
        for skill in listed {
            let key = format!("global:{}", skill.id);
            skills_by_key.entry(key).or_insert(skill);
        }
    }

    if let Some(workspace) = workspace_path.as_deref().map(str::trim) {
        if !workspace.is_empty() {
            for relative in &spec.project_rel_dirs {
                let project_dir = PathBuf::from(workspace).join(relative);
                locations.push(AgentSkillLocation {
                    scope: AgentSkillScope::Project,
                    path: project_dir.to_string_lossy().to_string(),
                    exists: project_dir.exists(),
                });
                let listed =
                    list_skills_from_dir(AgentSkillScope::Project, &project_dir, spec.kind)?;
                for skill in listed {
                    let key = format!("project:{}", skill.id);
                    skills_by_key.entry(key).or_insert(skill);
                }
            }
        }
    }

    let mut skills = skills_by_key.into_values().collect::<Vec<_>>();
    for skill in &mut skills {
        if is_read_only_skill_path(agent_type, Path::new(&skill.path)) {
            skill.read_only = true;
        }
    }
    skills.sort_by(|a, b| {
        scope_rank(a.scope)
            .cmp(&scope_rank(b.scope))
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(AgentSkillsListResult {
        supported: true,
        message: None,
        locations,
        skills,
    })
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_read_agent_skill(
    agent_type: AgentType,
    scope: AgentSkillScope,
    skill_id: String,
    workspace_path: Option<String>,
) -> Result<AgentSkillContent, AcpError> {
    let Some(spec) = skill_storage_spec(agent_type) else {
        return Err(AcpError::protocol(format!(
            "{agent_type} skills are not supported in Settings yet"
        )));
    };
    let id = validate_skill_id(&skill_id)?;
    let dirs = scoped_skill_dirs(agent_type, scope, workspace_path.as_deref())?;

    let mut skill = locate_existing_skill_across_dirs(&dirs, spec.kind, &id, scope)
        .ok_or_else(|| AcpError::protocol(format!("skill not found: {id}")))?;
    if is_read_only_skill_path(agent_type, Path::new(&skill.path)) {
        skill.read_only = true;
    }
    let content_path = skill_content_path(skill.layout, Path::new(&skill.path));
    let content = fs::read_to_string(&content_path)
        .map_err(|e| AcpError::protocol(format!("failed to read skill content: {e}")))?;
    Ok(AgentSkillContent { skill, content })
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_save_agent_skill(
    agent_type: AgentType,
    scope: AgentSkillScope,
    skill_id: String,
    content: String,
    workspace_path: Option<String>,
    layout: Option<AgentSkillLayout>,
) -> Result<AgentSkillItem, AcpError> {
    let Some(spec) = skill_storage_spec(agent_type) else {
        return Err(AcpError::protocol(format!(
            "{agent_type} skills are not supported in Settings yet"
        )));
    };
    let id = validate_skill_id(&skill_id)?;
    let dirs = scoped_skill_dirs(agent_type, scope, workspace_path.as_deref())?;
    let preferred_dir = preferred_scope_skill_dir(agent_type, scope, workspace_path.as_deref())?;

    fs::create_dir_all(&preferred_dir)
        .map_err(|e| AcpError::protocol(format!("failed to create skills directory: {e}")))?;

    let existing = locate_existing_skill_across_dirs(&dirs, spec.kind, &id, scope);
    if let Some(ref item) = existing {
        if is_read_only_skill_path(agent_type, Path::new(&item.path)) {
            return Err(AcpError::protocol(format!(
                "skill '{id}' is a built-in system skill and cannot be modified"
            )));
        }
    }
    let mut skill = if let Some(item) = existing {
        item
    } else {
        let new_layout = match spec.kind {
            SkillStorageKind::SkillDirectoryOnly => AgentSkillLayout::SkillDirectory,
            SkillStorageKind::SkillDirectoryOrMarkdownFile => {
                layout.unwrap_or(AgentSkillLayout::MarkdownFile)
            }
        };
        let skill_path = match new_layout {
            AgentSkillLayout::SkillDirectory => preferred_dir.join(&id),
            AgentSkillLayout::MarkdownFile => preferred_dir.join(format!("{id}.md")),
        };
        build_skill_item(id.clone(), scope, new_layout, skill_path)
    };

    let skill_path = PathBuf::from(&skill.path);
    let content_path = skill_content_path(skill.layout, &skill_path);

    if skill.layout == AgentSkillLayout::SkillDirectory {
        fs::create_dir_all(&skill_path).map_err(|e| {
            AcpError::protocol(format!(
                "failed to create skill directory '{}': {e}",
                skill.path
            ))
        })?;
    } else if let Some(parent) = content_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            AcpError::protocol(format!("failed to create skill parent directory: {e}"))
        })?;
    }

    fs::write(&content_path, content)
        .map_err(|e| AcpError::protocol(format!("failed to write skill content: {e}")))?;

    skill.description = read_skill_description(&content_path);

    Ok(skill)
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_delete_agent_skill(
    agent_type: AgentType,
    scope: AgentSkillScope,
    skill_id: String,
    workspace_path: Option<String>,
) -> Result<(), AcpError> {
    let Some(spec) = skill_storage_spec(agent_type) else {
        return Err(AcpError::protocol(format!(
            "{agent_type} skills are not supported in Settings yet"
        )));
    };
    let id = validate_skill_id(&skill_id)?;
    let dirs = scoped_skill_dirs(agent_type, scope, workspace_path.as_deref())?;

    let skill = locate_existing_skill_across_dirs(&dirs, spec.kind, &id, scope)
        .ok_or_else(|| AcpError::protocol(format!("skill not found: {id}")))?;
    if is_read_only_skill_path(agent_type, Path::new(&skill.path)) {
        return Err(AcpError::protocol(format!(
            "skill '{id}' is a built-in system skill and cannot be deleted"
        )));
    }
    let skill_path = PathBuf::from(&skill.path);
    remove_skill_entry(&skill_path)
        .map_err(|e| AcpError::protocol(format!("failed to delete skill entry: {e}")))?;
    Ok(())
}

pub(crate) async fn opencode_list_plugins_core() -> Result<PluginCheckSummary, AcpError> {
    opencode_plugins::check_opencode_plugins(None).map_err(AcpError::Protocol)
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn opencode_list_plugins() -> Result<PluginCheckSummary, AcpError> {
    opencode_list_plugins_core().await
}

pub(crate) async fn opencode_install_plugins_core(
    names: Option<Vec<String>>,
    task_id: String,
    emitter: &EventEmitter,
) -> Result<(), AcpError> {
    opencode_plugins::install_missing_plugins(names, task_id, emitter)
        .await
        .map_err(AcpError::Protocol)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn opencode_install_plugins(
    names: Option<Vec<String>>,
    task_id: String,
    app: tauri::AppHandle,
) -> Result<(), AcpError> {
    let emitter = EventEmitter::Tauri(app);
    opencode_install_plugins_core(names, task_id, &emitter).await
}

pub(crate) async fn opencode_uninstall_plugin_core(
    name: String,
) -> Result<PluginCheckSummary, AcpError> {
    opencode_plugins::uninstall_plugin(name)
        .await
        .map_err(AcpError::Protocol)
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn opencode_uninstall_plugin(name: String) -> Result<PluginCheckSummary, AcpError> {
    opencode_uninstall_plugin_core(name).await
}

// ─── Codex Device Code OAuth ───

const CODEX_OAUTH_ISSUER: &str = "https://auth.openai.com";
const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexDeviceCodeResponse {
    pub user_code: String,
    pub verification_url: String,
    pub device_auth_id: String,
    pub interval: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexDeviceCodePollResult {
    pub status: String,
    pub message: Option<String>,
    pub id_token: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Deserialize)]
struct DeviceCodeUserCodeResp {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    #[serde(
        default = "default_interval",
        deserialize_with = "deserialize_interval"
    )]
    interval: u64,
}

fn default_interval() -> u64 {
    5
}

fn extract_jwt_account_id(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let decoded =
        base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn deserialize_interval<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;
    let value = serde_json::Value::deserialize(deserializer)?;
    match &value {
        serde_json::Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| de::Error::custom(format!("invalid interval number: {n}"))),
        serde_json::Value::String(s) => s.trim().parse::<u64>().map_err(de::Error::custom),
        _ => Err(de::Error::custom(format!(
            "unexpected interval type: {value}"
        ))),
    }
}

#[derive(Deserialize)]
struct DeviceCodeTokenResp {
    authorization_code: String,
    #[allow(dead_code)]
    code_challenge: String,
    code_verifier: String,
}

#[derive(Deserialize)]
struct OAuthTokenResp {
    id_token: String,
    access_token: String,
    refresh_token: String,
}

pub(crate) async fn codex_request_device_code_core() -> Result<CodexDeviceCodeResponse, AcpError> {
    let client = reqwest::Client::new();
    let url = format!("{CODEX_OAUTH_ISSUER}/api/accounts/deviceauth/usercode");
    let body = serde_json::json!({ "client_id": CODEX_OAUTH_CLIENT_ID });

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| AcpError::protocol(format!("device code request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(AcpError::protocol(format!(
            "device code request returned {status}: {text}"
        )));
    }

    let raw_body = resp
        .text()
        .await
        .map_err(|e| AcpError::protocol(format!("read device code response failed: {e}")))?;
    let uc: DeviceCodeUserCodeResp = serde_json::from_str(&raw_body).map_err(|e| {
        AcpError::protocol(format!(
            "parse device code response failed: {e} | body: {raw_body}"
        ))
    })?;

    Ok(CodexDeviceCodeResponse {
        user_code: uc.user_code,
        verification_url: format!("{CODEX_OAUTH_ISSUER}/codex/device"),
        device_auth_id: uc.device_auth_id,
        interval: uc.interval,
    })
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn codex_request_device_code() -> Result<CodexDeviceCodeResponse, AcpError> {
    codex_request_device_code_core().await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn codex_poll_device_code(
    device_auth_id: String,
    user_code: String,
) -> Result<CodexDeviceCodePollResult, AcpError> {
    codex_poll_device_code_core(device_auth_id, user_code).await
}

pub(crate) async fn codex_poll_device_code_core(
    device_auth_id: String,
    user_code: String,
) -> Result<CodexDeviceCodePollResult, AcpError> {
    let client = reqwest::Client::new();
    let poll_url = format!("{CODEX_OAUTH_ISSUER}/api/accounts/deviceauth/token");
    let poll_body = serde_json::json!({
        "device_auth_id": device_auth_id,
        "user_code": user_code,
    });

    let resp = client
        .post(&poll_url)
        .json(&poll_body)
        .send()
        .await
        .map_err(|e| AcpError::protocol(format!("device code poll failed: {e}")))?;

    if !resp.status().is_success() {
        return Ok(CodexDeviceCodePollResult {
            status: "pending".into(),
            message: None,
            id_token: None,
            access_token: None,
            refresh_token: None,
            account_id: None,
        });
    }

    let code_resp: DeviceCodeTokenResp = resp
        .json()
        .await
        .map_err(|e| AcpError::protocol(format!("parse poll response failed: {e}")))?;

    let redirect_uri = format!("{CODEX_OAUTH_ISSUER}/deviceauth/callback");
    let token_url = format!("{CODEX_OAUTH_ISSUER}/oauth/token");

    let token_resp = client
        .post(&token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
            urlencoding::encode(&code_resp.authorization_code),
            urlencoding::encode(&redirect_uri),
            urlencoding::encode(CODEX_OAUTH_CLIENT_ID),
            urlencoding::encode(&code_resp.code_verifier),
        ))
        .send()
        .await
        .map_err(|e| AcpError::protocol(format!("token exchange failed: {e}")))?;

    if !token_resp.status().is_success() {
        let status = token_resp.status();
        let text = token_resp.text().await.unwrap_or_default();
        return Ok(CodexDeviceCodePollResult {
            status: "error".into(),
            message: Some(format!("token exchange returned {status}: {text}")),
            id_token: None,
            access_token: None,
            refresh_token: None,
            account_id: None,
        });
    }

    let tokens: OAuthTokenResp = token_resp
        .json()
        .await
        .map_err(|e| AcpError::protocol(format!("parse token response failed: {e}")))?;

    let account_id = extract_jwt_account_id(&tokens.id_token).unwrap_or_default();

    Ok(CodexDeviceCodePollResult {
        status: "success".into(),
        message: None,
        id_token: Some(tokens.id_token),
        access_token: Some(tokens.access_token),
        refresh_token: Some(tokens.refresh_token),
        account_id: Some(account_id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("codeg-acp-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create test directory");
        dir
    }

    #[test]
    fn sanitize_custom_version_accepts_version_like_inputs() {
        assert_eq!(sanitize_custom_version("0.44.1").as_deref(), Some("0.44.1"));
        assert_eq!(
            sanitize_custom_version("  v1.2.3 ").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            sanitize_custom_version("2026.5.20").as_deref(),
            Some("2026.5.20")
        );
        assert_eq!(
            sanitize_custom_version("1.2.3-beta.1").as_deref(),
            Some("1.2.3-beta.1")
        );
        assert_eq!(
            sanitize_custom_version("1.0.0+build.5").as_deref(),
            Some("1.0.0+build.5")
        );
    }

    #[test]
    fn sanitize_custom_version_rejects_invalid_inputs() {
        for bad in [
            "",
            "   ",
            "latest",
            "next",
            "v",
            "2",
            "v9",
            "1.2 .3",
            "1.2.3@evil",
            "../etc",
        ] {
            assert_eq!(sanitize_custom_version(bad), None, "expected {bad:?} rejected");
        }
    }

    #[test]
    fn build_npm_install_spec_uses_registry_when_no_override() {
        assert_eq!(
            build_npm_install_spec("@google/gemini-cli@0.44.1", None).unwrap(),
            "@google/gemini-cli@0.44.1"
        );
        assert_eq!(
            build_npm_install_spec("@google/gemini-cli@0.44.1", Some("  ")).unwrap(),
            "@google/gemini-cli@0.44.1"
        );
    }

    #[test]
    fn build_npm_install_spec_applies_custom_version() {
        assert_eq!(
            build_npm_install_spec("@google/gemini-cli@0.44.1", Some("0.43.0")).unwrap(),
            "@google/gemini-cli@0.43.0"
        );
        // Scoped/plain package name is preserved; a leading `v` is stripped.
        assert_eq!(
            build_npm_install_spec("cline@3.0.9", Some("v2.0.0")).unwrap(),
            "cline@2.0.0"
        );
    }

    #[test]
    fn build_npm_install_spec_rejects_invalid_override() {
        assert!(build_npm_install_spec("cline@3.0.9", Some("latest")).is_err());
    }

    #[test]
    fn apply_custom_version_to_url_substitutes_all_occurrences() {
        // Codex URL embeds the version twice (path tag + asset filename).
        let codex = "https://github.com/zed-industries/codex-acp/releases/download/v0.15.0/codex-acp-0.15.0-aarch64-apple-darwin.tar.gz";
        assert_eq!(
            apply_custom_version_to_url(codex, "0.15.0", "0.14.0"),
            "https://github.com/zed-industries/codex-acp/releases/download/v0.14.0/codex-acp-0.14.0-aarch64-apple-darwin.tar.gz"
        );

        // OpenCode URL embeds the version once (path tag only).
        let opencode = "https://github.com/anomalyco/opencode/releases/download/v1.15.12/opencode-darwin-arm64.zip";
        assert_eq!(
            apply_custom_version_to_url(opencode, "1.15.12", "1.16.0"),
            "https://github.com/anomalyco/opencode/releases/download/v1.16.0/opencode-darwin-arm64.zip"
        );
    }

    #[test]
    fn parses_npm_global_prefix_stdout() {
        let prefix = npm_global_prefix_from_stdout(b"npm-prefix\n");
        assert_eq!(prefix.as_deref(), Some(Path::new("npm-prefix")));

        assert_eq!(npm_global_prefix_from_stdout(b"\n"), None);
    }

    #[test]
    fn resolves_npx_command_from_npm_prefix_bin_dir() {
        let prefix = unique_test_dir("npm-prefix");
        let bin_dir = npm_prefix_bin_dir(&prefix);
        std::fs::create_dir_all(&bin_dir).expect("create npm prefix bin directory");

        #[cfg(windows)]
        let command_path = bin_dir.join("gemini.cmd");
        #[cfg(not(windows))]
        let command_path = bin_dir.join("gemini");

        std::fs::write(&command_path, "").expect("write command shim");
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = std::fs::metadata(&command_path)
                .expect("read command shim metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&command_path, permissions)
                .expect("mark command shim executable");
        }

        let resolved = resolve_npx_command_from_npm_prefix("gemini", &prefix);

        assert_eq!(resolved.as_deref(), Some(command_path.as_path()));
        let _ = std::fs::remove_dir_all(prefix);
    }

    #[tokio::test]
    async fn does_not_cache_failed_npm_global_prefix_resolution() {
        let cache = tokio::sync::OnceCell::const_new();
        let first = cached_npm_global_prefix_with(&cache, || async { None }).await;
        assert_eq!(first, None);

        let expected = PathBuf::from("npm-prefix");
        let second =
            cached_npm_global_prefix_with(&cache, || async { Some(expected.clone()) }).await;

        assert_eq!(second, Some(expected));
    }

    #[cfg(not(windows))]
    #[test]
    fn ignores_non_executable_npx_command_from_npm_prefix_bin_dir() {
        let prefix = unique_test_dir("npm-prefix-non-executable");
        let bin_dir = npm_prefix_bin_dir(&prefix);
        std::fs::create_dir_all(&bin_dir).expect("create npm prefix bin directory");
        let command_path = bin_dir.join("gemini");
        std::fs::write(&command_path, "").expect("write command shim");

        let resolved = resolve_npx_command_from_npm_prefix("gemini", &prefix);

        assert_eq!(resolved, None);
        let _ = std::fs::remove_dir_all(prefix);
    }

    fn write_skill_md(name: &str, body: &str) -> (PathBuf, PathBuf) {
        let dir = unique_test_dir(name);
        let path = dir.join("SKILL.md");
        std::fs::write(&path, body).expect("write skill markdown");
        (dir, path)
    }

    #[test]
    fn frontmatter_scalar_strips_quotes_and_rejects_blocks() {
        assert_eq!(
            parse_frontmatter_scalar(" \"hello world\"  ").as_deref(),
            Some("hello world")
        );
        assert_eq!(
            parse_frontmatter_scalar(" 'single quoted' ").as_deref(),
            Some("single quoted")
        );
        assert_eq!(
            parse_frontmatter_scalar("  unquoted value  ").as_deref(),
            Some("unquoted value")
        );
        assert_eq!(parse_frontmatter_scalar("   ").as_deref(), None);
        assert_eq!(parse_frontmatter_scalar(" |").as_deref(), None);
        assert_eq!(parse_frontmatter_scalar(" > folded").as_deref(), None);
    }

    #[test]
    fn skill_description_reads_top_level_description() {
        let (dir, path) = write_skill_md(
            "skill-top-desc",
            "---\nname: demo\ndescription: top level desc\n---\nbody\n",
        );
        assert_eq!(
            read_skill_description(&path).as_deref(),
            Some("top level desc")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn skill_description_prefers_nested_short_description() {
        let (dir, path) = write_skill_md(
            "skill-short-desc",
            "---\nname: demo\ndescription: long fallback\nmetadata:\n  short-description: pithy summary\n---\nbody\n",
        );
        assert_eq!(
            read_skill_description(&path).as_deref(),
            Some("pithy summary")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn skill_description_falls_back_when_no_short() {
        let (dir, path) = write_skill_md(
            "skill-fallback",
            "---\nname: demo\ndescription: \"quoted fallback\"\nmetadata:\n  other: value\n---\nbody\n",
        );
        assert_eq!(
            read_skill_description(&path).as_deref(),
            Some("quoted fallback")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn skill_description_ignores_nested_description_key() {
        // A nested `description:` (e.g. inside `metadata:` or a tool block)
        // must not be picked up as the top-level fallback.
        let (dir, path) = write_skill_md(
            "skill-nested-desc",
            "---\nname: demo\nmetadata:\n  description: nested only\n---\nbody\n",
        );
        assert_eq!(read_skill_description(&path), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn skill_description_requires_frontmatter_fence() {
        let (dir, path) = write_skill_md(
            "skill-no-fence",
            "name: demo\ndescription: not really frontmatter\n",
        );
        assert_eq!(read_skill_description(&path), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn skill_description_stops_at_closing_fence() {
        let (dir, path) = write_skill_md(
            "skill-closed",
            "---\nname: demo\n---\ndescription: in body, not frontmatter\n",
        );
        assert_eq!(read_skill_description(&path), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn skill_description_handles_utf8_content() {
        let (dir, path) = write_skill_md(
            "skill-utf8",
            "---\nname: demo\ndescription: 中文 描述 🚀\n---\nbody\n",
        );
        assert_eq!(
            read_skill_description(&path).as_deref(),
            Some("中文 描述 🚀")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn skill_description_returns_none_for_missing_file() {
        let dir = unique_test_dir("skill-missing");
        let path = dir.join("does-not-exist.md");
        assert_eq!(read_skill_description(&path), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
