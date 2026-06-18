use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::acp::error::AcpError;
use crate::acp::registry;
use crate::models::agent::AgentType;

/// Process-local counter appended to rename-aside trash directory names. Guards
/// against the rare case where two `clear_agent_cache` calls land in the same
/// `SystemTime::now()` tick (Windows `GetSystemTimePreciseAsFileTime` has ~100ns
/// resolution) and would otherwise collide on the rename target.
static TRASH_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn cache_dir() -> Result<PathBuf, AcpError> {
    let base = dirs::cache_dir()
        .ok_or_else(|| AcpError::DownloadFailed("cannot determine cache directory".into()))?;
    Ok(base.join("app.codeg").join("acp-binaries"))
}

/// Directory where codeg caches a managed `uv` toolchain (`uv` + `uvx`),
/// downloaded on demand when the user has no system `uv` (used to launch
/// Python ACP agents such as Hermes). Layout:
/// `<cache_dir>/uv-tool/<platform>/{uv,uvx}`.
pub(crate) fn uv_tool_dir() -> Result<PathBuf, AcpError> {
    Ok(cache_dir()?
        .join("uv-tool")
        .join(registry::current_platform()))
}

/// Locate a codeg-managed uv tool binary (`uv` or `uvx`) if it has already
/// been downloaded into the cache. Returns `None` when not present, so
/// callers fall back to PATH / common install locations.
pub fn find_cached_uv_tool(tool: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    };
    let path = uv_tool_dir().ok()?.join(exe);
    path.is_file().then_some(path)
}

/// Pinned `uv` toolchain version codeg downloads on demand when the user has no
/// system `uv` (used to launch Python ACP agents such as Hermes).
const UV_TOOL_VERSION: &str = "0.8.10";

/// Build the astral-sh/uv release archive URL for the current platform.
fn uv_archive_url() -> Option<String> {
    let (target, ext) = match registry::current_platform() {
        "darwin-aarch64" => ("aarch64-apple-darwin", "tar.gz"),
        "darwin-x86_64" => ("x86_64-apple-darwin", "tar.gz"),
        "linux-aarch64" => ("aarch64-unknown-linux-gnu", "tar.gz"),
        "linux-x86_64" => ("x86_64-unknown-linux-gnu", "tar.gz"),
        "windows-aarch64" => ("aarch64-pc-windows-msvc", "zip"),
        "windows-x86_64" => ("x86_64-pc-windows-msvc", "zip"),
        _ => return None,
    };
    Some(format!(
        "https://github.com/astral-sh/uv/releases/download/{UV_TOOL_VERSION}/uv-{target}.{ext}"
    ))
}

