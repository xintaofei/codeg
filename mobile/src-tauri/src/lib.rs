/// Mobile-only Tauri entry point.
///
/// Deliberately contains no Agent, ACP, PTY, Git, sidecar, updater, tray,
/// multi-window, or desktop IPC state. All task execution remains on the
/// configured Codeg server and the bundled React client communicates through
/// HTTPS and WebSocket transports.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_secure_vault::init())
        .run(tauri::generate_context!())
        .expect("error while running Codeg Mobile");
}
