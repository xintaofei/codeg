use tauri::{AppHandle, command, Runtime};

use crate::models::*;
use crate::Result;
use crate::SecureVaultExt;

#[command]
pub(crate) async fn store_secret<R: Runtime>(
    app: AppHandle<R>,
    payload: SecretValueRequest,
) -> Result<()> {
    app.secure_vault().store_secret(payload)
}

#[command]
pub(crate) async fn load_secret<R: Runtime>(
    app: AppHandle<R>,
    payload: SecretKeyRequest,
) -> Result<SecretValueResponse> {
    app.secure_vault().load_secret(payload)
}

#[command]
pub(crate) async fn delete_secret<R: Runtime>(
    app: AppHandle<R>,
    payload: SecretKeyRequest,
) -> Result<()> {
    app.secure_vault().delete_secret(payload)
}