/// Download + cache the `uv` toolchain (`uv` + `uvx`) into codeg's cache when no
/// system `uv` is available, so Python ACP agents work with zero prerequisites.
/// Idempotent: returns the cached `uvx` path immediately if already present.
pub async fn ensure_uv_tool(on_progress: impl Fn(&str)) -> Result<PathBuf, AcpError> {
    if let Some(uvx) = find_cached_uv_tool("uvx") {
        on_progress("uv already cached, skipping download");
        return Ok(uvx);
    }

    let url = uv_archive_url().ok_or_else(|| {
        AcpError::PlatformNotSupported(format!(
            "uv is not available for platform {}",
            registry::current_platform()
        ))
    })?;

    let dir = uv_tool_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to create uv cache dir: {e}")))?;
    let tmp_dir = dir.join(".tmp");
    if tmp_dir.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to create tmp dir: {e}")))?;

    let (uv_name, uvx_name) = if cfg!(windows) {
        ("uv.exe", "uvx.exe")
    } else {
        ("uv", "uvx")
    };

    let result: Result<PathBuf, AcpError> = async {
        let archive_path = tmp_dir.join("archive");
        on_progress(&format!("Downloading uv {UV_TOOL_VERSION}..."));
        download_file_with_progress(&url, &archive_path, &on_progress).await?;

        let extract_dir = tmp_dir.join("extracted");
        std::fs::create_dir_all(&extract_dir)
            .map_err(|e| AcpError::DownloadFailed(format!("failed to create extract dir: {e}")))?;

        on_progress("Extracting uv...");
        if url.ends_with(".tar.gz") {
            extract_tar_gz(&archive_path, &extract_dir)?;
        } else if url.ends_with(".zip") {
            extract_zip(&archive_path, &extract_dir)?;
        } else {
            return Err(AcpError::DownloadFailed(format!(
                "unsupported uv archive format: {url}"
            )));
        }

        // The uv archive ships both `uv` and `uvx`; cache both so the resolver
        // and any direct `uv` invocation find them.
        let mut uvx_path: Option<PathBuf> = None;
        for name in [uv_name, uvx_name] {
            let extracted = find_binary_recursive(&extract_dir, name).ok_or_else(|| {
                AcpError::DownloadFailed(format!("'{name}' not found in uv archive"))
            })?;
            let final_path = dir.join(name);
            std::fs::copy(&extracted, &final_path)
                .map_err(|e| AcpError::DownloadFailed(format!("failed to copy {name}: {e}")))?;
            set_executable_permissions(&final_path)?;
            if name == uvx_name {
                uvx_path = Some(final_path);
            }
        }
        on_progress("uv installed successfully");
        uvx_path.ok_or_else(|| AcpError::DownloadFailed("uvx missing after install".into()))
    }
    .await;

    // Only clean the temp extraction dir. Unlike per-agent binary caches,
    // `uv_tool_dir` is shared across all Uvx agents, so removing it on failure
    // could delete a `uv`/`uvx` that a concurrent install (or a live connect)
    // just wrote. A half-written binary is harmless — the next attempt
    // overwrites it, and `find_cached_uv_tool` only reports ready when `uvx` is
    // actually present.
    let _ = std::fs::remove_dir_all(&tmp_dir);
    result
}

/// Marker recording that a `Uvx` agent's package has been pre-fetched into
/// uvx's cache (written by the prepare step). The file content is the prepared
/// version string. Lets the connect/status paths report readiness without
/// introspecting uvx's internal cache or triggering a download.
fn uvx_prepared_marker(registry_id: &str) -> Result<PathBuf, AcpError> {
    Ok(cache_dir()?.join("uvx-prepared").join(registry_id))
}

/// Return the prepared version for a Uvx agent, or `None` if it has not been
/// prepared yet.
pub fn uvx_prepared_version(agent_type: AgentType) -> Option<String> {
    let path = uvx_prepared_marker(registry::registry_id_for(agent_type)).ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let v = raw.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// Record that a Uvx agent's package (at `version`) has been pre-fetched.
pub fn mark_uvx_agent_prepared(agent_type: AgentType, version: &str) -> Result<(), AcpError> {
    let path = uvx_prepared_marker(registry::registry_id_for(agent_type))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AcpError::DownloadFailed(format!("create uvx marker dir failed: {e}")))?;
    }
    std::fs::write(&path, version.as_bytes())
        .map_err(|e| AcpError::DownloadFailed(format!("write uvx marker failed: {e}")))
}

/// Remove a Uvx agent's prepared marker (used on uninstall). Absent marker is OK.
pub fn clear_uvx_agent_prepared(agent_type: AgentType) -> Result<(), AcpError> {
    let path = uvx_prepared_marker(registry::registry_id_for(agent_type))?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AcpError::DownloadFailed(format!(
            "remove uvx marker failed: {e}"
        ))),
    }
}

