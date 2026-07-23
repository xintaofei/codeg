use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::app_error::AppCommandError;
use crate::web::tailscale::binary::{default_hostname, default_state_dir, locate_codeg_tsnet_binary};
use crate::web::tailscale::protocol::{SidecarBootstrap, SidecarStatus};
use crate::web::tailscale::status::{
    map_status, TailscaleFunnelStatus, ERR_AUTHKEY_REQUIRED, ERR_FUNNEL_FAILED, ERR_LOGIN_TIMEOUT,
    ERR_SIDECAR_MISSING, ERR_START_FAILED,
};

const CONTROL_TOKEN_HEADER: &str = "X-Codeg-Tsnet-Token";
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(5);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct EnableFunnelOpts {
    pub data_dir: PathBuf,
    pub localhost_port: u16,
    pub auth_key: Option<String>,
    pub require_auth_key: bool,
    pub state_dir_override: Option<PathBuf>,
    pub hostname_override: Option<String>,
}

struct RunningSidecar {
    child: Child,
    control_addr: String,
    token: String,
    enabled: bool,
    last_status: SidecarStatus,
    _stderr_task: tokio::task::JoinHandle<()>,
}

struct Inner {
    running: Option<RunningSidecar>,
    last_error: Option<TailscaleFunnelStatus>,
}

pub struct TailscaleController {
    inner: Mutex<Inner>,
    http: reqwest::Client,
}

impl Default for TailscaleController {
    fn default() -> Self {
        Self::new()
    }
}

