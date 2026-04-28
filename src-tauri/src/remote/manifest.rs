// Embedded URL pattern + runtime manifest fetcher.
// Path C from architecture §12 / dev-design §2.4: do NOT embed sha256 at compile
// time. Fetch the latest manifest.json from GitHub Release at runtime, cache for
// 24 h, fall back to URL pattern (without sha256) if fetch fails.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{OnceCell, RwLock};

/// Compile-time constants embedded into the desktop binary.
pub const REMOTE_DAEMON_VERSION: &str = env!("CARGO_PKG_VERSION");

const GITHUB_REPO: &str = "xintaofei/codeg";
const MANIFEST_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteDaemonBinary {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub exec_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteDaemonManifest {
    pub version: String,
    pub schema_version: String,
    pub generated_at: String,
    pub binaries: BTreeMap<String, RemoteDaemonBinary>,
}

static MANIFEST_CACHE: OnceCell<RwLock<CachedManifest>> = OnceCell::const_new();

#[derive(Default)]
struct CachedManifest {
    manifest: Option<RemoteDaemonManifest>,
    fetched_at: u64,
}

async fn cache() -> &'static RwLock<CachedManifest> {
    MANIFEST_CACHE
        .get_or_init(|| async { RwLock::new(CachedManifest::default()) })
        .await
}

fn manifest_url(version: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/v{}/codeg-remote-manifest.json",
        GITHUB_REPO, version
    )
}

fn fallback_binary_url(version: &str, platform: &str) -> String {
    let ext = if platform.starts_with("windows-") {
        "zip"
    } else {
        "tar.gz"
    };
    format!(
        "https://github.com/{}/releases/download/v{}/codeg-remote-{}.{}",
        GITHUB_REPO, version, platform, ext
    )
}

fn fallback_exec_name(platform: &str) -> &'static str {
    if platform.starts_with("windows-") {
        "codeg-server.exe"
    } else {
        "codeg-server"
    }
}

/// Fetch (or read from cache) the manifest for the given version.
/// On fetch failure, returns a synthesized "fallback" manifest with URL pattern
/// only — sha256 is empty, callers must skip integrity check or refuse to deploy.
pub async fn get_manifest(version: &str) -> Result<RemoteDaemonManifest, ManifestError> {
    let cache = cache().await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    {
        let guard = cache.read().await;
        if let Some(m) = &guard.manifest {
            if m.version == version
                && now.saturating_sub(guard.fetched_at) < MANIFEST_CACHE_TTL.as_secs()
            {
                return Ok(m.clone());
            }
        }
    }

    let url = manifest_url(version);
    let response = reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await;

    let manifest = match response {
        Ok(resp) if resp.status().is_success() => match resp.json::<RemoteDaemonManifest>().await {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[Remote] manifest parse failed: {e}, using fallback");
                fallback_manifest(version)
            }
        },
        Ok(resp) => {
            eprintln!(
                "[Remote] manifest fetch returned {} for {}, using fallback",
                resp.status(),
                url
            );
            fallback_manifest(version)
        }
        Err(e) => {
            eprintln!("[Remote] manifest fetch failed: {e}, using fallback");
            fallback_manifest(version)
        }
    };

    {
        let mut guard = cache.write().await;
        guard.manifest = Some(manifest.clone());
        guard.fetched_at = now;
    }

    Ok(manifest)
}

fn fallback_manifest(version: &str) -> RemoteDaemonManifest {
    let mut binaries = BTreeMap::new();
    for platform in [
        "linux-x64-musl",
        "linux-arm64-musl",
        "darwin-x64",
        "darwin-arm64",
        "windows-x64",
    ] {
        binaries.insert(
            platform.to_string(),
            RemoteDaemonBinary {
                url: fallback_binary_url(version, platform),
                sha256: String::new(),
                size: 0,
                exec_name: fallback_exec_name(platform).to_string(),
            },
        );
    }

    RemoteDaemonManifest {
        version: version.to_string(),
        schema_version: "v3".to_string(),
        generated_at: String::new(),
        binaries,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest network error: {0}")]
    Network(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_url_pattern_correct() {
        let url = fallback_binary_url("0.12.0", "linux-x64-musl");
        assert_eq!(
            url,
            "https://github.com/xintaofei/codeg/releases/download/v0.12.0/codeg-remote-linux-x64-musl.tar.gz"
        );
    }

    #[test]
    fn fallback_url_windows_uses_zip() {
        let url = fallback_binary_url("0.12.0", "windows-x64");
        assert!(url.ends_with("codeg-remote-windows-x64.zip"));
    }

    #[test]
    fn fallback_manifest_has_all_platforms() {
        let m = fallback_manifest("0.12.0");
        assert_eq!(m.binaries.len(), 5);
        assert!(m.binaries.contains_key("linux-x64-musl"));
        assert!(m.binaries.contains_key("windows-x64"));
        assert!(m.binaries["linux-x64-musl"].sha256.is_empty());
    }

    #[test]
    fn fallback_exec_name_picks_extension() {
        assert_eq!(fallback_exec_name("linux-x64-musl"), "codeg-server");
        assert_eq!(fallback_exec_name("darwin-arm64"), "codeg-server");
        assert_eq!(fallback_exec_name("windows-x64"), "codeg-server.exe");
    }

    #[test]
    fn manifest_url_format() {
        assert_eq!(
            manifest_url("0.12.0"),
            "https://github.com/xintaofei/codeg/releases/download/v0.12.0/codeg-remote-manifest.json"
        );
    }
}