fn normalize_version_label(version: &str) -> String {
    let trimmed = version.trim();
    if let Some(stripped) = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
    {
        stripped.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn agent_cache_key(agent_type: AgentType) -> String {
    registry::registry_id_for(agent_type).to_string()
}

pub(crate) fn binary_dir(agent_id: &str, version: &str) -> Result<PathBuf, AcpError> {
    let version = normalize_version_label(version);
    if version.is_empty() {
        return Err(AcpError::DownloadFailed(
            "binary version is empty".to_string(),
        ));
    }

    Ok(cache_dir()?
        .join(agent_id)
        .join(version)
        .join(registry::current_platform()))
}

pub fn clear_agent_cache(agent_type: AgentType) -> Result<(), AcpError> {
    let agent_id = agent_cache_key(agent_type);
    let dir = cache_dir()?.join(&agent_id);
    if !dir.exists() {
        return Ok(());
    }

    if std::fs::remove_dir_all(&dir).is_ok() {
        return Ok(());
    }

    // Windows: a running `<cmd>.exe` (ours or anti-virus scanning it) keeps the
    // file locked, so `remove_dir_all` returns ERROR_ACCESS_DENIED. NTFS allows
    // renaming a directory whose children are locked because rename only
    // updates the parent directory entry; the locked file's FILE_OBJECT keeps
    // working under the new path. The aside is swept on next startup.
    let trash_root = cache_dir()?.join(".trash");
    let _ = std::fs::create_dir_all(&trash_root);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = TRASH_COUNTER.fetch_add(1, Ordering::Relaxed);
    let aside = trash_root.join(format!("{agent_id}-{stamp}-{counter}"));
    std::fs::rename(&dir, &aside)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to clear cache: {e}")))?;

    let _ = std::fs::remove_dir_all(&aside);
    Ok(())
}

/// Best-effort cleanup of trash directories left behind by
/// `clear_agent_cache`'s rename-aside fallback. Designed to be run from a
/// detached OS thread at startup: every error path is silently swallowed,
/// no logs, no panics escape, no subprocesses spawned. Whatever cannot be
/// removed (e.g. a binary still locked by an external process) is left for
/// the next startup.
///
/// Iterates children rather than nuking the parent so that a concurrent
/// `clear_agent_cache` racing to rename a fresh entry into `.trash/` cannot
/// have its target directory yanked out from under it.
pub fn sweep_trash() {
    let Ok(base) = cache_dir() else { return };
    let trash = base.join(".trash");
    let Ok(entries) = std::fs::read_dir(&trash) else {
        return;
    };
    for entry in entries.flatten() {
        let _ = std::fs::remove_dir_all(entry.path());
    }
}

fn installed_binary_path(agent_id: &str, version: &str, cmd_name: &str) -> Option<PathBuf> {
    let bin_name = if cfg!(target_os = "windows") {
        format!("{cmd_name}.exe")
    } else {
        cmd_name.to_string()
    };

    let normalized = normalize_version_label(version);
    if normalized.is_empty() {
        return None;
    }

    let path = cache_dir()
        .ok()?
        .join(agent_id)
        .join(normalized)
        .join(registry::current_platform())
        .join(bin_name);

    if !path.exists() {
        return None;
    }
    if is_binary_file_compatible(path.as_path()) {
        return Some(path);
    }
    let _ = std::fs::remove_file(path);
    None
}

fn installed_version_labels(agent_id: &str, cmd_name: &str) -> Result<Vec<String>, AcpError> {
    let root = cache_dir()?.join(agent_id);
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut versions = Vec::new();
    let mut seen = HashSet::new();
    let entries = std::fs::read_dir(&root)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to read cache dir: {e}")))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let raw_version = entry.file_name().to_string_lossy().to_string();
        let normalized = normalize_version_label(&raw_version);
        if normalized.is_empty() {
            continue;
        }

        if installed_binary_path(agent_id, &normalized, cmd_name).is_some()
            && seen.insert(normalized.clone())
        {
            versions.push(normalized);
        }
    }

    Ok(versions)
}

fn installed_version_for_agent(
    agent_type: AgentType,
    cmd_name: &str,
) -> Result<Option<String>, AcpError> {
    let agent_id = agent_cache_key(agent_type);
    let mut versions = installed_version_labels(&agent_id, cmd_name)?;
    if versions.is_empty() {
        return Ok(None);
    }
    versions.sort_by(|a, b| version_cmp(a, b));
    Ok(versions.pop())
}

pub fn detect_installed_version(
    agent_type: AgentType,
    cmd_name: &str,
) -> Result<Option<String>, AcpError> {
    installed_version_for_agent(agent_type, cmd_name)
}

/// Resolve a user-managed binary that can launch the registry command.
///
/// Most binary agents expose the same command name on PATH. OpenCode Desktop
/// for macOS is the notable exception: it installs an app bundle whose ACP-capable
/// CLI is `Contents/MacOS/opencode-cli`, not a PATH-visible `opencode`.
pub fn resolve_system_binary_for_agent(agent_type: AgentType, cmd_name: &str) -> Option<PathBuf> {
    if let Ok(path) = which::which(cmd_name) {
        return Some(path);
    }

    if agent_type == AgentType::OpenCode && cmd_name == "opencode" {
        let mut candidates = vec![PathBuf::from(
            "/Applications/OpenCode.app/Contents/MacOS/opencode-cli",
        )];
        if let Some(home) = dirs::home_dir() {
            candidates.push(
                home.join("Applications")
                    .join("OpenCode.app")
                    .join("Contents")
                    .join("MacOS")
                    .join("opencode-cli"),
            );
        }
        return candidates.into_iter().find(|path| path.is_file());
    }

    None
}

/// Return the best cached binary across all installed versions.
///
/// This returns the path + version label of the highest semver-ish
/// version cached on disk, regardless of what the registry considers
/// the "recommended" version. The session-page connect path uses this
/// to tolerate older-but-still-usable cached binaries (e.g. the user
/// hasn't upgraded yet) — the Settings page will continue to surface
/// an "upgrade available" hint via the separate version-badge path.
///
/// Returns Ok(None) when no usable binary is cached.
pub fn find_best_cached_binary_for_agent(
    agent_type: AgentType,
    cmd_name: &str,
) -> Result<Option<(PathBuf, String)>, AcpError> {
    let agent_id = agent_cache_key(agent_type);
    let mut versions = installed_version_labels(&agent_id, cmd_name)?;
    if versions.is_empty() {
        return Ok(None);
    }
    versions.sort_by(|a, b| version_cmp(a, b));
    while let Some(version) = versions.pop() {
        if let Some(path) = installed_binary_path(&agent_id, &version, cmd_name) {
            return Ok(Some((path, version)));
        }
    }
    Ok(None)
}

fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut a_parts = parse_version_parts(a);
    let mut b_parts = parse_version_parts(b);
    let len = a_parts.len().max(b_parts.len());
    a_parts.resize(len, 0);
    b_parts.resize(len, 0);

    for i in 0..len {
        match a_parts[i].cmp(&b_parts[i]) {
            std::cmp::Ordering::Equal => continue,
            order => return order,
        }
    }
    a.cmp(b)
}

