//! Bundled userspace Tailscale (`codeg-tsnet`). No Tailscale app on the PC.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::time::timeout;

use super::funnel::{funnel_target, FunnelStatus};
use crate::app_error::{AppCommandError, AppErrorCode};

const START_DEADLINE: Duration = Duration::from_secs(180);

#[derive(Debug, Clone)]
struct SidecarState {
    public: bool,
    url: Option<String>,
    login_url: Option<String>,
    target: String,
}

struct SidecarProc {
    child: Child,
    state: SidecarState,
}

static SIDECAR: Mutex<Option<SidecarProc>> = Mutex::new(None);

#[derive(Debug, Deserialize)]
struct SidecarEvent {
    event: String,
    url: Option<String>,
    #[allow(dead_code)]
    mode: Option<String>,
    message: Option<String>,
}

pub fn locate_codeg_tsnet() -> Option<PathBuf> {
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
    which::which(filename).ok().filter(|p| is_executable_file(p))
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file() && std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false)
}

fn state_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("app.codeg")
        .join("tsnet")
}

pub fn sidecar_status(want_public: bool) -> Option<FunnelStatus> {
    let guard = SIDECAR.lock().ok()?;
    let proc = guard.as_ref()?;
    if proc.state.public != want_public {
        return Some(FunnelStatus {
            enabled: false,
            url: None,
            target: None,
            login_url: None,
            unavailable_reason: None,
        });
    }
    Some(FunnelStatus {
        enabled: proc.state.url.is_some() || proc.state.login_url.is_some(),
        url: proc.state.url.clone(),
        target: Some(proc.state.target.clone()),
        login_url: proc.state.login_url.clone(),
        unavailable_reason: None,
    })
}

pub async fn sidecar_enable(port: u16, public: bool) -> Result<FunnelStatus, AppCommandError> {
    sidecar_disable().await;
    let binary = locate_codeg_tsnet().ok_or_else(|| {
        AppCommandError::new(
            AppErrorCode::DependencyMissing,
            "Codeg private networking is not in this build. Rebuild Codeg (it bundles Tailscale).",
        )
    })?;
    let target = funnel_target(port);
    let mut cmd = crate::process::tokio_command(&binary);
    cmd.arg("--target")
        .arg(&target)
        .arg("--hostname")
        .arg("codeg")
        .arg("--state-dir")
        .arg(state_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if public {
        cmd.arg("--public");
    }
    let mut child = cmd.spawn().map_err(|err| {
        AppCommandError::new(
            AppErrorCode::ExternalCommandFailed,
            "Could not start Codeg private networking",
        )
        .with_detail(err.to_string())
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        AppCommandError::new(AppErrorCode::ExternalCommandFailed, "sidecar stdout missing")
    })?;

    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let mut login_url = None;
    let mut url = None;
    let started = timeout(START_DEADLINE, async {
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await.map_err(|err| {
                AppCommandError::new(AppErrorCode::ExternalCommandFailed, "sidecar read")
                    .with_detail(err.to_string())
            })?;
            if n == 0 {
                return Err(AppCommandError::new(
                    AppErrorCode::ExternalCommandFailed,
                    "Codeg private networking exited before it was ready",
                ));
            }
            let Ok(event) = serde_json::from_str::<SidecarEvent>(line.trim()) else {
                continue;
            };
            match event.event.as_str() {
                "auth_url" => {
                    login_url = event.url;
                    if url.is_some() {
                        return Ok(());
                    }
                }
                "ready" => {
                    url = event.url;
                    return Ok(());
                }
                "error" => {
                    return Err(AppCommandError::new(
                        AppErrorCode::ExternalCommandFailed,
                        event.message.unwrap_or_else(|| "sidecar failed".into()),
                    ));
                }
                _ => {}
            }
        }
    })
    .await;

    match started {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            let _ = child.kill().await;
            return Err(err);
        }
        Err(_) => {
            let _ = child.kill().await;
            return Err(AppCommandError::new(
                AppErrorCode::ExternalCommandFailed,
                "Timed out waiting for Codeg private networking",
            ));
        }
    }

    let status = FunnelStatus {
        enabled: url.is_some() || login_url.is_some(),
        url: url.clone(),
        target: Some(target.clone()),
        login_url: login_url.clone(),
        unavailable_reason: None,
    };
    *SIDECAR.lock().unwrap() = Some(SidecarProc {
        child,
        state: SidecarState {
            public,
            url,
            login_url,
            target,
        },
    });
    Ok(status)
}

pub async fn sidecar_disable() {
    let child = SIDECAR.lock().ok().and_then(|mut g| g.take());
    if let Some(mut proc) = child {
        let _ = proc.child.kill().await;
        let _ = proc.child.wait().await;
    }
}

pub fn sidecar_available() -> bool {
    locate_codeg_tsnet().is_some()
}