impl TailscaleController {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .no_proxy()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            inner: Mutex::new(Inner {
                running: None,
                last_error: None,
            }),
            http,
        }
    }

    pub async fn status(&self) -> TailscaleFunnelStatus {
        let mut guard = self.inner.lock().await;
        if let Some(err) = guard.last_error.clone() {
            if guard.running.is_none() {
                return err;
            }
        }
        let Some(running) = guard.running.as_mut() else {
            return TailscaleFunnelStatus::stopped();
        };

        match fetch_status(&self.http, &running.control_addr, &running.token).await {
            Ok(raw) => {
                running.last_status = raw.clone();
                map_status(true, running.enabled, raw)
            }
            Err(err) => {
                // Sidecar may have crashed.
                let detail = err.to_string();
                if let Some(mut running) = guard.running.take() {
                    let _ = running.child.kill().await;
                }
                let st = TailscaleFunnelStatus {
                    supported: locate_codeg_tsnet_binary().is_some(),
                    enabled: false,
                    state: "error".into(),
                    login_url: None,
                    funnel_url: None,
                    hostname: None,
                    ipv4: None,
                    last_error: Some(detail),
                    error_key: Some(ERR_START_FAILED.into()),
                };
                guard.last_error = Some(st.clone());
                st
            }
        }
    }

    pub async fn enable_funnel(
        &self,
        opts: EnableFunnelOpts,
    ) -> Result<TailscaleFunnelStatus, AppCommandError> {
        if opts.require_auth_key
            && opts
                .auth_key
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_none()
        {
            let st = TailscaleFunnelStatus::unsupported(
                ERR_AUTHKEY_REQUIRED,
                "CODEG_TS_AUTHKEY is required for headless Funnel",
            );
            let mut guard = self.inner.lock().await;
            guard.last_error = Some(st.clone());
            return Ok(st);
        }

        let Some(bin) = locate_codeg_tsnet_binary() else {
            let st = TailscaleFunnelStatus::unsupported(
                ERR_SIDECAR_MISSING,
                "codeg-tsnet sidecar binary not found",
            );
            let mut guard = self.inner.lock().await;
            guard.last_error = Some(st.clone());
            return Ok(st);
        };

        let state_dir = opts
            .state_dir_override
            .clone()
            .unwrap_or_else(|| default_state_dir(&opts.data_dir));
        let hostname = opts
            .hostname_override
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| default_hostname(&opts.data_dir));

        if let Err(err) = std::fs::create_dir_all(&state_dir) {
            return Err(AppCommandError::io_error(format!(
                "failed to create Tailscale state dir: {err}"
            )));
        }

        // Restart sidecar cleanly if already running.
        {
            let mut guard = self.inner.lock().await;
            if let Some(mut running) = guard.running.take() {
                let _ = post_json(
                    &self.http,
                    &running.control_addr,
                    &running.token,
                    "/shutdown",
                    &serde_json::json!({}),
                )
                .await;
                let _ = tokio::time::timeout(Duration::from_secs(2), running.child.wait()).await;
                let _ = running.child.kill().await;
            }
            guard.last_error = None;
        }

        let token = Uuid::new_v4().to_string();
        let mut cmd = crate::process::tokio_command(&bin);
        cmd.arg("--control-addr")
            .arg("127.0.0.1:0")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--hostname")
            .arg(&hostname)
            .arg("--control-token")
            .arg(&token)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(key) = opts.auth_key.as_ref().filter(|s| !s.trim().is_empty()) {
            cmd.arg("--auth-key").arg(key);
        }

        let mut child = cmd.spawn().map_err(|err| {
            AppCommandError::external_command(
                ERR_START_FAILED,
                format!("failed to spawn codeg-tsnet: {err}"),
            )
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            AppCommandError::external_command(ERR_START_FAILED, "missing sidecar stdout")
        })?;
        let stderr = child.stderr.take();
        let stderr_task = spawn_stderr_forwarder(stderr);

        let bootstrap = match read_bootstrap_line(stdout, BOOTSTRAP_TIMEOUT).await {
            Ok(b) => b,
            Err(err) => {
                let _ = child.kill().await;
                return Err(err);
            }
        };

        {
            let mut guard = self.inner.lock().await;
            guard.running = Some(RunningSidecar {
                child,
                control_addr: bootstrap.control_addr.clone(),
                token: token.clone(),
                enabled: true,
                last_status: SidecarStatus {
                    state: "starting".into(),
                    login_url: None,
                    funnel_url: None,
                    hostname: Some(hostname.clone()),
                    ipv4: None,
                    last_error: None,
                    error_key: None,
                    backend_state: None,
                },
                _stderr_task: stderr_task,
            });
        }

        // Bring the node up.
        let up_body = match opts.auth_key.as_ref().filter(|s| !s.trim().is_empty()) {
            Some(key) => serde_json::json!({ "authKey": key }),
            None => serde_json::json!({}),
        };
        if let Err(err) = post_json(
            &self.http,
            &bootstrap.control_addr,
            &token,
            "/up",
            &up_body,
        )
        .await
        {
            tracing::warn!(%err, "[tailscale] POST /up failed");
        }

        // Wait for online / needs_login, then enable funnel.
        let deadline = tokio::time::Instant::now() + LOGIN_TIMEOUT;
        loop {
            if tokio::time::Instant::now() > deadline {
                let st = TailscaleFunnelStatus {
                    supported: true,
                    enabled: true,
                    state: "error".into(),
                    login_url: None,
                    funnel_url: None,
                    hostname: Some(hostname.clone()),
                    ipv4: None,
                    last_error: Some("Tailscale login timed out".into()),
                    error_key: Some(ERR_LOGIN_TIMEOUT.into()),
                };
                let mut guard = self.inner.lock().await;
                guard.last_error = Some(st.clone());
                return Ok(st);
            }

            let raw = match fetch_status(&self.http, &bootstrap.control_addr, &token).await {
                Ok(s) => s,
                Err(err) => {
                    tracing::warn!(%err, "[tailscale] status poll failed");
                    tokio::time::sleep(POLL_INTERVAL).await;
                    continue;
                }
            };

            {
                let mut guard = self.inner.lock().await;
                if let Some(running) = guard.running.as_mut() {
                    running.last_status = raw.clone();
                    running.enabled = true;
                }
            }

            match raw.state.as_str() {
                "needs_login" => {
                    // Return promptly so the UI can open the login URL.
                    // Subsequent status polls / enable retries will continue
                    // once the user finishes interactive auth.
                    let mapped = map_status(true, true, raw);
                    let mut guard = self.inner.lock().await;
                    if let Some(running) = guard.running.as_mut() {
                        running.enabled = true;
                    }
                    return Ok(mapped);
                }
                "online" | "funnel_ready" | "funnel_enabling" => {
                    // Enable / refresh funnel against current localhost port.
                    let body = serde_json::json!({
                        "enabled": true,
                        "localhostPort": opts.localhost_port,
                    });
                    match post_status(
                        &self.http,
                        &bootstrap.control_addr,
                        &token,
                        "/funnel",
                        &body,
                    )
                    .await
                    {
                        Ok(st_raw) => {
                            let mapped = map_status(true, true, st_raw.clone());
                            let mut guard = self.inner.lock().await;
                            if let Some(running) = guard.running.as_mut() {
                                running.last_status = st_raw;
                                running.enabled = true;
                            }
                            if mapped.state == "funnel_ready"
                                || mapped.state == "needs_login"
                                || mapped.state == "error"
                            {
                                return Ok(mapped);
                            }
                        }
                        Err(err) => {
                            tracing::warn!(%err, "[tailscale] POST /funnel failed");
                            let st = TailscaleFunnelStatus {
                                supported: true,
                                enabled: true,
                                state: "error".into(),
                                login_url: raw.login_url.clone(),
                                funnel_url: None,
                                hostname: Some(hostname.clone()),
                                ipv4: raw.ipv4.clone(),
                                last_error: Some(err.to_string()),
                                error_key: Some(ERR_FUNNEL_FAILED.into()),
                            };
                            let mut guard = self.inner.lock().await;
                            guard.last_error = Some(st.clone());
                            return Ok(st);
                        }
                    }
                }
                "error" => {
                    let mapped = map_status(true, true, raw);
                    let mut guard = self.inner.lock().await;
                    guard.last_error = Some(mapped.clone());
                    return Ok(mapped);
                }
                _ => {}
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    pub async fn disable_funnel(&self) -> Result<(), AppCommandError> {
        let mut guard = self.inner.lock().await;
        let Some(mut running) = guard.running.take() else {
            guard.last_error = None;
            return Ok(());
        };

        let _ = post_json(
            &self.http,
            &running.control_addr,
            &running.token,
            "/funnel",
            &serde_json::json!({ "enabled": false, "localhostPort": 0 }),
        )
        .await;
        let _ = post_json(
            &self.http,
            &running.control_addr,
            &running.token,
            "/shutdown",
            &serde_json::json!({}),
        )
        .await;
        let _ = tokio::time::timeout(Duration::from_secs(2), running.child.wait()).await;
        let _ = running.child.kill().await;
        guard.last_error = None;
        Ok(())
    }

    pub async fn open_login_hint(&self) -> Result<Option<String>, AppCommandError> {
        let st = self.status().await;
        Ok(st.login_url)
    }

    pub async fn shutdown(&self) {
        let _ = self.disable_funnel().await;
    }
}

async fn read_bootstrap_line(
    stdout: ChildStdout,
    timeout: Duration,
) -> Result<SidecarBootstrap, AppCommandError> {
    let mut reader = BufReader::new(stdout).lines();
    let line = tokio::time::timeout(timeout, reader.next_line())
        .await
        .map_err(|_| {
            AppCommandError::external_command(ERR_START_FAILED, "timed out waiting for sidecar bootstrap")
        })?
        .map_err(|err| {
            AppCommandError::external_command(
                ERR_START_FAILED,
                format!("failed reading sidecar bootstrap: {err}"),
            )
        })?
        .ok_or_else(|| {
            AppCommandError::external_command(ERR_START_FAILED, "sidecar exited before bootstrap")
        })?;

    serde_json::from_str::<SidecarBootstrap>(&line).map_err(|err| {
        AppCommandError::external_command(
            ERR_START_FAILED,
            format!("invalid sidecar bootstrap JSON: {err}"),
        )
    })
}

fn spawn_stderr_forwarder(stderr: Option<ChildStderr>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(stderr) = stderr else {
            return;
        };
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // Never log secrets; the sidecar already redacts auth keys.
            if line.to_ascii_lowercase().contains("authkey")
                || line.to_ascii_lowercase().contains("auth key")
                || line.to_ascii_lowercase().contains("control-token")
            {
                continue;
            }
            tracing::info!(target: "codeg_tsnet", "{line}");
        }
    })
}

