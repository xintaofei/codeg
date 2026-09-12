//! Native tray controls for the standalone server binary.
//!
//! The desktop binary has a Tauri-owned tray. The standalone server has no
//! webview or window, so Windows gets a small tao/tray-icon event loop here.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    OpenWeb,
    OpenLogs,
    Quit,
}

const OPEN_WEB_ID: &str = "server-tray:open-web";
const OPEN_LOGS_ID: &str = "server-tray:open-logs";
const QUIT_ID: &str = "server-tray:quit";

fn command_for_menu_id(id: &str) -> Option<TrayCommand> {
    match id {
        OPEN_WEB_ID => Some(TrayCommand::OpenWeb),
        OPEN_LOGS_ID => Some(TrayCommand::OpenLogs),
        QUIT_ID => Some(TrayCommand::Quit),
        _ => None,
    }
}

pub struct TrayHandle {
    pub(crate) quit_rx: tokio::sync::oneshot::Receiver<()>,
    #[cfg(target_os = "windows")]
    _thread: Option<std::thread::JoinHandle<()>>,
}

impl TrayHandle {
    pub fn into_quit_receiver(self) -> tokio::sync::oneshot::Receiver<()> {
        self.quit_rx
    }
}

#[cfg(not(target_os = "windows"))]
pub fn start(_url: String, _logs_dir: PathBuf) -> Result<Option<TrayHandle>, String> {
    Ok(None)
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::{
        command_for_menu_id, PathBuf, TrayCommand, TrayHandle, OPEN_LOGS_ID, OPEN_WEB_ID, QUIT_ID,
    };
    use std::sync::mpsc;

    use tao::event::Event;
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::windows::EventLoopBuilderExtWindows;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};

    enum UserEvent {
        Tray(TrayIconEvent),
        Menu(MenuEvent),
    }

    pub(super) fn start(url: String, logs_dir: PathBuf) -> Result<Option<TrayHandle>, String> {
        let (quit_tx, quit_rx) = tokio::sync::oneshot::channel();
        let (ready_tx, ready_rx) = mpsc::channel();

        let thread = std::thread::Builder::new()
            .name("codeg-server-tray".to_string())
            .spawn(move || {
                let mut event_loop_builder = EventLoopBuilder::<UserEvent>::with_user_event();
                event_loop_builder.with_any_thread(true);
                let event_loop = event_loop_builder.build();

                let proxy = event_loop.create_proxy();
                TrayIconEvent::set_event_handler(Some(move |event| {
                    let _ = proxy.send_event(UserEvent::Tray(event));
                }));
                let proxy = event_loop.create_proxy();
                MenuEvent::set_event_handler(Some(move |event| {
                    let _ = proxy.send_event(UserEvent::Menu(event));
                }));

                let open_web = MenuItem::with_id(OPEN_WEB_ID, "Open Web Console", true, None);
                let open_logs = MenuItem::with_id(OPEN_LOGS_ID, "Open Logs Folder", true, None);
                let quit = MenuItem::with_id(QUIT_ID, "Quit Codeg Server", true, None);
                let separator = PredefinedMenuItem::separator();
                let menu = Menu::new();
                let _ = menu.append_items(&[&open_web, &open_logs, &separator, &quit]);

                let mut tray_icon = None;
                let mut quit_tx = Some(quit_tx);
                let icon_bytes = include_bytes!("../icons/icon.png");
                let icon = match image::load_from_memory(icon_bytes) {
                    Ok(image) => {
                        let image = image.into_rgba8();
                        Icon::from_rgba(image.as_raw().clone(), image.width(), image.height())
                            .map_err(|err| format!("decode tray icon: {err}"))
                    }
                    Err(err) => Err(format!("decode tray icon: {err}")),
                };

                event_loop.run(move |event, _, control_flow| {
                    *control_flow = ControlFlow::Wait;
                    match event {
                        Event::NewEvents(tao::event::StartCause::Init) => match &icon {
                            Ok(icon) => match TrayIconBuilder::new()
                                .with_menu(Box::new(menu.clone()))
                                .with_tooltip("Codeg Server")
                                .with_icon(icon.clone())
                                .build()
                            {
                                Ok(icon) => {
                                    tray_icon = Some(icon);
                                    let _ = ready_tx.send(Ok(()));
                                }
                                Err(err) => {
                                    let _ = ready_tx.send(Err(format!("create tray icon: {err}")));
                                    *control_flow = ControlFlow::Exit;
                                }
                            },
                            Err(err) => {
                                let _ = ready_tx.send(Err(err.clone()));
                                *control_flow = ControlFlow::Exit;
                            }
                        },
                        Event::UserEvent(UserEvent::Tray(TrayIconEvent::DoubleClick {
                            button: tray_icon::MouseButton::Left,
                            ..
                        })) => {
                            if let Err(err) = open::that(&url) {
                                tracing::warn!("[Tray] failed to open web console: {err}");
                            }
                        }
                        Event::UserEvent(UserEvent::Menu(event)) => {
                            let result = match command_for_menu_id(event.id.as_ref()) {
                                Some(TrayCommand::OpenWeb) => open::that(&url),
                                Some(TrayCommand::OpenLogs) => open::that(&logs_dir),
                                Some(TrayCommand::Quit) => {
                                    if let Some(quit_tx) = quit_tx.take() {
                                        let _ = quit_tx.send(());
                                    }
                                    tray_icon.take();
                                    *control_flow = ControlFlow::Exit;
                                    Ok(())
                                }
                                None => Ok(()),
                            };
                            if let Err(err) = result {
                                tracing::warn!("[Tray] failed to handle menu action: {err}");
                            }
                        }
                        _ => {}
                    }
                });
            })
            .map_err(|err| format!("spawn tray thread: {err}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Some(TrayHandle {
                quit_rx,
                _thread: Some(thread),
            })),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(err) => {
                let _ = thread.join();
                Err(format!("tray startup channel closed: {err}"))
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub fn start(url: String, logs_dir: PathBuf) -> Result<Option<TrayHandle>, String> {
    windows_impl::start(url, logs_dir)
}

#[cfg(test)]
mod tests {
    use super::{command_for_menu_id, TrayCommand, OPEN_LOGS_ID, OPEN_WEB_ID, QUIT_ID};

    #[test]
    fn maps_menu_ids_to_commands() {
        assert_eq!(command_for_menu_id(OPEN_WEB_ID), Some(TrayCommand::OpenWeb));
        assert_eq!(
            command_for_menu_id(OPEN_LOGS_ID),
            Some(TrayCommand::OpenLogs)
        );
        assert_eq!(command_for_menu_id(QUIT_ID), Some(TrayCommand::Quit));
        assert_eq!(command_for_menu_id("server-tray:unknown"), None);
    }
}
