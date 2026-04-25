use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app_error::AppCommandError;

const BUILD_MISMATCH_OVERRIDE_ENV: &str = "CODEG_ALLOW_BUILD_MISMATCH";
const INSECURE_REMOTE_OVERRIDE_ENV: &str = "CODEG_ALLOW_INSECURE_REMOTE";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendBuildInfo {
    pub version: String,
    pub git_commit: Option<String>,
    pub built_at: Option<String>,
    pub profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendBuildManifest {
    pub version: String,
    #[serde(default)]
    pub git_commit: Option<String>,
    #[serde(default)]
    pub built_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConsistencyInfo {
    pub status: String,
    pub message: String,
    pub backend: BackendBuildInfo,
    pub frontend: Option<FrontendBuildManifest>,
    pub manifest_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeSecurityInfo {
    pub mode: String,
    pub host: Option<String>,
    pub auth_enabled: bool,
    pub allow_remote_access: bool,
    pub insecure: bool,
    pub override_active: bool,
    pub static_dir: Option<String>,
    pub data_dir: Option<String>,
}

pub fn backend_build_info() -> BackendBuildInfo {
    BackendBuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_commit: option_env!("CODEG_BUILD_GIT_COMMIT")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        built_at: option_env!("CODEG_BUILD_TIMESTAMP")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        profile: option_env!("CODEG_BUILD_PROFILE")
            .unwrap_or("unknown")
            .to_string(),
    }
}

pub fn build_manifest_path(static_dir: &Path) -> PathBuf {
    static_dir.join("codeg-build.json")
}

pub fn load_frontend_build_manifest(
    static_dir: &Path,
) -> Result<Option<FrontendBuildManifest>, AppCommandError> {
    let manifest_path = build_manifest_path(static_dir);
    if !manifest_path.exists() {
        return Ok(None);
    }

    let raw = std::fs::read_to_string(&manifest_path).map_err(AppCommandError::io)?;
    let manifest = serde_json::from_str::<FrontendBuildManifest>(&raw).map_err(|error| {
        AppCommandError::configuration_invalid("Failed to parse frontend build manifest")
            .with_detail(error.to_string())
    })?;
    Ok(Some(manifest))
}

pub fn evaluate_build_consistency(static_dir: &Path) -> BuildConsistencyInfo {
    let backend = backend_build_info();
    let manifest_path = build_manifest_path(static_dir);

    match load_frontend_build_manifest(static_dir) {
        Ok(Some(frontend)) => {
            let version_matches = frontend.version == backend.version;
            let commit_matches = match (&frontend.git_commit, &backend.git_commit) {
                (Some(frontend_commit), Some(backend_commit)) => frontend_commit == backend_commit,
                _ => true,
            };

            let (status, message) = if version_matches && commit_matches {
                (
                    "ok".to_string(),
                    "Frontend and backend builds are aligned".to_string(),
                )
            } else {
                let mut reasons = Vec::new();
                if !version_matches {
                    reasons.push(format!(
                        "version mismatch: frontend={} backend={}",
                        frontend.version, backend.version
                    ));
                }
                if !commit_matches {
                    reasons.push(format!(
                        "git commit mismatch: frontend={} backend={}",
                        frontend.git_commit.as_deref().unwrap_or("unknown"),
                        backend.git_commit.as_deref().unwrap_or("unknown")
                    ));
                }
                (
                    "mismatch".to_string(),
                    format!("Static assets are out of sync: {}", reasons.join(", ")),
                )
            };

            BuildConsistencyInfo {
                status,
                message,
                backend,
                frontend: Some(frontend),
                manifest_path: manifest_path.to_string_lossy().to_string(),
            }
        }
        Ok(None) => BuildConsistencyInfo {
            status: "missing".to_string(),
            message: "Frontend build manifest is missing".to_string(),
            backend,
            frontend: None,
            manifest_path: manifest_path.to_string_lossy().to_string(),
        },
        Err(error) => BuildConsistencyInfo {
            status: "error".to_string(),
            message: format!("Failed to read frontend build manifest: {error}"),
            backend,
            frontend: None,
            manifest_path: manifest_path.to_string_lossy().to_string(),
        },
    }
}

pub fn enforce_build_consistency(
    static_dir: &Path,
) -> Result<BuildConsistencyInfo, AppCommandError> {
    let info = evaluate_build_consistency(static_dir);
    if info.status == "mismatch" && !env_flag(BUILD_MISMATCH_OVERRIDE_ENV) {
        return Err(AppCommandError::configuration_invalid(
            "Frontend and backend builds do not match",
        )
        .with_detail(info.message.clone()));
    }
    Ok(info)
}

pub fn build_security_info(
    mode: &str,
    host: Option<&str>,
    auth_enabled: bool,
    static_dir: Option<&Path>,
    data_dir: Option<&Path>,
) -> RuntimeSecurityInfo {
    let override_active = env_flag(INSECURE_REMOTE_OVERRIDE_ENV);
    let allow_remote_access = host.is_some_and(|value| !is_loopback_host(value));
    let insecure = allow_remote_access && !auth_enabled;

    RuntimeSecurityInfo {
        mode: mode.to_string(),
        host: host.map(str::to_string),
        auth_enabled,
        allow_remote_access,
        insecure,
        override_active,
        static_dir: static_dir.map(|path| path.to_string_lossy().to_string()),
        data_dir: data_dir.map(|path| path.to_string_lossy().to_string()),
    }
}

pub fn enforce_startup_security(
    mode: &str,
    host: &str,
    auth_enabled: bool,
    static_dir: Option<&Path>,
    data_dir: Option<&Path>,
) -> Result<RuntimeSecurityInfo, AppCommandError> {
    let info = build_security_info(mode, Some(host), auth_enabled, static_dir, data_dir);
    if info.insecure && !info.override_active {
        return Err(AppCommandError::configuration_invalid(
            "Refusing insecure startup: auth is disabled while listening on a non-loopback address",
        )
        .with_detail(format!(
            "Set {INSECURE_REMOTE_OVERRIDE_ENV}=1 to override intentionally."
        )));
    }
    Ok(info)
}

pub fn is_loopback_host(host: &str) -> bool {
    let normalized = host.trim().trim_matches('[').trim_matches(']');
    if normalized.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match normalized.parse::<IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => false,
    }
}

pub fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}
