use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use url::Url;

#[derive(Clone, Deserialize)]
pub struct DeviceConfig {
    /// Unpadded base64url-encoded 256-bit root established during pairing.
    pub pairing_root: String,
}

#[derive(Clone, Deserialize)]
pub struct BridgeConfig {
    pub relay_url: String,
    pub desktop_id: String,
    pub relay_token: String,
    pub local_url: String,
    pub codeg_token: String,
    #[serde(default)]
    pub devices: HashMap<String, DeviceConfig>,
}

impl BridgeConfig {
    pub async fn load(path: &Path) -> anyhow::Result<Self> {
        ensure_private_permissions(path)?;
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("failed to read bridge config {}", path.display()))?;
        let config: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid bridge config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        let relay = Url::parse(&self.relay_url).context("relay_url is invalid")?;
        if !matches!(relay.scheme(), "ws" | "wss") {
            bail!("relay_url must use ws or wss");
        }
        let local = Url::parse(&self.local_url).context("local_url is invalid")?;
        if !matches!(local.scheme(), "http" | "https") {
            bail!("local_url must use http or https");
        }
        if !valid_id(&self.desktop_id) {
            bail!("desktop_id is invalid");
        }
        if self.relay_token.len() < 32 {
            bail!("relay_token must contain at least 32 characters");
        }
        if self.codeg_token.trim().is_empty() {
            bail!("codeg_token must not be empty");
        }
        for (device_id, device) in &self.devices {
            if !valid_id(device_id) {
                bail!("device id {device_id:?} is invalid");
            }
            let root = URL_SAFE_NO_PAD
                .decode(&device.pairing_root)
                .with_context(|| format!("pairing root for {device_id} is not base64url"))?;
            if root.len() != 32 {
                bail!("pairing root for {device_id} must contain 32 bytes");
            }
        }
        Ok(())
    }

    pub fn pairing_root(&self, device_id: &str) -> Option<[u8; 32]> {
        let encoded = &self.devices.get(device_id)?.pairing_root;
        URL_SAFE_NO_PAD.decode(encoded).ok()?.try_into().ok()
    }
}

pub fn default_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("CODEG_RELAY_BRIDGE_CONFIG") {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("Codeg").join("relay-bridge.json")
    }
    #[cfg(target_os = "macos")]
    {
        let base = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("Library")
            .join("Application Support")
            .join("Codeg")
            .join("relay-bridge.json")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("codeg").join("relay-bridge.json")
    }
}

fn valid_id(value: &str) -> bool {
    (3..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

#[cfg(unix)]
fn ensure_private_permissions(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        bail!(
            "bridge config {} must not be readable by group or others (current mode {mode:o})",
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_permissions(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_minimal_config() {
        let root = URL_SAFE_NO_PAD.encode([7_u8; 32]);
        let config = BridgeConfig {
            relay_url: "wss://relay.example.test/v1/ws".into(),
            desktop_id: "d_test".into(),
            relay_token: "r".repeat(32),
            local_url: "http://127.0.0.1:3080".into(),
            codeg_token: "local-secret".into(),
            devices: HashMap::from([("m_phone".into(), DeviceConfig { pairing_root: root })]),
        };
        config.validate().unwrap();
        assert_eq!(config.pairing_root("m_phone"), Some([7_u8; 32]));
    }
}
