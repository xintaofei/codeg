use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::lsp::error::LspError;

pub(crate) fn cache_dir() -> Result<PathBuf, LspError> {
    let base = dirs::cache_dir()
        .ok_or_else(|| LspError::DownloadFailed("cannot determine cache directory".into()))?;
    Ok(base.join("app.codeg").join("lsp-binaries"))
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

pub(crate) fn binary_dir(server_id: &str, version: &str) -> Result<PathBuf, LspError> {
    let version = normalize_version_label(version);
    if version.is_empty() {
        return Err(LspError::DownloadFailed(
            "binary version is empty".to_string(),
        ));
    }

    Ok(cache_dir()?
        .join(server_id)
        .join(version)
        .join(crate::acp::registry::current_platform()))
}

pub fn clear_server_cache(server_id: &str) -> Result<(), LspError> {
    let dir = cache_dir()?.join(server_id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| LspError::DownloadFailed(format!("failed to clear cache: {e}")))?;
    }
    Ok(())
}

fn installed_binary_path(server_id: &str, version: &str, cmd_name: &str) -> Option<PathBuf> {
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
        .join(server_id)
        .join(normalized)
        .join(crate::acp::registry::current_platform())
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

fn installed_version_labels(server_id: &str, cmd_name: &str) -> Result<Vec<String>, LspError> {
    let root = cache_dir()?.join(server_id);
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut versions = Vec::new();
    let mut seen = HashSet::new();
    let entries = std::fs::read_dir(&root)
        .map_err(|e| LspError::DownloadFailed(format!("failed to read cache dir: {e}")))?;

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

        if installed_binary_path(server_id, &normalized, cmd_name).is_some()
            && seen.insert(normalized.clone())
        {
            versions.push(normalized);
        }
    }

    Ok(versions)
}

pub fn detect_installed_version(
    server_id: &str,
    cmd_name: &str,
) -> Result<Option<String>, LspError> {
    let mut versions = installed_version_labels(server_id, cmd_name)?;
    if versions.is_empty() {
        return Ok(None);
    }
    versions.sort_by(|a, b| version_cmp(a, b));
    Ok(versions.pop())
}

pub(crate) fn find_cached_binary(
    server_id: &str,
    version: &str,
    cmd_name: &str,
) -> Result<Option<PathBuf>, LspError> {
    Ok(installed_binary_path(server_id, version, cmd_name))
}

pub async fn ensure_binary(
    server_id: &str,
    version: &str,
    archive_url: &str,
    cmd_name: &str,
) -> Result<PathBuf, LspError> {
    if let Some(path) = find_cached_binary(server_id, version, cmd_name)? {
        return Ok(path);
    }

    let dir = binary_dir(server_id, version)?;
    let bin_name = if cfg!(target_os = "windows") {
        format!("{cmd_name}.exe")
    } else {
        cmd_name.to_string()
    };

    std::fs::create_dir_all(&dir)
        .map_err(|e| LspError::DownloadFailed(format!("failed to create cache dir: {e}")))?;

    let tmp_dir = dir.join(".tmp");
    if tmp_dir.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| LspError::DownloadFailed(format!("failed to create tmp dir: {e}")))?;

    let result: Result<PathBuf, LspError> = async {
        let archive_path = tmp_dir.join("archive");
        download_file(archive_url, &archive_path).await?;

        // Handle .gz files (single file, not tar.gz)
        if archive_url.ends_with(".gz") && !archive_url.ends_with(".tar.gz") {
            extract_gz_single(&archive_path, &dir, &bin_name)?;
        } else {
            let extract_dir = tmp_dir.join("extracted");
            std::fs::create_dir_all(&extract_dir).map_err(|e| {
                LspError::DownloadFailed(format!("failed to create extract dir: {e}"))
            })?;

            if archive_url.ends_with(".tar.gz") || archive_url.ends_with(".tgz") {
                extract_tar_gz(&archive_path, &extract_dir)?;
            } else if archive_url.ends_with(".zip") {
                extract_zip(&archive_path, &extract_dir)?;
            } else {
                return Err(LspError::DownloadFailed(format!(
                    "unsupported archive format: {archive_url}"
                )));
            }

            let extracted_bin =
                find_binary_recursive(&extract_dir, &bin_name).ok_or_else(|| {
                    LspError::DownloadFailed(format!("binary '{bin_name}' not found in archive"))
                })?;

            let final_path = dir.join(&bin_name);
            std::fs::copy(&extracted_bin, &final_path)
                .map_err(|e| LspError::DownloadFailed(format!("failed to copy binary: {e}")))?;
        }

        let final_path = dir.join(&bin_name);
        if !is_binary_file_compatible(&final_path) {
            let _ = std::fs::remove_file(&final_path);
            return Err(LspError::DownloadFailed(
                "downloaded binary format is invalid for current platform".into(),
            ));
        }
        set_executable_permissions(&final_path)?;
        Ok(final_path)
    }
    .await;

    let _ = std::fs::remove_dir_all(&tmp_dir);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }

    result
}

fn extract_gz_single(archive: &PathBuf, dest_dir: &Path, bin_name: &str) -> Result<(), LspError> {
    let file = std::fs::File::open(archive)
        .map_err(|e| LspError::DownloadFailed(format!("failed to open archive: {e}")))?;
    let mut gz = flate2::read::GzDecoder::new(file);
    let mut buf = Vec::new();
    gz.read_to_end(&mut buf)
        .map_err(|e| LspError::DownloadFailed(format!("failed to decompress gz: {e}")))?;
    let final_path = dest_dir.join(bin_name);
    std::fs::write(&final_path, &buf)
        .map_err(|e| LspError::DownloadFailed(format!("failed to write binary: {e}")))?;
    Ok(())
}

async fn download_file(url: &str, dest: &PathBuf) -> Result<(), LspError> {
    let response = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| LspError::DownloadFailed(format!("HTTP request failed: {e}")))?;

    if !response.status().is_success() {
        return Err(LspError::DownloadFailed(format!(
            "HTTP {} for {url}",
            response.status()
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| LspError::DownloadFailed(format!("failed to read response: {e}")))?;

    std::fs::write(dest, &bytes)
        .map_err(|e| LspError::DownloadFailed(format!("failed to write archive: {e}")))?;

    Ok(())
}

fn extract_tar_gz(archive: &PathBuf, dest: &PathBuf) -> Result<(), LspError> {
    let file = std::fs::File::open(archive)
        .map_err(|e| LspError::DownloadFailed(format!("failed to open archive: {e}")))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(gz);
    tar.unpack(dest)
        .map_err(|e| LspError::DownloadFailed(format!("failed to extract tar.gz: {e}")))?;
    Ok(())
}

fn extract_zip(archive: &PathBuf, dest: &PathBuf) -> Result<(), LspError> {
    let file = std::fs::File::open(archive)
        .map_err(|e| LspError::DownloadFailed(format!("failed to open archive: {e}")))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| LspError::DownloadFailed(format!("failed to read zip: {e}")))?;
    zip.extract(dest)
        .map_err(|e| LspError::DownloadFailed(format!("failed to extract zip: {e}")))?;
    Ok(())
}

fn find_binary_recursive(dir: &PathBuf, name: &str) -> Option<PathBuf> {
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

fn set_executable_permissions(path: &Path) -> Result<(), LspError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .map_err(|e| LspError::DownloadFailed(e.to_string()))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)
            .map_err(|e| LspError::DownloadFailed(e.to_string()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn is_binary_file_compatible(path: &Path) -> bool {
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
