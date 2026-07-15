use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

use crate::models::*;

pub fn init<R: Runtime, C: DeserializeOwned>(
  app: &AppHandle<R>,
  _api: PluginApi<R, C>,
) -> crate::Result<SecureVault<R>> {
  Ok(SecureVault(app.clone()))
}

/// Access to the secure-vault APIs.
pub struct SecureVault<R: Runtime>(AppHandle<R>);

impl<R: Runtime> SecureVault<R> {
  pub fn store_secret(&self, _payload: SecretValueRequest) -> crate::Result<()> {
    Ok(())
  }

  pub fn load_secret(&self, _payload: SecretKeyRequest) -> crate::Result<SecretValueResponse> {
    Ok(SecretValueResponse { value: None })
  }

  pub fn delete_secret(&self, _payload: SecretKeyRequest) -> crate::Result<()> {
    Ok(())
  }
}
