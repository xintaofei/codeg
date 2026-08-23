import { isLocalDesktop } from "@/lib/platform"

export interface BrowserSurfaceTab {
  id: string
  url: string
  title: string
}

export interface BrowserSurfaceSnapshot {
  sessionId: string
  surfaceKind: "frame" | "native"
  tabs: BrowserSurfaceTab[]
  activeTargetId: string | null
  active: {
    tab: BrowserSurfaceTab
    loading: boolean
    canGoBack: boolean
    canGoForward: boolean
  } | null
}

export interface BrowserSurfaceFrame {
  targetId: string
  data: string
  mimeType: "image/jpeg"
  deviceWidth: number
  deviceHeight: number
  pageScaleFactor: number
}

export type BrowserSurfaceEvent =
  | { type: "snapshot"; snapshot: BrowserSurfaceSnapshot }
  | { type: "frame"; frame: BrowserSurfaceFrame }
  | {
      type: "error"
      code: string
      message: string
      retryable: boolean
    }

export type BrowserSurfaceInput =
  | {
      kind: "mouse"
      event: "pressed" | "released" | "moved" | "wheel"
      x: number
      y: number
      button?: "none" | "left" | "middle" | "right"
      deltaX?: number
      deltaY?: number
      modifiers?: number
    }
  | {
      kind: "key"
      event: "down" | "up"
      key: string
      code?: string
      text?: string
      modifiers?: number
    }
  | { kind: "text"; text: string }

export type BrowserSurfaceAction =
  | { action: "open"; url?: string }
  | { action: "focus"; targetId: string }
  | { action: "close"; targetId: string }
  | {
      action: "surface"
      bounds: { x: number; y: number; width: number; height: number }
      visible: boolean
    }
  | { action: "resize"; width: number; height: number }
  | { action: "navigate"; url: string }
  | { action: "back" }
  | { action: "forward" }
  | { action: "reload" }
  | { action: "stop" }
  | { action: "input"; input: BrowserSurfaceInput }

async function core() {
  if (!isLocalDesktop()) {
    throw new Error(
      "Browser surface is only available in the local desktop client"
    )
  }
  return await import("@tauri-apps/api/core")
}

export async function attachBrowserSurface(
  connectionId: string,
  onEvent: (event: BrowserSurfaceEvent) => void
): Promise<BrowserSurfaceSnapshot> {
  const { Channel, invoke } = await core()
  const channel = new Channel<BrowserSurfaceEvent>(onEvent)
  return await invoke<BrowserSurfaceSnapshot>("browser_surface_attach", {
    connectionId,
    onEvent: channel,
  })
}

export async function detachBrowserSurface(
  connectionId: string
): Promise<void> {
  const { invoke } = await core()
  await invoke("browser_surface_detach", { connectionId })
}

export async function closeBrowserSurface(connectionId: string): Promise<void> {
  const { invoke } = await core()
  await invoke("browser_surface_close", { connectionId })
}

export async function runBrowserSurfaceAction(
  connectionId: string,
  action: BrowserSurfaceAction
): Promise<BrowserSurfaceSnapshot> {
  const { invoke } = await core()
  return await invoke<BrowserSurfaceSnapshot>("browser_surface_action", {
    connectionId,
    action,
  })
}
