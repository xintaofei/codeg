// Thin transport wrappers over the `browser_*` commands. Desktop only: the
// commands exist in the Tauri runtime alone, and `browserCapabilities()`
// answers `{ available: false }` everywhere else so callers can branch on one
// value instead of on the runtime.

import { getTransport, isDesktop } from "@/lib/transport"

import type {
  Bounds,
  BrowserCapabilities,
  BrowserTabState,
  SurfaceChoice,
} from "./types"

const UNAVAILABLE: BrowserCapabilities = {
  available: false,
  surface: null,
  platform: "web",
  channel: "degraded",
  reasons: ["built-in browser needs the desktop runtime"],
}

let capabilitiesPromise: Promise<BrowserCapabilities> | null = null

/** Cached for the session: the answer cannot change while the app runs. */
export function browserCapabilities(): Promise<BrowserCapabilities> {
  if (!isDesktop()) return Promise.resolve(UNAVAILABLE)
  if (!capabilitiesPromise) {
    capabilitiesPromise = getTransport()
      .call<BrowserCapabilities>("browser_capabilities", {})
      .catch((error: unknown) => {
        capabilitiesPromise = null
        return {
          ...UNAVAILABLE,
          reasons: [`browser_capabilities failed: ${String(error)}`],
        }
      })
  }
  return capabilitiesPromise
}

/** Tests only. */
export function resetBrowserCapabilitiesCacheForTests(): void {
  capabilitiesPromise = null
}

export interface OpenBrowserTabParams {
  tabId: string
  url: string
  bounds: Bounds
  background?: boolean
  surface?: SurfaceChoice
  folderId?: number | null
}

export function browserOpenTab(
  params: OpenBrowserTabParams
): Promise<BrowserTabState> {
  return getTransport().call<BrowserTabState>("browser_open_tab", {
    tabId: params.tabId,
    url: params.url,
    bounds: params.bounds,
    background: params.background ?? false,
    surface: params.surface ?? "auto",
    folderId: params.folderId ?? null,
  })
}

export function browserClose(tabId: string): Promise<void> {
  return getTransport().call<void>("browser_close", { tabId })
}

export function browserSetBounds(tabId: string, bounds: Bounds): Promise<void> {
  return getTransport().call<void>("browser_set_bounds", { tabId, bounds })
}

export function browserSetVisible(
  tabId: string,
  visible: boolean,
  handoffFocus = false
): Promise<void> {
  return getTransport().call<void>("browser_set_visible", {
    tabId,
    visible,
    handoffFocus,
  })
}

export function browserNavigate(
  tabId: string,
  url: string
): Promise<BrowserTabState> {
  return getTransport().call<BrowserTabState>("browser_navigate", {
    tabId,
    url,
  })
}

export function browserReload(tabId: string): Promise<void> {
  return getTransport().call<void>("browser_reload", { tabId })
}

export function browserStop(tabId: string): Promise<void> {
  return getTransport().call<void>("browser_stop", { tabId })
}

export function browserGoBack(tabId: string): Promise<void> {
  return getTransport().call<void>("browser_go_back", { tabId })
}

export function browserGoForward(tabId: string): Promise<void> {
  return getTransport().call<void>("browser_go_forward", { tabId })
}

export function browserGetState(tabId: string): Promise<BrowserTabState> {
  return getTransport().call<BrowserTabState>("browser_get_state", { tabId })
}

export function browserListTabs(): Promise<BrowserTabState[]> {
  return getTransport().call<BrowserTabState[]>("browser_list_tabs", {})
}
