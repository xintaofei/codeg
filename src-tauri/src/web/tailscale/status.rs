use serde::{Deserialize, Serialize};

use crate::web::tailscale::binary::locate_codeg_tsnet_binary;
use crate::web::tailscale::protocol::SidecarStatus;

pub const ERR_SIDECAR_MISSING: &str = "tailscale.sidecar_missing";
pub const ERR_START_FAILED: &str = "tailscale.start_failed";
pub const ERR_LOGIN_TIMEOUT: &str = "tailscale.login_timeout";
pub const ERR_FUNNEL_DENIED: &str = "tailscale.funnel_denied";
pub const ERR_FUNNEL_FAILED: &str = "tailscale.funnel_failed";
pub const ERR_AUTHKEY_REQUIRED: &str = "tailscale.authkey_required";
pub const ERR_UNSUPPORTED: &str = "tailscale.unsupported";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleFunnelStatus {
    pub supported: bool,
    pub enabled: bool,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funnel_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_key: Option<String>,
}

impl TailscaleFunnelStatus {
    pub fn unsupported(error_key: &str, last_error: impl Into<String>) -> Self {
        Self {
            supported: false,
            enabled: false,
            state: "error".into(),
            login_url: None,
            funnel_url: None,
            hostname: None,
            ipv4: None,
            last_error: Some(last_error.into()),
            error_key: Some(error_key.to_string()),
        }
    }

    pub fn stopped() -> Self {
        Self {
            supported: locate_codeg_tsnet_binary().is_some(),
            enabled: false,
            state: "stopped".into(),
            login_url: None,
            funnel_url: None,
            hostname: None,
            ipv4: None,
            last_error: None,
            error_key: None,
        }
    }
}

pub fn map_status(supported: bool, enabled: bool, raw: SidecarStatus) -> TailscaleFunnelStatus {
    TailscaleFunnelStatus {
        supported,
        enabled,
        state: raw.state,
        login_url: empty_to_none(raw.login_url),
        funnel_url: empty_to_none(raw.funnel_url),
        hostname: empty_to_none(raw.hostname),
        ipv4: empty_to_none(raw.ipv4),
        last_error: empty_to_none(raw.last_error),
        error_key: empty_to_none(raw.error_key),
    }
}

fn empty_to_none(v: Option<String>) -> Option<String> {
    v.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(st.enabled);
        assert!(st.supported);
    }
}
