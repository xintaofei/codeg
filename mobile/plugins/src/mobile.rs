use serde::de::DeserializeOwned;
use tauri::{
  plugin::{PluginApi, PluginHandle},
  AppHandle, Runtime,
};

use crate::models::*;

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_secure_vault);

// initializes the Kotlin or Swift plugin classes
pub fn init<R: Runtime, C: DeserializeOwned>(
  _app: &AppHandle<R>,
  api: PluginApi<R, C>,
) -> crate::Result<SecureVault<R>> {
  #[cfg(target_os = "android")]
  let handle = api.register_android_plugin("cn.crain.codeg.securevault", "SecureVaultPlugin")?;
  #[cfg(target_os = "ios")]
  let handle = api.register_ios_plugin(init_plugin_secure_vault)?;
  Ok(SecureVault(handle))
}

/// Access to the secure-vault APIs.
pub struct SecureVault<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> SecureVault<R> {
  pub fn store_secret(&self, payload: SecretValueRequest) -> crate::Result<()> {
    self
      .0
      .run_mobile_plugin("storeSecret", payload)
      .map_err(Into::into)
  }

  pub fn load_secret(&self, payload: SecretKeyRequest) -> crate::Result<SecretValueResponse> {
    self
      .0
      .run_mobile_plugin("loadSecret", payload)
      .map_err(Into::into)
  }

  pub fn delete_secret(&self, payload: SecretKeyRequest) -> crate::Result<()> {
    self
      .0
      .run_mobile_plugin("deleteSecret", payload)
      .map_err(Into::into)
  }
}