fn parse_version_parts(input: &str) -> Vec<u32> {
    input
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .split('.')
        .map(|part| {
            let numeric: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            numeric.parse::<u32>().unwrap_or(0)
        })
        .collect()
}

/// Same as `ensure_binary_for_agent` but calls `on_progress` with human-readable
/// status messages during download / extraction.
pub async fn ensure_binary_for_agent_with_progress(
    agent_type: AgentType,
    version: &str,
    archive_url: &str,
    cmd_name: &str,
    on_progress: impl Fn(&str),
) -> Result<PathBuf, AcpError> {
    if let Some(path) = find_cached_binary_for_agent(agent_type, version, cmd_name)? {
        on_progress("Binary already cached, skipping download");
        return Ok(path);
    }

    let agent_id = agent_cache_key(agent_type);
    ensure_binary_with_progress(&agent_id, version, archive_url, cmd_name, on_progress).await
}

async fn ensure_binary_with_progress(
    agent_id: &str,
    version: &str,
    archive_url: &str,
    cmd_name: &str,
    on_progress: impl Fn(&str),
) -> Result<PathBuf, AcpError> {
    if let Some(path) = find_cached_binary(agent_id, version, cmd_name)? {
        return Ok(path);
    }

    let dir = binary_dir(agent_id, version)?;
    let bin_name = if cfg!(target_os = "windows") {
        format!("{cmd_name}.exe")
    } else {
        cmd_name.to_string()
    };

    // Download and extract
    std::fs::create_dir_all(&dir)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to create cache dir: {e}")))?;

    let tmp_dir = dir.join(".tmp");
    if tmp_dir.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to create tmp dir: {e}")))?;

    let result: Result<PathBuf, AcpError> = async {
        let archive_path = tmp_dir.join("archive");
        on_progress(&format!("Downloading {archive_url}"));
        download_file_with_progress(archive_url, &archive_path, &on_progress).await?;

        let extract_dir = tmp_dir.join("extracted");
        std::fs::create_dir_all(&extract_dir)
            .map_err(|e| AcpError::DownloadFailed(format!("failed to create extract dir: {e}")))?;

        on_progress("Extracting archive...");
        if archive_url.ends_with(".tar.gz") || archive_url.ends_with(".tgz") {
            extract_tar_gz(&archive_path, &extract_dir)?;
        } else if archive_url.ends_with(".tar.bz2") || archive_url.ends_with(".tbz2") {
            extract_tar_bz2(&archive_path, &extract_dir)?;
        } else if archive_url.ends_with(".zip") {
            extract_zip(&archive_path, &extract_dir)?;
        } else {
            return Err(AcpError::DownloadFailed(format!(
                "unsupported archive format: {archive_url}"
            )));
        }

        // Find the binary in extracted files and move to final location.
        on_progress("Locating binary...");
        let extracted_bin = find_binary_recursive(&extract_dir, &bin_name).ok_or_else(|| {
            AcpError::DownloadFailed(format!("binary '{bin_name}' not found in archive"))
        })?;

        let final_path = dir.join(&bin_name);
        std::fs::copy(&extracted_bin, &final_path)
            .map_err(|e| AcpError::DownloadFailed(format!("failed to copy binary: {e}")))?;

        if !is_binary_file_compatible(&final_path) {
            let _ = std::fs::remove_file(&final_path);
            return Err(AcpError::DownloadFailed(
                "downloaded binary format is invalid for current platform".into(),
            ));
        }
        set_executable_permissions(&final_path)?;
        on_progress("Binary installed successfully");
        Ok(final_path)
    }
    .await;

    // Always clean up temp extraction artifacts.
    let _ = std::fs::remove_dir_all(&tmp_dir);
    if result.is_err() {
        // Avoid leaving empty version/platform directories on failed downloads.
        let _ = std::fs::remove_dir_all(&dir);
    }

    result
}

