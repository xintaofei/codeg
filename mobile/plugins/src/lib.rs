use tauri::{
  plugin::{Builder, TauriPlugin},
  Manager, Runtime,
};

pub use models::*;

#[cfg(desktop)]
mod desktop;
#[cfg(mobile)]
mod mobile;

mod commands;
mod error;
mod models;

pub use error::{Error, Result};

#[cfg(desktop)]
use desktop::SecureVault;
#[cfg(mobile)]
use mobile::SecureVault;

/// Extensions to [`tauri::App`], [`tauri::AppHandle`] and [`tauri::Window`] to access the secure-vault APIs.
pub trait SecureVaultExt<R: Runtime> {
  fn secure_vault(&self) -> &SecureVault<R>;
}

impl<R: Runtime, T: Manager<R>> crate::SecureVaultExt<R> for T {
  fn secure_vault(&self) -> &SecureVault<R> {
    self.state::<SecureVault<R>>().inner()
  }
}

/// Initializes the plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
  Builder::new("secure-vault")
    .invoke_handler(tauri::generate_handler![
      commands::store_secret,
      commands::load_secret,
      commands::delete_secret
    ])
    .setup(|app, api| {
      #[cfg(mobile)]
      let secure_vault = mobile::init(app, api)?;
      #[cfg(desktop)]
      let secure_vault = desktop::init(app, api)?;
      app.manage(secure_vault);
      Ok(())
    })
    .build()
}
