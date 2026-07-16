use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use codeg_relay_bridge::{
    config::DeviceConfig as BridgeDeviceConfig, crypto::DesktopPairingSecret, Bridge, BridgeConfig,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
const PAIR_ACCEPT_ATTEMPTS: usize = 3;

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
    pair_id: String,
    expires_at: i64,
    payload: String,
}

#[derive(Deserialize)]
struct RelayPairingStatusResponse {
    status: String,
    expires_at: i64,
    device_id: Option<String>,
    device_name: Option<String>,
    mobile_public_key: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayPairingStatus {
    status: String,
    expires_at: i64,
    device_id: Option<String>,
    device_name: Option<String>,
    sas: Option<String>,
}

struct PendingDesktopPairing {
    crypto: DesktopPairingSecret,
    friendly_name: String,
    expires_at: i64,
}

pub struct MobileRelayState {
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    running: Arc<AtomicBool>,
    pending_pairings: Mutex<HashMap<String, PendingDesktopPairing>>,
}

impl MobileRelayState {
    pub fn new() -> Self {
        Self {
            task: Mutex::new(None),
            running: Arc::new(AtomicBool::new(false)),
            pending_pairings: Mutex::new(HashMap::new()),
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
    let host = url
        .host_str()
        .ok_or_else(|| AppCommandError::invalid_input("Relay URL must include a host"))?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if (url.scheme() != "wss" && !(url.scheme() == "ws" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AppCommandError::invalid_input(
            "Relay URL must use wss without credentials, query, or fragment; ws is allowed only for loopback development",
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

fn sha256_hex(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn fetch_pairing_status(
    settings: &PersistedSettings,
    relay_token: &str,
    pair_id: &str,
) -> Result<RelayPairingStatusResponse, AppCommandError> {
    let endpoint = relay_http_url(&settings.relay_url, &format!("/v1/pairings/{pair_id}"))?;
    let response = reqwest::Client::new()
        .get(endpoint)
        .bearer_auth(relay_token)
        .query(&[("desktop_id", settings.desktop_id.as_str())])
        .send()
        .await
        .map_err(|error| {
            AppCommandError::network("Could not reach Codeg Relay").with_detail(error.to_string())
        })?;
    if !response.status().is_success() {
        return Err(AppCommandError::network(format!(
            "Codeg Relay returned HTTP {} while checking pairing",
            response.status().as_u16()
        )));
    }
    response.json().await.map_err(|error| {
        AppCommandError::network("Codeg Relay returned an invalid pairing status")
            .with_detail(error.to_string())
    })
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
    db: tauri::State<'_, AppDatabase>,
    relay_state: tauri::State<'_, MobileRelayState>,
    device_name: String,
) -> Result<RelayPairing, AppCommandError> {
    let settings = load_settings(&db).await?;
    if settings.relay_url.is_empty() || settings.desktop_id.is_empty() {
        return Err(AppCommandError::configuration_missing(
            "Save Mobile Relay settings before pairing a phone",
        ));
    }
    let relay_token = desktop_token()?;
    let now = chrono::Utc::now().timestamp();
    let expires_at = now + 300;
    let mut pair_id_bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut pair_id_bytes);
    let pair_id = format!("p_{}", URL_SAFE_NO_PAD.encode(pair_id_bytes));
    let crypto = DesktopPairingSecret::generate();
    let endpoint = relay_http_url(&settings.relay_url, "/v1/pairings")?;
    let response = reqwest::Client::new()
        .post(endpoint)
        .bearer_auth(&relay_token)
        .json(&serde_json::json!({
            "desktop_id": settings.desktop_id,
            "pair_id_hash": sha256_hex(pair_id.as_bytes()),
            "expires_at": expires_at
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
    let friendly_name = device_name.trim();
    let friendly_name = if friendly_name.is_empty() {
        "Mobile device".to_string()
    } else {
        friendly_name.chars().take(80).collect()
    };
    let pair_secret = crypto.pair_secret_encoded();
    let desktop_public_key = crypto.public_key().to_owned();
    {
        let mut pending = relay_state
            .pending_pairings
            .lock()
            .expect("Relay pairing mutex poisoned");
        pending.retain(|_, pairing| pairing.expires_at > now);
        pending.insert(
            pair_id.clone(),
            PendingDesktopPairing {
                crypto,
                friendly_name,
                expires_at,
            },
        );
    }
    let payload = serde_json::json!({
        "v": 2,
        "relay_url": settings.relay_url,
        "desktop_id": settings.desktop_id,
        "pair_id": pair_id,
        "pair_secret": pair_secret,
        "desktop_public_key": desktop_public_key,
        "expires_at": expires_at
    })
    .to_string();
    Ok(RelayPairing {
        pair_id,
        expires_at,
        payload,
    })
}

#[tauri::command]
pub async fn get_mobile_relay_pairing_status(
    db: tauri::State<'_, AppDatabase>,
    relay_state: tauri::State<'_, MobileRelayState>,
    pair_id: String,
) -> Result<RelayPairingStatus, AppCommandError> {
    let settings = load_settings(&db).await?;
    let relay_token = desktop_token()?;
    let remote = fetch_pairing_status(&settings, &relay_token, &pair_id).await?;
    let mut view = RelayPairingStatus {
        status: remote.status.clone(),
        expires_at: remote.expires_at,
        device_id: remote.device_id.clone(),
        device_name: remote.device_name.clone(),
        sas: None,
    };
    if remote.status == "requested" {
        let device_id = remote.device_id.as_deref().ok_or_else(|| {
            AppCommandError::network("Relay pairing request omitted the device id")
        })?;
        let mobile_public_key = remote.mobile_public_key.as_deref().ok_or_else(|| {
            AppCommandError::network("Relay pairing request omitted the mobile public key")
        })?;
        let pending = relay_state
            .pending_pairings
            .lock()
            .expect("Relay pairing mutex poisoned");
        let pairing = pending
            .get(&pair_id)
            .ok_or_else(|| AppCommandError::not_found("The local one-time pairing has expired"))?;
        view.sas = Some(
            pairing
                .crypto
                .derive_material(&settings.desktop_id, &pair_id, device_id, mobile_public_key)
                .map_err(|error| {
                    AppCommandError::configuration_invalid(
                        "Mobile pairing public key verification failed",
                    )
                    .with_detail(error.to_string())
                })?
                .sas,
        );
    }
    Ok(view)
}

#[tauri::command]
pub async fn confirm_mobile_relay_pairing(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    web_state: tauri::State<'_, WebServerState>,
    relay_state: tauri::State<'_, MobileRelayState>,
    pair_id: String,
) -> Result<(), AppCommandError> {
    let mut settings = load_settings(&db).await?;
    let original_settings = settings.clone();
    let relay_token = desktop_token()?;
    let remote = fetch_pairing_status(&settings, &relay_token, &pair_id).await?;
    if matches!(remote.status.as_str(), "accepted" | "consumed") {
        let device_id = remote.device_id.ok_or_else(|| {
            AppCommandError::network("Relay pairing result omitted the device id")
        })?;
        let local_device_exists = settings
            .devices
            .iter()
            .any(|device| device.device_id == device_id && device.revoked_at.is_none());
        let local_root_exists = keyring_store::get_app_secret(&pair_root_key(&device_id)).is_some();
        if local_device_exists && local_root_exists {
            relay_state
                .pending_pairings
                .lock()
                .expect("Relay pairing mutex poisoned")
                .remove(&pair_id);
            return Ok(());
        }
        return Err(AppCommandError::configuration_invalid(
            "Relay accepted the phone, but the local pairing credential is missing; revoke it and pair again",
        ));
    }
    if remote.status != "requested" {
        return Err(AppCommandError::invalid_input(
            "No mobile is waiting for confirmation",
        ));
    }
    let device_id = remote
        .device_id
        .ok_or_else(|| AppCommandError::network("Relay pairing request omitted the device id"))?;
    let mobile_public_key = remote.mobile_public_key.ok_or_else(|| {
        AppCommandError::network("Relay pairing request omitted the mobile public key")
    })?;
    let device_name = remote
        .device_name
        .unwrap_or_else(|| "Codeg Mobile".to_string());
    let (material, friendly_name) = {
        let pending = relay_state
            .pending_pairings
            .lock()
            .expect("Relay pairing mutex poisoned");
        let pairing = pending
            .get(&pair_id)
            .ok_or_else(|| AppCommandError::not_found("The local one-time pairing has expired"))?;
        let material = pairing
            .crypto
            .derive_material(
                &settings.desktop_id,
                &pair_id,
                &device_id,
                &mobile_public_key,
            )
            .map_err(|error| {
                AppCommandError::configuration_invalid(
                    "Mobile pairing public key verification failed",
                )
                .with_detail(error.to_string())
            })?;
        let name = if pairing.friendly_name == "Mobile device" {
            device_name
        } else {
            pairing.friendly_name.clone()
        };
        (material, name)
    };

    let mut token_bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut token_bytes);
    let routing_token = format!("mrt_{}", URL_SAFE_NO_PAD.encode(token_bytes));
    let accept_plaintext = serde_json::to_vec(&serde_json::json!({
        "v": 2,
        "desktop_id": settings.desktop_id,
        "device_id": device_id,
        "routing_token": routing_token,
        "expires_at": remote.expires_at
    }))
    .map_err(|error| {
        AppCommandError::configuration_invalid("Could not serialize pairing acceptance")
            .with_detail(error.to_string())
    })?;
    let sealed = material
        .seal_accept(
            &settings.desktop_id,
            &pair_id,
            &device_id,
            &accept_plaintext,
        )
        .map_err(|error| {
            AppCommandError::configuration_invalid("Could not encrypt pairing acceptance")
                .with_detail(error.to_string())
        })?;
    let pairing_root = URL_SAFE_NO_PAD.encode(material.pairing_root);
    keyring_store::set_app_secret(&pair_root_key(&device_id), &pairing_root).map_err(|error| {
        AppCommandError::io_error("Could not save device pairing root").with_detail(error)
    })?;
    settings
        .devices
        .retain(|device| device.device_id != device_id);
    settings.devices.push(RelayDevice {
        device_id: device_id.clone(),
        name: friendly_name,
        created_at: chrono::Utc::now().timestamp(),
        last_seen_at: None,
        revoked_at: None,
    });
    if let Err(error) = persist_settings(&db, &settings).await {
        let _ = keyring_store::delete_app_secret(&pair_root_key(&device_id));
        return Err(error);
    }
    if settings.enabled {
        if let Err(error) = start_bridge(&app, &db, &web_state, &relay_state, &settings).await {
            let _ = keyring_store::delete_app_secret(&pair_root_key(&device_id));
            let _ = persist_settings(&db, &original_settings).await;
            let _ = start_bridge(&app, &db, &web_state, &relay_state, &original_settings).await;
            return Err(error);
        }
    }

    let endpoint = relay_http_url(
        &settings.relay_url,
        &format!("/v1/pairings/{pair_id}/accept"),
    )?;
    let acceptance = serde_json::json!({
        "desktop_id": settings.desktop_id,
        "device_id": device_id,
        "token_sha256": sha256_hex(routing_token.as_bytes()),
        "nonce": sealed.nonce,
        "ciphertext": sealed.ciphertext
    });
    let client = reqwest::Client::new();
    let mut accepted = false;
    let mut final_state_unknown = true;
    for attempt in 0..PAIR_ACCEPT_ATTEMPTS {
        let response = client
            .post(endpoint.clone())
            .bearer_auth(&relay_token)
            .json(&acceptance)
            .send()
            .await;
        if response
            .as_ref()
            .is_ok_and(|response| response.status().is_success())
        {
            accepted = true;
            break;
        }
        match fetch_pairing_status(&settings, &relay_token, &pair_id).await {
            Ok(status) if matches!(status.status.as_str(), "accepted" | "consumed") => {
                accepted = true;
                break;
            }
            Ok(_) => final_state_unknown = false,
            Err(_) => final_state_unknown = true,
        }
        if attempt + 1 < PAIR_ACCEPT_ATTEMPTS {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    if !accepted {
        if !final_state_unknown {
            let _ = keyring_store::delete_app_secret(&pair_root_key(&device_id));
            let _ = persist_settings(&db, &original_settings).await;
            if original_settings.enabled {
                let _ = start_bridge(&app, &db, &web_state, &relay_state, &original_settings).await;
            }
            return Err(AppCommandError::network(
                "Relay rejected the mobile pairing",
            ));
        }
        return Err(AppCommandError::network(
            "Relay confirmation is pending because the network response was lost; local credentials were retained safely and confirmation can be retried",
        ));
    }
    relay_state
        .pending_pairings
        .lock()
        .expect("Relay pairing mutex poisoned")
        .remove(&pair_id);
    Ok(())
}

#[tauri::command]
pub async fn reject_mobile_relay_pairing(
    db: tauri::State<'_, AppDatabase>,
    relay_state: tauri::State<'_, MobileRelayState>,
    pair_id: String,
    device_id: Option<String>,
) -> Result<(), AppCommandError> {
    let settings = load_settings(&db).await?;
    let relay_token = desktop_token()?;
    let endpoint = relay_http_url(
        &settings.relay_url,
        &format!("/v1/pairings/{pair_id}/reject"),
    )?;
    let response = reqwest::Client::new()
        .post(endpoint)
        .bearer_auth(relay_token)
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
            "Codeg Relay returned HTTP {} while rejecting pairing",
            response.status().as_u16()
        )));
    }
    relay_state
        .pending_pairings
        .lock()
        .expect("Relay pairing mutex poisoned")
        .remove(&pair_id);
    Ok(())
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
            tracing::info!(
                device_count = settings.devices.len(),
                "Mobile Relay auto-starting"
            );
            if let Err(error) = start_bridge(app, &db, &web_state, &relay_state, &settings).await {
                tracing::error!(error = %error, "Mobile Relay auto-start failed");
            } else {
                tracing::info!("Mobile Relay bridge task started");
            }
        }
        Ok(_) => tracing::info!("Mobile Relay auto-start skipped because it is disabled"),
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
        assert!(validate_relay_url("ws://relay.example.test/v1/ws").is_err());
        assert_eq!(
            validate_relay_url("ws://127.0.0.1:8787").unwrap(),
            "ws://127.0.0.1:8787/v1/ws"
        );
    }
}
