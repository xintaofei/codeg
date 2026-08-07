## Problem

On Linux (and other platforms), clicking the main window's close button always exits the entire application. There is no way to minimize to the system tray and keep the app running in the background.

The existing `can_hide_to_tray()` check was already able to detect tray availability, but the close button simply checked `can_hide_to_tray()` without consulting any user preference — if the tray was available, it always hid; if not, it always exited. There was no UI for the user to choose their preferred behavior.

## Solution

Add a configurable close-behavior setting with two options:

1. **Hide to tray (background)** — default. When the close button is clicked and tray is available, the window hides to the system tray. The app keeps running, and the tray icon restores the window. When tray is not available (e.g., GNOME 45+ without AppIndicator), this falls back to exiting.
2. **Exit application** — always exits the app on close button click, regardless of tray availability.

### Changes

**Backend (Rust):**
- `models/system.rs`: New `CloseAction` enum (`HideToTray` / `Exit`) and `SystemCloseSettings` struct, persisted via `app_metadata_service`.
- `commands/system_settings.rs`: `load_system_close_settings`, `get_system_close_settings`, `update_system_close_settings` — all gated behind `tauri-runtime` to avoid dead_code warnings in sidecar builds.
- `lib.rs`: Close button handler reads the stored setting and uses `CloseAction::HideToTray && can_hide_to_tray()` instead of `can_hide_to_tray()` alone.

**Frontend (TypeScript/React):**
- `lib/types.ts`: `CloseAction` type and `SystemCloseSettings` interface.
- `lib/api.ts`: `getSystemCloseSettings()` / `updateSystemCloseSettings()` transport wrappers.
- `components/settings/close-behavior-settings.tsx`: Radio-button UI with loading/saving states and error toast.
- `components/settings/general-settings.tsx`: Integrates the new section.
- `i18n/messages/*.json`: All 10 locales updated with the 4 new strings.

## Testing

- [x] 3,479 existing frontend tests pass (no regression).
- [x] Sidecar compiles cleanly (`cargo build --no-default-features --bin codeg-mcp`).
- [x] Main binary compiles cleanly (`cargo build --release --bin codeg`).
- [x] Setting persists across app restarts.
- [x] "Hide to tray" → close button hides window; tray icon restores it.
- [x] "Exit" → close button exits the app.
- [x] Default is "Hide to tray" (backward-compatible with existing behavior on tray-capable platforms).
- [x] Linux without tray: `can_hide_to_tray()` returns false, so both settings exit the app (no stranded process).
