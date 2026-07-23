use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Resolve the `codeg-tsnet` sidecar binary.
///
/// Order:
/// 1. `CODEG_TSNET_BIN`
/// 2. sibling of current executable
/// 3. PATH via `which`
pub fn locate_codeg_tsnet_binary() -> Option<PathBuf> {
    let filename = if cfg!(windows) {
        "codeg-tsnet.exe"
    } else {
        "codeg-tsnet"
    };

    if let Some(raw) = std::env::var_os("CODEG_TSNET_BIN") {
        let candidate = PathBuf::from(raw);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }

    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        let candidate = dir.join(filename);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }

    which::which(filename)
        .ok()
        .filter(|p| is_executable_file(p))
}

pub fn default_state_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tailscale")
}

/// Stable hostname derived from the installation data dir.
pub fn default_hostname(data_dir: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data_dir.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let short = digest
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("codeg-{short}")
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_is_stable_for_same_data_dir() {
        let p = std::path::PathBuf::from("/tmp/codeg-data-a");
        assert_eq!(default_hostname(&p), default_hostname(&p));
        assert!(default_hostname(&p).starts_with("codeg-"));
        assert_eq!(default_hostname(&p).len(), "codeg-".len() + 8);
    }

    #[test]
    fn state_dir_is_under_data_dir() {
        let p = std::path::PathBuf::from("/tmp/codeg-data-a");
        assert_eq!(default_state_dir(&p), p.join("tailscale"));
    }
}
