//! Single-endpoint selection and its runtime state (PR1).
//!
//! PR1 dispatches every request to ONE endpoint: the first enabled, complete
//! entry in the stored provider list. What the rotation pool used to do per
//! member shrinks to one piece of process-wide runtime memory here — the
//! consecutive-failure streak and the cooldown it engages — so a misbehaving
//! endpoint is still benched without the caller hammering it, and a manual
//! "it's fixed" action is still possible.

use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use serde::Serialize;

use crate::app_error::AppCommandError;
use crate::translation::settings::{ProviderConfig, TranslationSettings};

/// No endpoint is callable: the feature is on but nothing enabled and
/// complete exists to receive a request.
pub const ERR_NO_ENDPOINT: &str = "No enabled translation endpoint is configured";

/// Pick the endpoint every request goes to: the FIRST enabled and complete
/// entry in the provider list. The list is user-ordered, so "first" is the
/// user's own priority; incomplete or disabled entries are settings-page
/// drafts and skipped, and an empty list is the unconfigured draft state.
pub fn select_endpoint(settings: &TranslationSettings) -> Result<ProviderConfig, AppCommandError> {
    settings
        .active_providers()
        .into_iter()
        .next()
        .ok_or_else(|| AppCommandError::configuration_missing(ERR_NO_ENDPOINT))
}

/// The single endpoint's runtime state. `disabled` is the session-scoped
/// "keep this endpoint out" switch (cleared by a reset or a restart); the
/// cooldown is the automatic bench after the settings' failure threshold
/// consecutive failures.
#[derive(Debug, Default)]
struct EndpointRuntime {
    consecutive_failures: u32,
    cooldown_until: Option<SystemTime>,
    disabled: bool,
}

static RUNTIME: OnceLock<Mutex<EndpointRuntime>> = OnceLock::new();

fn runtime() -> &'static Mutex<EndpointRuntime> {
    RUNTIME.get_or_init(|| Mutex::new(EndpointRuntime::default()))
}

/// Whether the selected endpoint may receive a request right now. The
/// cooldown is engaged lazily here: an expired window clears itself on the
/// first post-cooldown call instead of needing a timer.
///
/// The settings-page connection test deliberately does NOT go through this —
/// the whole point of the test is to judge the endpoint as it is right now.
pub fn ensure_available() -> Result<(), AppCommandError> {
    let mut runtime = runtime()
        .lock()
        .expect("translation endpoint runtime lock is never poisoned across a panic-free run");
    if runtime.disabled {
        return Err(AppCommandError::configuration_missing(
            "The translation endpoint is disabled for this session — reset it to re-enable",
        ));
    }
    if let Some(until) = runtime.cooldown_until {
        let now = SystemTime::now();
        if now < until {
            let remaining = until
                .duration_since(now)
                .map(|d| d.as_secs().max(1))
                .unwrap_or(1);
            return Err(AppCommandError::network(format!(
                "The translation endpoint is cooling down after {consecutive} consecutive failures — retry in about {remaining}s",
                consecutive = runtime.consecutive_failures
            )));
        }
        runtime.cooldown_until = None;
    }
    Ok(())
}

/// A clean, gate-accepted reply resets the failure streak. Recorded by the
/// client only AFTER the quality gate accepted the reply — a parseable but
/// refused answer is the endpoint failing, not succeeding.
pub fn report_success() {
    let mut runtime = runtime()
        .lock()
        .expect("translation endpoint runtime lock is never poisoned across a panic-free run");
    runtime.consecutive_failures = 0;
}

/// One more endpoint failure. When the streak reaches the settings'
/// consecutive-failure threshold the endpoint parks for the settings'
/// cooldown length — both read per request, so a settings change takes
/// effect on the very next failure.
pub fn report_failure(settings: &TranslationSettings) {
    let mut runtime = runtime()
        .lock()
        .expect("translation endpoint runtime lock is never poisoned across a panic-free run");
    runtime.consecutive_failures = runtime.consecutive_failures.saturating_add(1);
    if runtime.consecutive_failures >= settings.failure_threshold() {
        runtime.cooldown_until =
            Some(SystemTime::now() + std::time::Duration::from_secs(settings.cooldown_seconds()));
        tracing::warn!(
            "[translation] endpoint parked for {}s after {} consecutive failures",
            settings.cooldown_seconds(),
            runtime.consecutive_failures
        );
    }
}

/// The status the settings page's strip renders, for the one endpoint.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointStatus {
    /// Whether any callable endpoint exists in the stored settings at all.
    pub configured: bool,
    /// The selected endpoint's id, when one is configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Failures in a row since the last accepted reply.
    pub consecutive_failures: u32,
    /// Milliseconds left in the automatic cooldown; 0 when none.
    pub cooldown_remaining_ms: u64,
    /// The session-scoped manual disable.
    pub disabled: bool,
}

