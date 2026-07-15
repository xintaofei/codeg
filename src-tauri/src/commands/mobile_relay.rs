use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use codeg_relay_bridge::{config::DeviceConfig as BridgeDeviceConfig, Bridge, BridgeConfig};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::{
    app_error::AppCommandError,
    db::{service::app_metadata_service, AppDatabase},
    keyring_store,
    web::{self, WebServerState},
};

const SETTINGS_KEY: &str = "mobile_relay.settings.v1";
const DESKTOP_TOKEN_KEY: &str = "mobile-relay:desktop-token";
const PAIR_ROOT_PREFIX: &str = "mobile-relay:pair-root:";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedSettings {
    enabled: bool,
    relay_url: String,
    desktop_id: String,
    #[serde(default)]
    devices: Vec<RelayDevice>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayDevice {
    device_id: String,
    name: String,
    created_at: i64,
    #[serde(default)]
    last_seen_at: Option<i64>,
    revoked_at: Option<i64>,
}

#[derive(Deserialize)]
struct RelayDeviceSummary {
    device_id: String,
    last_seen_at: Option<i64>,
    revoked_at: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelaySettingsView {
    enabled: bool,
    relay_url: String,
    desktop_id: String,
    relay_token_configured: bool,
    bridge_running: bool,
    devices: Vec<RelayDevice>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayPairing {
    device_id: String,
    expires_at: i64,
    payload: String,
}

#[derive(Deserialize)]
struct IssuedDevice {
    token: String,
}

pub struct MobileRelayState {
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    running: Arc<AtomicBool>,
}

impl MobileRelayState {
    pub fn new() -> Self {
        Self {
            task: Mutex::new(None),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    fn stop(&self) {
        if let Some(task) = self.task.lock().expect("Relay task mutex poisoned").take() {
            task.abort();
        }
        self.running.store(false, Ordering::Release);
    }

    fn set_task(&self, task: tauri::async_runtime::JoinHandle<()>) {
        *self.task.lock().expect("Relay task mutex poisoned") = Some(task);
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

impl Default for MobileRelayState {
    fn default() -> Self {
        Self::new()
    }
}

async fn load_settings(db: &AppDatabase) -> Result<PersistedSettings, AppCommandError> {
    let Some(raw) = app_metadata_service::get_value(&db.conn, SETTINGS_KEY).await? else {
        return Ok(PersistedSettings::default());
    };
    serde_json::from_str(&raw).map_err(|error| {
        AppCommandError::configuration_invalid("Mobile Relay settings are invalid")
            .with_detail(error.to_string())
    })
}

async fn persist_settings(
    db: &AppDatabase,
    settings: &PersistedSettings,
) -> Result<(), AppCommandError> {
    let value = serde_json::to_string(settings).map_err(|error| {
        AppCommandError::configuration_invalid("Could not serialize Mobile Relay settings")
            .with_detail(error.to_string())
    })?;
    app_metadata_service::upsert_value(&db.conn, SETTINGS_KEY, &value).await?;
    Ok(())
}

async fn refresh_device_activity(db: &AppDatabase, settings: &mut PersistedSettings) {
    if settings.relay_url.is_empty() || settings.desktop_id.is_empty() {
        return;
    }
    let Some(relay_token) = keyring_store::get_app_secret(DESKTOP_TOKEN_KEY) else {
        return;
    };
    let Ok(endpoint) = relay_http_url(&settings.relay_url, "/v1/devices") else {
        return;
    };
    let response = match reqwest::Client::new()
        .get(endpoint)
        .bearer_auth(relay_token)
        .query(&[("desktop_id", settings.desktop_id.as_str())])
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        _ => return,
    };
    let Ok(remote_devices) = response.json::<Vec<RelayDeviceSummary>>().await else {
        return;
    };
    let before = settings.devices.clone();
    for remote in remote_devices {
        if let Some(local) = settings
            .devices
            .iter_mut()
            .find(|device| device.device_id == remote.device_id)
        {
            local.last_seen_at = remote.last_seen_at;
            local.revoked_at = remote.revoked_at.or(local.revoked_at);
        }
    }
    if settings.devices != before {
        if let Err(error) = persist_settings(db, settings).await {
            tracing::warn!(error = %error, "Could not persist Relay device activity");
        }
    }
}

fn validate_relay_url(value: &str) -> Result<String, AppCommandError> {
    let mut url = reqwest::Url::parse(value.trim())
        .map_err(|_| AppCommandError::invalid_input("Relay URL is invalid"))?;
    if !matches!(url.scheme(), "ws" | "wss")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AppCommandError::invalid_input(
            "Relay URL must be a ws/wss URL without credentials, query, or fragment",
        ));
    }
    url.set_path("/v1/ws");
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn relay_http_url(relay_url: &str, path: &str) -> Result<reqwest::Url, AppCommandError> {
    let mut url = reqwest::Url::parse(relay_url)
        .map_err(|_| AppCommandError::configuration_invalid("Relay URL is invalid"))?;
    let http_scheme = if url.scheme() == "wss" {
        "https"
    } else {
        "http"
    };
    url.set_scheme(http_scheme)
        .map_err(|_| AppCommandError::configuration_invalid("Relay URL scheme is invalid"))?;
    url.set_path(path);
    url.set_query(None);
    Ok(url)
}

fn desktop_token() -> Result<String, AppCommandError> {
    keyring_store::get_app_secret(DESKTOP_TOKEN_KEY)
        .filter(|token| token.len() >= 32)
        .ok_or_else(|| AppCommandError::configuration_missing("Relay desktop token is not set"))
}

fn pair_root_key(device_id: &str) -> String {
    format!("{PAIR_ROOT_PREFIX}{device_id}")
}

async fn start_bridge(
    app: &tauri::AppHandle,
    db: &AppDatabase,
    web_state: &WebServerState,
    relay_state: &MobileRelayState,
    settings: &PersistedSettings,
) -> Result<(), AppCommandError> {
    let relay_token = desktop_token()?;
    let web_info = match web::do_get_web_server_status(web_state) {
        Some(info) => info,
        None => {
            let config = web::load_web_service_config(&db.conn).await?;
            web::do_start_web_server_tauri(
                app.clone(),
                web_state,
                config.port,
                Some("127.0.0.1".to_string()),
                config.token,
            )
            .await?
        }
    };

    let mut devices = HashMap::new();
    for device in settings
        .devices
        .iter()
        .filter(|device| device.revoked_at.is_none())
    {
        let Some(pairing_root) = keyring_store::get_app_secret(&pair_root_key(&device.device_id))
        else {
            continue;
        };
        devices.insert(
            device.device_id.clone(),
            BridgeDeviceConfig { pairing_root },
        );
    }
    let bridge = Bridge::new(BridgeConfig {
        relay_url: settings.relay_url.clone(),
        desktop_id: settings.desktop_id.clone(),
        relay_token,
        local_url: format!("http://127.0.0.1:{}", web_info.port),
        codeg_token: web_info.token,
        devices,
    })
    .map_err(|error| {
        AppCommandError::configuration_invalid("Could not configure Mobile Relay bridge")
            .with_detail(error.to_string())
    })?;

    relay_state.stop();
    let running = relay_state.running.clone();
    let task = tauri::async_runtime::spawn(async move {
        running.store(true, Ordering::Release);
        if let Err(error) = bridge.run().await {
            tracing::error!(error = %error, "Mobile Relay bridge stopped");
        }
        running.store(false, Ordering::Release);
    });
    relay_state.set_task(task);
    Ok(())
}

#[tauri::command]
pub async fn get_mobile_relay_settings(
    db: tauri::State<'_, AppDatabase>,
    relay_state: tauri::State<'_, MobileRelayState>,
) -> Result<RelaySettingsView, AppCommandError> {
    let mut settings = load_settings(&db).await?;
    refresh_device_activity(&db, &mut settings).await;
    Ok(RelaySettingsView {
        enabled: settings.enabled,
        relay_url: settings.relay_url,
        desktop_id: settings.desktop_id,
        relay_token_configured: keyring_store::get_app_secret(DESKTOP_TOKEN_KEY).is_some(),
        bridge_running: relay_state.is_running(),
        devices: settings.devices,
    })
}

#[tauri::command]
pub async fn save_mobile_relay_settings(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    web_state: tauri::State<'_, WebServerState>,
    relay_state: tauri::State<'_, MobileRelayState>,
    relay_url: String,
    relay_token: Option<String>,
    enabled: bool,
) -> Result<RelaySettingsView, AppCommandError> {
    let mut settings = load_settings(&db).await?;
    settings.relay_url = validate_relay_url(&relay_url)?;
    if settings.desktop_id.is_empty() {
        settings.desktop_id = format!("d_{}", uuid::Uuid::new_v4().simple());
    }
    if let Some(token) = relay_token.map(|token| token.trim().to_string()) {
        if token.len() < 32 {
            return Err(AppCommandError::invalid_input(
                "Relay desktop token must contain at least 32 characters",
            ));
        }
        keyring_store::set_app_secret(DESKTOP_TOKEN_KEY, &token).map_err(|error| {
            AppCommandError::io_error("Could not save Relay token").with_detail(error)
        })?;
    }
    if enabled {
        let _ = desktop_token()?;
    }
    settings.enabled = enabled;
    persist_settings(&db, &settings).await?;
    if enabled {
        start_bridge(&app, &db, &web_state, &relay_state, &settings).await?;
    } else {
        relay_state.stop();
    }
    get_mobile_relay_settings(db, relay_state).await
}

#[tauri::command]
pub async fn create_mobile_relay_pairing(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    web_state: tauri::State<'_, WebServerState>,
    relay_state: tauri::State<'_, MobileRelayState>,
    device_name: String,
) -> Result<RelayPairing, AppCommandError> {
    let mut settings = load_settings(&db).await?;
    if settings.relay_url.is_empty() || settings.desktop_id.is_empty() {
        return Err(AppCommandError::configuration_missing(
            "Save Mobile Relay settings before pairing a phone",
        ));
    }
    let relay_token = desktop_token()?;
    let device_id = format!("m_{}", uuid::Uuid::new_v4().simple());
    let mut root = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut root);
    let pairing_root = URL_SAFE_NO_PAD.encode(root);
    let endpoint = relay_http_url(&settings.relay_url, "/v1/devices")?;
    let response = reqwest::Client::new()
        .post(endpoint)
        .bearer_auth(&relay_token)
        .json(&serde_json::json!({
            "desktop_id": settings.desktop_id,
            "device_id": device_id
        }))
        .send()
        .await
        .map_err(|error| {
            AppCommandError::network("Could not reach Codeg Relay").with_detail(error.to_string())
        })?;
    if !response.status().is_success() {
        return Err(AppCommandError::network(format!(
            "Codeg Relay returned HTTP {}",
            response.status().as_u16()
        )));
    }
    let issued: IssuedDevice = response.json().await.map_err(|error| {
        AppCommandError::network("Codeg Relay returned an invalid pairing response")
            .with_detail(error.to_string())
    })?;
    keyring_store::set_app_secret(&pair_root_key(&device_id), &pairing_root).map_err(|error| {
        AppCommandError::io_error("Could not save device pairing root").with_detail(error)
    })?;

    let now = chrono::Utc::now().timestamp();
    let name = device_name.trim();
    settings.devices.push(RelayDevice {
        device_id: device_id.clone(),
        name: if name.is_empty() {
            "Mobile device".to_string()
        } else {
            name.chars().take(80).collect()
        },
        created_at: now,
        last_seen_at: None,
        revoked_at: None,
    });
    persist_settings(&db, &settings).await?;
    if settings.enabled {
        start_bridge(&app, &db, &web_state, &relay_state, &settings).await?;
    }

    let expires_at = now + 300;
    let payload = serde_json::json!({
        "v": 1,
        "relay_url": settings.relay_url,
        "desktop_id": settings.desktop_id,
        "device_id": device_id,
        "routing_token": issued.token,
        "pairing_root": pairing_root,
        "expires_at": expires_at
    })
    .to_string();
    Ok(RelayPairing {
        device_id,
        expires_at,
        payload,
    })
}

#[tauri::command]
pub async fn revoke_mobile_relay_device(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    web_state: tauri::State<'_, WebServerState>,
    relay_state: tauri::State<'_, MobileRelayState>,
    device_id: String,
) -> Result<(), AppCommandError> {
    let mut settings = load_settings(&db).await?;
    let relay_token = desktop_token()?;
    let endpoint = relay_http_url(&settings.relay_url, &format!("/v1/devices/{device_id}"))?;
    let response = reqwest::Client::new()
        .delete(endpoint)
        .bearer_auth(relay_token)
        .query(&[("desktop_id", settings.desktop_id.as_str())])
        .send()
        .await
        .map_err(|error| {
            AppCommandError::network("Could not reach Codeg Relay").with_detail(error.to_string())
        })?;
    if !response.status().is_success() && response.status() != reqwest::StatusCode::NOT_FOUND {
        return Err(AppCommandError::network(format!(
            "Codeg Relay returned HTTP {}",
            response.status().as_u16()
        )));
    }
    let now = chrono::Utc::now().timestamp();
    let Some(device) = settings
        .devices
        .iter_mut()
        .find(|device| device.device_id == device_id)
    else {
        return Err(AppCommandError::not_found("Paired device was not found"));
    };
    device.revoked_at = Some(now);
    keyring_store::delete_app_secret(&pair_root_key(&device_id)).map_err(|error| {
        AppCommandError::io_error("Could not delete device pairing root").with_detail(error)
    })?;
    persist_settings(&db, &settings).await?;
    if settings.enabled {
        start_bridge(&app, &db, &web_state, &relay_state, &settings).await?;
    }
    Ok(())
}

pub async fn auto_start_mobile_relay(app: &tauri::AppHandle) {
    let db = app.state::<AppDatabase>();
    let web_state = app.state::<WebServerState>();
    let relay_state = app.state::<MobileRelayState>();
    match load_settings(&db).await {
        Ok(settings) if settings.enabled => {
            if let Err(error) = start_bridge(app, &db, &web_state, &relay_state, &settings).await {
                tracing::error!(error = %error, "Mobile Relay auto-start failed");
            }
        }
        Ok(_) => {}
        Err(error) => tracing::error!(error = %error, "Could not load Mobile Relay settings"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_relay_url_and_rejects_embedded_credentials() {
        assert_eq!(
            validate_relay_url("wss://relay.example.test/custom").unwrap(),
            "wss://relay.example.test/v1/ws"
        );
        assert!(validate_relay_url("wss://token@relay.example.test/v1/ws").is_err());
        assert!(validate_relay_url("https://relay.example.test").is_err());
    }
}
