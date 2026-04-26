import { detectEnvironment } from "./transport/detect"
import { getTransport } from "./transport"
import type { UnsubscribeFn } from "./transport"

/**
 * Platform-aware API wrappers for features that differ between
 * Tauri desktop and web browser environments.
 */

export function isDesktop(): boolean {
  return detectEnvironment() === "tauri"
}

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
 * Open a URL in the default browser (desktop) or new tab (web).
 */
export async function openUrl(url: string): Promise<void> {
  if (isDesktop()) {
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
  if (isDesktop()) {
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
  if (isDesktop()) {
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
  if (isDesktop()) {
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