/// The runtime view of the one endpoint, joined with the stored settings.
pub fn status(settings: &TranslationSettings) -> EndpointStatus {
    let configured = select_endpoint(settings).ok();
    let runtime = runtime()
        .lock()
        .expect("translation endpoint runtime lock is never poisoned across a panic-free run");
    let cooldown_remaining_ms = runtime
        .cooldown_until
        .and_then(|until| {
            until.duration_since(SystemTime::now()).ok()
        })
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    EndpointStatus {
        configured: configured.is_some(),
        provider_id: configured.map(|provider| provider.provider_id()),
        consecutive_failures: runtime.consecutive_failures,
        cooldown_remaining_ms,
        disabled: runtime.disabled,
    }
}

/// The manual "this endpoint is fixed" action: clear the failure streak, the
/// cooldown, and the session disable, so everything starts counting fresh.
/// Runtime memory only — no settings or db involved.
pub fn reset_session() {
    let mut runtime = runtime()
        .lock()
        .expect("translation endpoint runtime lock is never poisoned across a panic-free run");
    *runtime = EndpointRuntime::default();
}

/// The manual "keep this endpoint out of rotation" action, session-scoped: a
/// restart (or a reset) clears it.
pub fn disable_session() {
    let mut runtime = runtime()
        .lock()
        .expect("translation endpoint runtime lock is never poisoned across a panic-free run");
    runtime.disabled = true;
}

/// Park the endpoint for `seconds` without ending the session.
pub fn cooldown_for(seconds: u64) {
    let mut runtime = runtime()
        .lock()
        .expect("translation endpoint runtime lock is never poisoned across a panic-free run");
    runtime.cooldown_until = Some(SystemTime::now() + std::time::Duration::from_secs(seconds));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with(base_url: &str) -> TranslationSettings {
        TranslationSettings {
            enabled: true,
            base_url: base_url.to_string(),
            api_key: "k".to_string(),
            model: "m".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn the_first_enabled_and_complete_entry_wins() {
        let mut settings = settings_with("https://flat.example.com");
        settings.providers = vec![
            ProviderConfig {
                id: "off".to_string(),
                enabled: false,
                base_url: "https://disabled.example.com".to_string(),
                api_key: "k".to_string(),
                model: "m".to_string(),
                ..Default::default()
            },
            ProviderConfig {
                id: "on".to_string(),
                enabled: true,
                base_url: "https://active.example.com".to_string(),
                api_key: "k".to_string(),
                model: "m".to_string(),
                ..Default::default()
            },
            ProviderConfig {
                id: "draft".to_string(),
                enabled: true,
                base_url: String::new(),
                api_key: "k".to_string(),
                model: String::new(),
                ..Default::default()
            },
        ];
        let picked = select_endpoint(&settings).expect("one entry is callable");
        assert_eq!(picked.id, "on");
    }

    #[test]
    fn an_empty_callable_list_is_a_configuration_error() {
        let err = select_endpoint(&TranslationSettings::default())
            .expect_err("nothing configured");
        assert_eq!(err.message, ERR_NO_ENDPOINT);
    }

    #[test]
    fn failures_below_the_threshold_never_bench_the_endpoint() {
        reset_session();
        let settings = settings_with("https://api.example.com");
        for expected in 1..settings.failure_threshold() {
            report_failure(&settings);
            assert!(ensure_available().is_ok(), "failure {expected}");
        }
        reset_session();
    }

    #[test]
    fn the_threshold_failure_engages_the_cooldown() {
        reset_session();
        let settings = TranslationSettings {
            failure_threshold: Some(1),
            cooldown_seconds: Some(120),
            ..settings_with("https://api.example.com")
        };
        report_failure(&settings);
        let err = ensure_available().expect_err("cooled down");
        assert!(err.message.contains("cooling down"), "{err:?}");
        assert!(
            status(&settings).cooldown_remaining_ms > 0,
            "the status strip must see the bench"
        );
        reset_session();
        assert!(ensure_available().is_ok());
    }

    #[test]
    fn a_success_resets_the_streak() {
        reset_session();
        let settings = settings_with("https://api.example.com");
        report_failure(&settings);
        report_success();
        assert_eq!(status(&settings).consecutive_failures, 0);
        reset_session();
    }

    #[test]
    fn the_session_disable_blocks_requests_until_a_reset() {
        reset_session();
        let settings = settings_with("https://api.example.com");
        disable_session();
        let err = ensure_available().expect_err("disabled");
        assert!(err.message.contains("disabled for this session"), "{err:?}");
        assert!(status(&settings).disabled);
        reset_session();
        assert!(ensure_available().is_ok());
    }

    #[test]
    fn a_manual_cooldown_parks_without_touching_the_streak() {
        reset_session();
        let settings = settings_with("https://api.example.com");
        cooldown_for(60);
        assert!(ensure_available().is_err());
        assert_eq!(status(&settings).consecutive_failures, 0);
        reset_session();
    }
}
