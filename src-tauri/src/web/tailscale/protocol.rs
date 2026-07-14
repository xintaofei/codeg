use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SidecarBootstrap {
    pub control_addr: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SidecarStatus {
    pub state: String,
    #[serde(default)]
    pub login_url: Option<String>,
    #[serde(default)]
    pub funnel_url: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub ipv4: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub error_key: Option<String>,
    #[serde(default)]
    pub backend_state: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bootstrap_json() {
        let raw = r#"{"controlAddr":"127.0.0.1:9","pid":42}"#;
        let b: SidecarBootstrap = serde_json::from_str(raw).unwrap();
        assert_eq!(b.control_addr, "127.0.0.1:9");
        assert_eq!(b.pid, 42);
    }

    #[test]
    fn parses_status_json_camel_case() {
        let raw = r#"{
            "state":"needs_login",
            "loginUrl":"https://login.tailscale.com/a/x",
            "hostname":"codeg-test"
        }"#;
        let s: SidecarStatus = serde_json::from_str(raw).unwrap();
        assert_eq!(s.state, "needs_login");
        assert_eq!(
            s.login_url.as_deref(),
            Some("https://login.tailscale.com/a/x")
        );
        assert_eq!(s.hostname.as_deref(), Some("codeg-test"));
    }
}
