use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::acp::error::AcpError;
use crate::acp::registry;
use crate::models::agent::AgentType;

pub(crate) fn cache_dir() -> Result<PathBuf, AcpError> {
    let base = dirs::cache_dir()
        .ok_or_else(|| AcpError::DownloadFailed("cannot determine cache directory".into()))?;
    Ok(base.join("app.codeg").join("acp-binaries"))
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
    let dir = cache_dir()?.join(agent_id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| AcpError::DownloadFailed(format!("failed to clear cache: {e}")))?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct AcpCacheEntry {
    pub agent_type: AgentType,
    pub label: String,
    pub path: String,
    pub size_bytes: u64,
    pub file_count: usize,
    pub versions: Vec<String>,
    pub installed_version: Option<String>,
    pub recommended_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AcpCacheInventory {
    pub generated_at: String,
    pub root_path: String,
    pub total_size_bytes: u64,
    pub entries: Vec<AcpCacheEntry>,
}

pub fn inventory_agent_caches() -> Result<AcpCacheInventory, AcpError> {
    let root = cache_dir()?;
    let mut entries = Vec::new();
    let mut total_size_bytes = 0u64;

    for agent_type in AgentType::all() {
        let meta = registry::get_agent_meta(agent_type);
        let recommended_version = match &meta.distribution {
            registry::AgentDistribution::Binary { version, .. } => Some((*version).to_string()),
            _ => None,
        };
        if recommended_version.is_none() {
            continue;
        }

        let agent_id = agent_cache_key(agent_type);
        let dir = root.join(&agent_id);
        let (size_bytes, file_count) = directory_usage(&dir)?;
        let mut versions = match &meta.distribution {
            registry::AgentDistribution::Binary { cmd, .. } => {
                installed_version_labels(&agent_id, cmd)?
            }
            _ => Vec::new(),
        };
        versions.sort_by(|left, right| version_cmp(left, right));
        let installed_version = versions.last().cloned();

        total_size_bytes = total_size_bytes.saturating_add(size_bytes);
        entries.push(AcpCacheEntry {
            agent_type,
            label: meta.name.to_string(),
            path: dir.to_string_lossy().to_string(),
            size_bytes,
            file_count,
            versions,
            installed_version,
            recommended_version,
        });
    }

    Ok(AcpCacheInventory {
        generated_at: chrono::Utc::now().to_rfc3339(),
        root_path: root.to_string_lossy().to_string(),
        total_size_bytes,
        entries,
    })
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

fn directory_usage(path: &Path) -> Result<(u64, usize), AcpError> {
    if !path.exists() {
        return Ok((0, 0));
    }

    let mut total_size = 0u64;
    let mut file_count = 0usize;

    for entry in walkdir::WalkDir::new(path) {
        let entry = entry.map_err(|error| {
            AcpError::DownloadFailed(format!("failed to scan cache dir: {error}"))
        })?;
        if entry.file_type().is_file() {
            let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            total_size = total_size.saturating_add(size);
            file_count += 1;
        }
    }

    Ok((total_size, file_count))
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

    // Binary downloads can be tens of MB; use a generous total timeout. The
    // factory still injects user proxy settings so air-gapped corp networks
    // route through their internal mirror.
    let response = crate::network::http_client::build_client_with(
        crate::network::http_client::ClientConfig::with_timeouts(
            std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(600),
        ),
    )
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