pub(crate) fn find_cached_binary(
    agent_id: &str,
    version: &str,
    cmd_name: &str,
) -> Result<Option<PathBuf>, AcpError> {
    Ok(installed_binary_path(agent_id, version, cmd_name))
}

pub(crate) fn find_cached_binary_for_agent(
    agent_type: AgentType,
    version: &str,
    cmd_name: &str,
) -> Result<Option<PathBuf>, AcpError> {
    let agent_id = agent_cache_key(agent_type);
    find_cached_binary(&agent_id, version, cmd_name)
}

pub(crate) fn find_binary_recursive(dir: &PathBuf, name: &str) -> Option<PathBuf> {
    if !dir.exists() {
        return None;
    }
    for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
        if entry.file_type().is_file() && entry.file_name().to_string_lossy() == name {
            return Some(entry.into_path());
        }
    }
    None
}

async fn download_file_with_progress(
    url: &str,
    dest: &PathBuf,
    on_progress: &impl Fn(&str),
) -> Result<(), AcpError> {
    use futures_util::StreamExt;

    let response = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| AcpError::DownloadFailed(format!("HTTP request failed: {e}")))?;

    if !response.status().is_success() {
        return Err(AcpError::DownloadFailed(format!(
            "HTTP {} for {url}",
            response.status()
        )));
    }

    let total_size = response.content_length();
    let mut downloaded: u64 = 0;
    let mut last_reported_mb: u64 = 0;
    let mut stream = response.bytes_stream();
    let mut file = std::fs::File::create(dest)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to create archive file: {e}")))?;

    use std::io::Write;
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| AcpError::DownloadFailed(format!("failed to read chunk: {e}")))?;
        file.write_all(&chunk)
            .map_err(|e| AcpError::DownloadFailed(format!("failed to write archive: {e}")))?;
        downloaded += chunk.len() as u64;

        // Report progress every 1MB
        let current_mb = downloaded / (1024 * 1024);
        if current_mb > last_reported_mb {
            last_reported_mb = current_mb;
            if let Some(total) = total_size {
                let total_mb = total as f64 / (1024.0 * 1024.0);
                on_progress(&format!(
                    "Downloading... {current_mb:.0} MB / {total_mb:.1} MB"
                ));
            } else {
                on_progress(&format!("Downloading... {current_mb:.0} MB"));
            }
        }
    }

    if let Some(total) = total_size {
        let total_mb = total as f64 / (1024.0 * 1024.0);
        on_progress(&format!("Download complete ({total_mb:.1} MB)"));
    } else {
        let final_mb = downloaded as f64 / (1024.0 * 1024.0);
        on_progress(&format!("Download complete ({final_mb:.1} MB)"));
    }

    Ok(())
}

