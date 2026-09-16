# Windows Server Tray Design

## Goal

Give the standalone `codeg-server.exe` Windows release a native system-tray
presence with the same Codeg icon used by the web and desktop products. The
tray is a control surface for the headless server, not a hidden desktop window.

## Behavior

### Standalone server

- Tray icon: `src-tauri/icons/icon.ico` embedded in the Windows executable.
- Double click opens the server Web UI at the resolved local address
  (`HOST:PORT`). A single click has no side effect, matching normal Windows
  tray expectations and avoiding two browser launches for one double click.
- Menu item `Open Web Console` opens the same URL.
- Menu item `Open Logs Folder` opens the directory returned by
  `paths::codeg_logs_root()` in Windows Explorer. The directory contains the
  rotating `codeg-server.<date>.log` files.
- Menu item `Quit Codeg Server` requests graceful shutdown through the existing
  shutdown signal, allowing the HTTP server and log guard to flush before the
  process exits.
- Tray initialization failure is logged and does not prevent the HTTP server
  from starting.

### Desktop application

- Keep the existing left-click behavior: show and focus the native workspace.
- Add `Open Logs Folder` to the existing tray menu.
- Keep `Show Workspace` and `Quit Codeg` unchanged.
- Desktop and server use the same icon asset, but their activation targets are
  intentionally different because only the desktop build owns a native window.

## Architecture

The server build does not enable Tauri, so its tray implementation must live in
a small Windows-only module compiled into `codeg-server` under
`cfg(target_os = "windows")`. A native tray dependency with a message-loop
thread owns the icon and menu. The tray thread sends typed commands over a
channel to the server's async runtime:

```text
Windows tray callback
        |
        v
TrayCommand channel ----> server runtime
                              |-- open browser
                              |-- open Explorer
                              `-- trigger shutdown
```

The tray thread is started after the bind address and log directory are
resolved, so menu actions always use the actual runtime configuration. It is
stopped by the same shutdown path as the HTTP server. Non-Windows builds
compile a no-op starter and retain their current behavior.

The desktop menu is extended in the existing Tauri tray module. Its log action
calls the existing `open_logs_dir` command, while server mode opens the path
directly from the native tray handler because there is no Tauri command layer.

## Data and error handling

- The server passes a fully formed Web UI URL and an absolute logs directory to
  the tray starter; no environment is read from callbacks.
- Browser and Explorer launch failures are written to the normal server log,
  while the menu callback remains responsive.
- A closed command channel means the server is shutting down; callbacks become
  no-ops and never panic.
- Tray startup errors are non-fatal and include the target platform and error
  detail in the log.
- The icon is embedded at compile time so packaged releases do not depend on a
  neighboring `web/` file or the user's working directory.

## Testing and release checks

- Unit-test command routing and URL/path propagation without creating a real
  tray on non-Windows CI.
- Run `cargo check --no-default-features --bin codeg-server` on all existing
  targets.
- Extend the Windows release smoke test to assert the server binary is present;
  manual Windows validation confirms icon visibility, single/double activation,
  opening the logs directory, and graceful quit.
- Verify the existing desktop tray tests and `cargo check` remain unchanged on
  non-Windows targets.
