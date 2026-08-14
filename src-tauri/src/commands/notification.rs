#[cfg(feature = "tauri-runtime")]
use tauri::AppHandle;
use serde::Deserialize;

use crate::app_error::AppCommandError;

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn send_notification(
    app: AppHandle,
    title: String,
    body: String,
    target: Option<NotificationTarget>,
) -> Result<(), AppCommandError> {
    #[cfg(target_os = "macos")]
    {
        let app_id = if tauri::is_dev() {
            "com.apple.Terminal"
        } else {
            "app.codeg"
        };
        let _ = mac_notification_sys::set_application(app_id);

        let _ = mac_notification_sys::Notification::default()
            .title(&title)
            .message(&body)
            .send();
    }

    #[cfg(target_os = "windows")]
    {
        use tauri::Emitter;
        use tauri_winrt_notification::Toast;

        let app_for_click = app.clone();
        let target_for_click = target.clone();
        Toast::new("app.codeg")
            .title(&title)
            .text1(&body)
            .on_activated(move |_| {
                crate::commands::windows::show_main_window(&app_for_click);
                if let Some(target) = target_for_click.as_ref() {
                    let _ = app_for_click.emit_to(
                        "main",
                        "workspace://focus-conversation",
                        serde_json::json!({
                            "folderId": target.folder_id,
                            "conversationId": target.conversation_id,
                            "agent": target.agent,
                        }),
                    );
                }
                Ok(())
            })
            .show()
            .map_err(|error| AppCommandError::window("Failed to show notification", error.to_string()))?;
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        use tauri_plugin_notification::NotificationExt;
        let _ = app.notification().builder().title(title).body(body).show();
    }

    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
pub struct NotificationTarget {
    #[serde(rename = "folderId")]
    pub folder_id: i32,
    #[serde(rename = "conversationId")]
    pub conversation_id: i32,
    pub agent: String,
}
