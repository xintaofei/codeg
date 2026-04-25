//! Reconnect backoff helpers shared by chat-channel backends.
//!
//! Each backend used to roll its own exponential backoff with subtly
//! different parameters (lark: 2^n; weixin: 5*2^n cap 30s; telegram:
//! flat 5s). They also disagreed on whether HTTP 401/403/429 should
//! reset the loop, keep retrying, or honor `Retry-After`. This module
//! gives them one place to fix bugs and tune behavior.

use std::time::Duration;

/// Standard reconnect schedule used by chat-channel polling and WS loops.
///
/// Returns the delay to wait before the next attempt given the current
/// retry counter (0-based: 0 means "first failure since success").
///
/// Schedule: 1s, 2s, 4s, 8s, 16s, 32s, then capped at 60s. Keeps the
/// channel responsive on transient failures while avoiding hammering an
/// already-down upstream.
pub fn reconnect_delay(retry_count: u32) -> Duration {
    let secs = 1u64.checked_shl(retry_count.min(6)).unwrap_or(64).min(60);
    Duration::from_secs(secs)
}

/// Backoff policy for HTTP responses. Drives whether the channel should
/// stop reconnecting (auth failure), honor a server-provided cooldown,
/// or fall back to the standard exponential schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpRetryDecision {
    /// Auth failed (401/403). Stop reconnecting; user must fix
    /// credentials. Caller should mark status as `Error` and exit.
    StopAuth,
    /// Server told us when to retry (429 with Retry-After).
    RetryAfter(Duration),
    /// Generic transient failure, follow exponential schedule.
    ExponentialBackoff,
    /// 2xx — no retry needed.
    Success,
}

/// Classify an HTTP status + optional Retry-After header.
pub fn classify_http_status(
    status: reqwest::StatusCode,
    retry_after_secs: Option<u64>,
) -> HttpRetryDecision {
    if status.is_success() {
        return HttpRetryDecision::Success;
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return HttpRetryDecision::StopAuth;
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        if let Some(secs) = retry_after_secs {
            // Cap server-suggested delay at 5 minutes to avoid
            // pathological values from a misbehaving upstream.
            return HttpRetryDecision::RetryAfter(Duration::from_secs(secs.min(300)));
        }
    }
    HttpRetryDecision::ExponentialBackoff
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_caps_at_sixty_seconds() {
        assert_eq!(reconnect_delay(0), Duration::from_secs(1));
        assert_eq!(reconnect_delay(1), Duration::from_secs(2));
        assert_eq!(reconnect_delay(5), Duration::from_secs(32));
        assert_eq!(reconnect_delay(6), Duration::from_secs(60));
        assert_eq!(reconnect_delay(20), Duration::from_secs(60));
    }

    #[test]
    fn auth_codes_stop() {
        assert_eq!(
            classify_http_status(reqwest::StatusCode::UNAUTHORIZED, None),
            HttpRetryDecision::StopAuth
        );
        assert_eq!(
            classify_http_status(reqwest::StatusCode::FORBIDDEN, Some(10)),
            HttpRetryDecision::StopAuth
        );
    }

    #[test]
    fn retry_after_honored_then_capped() {
        assert_eq!(
            classify_http_status(reqwest::StatusCode::TOO_MANY_REQUESTS, Some(15)),
            HttpRetryDecision::RetryAfter(Duration::from_secs(15))
        );
        assert_eq!(
            classify_http_status(reqwest::StatusCode::TOO_MANY_REQUESTS, Some(9999)),
            HttpRetryDecision::RetryAfter(Duration::from_secs(300))
        );
    }
}
