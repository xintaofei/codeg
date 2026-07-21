//! Subscriber construction and per-binary initialization.
//!
//! Initialization is split into phases because the subscriber must be live as
//! early as possible (to capture startup diagnostics) but the persisted level
//! needs the database and the live-tail emitter needs `AppState`:
//!
//! 1. **Phase 1** — [`init_desktop`] / [`init_server`] / [`init_mcp`], called
//!    as the first statement in each binary's entry point. Resolves the logs
//!    dir from env (pure, no DB), installs the subscriber + [`crate::logging::
//!    hub::LogHub`], and returns a [`LogGuard`] the caller holds for the
//!    process lifetime so buffered file lines flush on a graceful exit.
//! 2. **Phase 2** — [`apply_persisted_level`], once the DB is open: overrides
//!    the default level from the `logging.level` KV value (unless `CODEG_LOG` /
//!    `RUST_LOG` is set, which wins).
//! 3. **Phase 3** — `LogHub::set_emitter`, once `AppState` exists: starts
//!    `logs://appended` delivery to the viewer.

use std::path::Path;

use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::Rotation;
use tracing_subscriber::{fmt, prelude::*, reload, EnvFilter, Registry};

use crate::logging::hub::LogHub;
use crate::logging::layer::BufferEmitLayer;
use crate::logging::{LogLevel, LogSettings, LOGGING_LEVEL_KEY};

/// Reload handle stored in [`LogHub`]. The wrapped `EnvFilter` is applied as a
/// global filter on the registry, so swapping it changes the level for all
/// three sinks (stderr, file, buffer) at once. The second type parameter is
/// `Registry` because the reload layer is the first layer added — boxing later
/// layers is therefore unnecessary.
pub type ReloadHandle = reload::Handle<EnvFilter, Registry>;

/// Holds the non-blocking appender's `WorkerGuard`. Bind it in `main` (or the
/// desktop `run()`); dropping it flushes and shuts down the writer thread.
#[must_use = "hold the guard for the process lifetime so buffered logs flush on exit"]
pub struct LogGuard {
    _guard: Option<WorkerGuard>,
}

/// Standing per-target backstops appended to EVERY constructed filter — the
/// default/configured level, a persisted level, AND an explicit `RUST_LOG` /
/// `CODEG_LOG` override. Two entries, both appended last so they win over a
/// broader global level:
///
/// - `kill_tree=warn` — the `kill_tree` crate emits one DEBUG line per inspected
///   PID (~1500 on macOS, mostly EPERM on SIP-protected processes) on every
///   `kill_tree()` call. Under a global `debug` level that firehose grew a
///   server's log file to 217GB. We never want it: real kill failures are
///   already surfaced at our own call sites via `error!`. Pinned here so even
///   `RUST_LOG=debug` can't re-open it.
/// - `codeg_lib::logging=off` — the logging stack must not log about itself
///   (cross-thread feedback-loop backstop; the layer's thread-local guard
///   handles the same-thread case).
const TARGET_BACKSTOPS: &str = "kill_tree=warn,codeg_lib::logging=off";

/// Directive string for an explicit env-override level `s`, with the standing
/// [`TARGET_BACKSTOPS`] appended. Extracted so the backstop application on the
/// env-override path is unit-testable without installing a global subscriber.
fn env_override_directives(s: &str) -> String {
    format!("{s},{TARGET_BACKSTOPS}")
}

/// Build the `EnvFilter` for the full settings: the global level followed by
/// each non-empty per-target override (`target=level`), then always appending
/// [`TARGET_BACKSTOPS`] (the `kill_tree` clamp + the logging module's own target
/// off). The backstops are appended last so they win. `parse_lossy` silently
/// drops any malformed target directive — the UI constrains the input to avoid
/// that.
pub fn build_env_filter(settings: &LogSettings) -> EnvFilter {
    let mut directives = settings.level.directive().to_string();
    for t in &settings.targets {
        // Validate server-side (don't trust the UI): only well-formed module
        // targets are interpolated, and the logging module can never be
        // overridden — it must stay `off` (the backstop appended below).
        let target = t.target.trim();
        if is_valid_target(target) {
            directives.push_str(&format!(",{}={}", target, t.level.directive()));
        }
    }
    directives.push(',');
    directives.push_str(TARGET_BACKSTOPS);
    EnvFilter::builder().parse_lossy(directives)
}

