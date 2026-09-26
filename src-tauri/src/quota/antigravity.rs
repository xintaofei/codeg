//! Antigravity quota parsing and probe helper module.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::models::{AgentQuotaInfo, QuotaWindow, SpendLimit};

/// Helper to parse an Antigravity quota window with percentage and reset time.
pub fn parse_antigravity_window(
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

/// Helper to parse an Antigravity quota window from raw usage and limit counters.
pub fn parse_antigravity_usage_window(
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
struct RawAntigravityQuotaPayload {
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    short_window: Option<RawWindowPayload>,
    #[serde(default)]
    weekly_window: Option<RawWindowPayload>,
    #[serde(default)]
    spend_limit: Option<RawSpendPayload>,
}

/// Parse a raw JSON payload from Antigravity/Gemini into `AgentQuotaInfo`.
pub fn parse_antigravity_quota_json(
    raw_json: &str,
    now: Option<DateTime<Utc>>,
) -> Result<AgentQuotaInfo, String> {
    let payload: RawAntigravityQuotaPayload =
        serde_json::from_str(raw_json).map_err(|e| format!("failed to parse antigravity quota JSON: {e}"))?;

    let short_window = payload.short_window.map(|w| {
        let label = w.label.unwrap_or_else(|| "Short-Term Quota".to_string());
        if let (Some(used), Some(limit)) = (w.used, w.limit) {
            parse_antigravity_usage_window(label, used, limit, w.resets_at, now)
        } else {
            let used_pct = w.used_percent.unwrap_or(0.0);
            parse_antigravity_window(label, used_pct, w.resets_at, now)
        }
    });

    let weekly_window = payload.weekly_window.map(|w| {
        let label = w.label.unwrap_or_else(|| "Weekly Quota".to_string());
        if let (Some(used), Some(limit)) = (w.used, w.limit) {
            parse_antigravity_usage_window(label, used, limit, w.resets_at, now)
        } else {
            let used_pct = w.used_percent.unwrap_or(0.0);
            parse_antigravity_window(label, used_pct, w.resets_at, now)
        }
    });

    let spend_limit = payload
        .spend_limit
        .map(|s| SpendLimit::new(s.used_usd, s.limit_usd));

    let last_updated = now.unwrap_or_else(Utc::now);

    Ok(AgentQuotaInfo {
        agent_type: "antigravity".to_string(),
        plan_name: payload.plan,
        short_window,
        weekly_window,
        spend_limit,
        last_updated,
    })
}

/// Skeleton fetcher for Antigravity quota.
pub async fn fetch_antigravity_quota() -> Result<AgentQuotaInfo, String> {
    // Return a default skeleton quota status for Antigravity
    let now = Utc::now();
    Ok(AgentQuotaInfo {
        agent_type: "antigravity".to_string(),
        plan_name: Some("Google Antigravity Free Tier".to_string()),
        short_window: Some(QuotaWindow::new(
            "Gemini 2.5 Pro Window",
            0.0,
            100.0,
            None,
            None,
        )),
        weekly_window: Some(QuotaWindow::new(
            "Weekly Window",
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
    fn test_parse_antigravity_window() {
        let now = Utc.with_ymd_and_hms(2026, 9, 26, 8, 0, 0).unwrap();
        let reset = Utc.with_ymd_and_hms(2026, 9, 26, 9, 0, 0).unwrap();

        let window = parse_antigravity_window("Flash Quota", 15.0, Some(reset), Some(now));
        assert_eq!(window.label, "Flash Quota");
        assert_eq!(window.used_percent, 15.0);
        assert_eq!(window.remaining_percent, 85.0);
        assert_eq!(window.resets_at, Some(reset));
        assert_eq!(window.reset_in_seconds, Some(3600));
    }

    #[test]
    fn test_parse_antigravity_usage_window() {
        let now = Utc.with_ymd_and_hms(2026, 9, 26, 8, 0, 0).unwrap();
        let reset = Utc.with_ymd_and_hms(2026, 9, 26, 12, 0, 0).unwrap();

        let window = parse_antigravity_usage_window("Requests", 45.0, 150.0, Some(reset), Some(now));
        assert_eq!(window.label, "Requests");
        assert_eq!(window.used_percent, 30.0);
        assert_eq!(window.remaining_percent, 70.0);
        assert_eq!(window.reset_in_seconds, Some(4 * 3600));
    }

    #[test]
    fn test_parse_antigravity_quota_json() {
        let now = Utc.with_ymd_and_hms(2026, 9, 26, 12, 0, 0).unwrap();
        let raw = r#"{
            "plan": "Google Workspace Enterprise",
            "short_window": {
                "label": "Pro Model Window",
                "used": 10.0,
                "limit": 50.0,
                "resets_at": "2026-09-26T14:00:00Z"
            },
            "weekly_window": {
                "label": "Weekly Allowance",
                "used_percent": 8.0,
                "resets_at": "2026-10-01T00:00:00Z"
            }
        }"#;

        let quota = parse_antigravity_quota_json(raw, Some(now)).expect("parse antigravity quota");
        assert_eq!(quota.agent_type, "antigravity");
        assert_eq!(
            quota.plan_name.as_deref(),
            Some("Google Workspace Enterprise")
        );

        let short = quota.short_window.unwrap();
        assert_eq!(short.label, "Pro Model Window");
        assert_eq!(short.used_percent, 20.0);
        assert_eq!(short.remaining_percent, 80.0);
        assert_eq!(short.reset_in_seconds, Some(2 * 3600));

        let weekly = quota.weekly_window.unwrap();
        assert_eq!(weekly.label, "Weekly Allowance");
        assert_eq!(weekly.used_percent, 8.0);
        assert_eq!(weekly.remaining_percent, 92.0);

        assert_eq!(quota.spend_limit, None);
    }
}
