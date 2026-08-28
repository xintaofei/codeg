# Windows Server Tray Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Windows-native tray icon and controls to `codeg-server.exe`, and add an “Open Logs Folder” action to the existing desktop tray.

**Architecture:** Add a `server_tray` library module compiled only for Windows. It owns a `tray-icon` + `tao` event-loop thread, embeds the existing PNG/ICO assets, and sends `OpenWeb`, `OpenLogs`, and `Quit` commands to the server runtime. The server wraps `axum::serve` with graceful shutdown driven by the tray’s `Quit` command. The existing Tauri tray menu receives one additional log item.

**Tech Stack:** Rust 2021, Axum, Tokio, `tray-icon` 0.21, `tao` 0.34, `open` 5, Tauri 2 desktop tray.

---

### Task 1: Add testable tray command routing

**Files:**
- Create: `src-tauri/src/server_tray.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/Cargo.lock`

- [ ] **Step 1: Define the Windows-only command API and non-Windows stub**

Expose `TrayCommand`, `TrayHandle`, and `start(url, logs_dir)` from `server_tray`. On non-Windows targets, `start` returns `Ok(None)` so server builds remain headless and testable.

- [ ] **Step 2: Add target-specific dependencies**

Add `tray-icon = "0.21.3"`, `tao = "0.34.5"`, and `open = "5"` under `target.'cfg(target_os = "windows")'.dependencies`. Keep the existing Tauri tray dependency and server dependency graph separate through `cfg`.

- [ ] **Step 3: Add pure tests for URL and log-path propagation**

Test the public `TrayConfig`/command construction without creating a native event loop. The test must assert the exact URL and absolute log directory passed to the platform starter.

- [ ] **Step 4: Run the focused library test**

Run from `src-tauri`:

```text
cargo test --no-default-features server_tray
```

Expected result: the non-Windows stub tests pass; on a Windows runner the same tests compile with the native module.

### Task 2: Implement the Windows tray event loop

**Files:**
- Modify: `src-tauri/src/server_tray.rs`
- Add binary asset reference: `src-tauri/icons/icon.png`

- [ ] **Step 1: Build the event-loop thread**

Create a `tao::EventLoop<UserEvent>` on a dedicated thread, forward `TrayIconEvent` and `MenuEvent` through `EventLoopProxy`, and create the tray icon during `StartCause::Init`.

- [ ] **Step 2: Build the menu and icon**

Use `muda::Menu`, `MenuItem`, and `PredefinedMenuItem` through `tray_icon::menu`. Use `include_bytes!("../icons/icon.png")` plus the existing `image` decoder to construct `tray_icon::Icon`. Menu ids must be stable constants: `server-tray:open-web`, `server-tray:open-logs`, and `server-tray:quit`.

- [ ] **Step 3: Route interactions**

Handle `TrayIconEvent::DoubleClick` with the left button by sending `OpenWeb`. Handle menu ids by sending `OpenWeb`, `OpenLogs`, or `Quit`; on `Quit`, drop the tray icon and set the event loop control flow to exit.

- [ ] **Step 4: Launch external applications safely**

Perform `open::that(url)` and `open::that(logs_dir)` inside the tray thread, logging failures with `tracing::warn!`. Callback failures must not panic or terminate the tray loop.

### Task 3: Connect the tray to server startup and graceful shutdown

**Files:**
- Modify: `src-tauri/src/bin/codeg_server.rs`
- Modify: `src-tauri/src/web/mod.rs`

- [ ] **Step 1: Add a server-owned shutdown sender**

Create a `tokio::sync::oneshot` channel after the listener binds. Start the tray with the final advertised URL and `codeg_logs_root()` path, then spawn a blocking receiver task that resolves when the tray sends `Quit`.

- [ ] **Step 2: Use Axum graceful shutdown**

Replace the bare `axum::serve(listener, router).await` with `with_graceful_shutdown` awaiting either the tray receiver or the existing process shutdown condition. Trigger `state.web_server_state.shutdown_signal()` before the graceful future resolves so WebSocket handlers drain consistently.

- [ ] **Step 3: Keep startup resilient**

Log tray initialization errors and continue serving HTTP. Keep the `TrayHandle` alive until `axum::serve` returns, then drop it before the existing office-watch cleanup.

- [ ] **Step 4: Run server checks**

Run:

```text
cargo check --no-default-features --bin codeg-server
cargo test --no-default-features --bin codeg-server --lib
```

Expected result: Linux/macOS compile without the tray dependency and existing server tests pass.

### Task 4: Extend the desktop tray menu

**Files:**
- Modify: `src-tauri/src/commands/windows.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add a stable “Open Logs Folder” menu id and localized labels**

Extend `TrayLabels` for all existing locales and add `TRAY_MENU_ID_LOGS`.

- [ ] **Step 2: Add the menu item in install and refresh paths**

Insert the log item between “Show Workspace” and the separator in both `install_tray_icon` and `refresh_tray_menu`.

- [ ] **Step 3: Dispatch the action**

Handle `TRAY_MENU_ID_LOGS` in the app-wide `on_menu_event` callback by invoking the existing `open_logs_dir` command/core implementation and logging any error.

- [ ] **Step 4: Run desktop checks**

Run `cargo check --features tauri-runtime` and the existing Rust test command for the desktop feature.

### Task 5: Verify Windows release packaging

**Files:**
- Modify: `.github/workflows/release.yml`
- Modify: `.github/workflows/test.yml` only if a focused Windows check is needed

- [ ] **Step 1: Verify the release job carries the embedded icon**

Keep the existing Windows packaging paths unchanged; the icon is inside the PE resource and requires no extra `web/` file.

- [ ] **Step 2: Add a Windows smoke assertion**

After building `codeg-server.exe`, use a PowerShell PE/resource inspection available on the hosted runner to assert the binary has an icon resource. Keep the existing executable-presence and `codeg-mcp --help` checks.

- [ ] **Step 3: Run formatting and diff checks**

Run `cargo fmt --check`, `git diff --check`, and inspect the final release diff for unrelated changes.

- [ ] **Step 4: Commit implementation changes**

```text
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/server_tray.rs src-tauri/src/bin/codeg_server.rs src-tauri/src/web/mod.rs src-tauri/src/commands/windows.rs src-tauri/src/lib.rs .github/workflows/release.yml
git commit -m "feat(server): add Windows tray controls"
```