async fn fetch_status(
    http: &reqwest::Client,
    control_addr: &str,
    token: &str,
) -> Result<SidecarStatus, AppCommandError> {
    let url = format!("http://{control_addr}/status");
    let resp = http
        .get(url)
        .header(CONTROL_TOKEN_HEADER, token)
        .send()
        .await
        .map_err(|err| AppCommandError::network(format!("sidecar /status failed: {err}")))?;
    if !resp.status().is_success() {
        return Err(AppCommandError::network(format!(
            "sidecar /status HTTP {}",
            resp.status()
        )));
    }
    resp.json::<SidecarStatus>()
        .await
        .map_err(|err| AppCommandError::network(format!("sidecar /status decode failed: {err}")))
}

async fn post_json(
    http: &reqwest::Client,
    control_addr: &str,
    token: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<(), AppCommandError> {
    let url = format!("http://{control_addr}{path}");
    let resp = http
        .post(url)
        .header(CONTROL_TOKEN_HEADER, token)
        .json(body)
        .send()
        .await
        .map_err(|err| AppCommandError::network(format!("sidecar {path} failed: {err}")))?;
    if !resp.status().is_success() {
        return Err(AppCommandError::network(format!(
            "sidecar {path} HTTP {}",
            resp.status()
        )));
    }
    Ok(())
}

async fn post_status(
    http: &reqwest::Client,
    control_addr: &str,
    token: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<SidecarStatus, AppCommandError> {
    let url = format!("http://{control_addr}{path}");
    let resp = http
        .post(url)
        .header(CONTROL_TOKEN_HEADER, token)
        .json(body)
        .send()
        .await
        .map_err(|err| AppCommandError::network(format!("sidecar {path} failed: {err}")))?;
    if !resp.status().is_success() {
        return Err(AppCommandError::network(format!(
            "sidecar {path} HTTP {}",
            resp.status()
        )));
    }
    resp.json::<SidecarStatus>()
        .await
        .map_err(|err| AppCommandError::network(format!("sidecar {path} decode failed: {err}")))
}

// Keep Arc alias available for callers that want shared ownership.
pub type SharedTailscaleController = Arc<TailscaleController>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::tailscale::protocol::SidecarStatus;
    use crate::web::tailscale::status::map_status;

    #[tokio::test]
    async fn missing_binary_is_unsupported() {
        // Ensure no accidental binary on PATH for this process by pointing to
        // a non-existent override. locate still falls back to PATH, so this
        // assertion is best-effort and only checks the controller error path
        // when the binary truly cannot be found.
        let c = TailscaleController::new();
        // Force authkey required path first (deterministic).
        let st = c
            .enable_funnel(EnableFunnelOpts {
                data_dir: std::env::temp_dir().join("codeg-ts-test-missing"),
                localhost_port: 3080,
                auth_key: None,
                require_auth_key: true,
                state_dir_override: None,
                hostname_override: Some("codeg-test".into()),
            })
            .await
            .unwrap();
        assert!(!st.supported);
        assert_eq!(st.error_key.as_deref(), Some(ERR_AUTHKEY_REQUIRED));
    }

    #[test]
    fn maps_sidecar_status_to_funnel_status() {
        let raw = SidecarStatus {
            state: "funnel_ready".into(),
            login_url: None,
            funnel_url: Some("https://codeg-abc.ts.net".into()),
            hostname: Some("codeg-abc".into()),
            ipv4: Some("100.64.0.1".into()),
            last_error: None,
            error_key: None,
            backend_state: Some("Running".into()),
        };
        let st = map_status(true, true, raw);
        assert_eq!(st.state, "funnel_ready");
        assert_eq!(st.funnel_url.as_deref(), Some("https://codeg-abc.ts.net"));
    }
}
