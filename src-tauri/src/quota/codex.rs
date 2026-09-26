//! Codex quota parsing and probe helper module.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::models::{AgentQuotaInfo, QuotaWindow, SpendLimit};

/// Helper to parse a quota window with percentage and reset time.
pub fn parse_codex_window(
    label: impl Into<String>,
    used_percent: f64,
    resets_at: Option<DateTime<Utc>>,
    now: Option<DateTime<Utc>>,
) -> QuotaWindow {
    let used_clamped = used_percent.clamp(0.0, 100.0);
    let remaining = (100.0 - used_clamped).max(0.0);
    let reset_in_seconds = resets_at.map(|reset_time| {
        let current = now.unwrap_or_else(Utc::now);
        (reset_time - current).num_seconds().max(0)
    });

    QuotaWindow::new(
        label,
        used_clamped,
        remaining,
        resets_at,
        reset_in_seconds,
    )
}

/// Helper to parse a quota window from raw usage and limit counters.
pub fn parse_codex_usage_window(
    label: impl Into<String>,
    used: f64,
    limit: f64,
    resets_at: Option<DateTime<Utc>>,
    now: Option<DateTime<Utc>>,
) -> QuotaWindow {
    QuotaWindow::from_usage(label, used, limit, resets_at, now)
}

#[derive(Debug, Deserialize)]
struct RawWindowPayload {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    used: Option<f64>,
    #[serde(default)]
    limit: Option<f64>,
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct RawSpendPayload {
    used_usd: f64,
    limit_usd: f64,
}

#[derive(Debug, Deserialize)]
struct RawCodexQuotaPayload {
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    short_window: Option<RawWindowPayload>,
    #[serde(default)]
    weekly_window: Option<RawWindowPayload>,
    #[serde(default)]
    spend_limit: Option<RawSpendPayload>,
}

/// Parse a raw JSON payload from Codex CLI or API into `AgentQuotaInfo`.
pub fn parse_codex_quota_json(
    raw_json: &str,
    now: Option<DateTime<Utc>>,
) -> Result<AgentQuotaInfo, String> {
    let payload: RawCodexQuotaPayload =
        serde_json::from_str(raw_json).map_err(|e| format!("failed to parse codex quota JSON: {e}"))?;

    let short_window = payload.short_window.map(|w| {
        let label = w.label.unwrap_or_else(|| "5-Hour Window".to_string());
        if let (Some(used), Some(limit)) = (w.used, w.limit) {
            parse_codex_usage_window(label, used, limit, w.resets_at, now)
        } else {
            let used_pct = w.used_percent.unwrap_or(0.0);
            parse_codex_window(label, used_pct, w.resets_at, now)
        }
    });

    let weekly_window = payload.weekly_window.map(|w| {
        let label = w.label.unwrap_or_else(|| "Weekly Limit".to_string());
        if let (Some(used), Some(limit)) = (w.used, w.limit) {
            parse_codex_usage_window(label, used, limit, w.resets_at, now)
        } else {
            let used_pct = w.used_percent.unwrap_or(0.0);
            parse_codex_window(label, used_pct, w.resets_at, now)
        }
    });

    let spend_limit = payload
        .spend_limit
        .map(|s| SpendLimit::new(s.used_usd, s.limit_usd));

    let last_updated = now.unwrap_or_else(Utc::now);

    Ok(AgentQuotaInfo {
        agent_type: "codex".to_string(),
        plan_name: payload.plan,
        short_window,
        weekly_window,
        spend_limit,
        last_updated,
    })
}

/// Skeleton fetcher for Codex quota.
pub async fn fetch_codex_quota() -> Result<AgentQuotaInfo, String> {
    // Return a default skeleton quota status for Codex
    let now = Utc::now();
    Ok(AgentQuotaInfo {
        agent_type: "codex".to_string(),
        plan_name: Some("Codex Standard".to_string()),
        short_window: Some(QuotaWindow::new(
            "5-Hour Window",
            0.0,
            100.0,
            None,
            None,
        )),
        weekly_window: Some(QuotaWindow::new(
            "Weekly Limit",
            0.0,
            100.0,
            None,
            None,
        )),
        spend_limit: None,
        last_updated: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_parse_codex_window() {
        let now = Utc.with_ymd_and_hms(2026, 9, 26, 12, 0, 0).unwrap();
        let reset = Utc.with_ymd_and_hms(2026, 9, 26, 17, 0, 0).unwrap();

        let window = parse_codex_window("5-Hour Window", 40.0, Some(reset), Some(now));
        assert_eq!(window.label, "5-Hour Window");
        assert_eq!(window.used_percent, 40.0);
        assert_eq!(window.remaining_percent, 60.0);
        assert_eq!(window.resets_at, Some(reset));
        assert_eq!(window.reset_in_seconds, Some(5 * 3600));
    }

    #[test]
    fn test_parse_codex_usage_window() {
        let now = Utc.with_ymd_and_hms(2026, 9, 26, 10, 0, 0).unwrap();
        let reset = Utc.with_ymd_and_hms(2026, 9, 26, 15, 0, 0).unwrap();

        let window = parse_codex_usage_window("5-Hour Window", 30.0, 100.0, Some(reset), Some(now));
        assert_eq!(window.used_percent, 30.0);
        assert_eq!(window.remaining_percent, 70.0);
        assert_eq!(window.reset_in_seconds, Some(5 * 3600));
    }

    #[test]
    fn test_parse_codex_quota_json() {
        let now = Utc.with_ymd_and_hms(2026, 9, 26, 12, 0, 0).unwrap();
        let raw = r#"{
            "plan": "ChatGPT Plus",
            "short_window": {
                "label": "5-Hour Window",
                "used": 20.0,
                "limit": 80.0,
                "resets_at": "2026-09-26T17:00:00Z"
            },
            "weekly_window": {
                "label": "Weekly Allotment",
                "used_percent": 15.0,
                "resets_at": "2026-10-01T00:00:00Z"
            },
            "spend_limit": {
                "used_usd": 3.5,
                "limit_usd": 20.0
            }
        }"#;

        let quota = parse_codex_quota_json(raw, Some(now)).expect("parse codex quota");
        assert_eq!(quota.agent_type, "codex");
        assert_eq!(quota.plan_name.as_deref(), Some("ChatGPT Plus"));

        let short = quota.short_window.unwrap();
        assert_eq!(short.label, "5-Hour Window");
        assert_eq!(short.used_percent, 25.0);
        assert_eq!(short.remaining_percent, 75.0);
        assert_eq!(short.reset_in_seconds, Some(5 * 3600));

        let weekly = quota.weekly_window.unwrap();
        assert_eq!(weekly.label, "Weekly Allotment");
        assert_eq!(weekly.used_percent, 15.0);
        assert_eq!(weekly.remaining_percent, 85.0);

        let spend = quota.spend_limit.unwrap();
        assert_eq!(spend.used_usd, 3.5);
        assert_eq!(spend.limit_usd, 20.0);
    }
}
