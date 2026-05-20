import {
  getActiveRemoteConnectionId,
  isDesktop,
  getTransport,
} from "./transport"
import type { EventStream, UnsubscribeFn } from "./transport/types"

/**
 * Platform-aware API wrappers for features that differ between
 * Tauri desktop and web browser environments.
 */

export { isDesktop }

/**
 * Subscribe to backend events.
 * Uses Tauri listen() in desktop mode, WebSocket in web mode.
 */
export async function subscribe<T>(
  event: string,
  handler: (payload: T) => void
): Promise<UnsubscribeFn> {
  return getTransport().subscribe(event, handler)
}

/**
 * Register a callback to fire after a WebSocket transport reconnects.
 * Returns an unsubscribe function. Returns `null` on IPC-only transports
 * (desktop Tauri) where there's no disconnect window to recover from —
 * callers that re-fetch state on reconnect can safely no-op in that case.
 *
 * Use this alongside `subscribe()` for state that must be re-synced after
 * a network blip: the broadcaster drops events while `receiver_count == 0`,
 * so anything fired during the disconnect window is lost.
 */
export function onTransportReconnect(
  callback: () => void
): UnsubscribeFn | null {
  return getTransport().onReconnect?.(callback) ?? null
}

/**
 * Per-connection Subscribe-with-Snapshot stream. Returns `null` only on
 * the desktop Tauri transport (which uses local IPC and is race-free, so
 * the legacy `subscribe()` flow stays as the fallback). Web and remote-
 * desktop transports always return an EventStream.
 *
 * The returned EventStream instance is owned by the transport: it survives
 * across calls and re-attaches its subscriptions on reconnect. Don't
 * cache it across remote-workspace swaps — call this each time you need
 * to attach so you bind to the currently-active transport.
 */
export function getEventStream(): EventStream | null {
  const transport = getTransport()
  const factory = transport.eventStream
  if (!factory) return null
  return factory.call(transport)
}

/**
 * Open a URL in the default browser (desktop) or new tab (web).
 */
export async function openUrl(url: string): Promise<void> {
  if (isDesktop() && getActiveRemoteConnectionId() === null) {
    const { openUrl: tauriOpenUrl } = await import("@tauri-apps/plugin-opener")
    await tauriOpenUrl(url)
  } else {
    window.open(url, "_blank")
  }
}

/**
 * Open a path in the system file manager (desktop only).
 * No-op in web mode.
 */
export async function openPath(path: string): Promise<void> {
  if (isDesktop() && getActiveRemoteConnectionId() === null) {
    const { openPath: tauriOpenPath } =
      await import("@tauri-apps/plugin-opener")
    await tauriOpenPath(path)
  }
}

/**
 * Reveal a file/directory in the system file manager (desktop only).
 * No-op in web mode.
 */
export async function revealItemInDir(path: string): Promise<void> {
  if (isDesktop() && getActiveRemoteConnectionId() === null) {
    const { revealItemInDir: tauriReveal } =
      await import("@tauri-apps/plugin-opener")
    await tauriReveal(path)
  }
}

/**
 * Open a native file/directory dialog (desktop) or fallback (web).
 */
export async function openFileDialog(options?: {
  directory?: boolean
  multiple?: boolean
  title?: string
  defaultPath?: string
}): Promise<string | string[] | null> {
  if (isDesktop() && getActiveRemoteConnectionId() === null) {
    const { open } = await import("@tauri-apps/plugin-dialog")
    return open(options ?? {})
  }
  // Web fallback: for directory selection, prompt for server-side path.
  // For file selection, use a hidden file input.
  if (options?.directory) {
    const path = window.prompt(
      options?.title ?? "输入服务端目录路径 (Enter server directory path)"
    )
    return path || null
  }
  return new Promise((resolve) => {
    const input = document.createElement("input")
    input.type = "file"
    if (options?.multiple) input.multiple = true
    input.onchange = () => {
      if (!input.files?.length) {
        resolve(null)
        return
      }
      const paths = Array.from(input.files).map((f) => f.name)
      resolve(options?.multiple ? paths : paths[0])
    }
    input.click()
  })
}

/**
 * Get the current Tauri window (desktop only).
 * Returns null in web mode.
 */
export async function getCurrentWindow() {
  if (isDesktop()) {
    const { getCurrentWindow: tauriGetCurrentWindow } =
      await import("@tauri-apps/api/window")
    return tauriGetCurrentWindow()
  }
  return null
}

/**
 * Close the current window.
 * Desktop: closes Tauri window. Web: navigates back or closes tab.
 */
export async function closeCurrentWindow(): Promise<void> {
  if (isDesktop()) {
    const win = await getCurrentWindow()
    await win?.close()
  } else {
    window.history.back()
  }
}
