// Thin transport wrappers over the `browser_*` commands. Desktop only: the
// commands exist in the Tauri runtime alone, and `browserCapabilities()`
// answers `{ available: false }` everywhere else so callers can branch on one
// value instead of on the runtime.

import { getTransport, isDesktop } from "@/lib/transport"

import type {
  Bounds,
  BrowserCapabilities,
  BrowserDownload,
  BrowserTabState,
  SurfaceChoice,
} from "./types"

const UNAVAILABLE: BrowserCapabilities = {
  available: false,
  surface: null,
  platform: "web",
  channel: "degraded",
  reasons: ["built-in browser needs the desktop runtime"],
  isolatedStorage: false,
  proxy: { url: null, applies: "unsupported", reason: null },
  downloadsDir: "",
}

let capabilitiesPromise: Promise<BrowserCapabilities> | null = null
let resolvedCapabilities: BrowserCapabilities | null = null

/** Cached for the session: the answer cannot change while the app runs. */
export function browserCapabilities(): Promise<BrowserCapabilities> {
  if (!isDesktop()) {
    resolvedCapabilities = UNAVAILABLE
    return Promise.resolve(UNAVAILABLE)
  }
  if (!capabilitiesPromise) {
    capabilitiesPromise = getTransport()
      .call<BrowserCapabilities>("browser_capabilities", {})
      .then((caps) => {
        resolvedCapabilities = caps
        return caps
      })
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

/**
 * Synchronous view of the answer, for decisions that must happen inside a
 * click's call stack. `null` until `browserCapabilities()` has resolved once
 * (the events bridge asks at startup); callers treat null as "not available"
 * and fall back to the system browser, which is always a safe answer.
 */
export function browserCapabilitiesSnapshot(): BrowserCapabilities | null {
  return resolvedCapabilities
}

/**
 * Fresh answer, bypassing the session cache: the proxy part changes whenever
 * the user edits the app's proxy setting, and the settings section shows it.
 */
export function browserCapabilitiesNow(): Promise<BrowserCapabilities> {
  if (!isDesktop()) return Promise.resolve(UNAVAILABLE)
  return getTransport().call<BrowserCapabilities>("browser_capabilities", {})
}

/** Tests only. */
export function resetBrowserCapabilitiesCacheForTests(): void {
  capabilitiesPromise = null
  resolvedCapabilities = null
}

/** Tests only: pretend the capabilities already resolved. */
export function setBrowserCapabilitiesForTests(
  caps: BrowserCapabilities | null
): void {
  resolvedCapabilities = caps
}

export interface OpenBrowserTabParams {
  tabId: string
  url: string
  bounds: Bounds
  background?: boolean
  surface?: SurfaceChoice
  folderId?: number | null
  /** Build the surface with the web inspector available. */
  devtools?: boolean
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
    devtools: params.devtools ?? false,
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

/** Wipe cookies, caches and storage shared by every built-in browser tab. */
export function browserClearData(): Promise<void> {
  return getTransport().call<void>("browser_clear_data", {})
}

/** Downloads this run started, oldest first. */
export function browserListDownloads(): Promise<BrowserDownload[]> {
  return getTransport().call<BrowserDownload[]>("browser_list_downloads", {})
}

/** Forget the records; the downloaded files stay where they are. */
export function browserClearDownloads(): Promise<void> {
  return getTransport().call<void>("browser_clear_downloads", {})
}