fn extract_tar_gz(archive: &PathBuf, dest: &PathBuf) -> Result<(), AcpError> {
    let file = std::fs::File::open(archive)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to open archive: {e}")))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(gz);
    tar.unpack(dest)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to extract tar.gz: {e}")))?;
    Ok(())
}

fn extract_tar_bz2(archive: &PathBuf, dest: &PathBuf) -> Result<(), AcpError> {
    let file = std::fs::File::open(archive)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to open archive: {e}")))?;
    let bz = bzip2::read::BzDecoder::new(file);
    let mut tar = tar::Archive::new(bz);
    tar.unpack(dest)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to extract tar.bz2: {e}")))?;
    Ok(())
}

fn extract_zip(archive: &PathBuf, dest: &PathBuf) -> Result<(), AcpError> {
    let file = std::fs::File::open(archive)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to open archive: {e}")))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to read zip: {e}")))?;
    zip.extract(dest)
        .map_err(|e| AcpError::DownloadFailed(format!("failed to extract zip: {e}")))?;
    Ok(())
}

fn set_executable_permissions(path: &Path) -> Result<(), AcpError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .map_err(|e| AcpError::DownloadFailed(e.to_string()))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).map_err(|e| AcpError::DownloadFailed(e.to_string()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

pub(crate) fn is_binary_file_compatible(path: &Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut header = [0_u8; 4];
    if file.read_exact(&mut header).is_err() {
        return false;
    }

    #[cfg(target_os = "macos")]
    {
        matches!(
            header,
            [0xFE, 0xED, 0xFA, 0xCE]
                | [0xCE, 0xFA, 0xED, 0xFE]
                | [0xFE, 0xED, 0xFA, 0xCF]
                | [0xCF, 0xFA, 0xED, 0xFE]
                | [0xCA, 0xFE, 0xBA, 0xBE]
                | [0xBE, 0xBA, 0xFE, 0xCA]
                | [0xCA, 0xFE, 0xBA, 0xBF]
                | [0xBF, 0xBA, 0xFE, 0xCA]
        )
    }

    #[cfg(target_os = "linux")]
    {
        header == [0x7F, b'E', b'L', b'F']
    }

    #[cfg(target_os = "windows")]
    {
        header[0] == b'M' && header[1] == b'Z'
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_uses_registry_id() {
        assert_eq!(agent_cache_key(AgentType::OpenCode), "opencode");
        assert_eq!(agent_cache_key(AgentType::Codex), "codex-acp");
    }

    #[test]
    fn version_normalization_is_consistent() {
        assert_eq!(normalize_version_label("v1.2.15"), "1.2.15");
        assert_eq!(normalize_version_label("V0.9.4 "), "0.9.4");
        assert_eq!(normalize_version_label("1.25.1"), "1.25.1");
    }
}