/// A well-formed tracing target: `ident(::ident)*`, `ident = [A-Za-z0-9_]+`
/// (mirrors the frontend grammar). Also rejects the logging module's own target
/// (and submodules) so a persisted or client-supplied override can't defeat the
/// recursion backstop.
fn is_valid_target(target: &str) -> bool {
    if target.is_empty()
        || target == "codeg_lib::logging"
        || target.starts_with("codeg_lib::logging::")
    {
        return false;
    }
    target.split("::").all(|seg| {
        !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}

/// Build a reload handle not attached to any installed subscriber, for tests
/// that construct a detached [`crate::logging::hub::LogHub`] without touching
/// the process-global subscriber.
#[cfg(any(test, feature = "test-utils"))]
pub fn detached_reload_handle() -> ReloadHandle {
    let (_layer, handle): (reload::Layer<EnvFilter, Registry>, ReloadHandle) =
        reload::Layer::new(build_env_filter(&LogSettings::default()));
    handle
}

/// Retention for rotated daily files. A disk-bound necessity (daily files would
/// otherwise accumulate forever), not a functional cap; generous default,
/// overridable via `CODEG_LOG_MAX_FILES`.
fn file_retention() -> usize {
    std::env::var("CODEG_LOG_MAX_FILES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(30)
}

/// The env-override directive, or `None`: the first non-empty (trimmed) value of
/// `CODEG_LOG` then `RUST_LOG`. Single source of truth for env-level precedence,
/// so the startup filter and the env-lock checks can't diverge — e.g. an empty
/// `CODEG_LOG` must not mask a valid `RUST_LOG` (a `var().or_else(var)` would
/// treat the empty `Ok("")` as present and skip `RUST_LOG`).
fn env_level_override() -> Option<String> {
    ["CODEG_LOG", "RUST_LOG"].iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

/// Whether an explicit `CODEG_LOG` / `RUST_LOG` is set (and non-empty). When
/// true the env var owns the live level: startup skips the persisted level and
/// the Settings UI locks the control (so it can't silently diverge).
pub fn env_level_is_set() -> bool {
    env_level_override().is_some()
}

/// Create the rotating file writer, returning `None` (degrading to stderr +
/// buffer only) if the directory or appender can't be initialized. Logging init
/// must never crash the app.
fn init_file_writer(dir: &Path, prefix: &str) -> Option<(NonBlocking, WorkerGuard)> {
    // Pre-subscriber bootstrap: the subscriber isn't installed yet, so these
    // diagnostics legitimately go straight to stderr rather than via tracing.
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("[logging] could not create log dir {}: {e}", dir.display());
        return None;
    }
    let appender = match tracing_appender::rolling::Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix(prefix)
        .filename_suffix("log")
        .max_log_files(file_retention())
        .build(dir)
    {
        Ok(appender) => appender,
        Err(e) => {
            eprintln!(
                "[logging] could not init log appender in {}: {e}",
                dir.display()
            );
            return None;
        }
    };
    Some(tracing_appender::non_blocking(appender))
}

/// Build and install the subscriber. `file_dir` is `None` for `codeg-mcp`
/// (stderr only). Returns the reload handle and the appender guard.
fn build_subscriber(
    initial: LogLevel,
    file_dir: Option<&Path>,
    file_prefix: &str,
) -> (ReloadHandle, Option<WorkerGuard>) {
    // Explicit env wins at startup; otherwise the passed-in default. Phase 2
    // (apply_persisted_level) later overrides the default from the DB. Uses the
    // same `env_level_override` precedence as `env_level_is_set`, so a level
    // applied here is exactly the one the UI reports as env-locked.
    let initial_filter = match env_level_override() {
        // Even an explicit env override gets the standing target backstops, so
        // `RUST_LOG=debug` still can't re-open the kill_tree per-PID firehose.
        Some(s) => EnvFilter::builder().parse_lossy(env_override_directives(&s)),
        // Phase 1 has no DB yet, so no persisted per-target overrides; just the
        // default level. Phase 2 (apply_persisted_level) rebuilds with targets.
        None => build_env_filter(&LogSettings {
            level: initial,
            targets: Vec::new(),
        }),
    };
    let (filter_layer, reload_handle) = reload::Layer::new(initial_filter);

    // stderr fmt layer — MUST target stderr (not the default stdout): in
    // codeg-mcp stdout is the JSON-RPC channel, and elsewhere the migrated
    // eprintln! all went to stderr.
    let file = file_dir.and_then(|dir| init_file_writer(dir, file_prefix));

    // The reloadable EnvFilter is added first, so it acts as a global filter
    // over every sibling layer; its reload handle's subscriber type is
    // therefore `Registry`. Building the optional file layer inline (per match
    // arm) lets the compiler infer each layer's subscriber type without boxing.
    let guard = match file {
        Some((non_blocking, guard)) => {
            Registry::default()
                .with(filter_layer)
                .with(fmt::layer().with_writer(std::io::stderr))
                .with(BufferEmitLayer)
                .with(fmt::layer().json().with_writer(non_blocking))
                .init();
            Some(guard)
        }
        None => {
            Registry::default()
                .with(filter_layer)
                .with(fmt::layer().with_writer(std::io::stderr))
                .with(BufferEmitLayer)
                .init();
            None
        }
    };

    (reload_handle, guard)
}

/// Phase 1 for the desktop `codeg` binary: stderr + file (`codeg.<date>.log`) +
/// buffer. Default level is env-or-`info`; [`apply_persisted_level`] refines it
/// once the DB is open.
pub fn init_desktop() -> LogGuard {
    init_with_file("codeg")
}

/// Phase 1 for `codeg-server`: stderr + file (`codeg-server.<date>.log`) +
/// buffer.
pub fn init_server() -> LogGuard {
    init_with_file("codeg-server")
}

fn init_with_file(prefix: &str) -> LogGuard {
    let dir = crate::paths::codeg_logs_root();
    let (reload, guard) = build_subscriber(LogLevel::default(), Some(&dir), prefix);
    LogHub::install(reload);
    LogGuard { _guard: guard }
}

/// Install a **stderr-only** subscriber (no file / buffer / hub / emitter) for
/// process modes that run before — or instead of — the full logging stack:
/// `codeg-mcp`, the `--supervise` supervisor, and the `--credential-helper`
/// subprocess. Without an installed subscriber, `tracing` calls on those paths
/// are silently dropped (unlike the old `eprintln!`, which always hit stderr).
///
/// stderr (not stdout) is mandatory: in `codeg-mcp` and the credential helper,
/// stdout is a protocol channel. No file appender: those modes are short-lived
/// or multi-process and must not clobber a shared rolling file. No hub: nothing
/// to buffer/emit, so `BufferEmitLayer` short-circuits.
pub fn init_stderr_only() -> LogGuard {
    let (_reload, guard) = build_subscriber(LogLevel::default(), None, "");
    LogGuard { _guard: guard }
}

/// Phase 1 for `codeg-mcp`. See [`init_stderr_only`].
pub fn init_mcp() -> LogGuard {
    init_stderr_only()
}

/// Phase 2: override the default level from the persisted `logging.level` KV
/// value, unless an explicit `CODEG_LOG` / `RUST_LOG` is set (env wins). No-op
/// when no hub is installed (mcp) or the value is absent/unparseable.
pub async fn apply_persisted_level(conn: &sea_orm::DatabaseConnection) {
    if env_level_is_set() {
        return;
    }
    let Some(hub) = crate::logging::hub::log_hub() else {
        return;
    };
    if let Ok(Some(raw)) =
        crate::db::service::app_metadata_service::get_value(conn, LOGGING_LEVEL_KEY).await
    {
        if let Ok(settings) = serde_json::from_str::<LogSettings>(&raw) {
            hub.apply_settings(&settings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::TargetDirective;

    #[test]
    fn build_env_filter_includes_per_target_directives() {
        let settings = LogSettings {
            level: LogLevel::Info,
            targets: vec![
                TargetDirective {
                    target: "codeg_lib::acp".into(),
                    level: LogLevel::Debug,
                },
                // Whitespace-only target is skipped, not emitted as a bare "=trace".
                TargetDirective {
                    target: "  ".into(),
                    level: LogLevel::Trace,
                },
                TargetDirective {
                    target: "codeg_lib::web".into(),
                    level: LogLevel::Warn,
                },
            ],
        };
        let rendered = build_env_filter(&settings).to_string();
        assert!(
            rendered.contains("codeg_lib::acp=debug"),
            "missing acp override: {rendered}"
        );
        assert!(
            rendered.contains("codeg_lib::web=warn"),
            "missing web override: {rendered}"
        );
        assert!(
            rendered.contains("codeg_lib::logging=off"),
            "missing backstop: {rendered}"
        );
        assert!(
            !rendered.contains("=trace"),
            "empty-target row should be skipped: {rendered}"
        );
    }

    #[test]
    fn build_env_filter_rejects_invalid_and_logging_targets() {
        let settings = LogSettings {
            level: LogLevel::Info,
            targets: vec![
                TargetDirective {
                    target: "bad-target!".into(), // invalid chars → dropped
                    level: LogLevel::Trace,
                },
                TargetDirective {
                    target: "codeg_lib::logging".into(), // backstop override attempt → dropped
                    level: LogLevel::Trace,
                },
                TargetDirective {
                    target: "codeg_lib::acp".into(),
                    level: LogLevel::Debug,
                },
            ],
        };
        let rendered = build_env_filter(&settings).to_string();
        assert!(rendered.contains("codeg_lib::acp=debug"), "{rendered}");
        assert!(!rendered.contains("bad-target"), "{rendered}");
        // The logging module stays off; the override attempt is dropped.
        assert!(rendered.contains("codeg_lib::logging=off"), "{rendered}");
        assert!(!rendered.contains("codeg_lib::logging=trace"), "{rendered}");
    }

    #[test]
    fn build_env_filter_pins_noisy_third_party_targets() {
        // Every constructed filter clamps `kill_tree` to warn — its per-PID
        // DEBUG firehose once grew a server log to 217GB — and keeps the
        // logging module's own target off.
        let rendered = build_env_filter(&LogSettings {
            level: LogLevel::Info,
            targets: Vec::new(),
        })
        .to_string();
        assert!(
            rendered.contains("kill_tree=warn"),
            "kill_tree must be pinned to warn: {rendered}"
        );
        assert!(
            rendered.contains("codeg_lib::logging=off"),
            "logging backstop must remain: {rendered}"
        );
    }

    #[test]
    fn env_override_appends_backstops_so_debug_cannot_reopen_kill_tree() {
        // The explicit RUST_LOG/CODEG_LOG path bypasses build_env_filter, so it
        // must apply the same backstops itself — otherwise `RUST_LOG=debug`
        // re-opens the kill_tree per-PID firehose.
        assert_eq!(
            env_override_directives("debug"),
            "debug,kill_tree=warn,codeg_lib::logging=off"
        );
        // And the parsed filter still carries the clamp under a global debug.
        let rendered = EnvFilter::builder()
            .parse_lossy(env_override_directives("debug"))
            .to_string();
        assert!(
            rendered.contains("kill_tree=warn"),
            "kill_tree clamp must survive a global debug override: {rendered}"
        );
    }

    #[test]
    fn is_valid_target_grammar() {
        assert!(is_valid_target("codeg_lib::acp"));
        assert!(is_valid_target("codeg_lib::acp::delegation"));
        assert!(is_valid_target("a"));
        assert!(!is_valid_target(""));
        assert!(!is_valid_target("bad-target"));
        assert!(!is_valid_target("::leading"));
        assert!(!is_valid_target("trailing::"));
        assert!(!is_valid_target("a:::b"));
        assert!(!is_valid_target("has space"));
        assert!(!is_valid_target("codeg_lib::logging"));
        assert!(!is_valid_target("codeg_lib::logging::hub"));
    }

    #[test]
    fn env_level_override_precedence() {
        // Both unset → no override.
        temp_env::with_vars(
            [("CODEG_LOG", None::<&str>), ("RUST_LOG", None::<&str>)],
            || {
                assert_eq!(env_level_override(), None);
                assert!(!env_level_is_set());
            },
        );
        // Empty CODEG_LOG must NOT mask a valid RUST_LOG (the bug Codex caught).
        temp_env::with_vars(
            [("CODEG_LOG", Some("")), ("RUST_LOG", Some("debug"))],
            || {
                assert_eq!(env_level_override().as_deref(), Some("debug"));
                assert!(env_level_is_set());
            },
        );
        // Both empty / whitespace-only → no override.
        temp_env::with_vars(
            [("CODEG_LOG", Some("  ")), ("RUST_LOG", Some(""))],
            || {
                assert_eq!(env_level_override(), None);
                assert!(!env_level_is_set());
            },
        );
        // CODEG_LOG wins when both are non-empty.
        temp_env::with_vars(
            [("CODEG_LOG", Some("trace")), ("RUST_LOG", Some("debug"))],
            || {
                assert_eq!(env_level_override().as_deref(), Some("trace"));
            },
        );
    }
}
