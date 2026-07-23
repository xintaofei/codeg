use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::web::event_bridge::{emit_event, EventEmitter};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatus {
    Installed,
    Missing,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub name: String,
    pub declared_spec: String,
    pub installed_version: Option<String>,
    pub status: PluginStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginCheckSummary {
    pub config_path: PathBuf,
    pub cache_dir: PathBuf,
    pub plugins: Vec<PluginInfo>,
    pub has_project_config_hint: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginInstallEventKind {
    Started,
    Log,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginInstallEvent {
    pub task_id: String,
    pub kind: PluginInstallEventKind,
    pub payload: String,
}

/// Well-known paths for opencode configuration and cache.
///
/// OpenCode follows XDG conventions on all platforms:
///   config: $XDG_CONFIG_HOME/opencode  or  ~/.config/opencode
///   cache:  $XDG_CACHE_HOME/opencode   or  ~/.cache/opencode
///
/// We must NOT use `dirs::config_dir()` / `dirs::cache_dir()` because on
/// macOS those return ~/Library/Application Support and ~/Library/Caches,
/// while opencode always uses the XDG paths.
fn opencode_config_path() -> Option<PathBuf> {
    xdg_config_home().map(|d| d.join("opencode").join("opencode.json"))
}

fn opencode_cache_dir() -> Option<PathBuf> {
    xdg_cache_home().map(|d| d.join("opencode"))
}

pub(crate) fn xdg_config_home() -> Option<PathBuf> {
    // An empty XDG_CONFIG_HOME must fall back to ~/.config, not resolve to a
    // relative "opencode/..." path (matches the XDG_DATA_HOME handling).
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
}

fn xdg_cache_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
}

/// Check whether a project directory contains any opencode configuration file.
fn has_project_opencode_config(project_root: &Path) -> bool {
    let candidates = [
        project_root.join("opencode.json"),
        project_root.join("opencode.jsonc"),
        project_root.join(".opencode").join("opencode.json"),
        project_root.join(".opencode").join("opencode.jsonc"),
    ];
    candidates.iter().any(|p| p.exists())
}

/// Inspect `~/.config/opencode/opencode.json` and `~/.cache/opencode/node_modules/`
/// to determine which declared plugins are installed and which are missing.
pub fn check_opencode_plugins(project_root: Option<&Path>) -> Result<PluginCheckSummary, String> {
    let config_path = opencode_config_path()
        .ok_or_else(|| "Cannot determine opencode config directory".to_string())?;
    let cache_dir = opencode_cache_dir()
        .ok_or_else(|| "Cannot determine opencode cache directory".to_string())?;

    let has_project_config_hint = project_root
        .map(has_project_opencode_config)
        .unwrap_or(false);

    // If config file doesn't exist, there's nothing to check
    if !config_path.exists() {
        return Ok(PluginCheckSummary {
            config_path,
            cache_dir,
            plugins: vec![],
            has_project_config_hint,
        });
    }

    // Read and parse JSON
    let raw = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read {}: {e}", config_path.display()))?;
    let doc: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("Failed to parse {}: {e}", config_path.display()))?;

    // Extract plugin[] array
    let plugin_array = match doc.get("plugin") {
        Some(serde_json::Value::Array(arr)) => arr,
        Some(_) => {
            return Ok(PluginCheckSummary {
                config_path,
                cache_dir,
                plugins: vec![],
                has_project_config_hint,
            });
        }
        None => {
            return Ok(PluginCheckSummary {
                config_path,
                cache_dir,
                plugins: vec![],
                has_project_config_hint,
            });
        }
    };

    // Parse specs, dedup by name
    let mut seen_names = HashSet::new();
    let mut plugins = Vec::new();

    for item in plugin_array {
        let spec_str = match item.as_str() {
            Some(s) => s,
            None => {
                tracing::warn!("[opencode_plugins] Skipping non-string plugin entry: {item}");
                continue;
            }
        };

        let (name, declared_spec) = match parse_plugin_spec(spec_str) {
            Some(pair) => pair,
            None => {
                tracing::warn!("[opencode_plugins] Skipping invalid plugin spec: {spec_str:?}");
                continue;
            }
        };

        if !seen_names.insert(name.clone()) {
            continue; // duplicate, skip
        }

        // Check node_modules/<name>/package.json
        let pkg_json_path = cache_dir
            .join("node_modules")
            .join(&name)
            .join("package.json");

        let (status, installed_version) = if pkg_json_path.exists() {
            let version = std::fs::read_to_string(&pkg_json_path)
                .ok()
                .and_then(|content| {
                    serde_json::from_str::<serde_json::Value>(&content)
                        .ok()?
                        .get("version")?
                        .as_str()
                        .map(|s| s.to_string())
                });
            (PluginStatus::Installed, version)
        } else {
            (PluginStatus::Missing, None)
        };

        plugins.push(PluginInfo {
            name,
            declared_spec,
            installed_version,
            status,
        });
    }

    Ok(PluginCheckSummary {
        config_path,
        cache_dir,
        plugins,
        has_project_config_hint,
    })
}

/// Locate a usable bun binary.
/// Priority: opencode-bundled bun → system bun → error.
pub fn resolve_bun_binary() -> Result<PathBuf, String> {
    let cache_dir = opencode_cache_dir();

    // Try opencode-bundled bun
    if let Some(ref dir) = cache_dir {
        let candidates = if cfg!(windows) {
            vec![dir.join("bin").join("bun.exe")]
        } else {
            vec![dir.join("bin").join("bun")]
        };
        for candidate in candidates {
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    // Fallback to system bun
    if let Ok(system_bun) = which::which("bun") {
        return Ok(system_bun);
    }

    Err(
        "bun binary not found. Neither opencode-bundled bun (~/.cache/opencode/bin/bun) \
         nor system bun is available."
            .to_string(),
    )
}

/// Detect whether a JSON string contains comments (// or /*).
fn json_has_comments(raw: &str) -> bool {
    raw.contains("//") || raw.contains("/*")
}

/// Write a timestamped backup of a file, keeping only the most recent `keep` copies.
fn write_backup_and_prune(path: &Path, content: &str, keep: usize) -> Result<(), String> {
    let now = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S");
    let backup_path = path.with_file_name(format!(
        "{}.bak.{now}",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    fs::write(&backup_path, content)
        .map_err(|e| format!("Failed to write backup {}: {e}", backup_path.display()))?;

    // Prune old backups
    let parent = path.parent().ok_or("No parent directory")?;
    let stem = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let prefix = format!("{stem}.bak.");

    let mut backups: Vec<_> = fs::read_dir(parent)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .collect();

    // Sort by name descending (timestamp in name → newest first)
    backups.sort_by_key(|b| std::cmp::Reverse(b.file_name()));

    for old in backups.iter().skip(keep) {
        let _ = fs::remove_file(old.path());
    }

    Ok(())
}

/// Atomically rewrite opencode.json: read → backup → mutate → write temp → rename.
pub(crate) fn atomic_rewrite_opencode_json(
    path: &Path,
    mutator: impl FnOnce(&mut serde_json::Value) -> Result<(), String>,
) -> Result<(), String> {
    let raw =
        fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;

    // Try parsing first.  If serde_json succeeds the file is valid JSON
    // and any "//" or "/*" sequences live inside string values — not real
    // comments.  Only reject when parsing *fails* and the raw text looks
    // like it might be JSONC (actual comments would cause a parse error).
    if serde_json::from_str::<serde_json::Value>(&raw).is_err() && json_has_comments(&raw) {
        return Err(
            "opencode.json appears to be JSONC (contains comments). Refusing to rewrite to avoid \
             data loss. Please edit the file manually."
                .to_string(),
        );
    }

    write_backup_and_prune(path, &raw, 3)?;

    let mut doc: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;

    mutator(&mut doc)?;

    let new_raw =
        serde_json::to_string_pretty(&doc).map_err(|e| format!("Failed to serialize JSON: {e}"))?;

    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, &new_raw).map_err(|e| format!("Failed to write temp file: {e}"))?;
    fs::rename(&tmp_path, path).map_err(|e| format!("Failed to rename temp file: {e}"))?;

    Ok(())
}

/// Check whether a plugin spec uses a floating version tag like `@latest`.
pub fn spec_has_floating_version(spec: &str) -> bool {
    if let Some((_, full)) = parse_plugin_spec(spec) {
        full.ends_with("@latest")
    } else {
        false
    }
}

/// After a successful install, replace `@latest` specs in opencode.json with
/// the actual installed version read from node_modules.  This prevents
/// opencode from hitting the npm registry on every startup.
fn pin_latest_specs(
    config_path: &Path,
    cache_dir: &Path,
    specs: &[(String, String)], // (name, original_declared_spec)
) -> Result<usize, String> {
    let mut pinned = 0;

    // Collect name → installed_version for specs that have @latest
    let mut pin_map: Vec<(String, String)> = Vec::new();
    for (name, declared) in specs {
        if !declared.ends_with("@latest") {
            continue;
        }
        let pkg_json = cache_dir
            .join("node_modules")
            .join(name)
            .join("package.json");
        if let Ok(content) = fs::read_to_string(&pkg_json) {
            if let Some(version) = serde_json::from_str::<serde_json::Value>(&content)
                .ok()
                .and_then(|v| v.get("version")?.as_str().map(|s| s.to_string()))
            {
                pin_map.push((name.clone(), version));
            }
        }
    }

    if pin_map.is_empty() {
        return Ok(0);
    }

    atomic_rewrite_opencode_json(config_path, |doc| {
        if let Some(arr) = doc
            .as_object_mut()
            .and_then(|obj| obj.get_mut("plugin"))
            .and_then(|v| v.as_array_mut())
        {
            for item in arr.iter_mut() {
                if let Some(spec_str) = item.as_str() {
                    if let Some((parsed_name, _)) = parse_plugin_spec(spec_str) {
                        if let Some((_, version)) = pin_map.iter().find(|(n, _)| *n == parsed_name)
                        {
                            *item = serde_json::Value::String(format!("{parsed_name}@{version}"));
                            pinned += 1;
                        }
                    }
                }
            }
        }
        Ok(())
    })?;

    Ok(pinned)
}

static PLUGIN_OP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const PLUGIN_INSTALL_EVENT: &str = "app://opencode-plugin-install";

/// Packages that must never be uninstalled (opencode internals).
fn is_protected_package(name: &str) -> bool {
    name.starts_with("@opencode-ai/")
}

fn emit_plugin_event(
    emitter: &EventEmitter,
    task_id: &str,
    kind: PluginInstallEventKind,
    payload: impl Into<String>,
) {
    emit_event(
        emitter,
        PLUGIN_INSTALL_EVENT,
        PluginInstallEvent {
            task_id: task_id.to_string(),
            kind,
            payload: payload.into(),
        },
    );
}

/// Install missing plugins by running `bun add` in the opencode cache directory.
/// Streams progress events to the given emitter.
pub async fn install_missing_plugins(
    names: Option<Vec<String>>,
    task_id: String,
    emitter: &EventEmitter,
) -> Result<(), String> {
    let _guard = PLUGIN_OP_LOCK
        .try_lock()
        .map_err(|_| "Another plugin operation is in progress".to_string())?;

    emit_plugin_event(emitter, &task_id, PluginInstallEventKind::Started, "");

    // Re-check current state
    let summary = check_opencode_plugins(None).inspect_err(|e| {
        emit_plugin_event(emitter, &task_id, PluginInstallEventKind::Failed, e);
    })?;

    let missing: Vec<&PluginInfo> = summary
        .plugins
        .iter()
        .filter(|p| p.status == PluginStatus::Missing)
        .filter(|p| match &names {
            Some(list) => list.contains(&p.name),
            None => true,
        })
        .collect();

    if missing.is_empty() {
        // Nothing to install, but still pin any @latest specs
        let all_specs: Vec<(String, String)> = summary
            .plugins
            .iter()
            .map(|p| (p.name.clone(), p.declared_spec.clone()))
            .collect();
        match pin_latest_specs(&summary.config_path, &summary.cache_dir, &all_specs) {
            Ok(n) if n > 0 => {
                emit_plugin_event(
                    emitter,
                    &task_id,
                    PluginInstallEventKind::Log,
                    format!("Pinned {n} @latest plugin(s) to installed versions in opencode.json"),
                );
                emit_plugin_event(
                    emitter,
                    &task_id,
                    PluginInstallEventKind::Completed,
                    format!("Pinned {n} @latest plugin(s) — no missing plugins to install"),
                );
            }
            Err(e) => {
                emit_plugin_event(
                    emitter,
                    &task_id,
                    PluginInstallEventKind::Failed,
                    format!("Failed to pin @latest versions: {e}"),
                );
            }
            _ => {
                emit_plugin_event(
                    emitter,
                    &task_id,
                    PluginInstallEventKind::Completed,
                    "Nothing to install — all plugins are already present",
                );
            }
        }
        return Ok(());
    }

    let specs: Vec<String> = missing.iter().map(|p| p.declared_spec.clone()).collect();
    let names_display: Vec<&str> = missing.iter().map(|p| p.name.as_str()).collect();

    // Resolve bun
    let bun = resolve_bun_binary().inspect_err(|e| {
        emit_plugin_event(emitter, &task_id, PluginInstallEventKind::Failed, e);
    })?;

    emit_plugin_event(
        emitter,
        &task_id,
        PluginInstallEventKind::Log,
        format!("Installing: {}", names_display.join(", ")),
    );

    // Spawn bun add
    let mut cmd = crate::process::tokio_command(&bun);
    cmd.arg("add")
        .args(&specs)
        .current_dir(&summary.cache_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        let msg = format!("Failed to spawn bun: {e}");
        emit_plugin_event(emitter, &task_id, PluginInstallEventKind::Failed, &msg);
        msg
    })?;

    // Stream stdout and stderr concurrently
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let emitter_clone = emitter.clone();
    let task_id_clone = task_id.clone();

    // `collect_lines_lossy` (not `Lines`/`next_line()`) matters here: bun/npm
    // can emit OEM-codepage bytes (e.g. GBK on a zh-CN Windows) for localized
    // OS-level error text, which `next_line()` chokes on and silently drops —
    // truncating the live install log at the first non-UTF-8 byte. The
    // collected return value is unused (the failure message below is built from
    // the exit code, not captured stderr), so it is discarded.
    let stdout_handle = tokio::spawn({
        let emitter = emitter_clone.clone();
        let task_id = task_id_clone.clone();
        async move {
            if let Some(stdout) = stdout {
                crate::process::collect_lines_lossy(tokio::io::BufReader::new(stdout), |line| {
                    emit_plugin_event(&emitter, &task_id, PluginInstallEventKind::Log, line);
                })
                .await;
            }
        }
    });

    let stderr_handle = tokio::spawn({
        let emitter = emitter_clone;
        let task_id = task_id_clone;
        async move {
            if let Some(stderr) = stderr {
                crate::process::collect_lines_lossy(tokio::io::BufReader::new(stderr), |line| {
                    emit_plugin_event(&emitter, &task_id, PluginInstallEventKind::Log, line);
                })
                .await;
            }
        }
    });

    let _ = tokio::join!(stdout_handle, stderr_handle);

    let exit_status = child.wait().await.map_err(|e| {
        let msg = format!("Failed to wait for bun process: {e}");
        emit_plugin_event(emitter, &task_id, PluginInstallEventKind::Failed, &msg);
        msg
    })?;

    if exit_status.success() {
        // Pin @latest specs to actual installed versions to avoid
        // opencode hitting the npm registry on every startup.
        // Pin ALL plugins (not just the ones we installed), so already-installed
        // @latest plugins also get pinned.
        let spec_pairs: Vec<(String, String)> = summary
            .plugins
            .iter()
            .map(|p| (p.name.clone(), p.declared_spec.clone()))
            .collect();
        match pin_latest_specs(&summary.config_path, &summary.cache_dir, &spec_pairs) {
            Ok(n) if n > 0 => {
                emit_plugin_event(
                    emitter,
                    &task_id,
                    PluginInstallEventKind::Log,
                    format!("Pinned {n} @latest plugin(s) to installed versions in opencode.json"),
                );
            }
            Err(e) => {
                emit_plugin_event(
                    emitter,
                    &task_id,
                    PluginInstallEventKind::Log,
                    format!("Warning: could not pin @latest versions: {e}"),
                );
            }
            _ => {}
        }

        emit_plugin_event(
            emitter,
            &task_id,
            PluginInstallEventKind::Completed,
            "All plugins installed successfully",
        );
        Ok(())
    } else {
        let msg = format!("bun exited with code {}", exit_status.code().unwrap_or(-1));
        emit_plugin_event(emitter, &task_id, PluginInstallEventKind::Failed, &msg);
        Err(msg)
    }
}

/// Uninstall a single plugin: remove from opencode.json, then `bun remove` from cache.
pub async fn uninstall_plugin(name: String) -> Result<PluginCheckSummary, String> {
    let _guard = PLUGIN_OP_LOCK
        .try_lock()
        .map_err(|_| "Another plugin operation is in progress".to_string())?;

    if is_protected_package(&name) {
        return Err(format!(
            "Cannot uninstall {name}: it is an internal opencode package"
        ));
    }

    let config_path = opencode_config_path()
        .ok_or_else(|| "Cannot determine opencode config directory".to_string())?;
    let cache_dir = opencode_cache_dir()
        .ok_or_else(|| "Cannot determine opencode cache directory".to_string())?;

    // Step 1: Remove from opencode.json if declared
    if config_path.exists() {
        let _ = atomic_rewrite_opencode_json(&config_path, |doc| {
            if let Some(arr) = doc
                .as_object_mut()
                .and_then(|obj| obj.get_mut("plugin"))
                .and_then(|v| v.as_array_mut())
            {
                arr.retain(|item| {
                    if let Some(spec) = item.as_str() {
                        match parse_plugin_spec(spec) {
                            Some((parsed_name, _)) => parsed_name != name,
                            None => true,
                        }
                    } else {
                        true
                    }
                });
            }
            Ok(())
        });
    }

    // Step 2: bun remove
    let bun = resolve_bun_binary()?;
    let output = crate::process::tokio_command(&bun)
        .arg("remove")
        .arg(&name)
        .current_dir(&cache_dir)
        .output()
        .await
        .map_err(|e| format!("Failed to run bun remove: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("not found") {
            return Err(format!("bun remove failed: {stderr}"));
        }
    }

    // Return fresh summary
    check_opencode_plugins(None)
}

/// Parse a plugin spec string from opencode.json `plugin[]` into (package_name, full_spec).
///
/// Examples:
/// - `"foo"` → `Some(("foo", "foo"))`
/// - `"foo@latest"` → `Some(("foo", "foo@latest"))`
/// - `"foo@1.2.3"` → `Some(("foo", "foo@1.2.3"))`
/// - `"@scope/name"` → `Some(("@scope/name", "@scope/name"))`
/// - `"@scope/name@1.2.3"` → `Some(("@scope/name", "@scope/name@1.2.3"))`
/// - `""` → `None`
pub fn parse_plugin_spec(spec: &str) -> Option<(String, String)> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }

    let full_spec = spec.to_string();

    if spec.starts_with('@') {
        // Scoped package: @scope/name or @scope/name@version
        let without_at = spec.strip_prefix('@')?;
        let slash_pos = without_at.find('/')?;
        let after_slash = &without_at[slash_pos + 1..];
        // Look for @ that separates name from version
        if let Some(version_at) = after_slash.find('@') {
            let name = &spec[..1 + slash_pos + 1 + version_at]; // @scope/name
            Some((name.to_string(), full_spec))
        } else {
            // No version part
            Some((spec.to_string(), full_spec))
        }
    } else {
        // Unscoped: name or name@version
        if let Some(at_pos) = spec.find('@') {
            let name = &spec[..at_pos];
            if name.is_empty() {
                return None; // bare "@" is invalid
            }
            Some((name.to_string(), full_spec))
        } else {
            Some((spec.to_string(), full_spec))
        }
    }
}
