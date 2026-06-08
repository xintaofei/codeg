//! Runtime self-knowledge for the in-place updater: where our own
//! executable lives, whether we run under our supervisor, and how the
//! restart will be carried out.
//!
//! All of this is server/CLI-only — the desktop (Tauri) build never
//! reaches this module; it self-updates through `tauri-plugin-updater`.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Exit code the worker uses to ask the supervisor for an upgrade restart.
/// Distinct from a crash so the supervisor can apply the configured
/// relaunch delay deterministically. `86` is otherwise unused by us.
pub const EXIT_RESTART: i32 = 86;

/// Set to `1` by the supervisor on the worker it spawns.
pub const ENV_SUPERVISED: &str = "CODEG_SUPERVISED";
/// Relaunch delay (milliseconds) the supervisor waits before respawning a
/// worker that exited with [`EXIT_RESTART`]. The worker reports the same
/// value to the frontend so its countdown matches reality.
pub const ENV_RESTART_DELAY_MS: &str = "CODEG_RESTART_DELAY_MS";
/// Deployment marker baked into the Docker image (`docker`). Only used for
/// user-facing messaging ("permanent across recreation needs a pull").
pub const ENV_RUNTIME: &str = "CODEG_RUNTIME";

/// Default relaunch delay when `CODEG_RESTART_DELAY_MS` is unset.
pub const DEFAULT_RESTART_DELAY_MS: u64 = 2000;

/// Grace window (seconds) the supervisor gives a freshly-upgraded worker to
/// prove it can stay up. A worker spawned *after* an upgrade that exits
/// abnormally within this window is treated as a failed boot and auto-rolled
/// back to the previous version; a crash after it has run longer is treated
/// as an ordinary runtime fault and propagated (the new version had already
/// demonstrated it can start).
pub const ENV_UPGRADE_TRIAL_SECS: &str = "CODEG_UPGRADE_TRIAL_SECS";

/// Default trial window when `CODEG_UPGRADE_TRIAL_SECS` is unset. A binary
/// that cannot boot fails near-instantly; this is generous enough to never
/// misfire on a server that is genuinely (if slowly) coming up.
pub const DEFAULT_UPGRADE_TRIAL_SECS: u64 = 30;

/// How the running server applies an in-place update + restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateCapability {
    /// Worker runs under our supervisor (PID 1 in Docker). Upgrade = swap
    /// files on disk, then `exit(EXIT_RESTART)`; the supervisor relaunches.
    Supervised,
    /// Standalone, no supervisor. Upgrade = swap files, then re-exec self.
    Reexec,
}

/// True when this process was spawned by our `--supervise` parent.
pub fn is_supervised() -> bool {
    std::env::var(ENV_SUPERVISED)
        .map(|v| v == "1")
        .unwrap_or(false)
}

pub fn capability() -> UpdateCapability {
    if is_supervised() {
        UpdateCapability::Supervised
    } else {
        UpdateCapability::Reexec
    }
}

/// Best-effort container detection. Explicit env marker first (we set it in
/// the image), then the `/.dockerenv` sentinel as a fallback.
pub fn is_docker() -> bool {
    if std::env::var(ENV_RUNTIME)
        .map(|v| v.eq_ignore_ascii_case("docker"))
        .unwrap_or(false)
    {
        return true;
    }
    std::path::Path::new("/.dockerenv").exists()
}

/// `"docker"` or `"standalone"` — only drives frontend messaging.
pub fn runtime_label() -> &'static str {
    if is_docker() {
        "docker"
    } else {
        "standalone"
    }
}

/// Relaunch delay in milliseconds, read from the environment with a sane
/// default. Clamped to a minimum so the frontend countdown never lands on
/// zero and starts polling before the process is even gone.
pub fn restart_delay_ms() -> u64 {
    std::env::var(ENV_RESTART_DELAY_MS)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_RESTART_DELAY_MS)
        .max(200)
}

/// Upgrade trial window in seconds, read from the environment with a sane
/// default. See [`ENV_UPGRADE_TRIAL_SECS`].
pub fn upgrade_trial_secs() -> u64 {
    std::env::var(ENV_UPGRADE_TRIAL_SECS)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_UPGRADE_TRIAL_SECS)
        .max(1)
}

/// Absolute path of our own executable, captured once at startup so it
/// survives the in-place swap (after which `current_exe()` may resolve to a
/// `" (deleted)"` path on Linux). [`prime_self_exe`] must be called before
/// the first update swaps the binary.
static SELF_EXE: OnceLock<PathBuf> = OnceLock::new();

/// Capture and cache the executable path. Call once, early in `main`,
/// before any thread or update can rename the binary.
pub fn prime_self_exe() {
    let _ = self_exe();
}

pub fn self_exe() -> PathBuf {
    SELF_EXE
        .get_or_init(|| {
            std::env::current_exe().unwrap_or_else(|_| {
                // Last-resort fallback: resolve via PATH at exec time.
                PathBuf::from(if cfg!(windows) {
                    "codeg-server.exe"
                } else {
                    "codeg-server"
                })
            })
        })
        .clone()
}
