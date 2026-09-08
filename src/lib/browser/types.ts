// Wire types of the built-in browser — the TypeScript mirror of
// `src-tauri/src/browser/types.rs`. Field names are camelCase and enum values
// kebab-case on both sides; change them together.

export type SurfaceKind = "child" | "window"

export type ChannelKind = "native" | "degraded" | "legacy"

/** Logical pixels, the unit `getBoundingClientRect()` reports. */
export interface Bounds {
  x: number
  y: number
  width: number
  height: number
}

export type BrowserErrorKind =
  | "dns"
  | "tls"
  | "blocked"
  | "failed"
  | "popup-denied"

export interface BrowserErrorInfo {
  kind: BrowserErrorKind
  message: string
  url: string | null
}

/** Full per-tab state; every `browser://state` event carries one. */
export interface BrowserTabState {
  tabId: string
  ownerWindow: string
  surface: SurfaceKind
  channel: ChannelKind
  /** Last committed URL ("" until the first document commits). */
  url: string
  /** URL the last navigation asked for. */
  requestedUrl: string
  title: string
  favicon: string | null
  loading: boolean
  canGoBack: boolean
  canGoForward: boolean
  origin: string | null
  zoom: number
  error: BrowserErrorInfo | null
  remoteHost: string | null
  /** Set on a tab adopted from a page-initiated new-window request. */
  openerTabId: string | null
}

/** How the platform takes a change of the app's proxy setting. */
export type BrowserProxyApplies =
  | "live"
  | "next-tab"
  | "restart"
  | "unsupported"

export interface BrowserProxyStatus {
  /** Proxy browser tabs use, as `scheme://host:port`; null = direct. */
  url: string | null
  applies: BrowserProxyApplies
  /** Why `url` is null although a proxy is configured, or why the shown proxy
   *  is not the configured one (Windows until a restart). */
  reason: string | null
}

export interface BrowserCapabilities {
  available: boolean
  surface: SurfaceKind | null
  platform: string
  channel: ChannelKind
  reasons: string[]
  /** Browsing data lives apart from the app's own web storage. */
  isolatedStorage: boolean
  proxy: BrowserProxyStatus
  /** Absolute path a page's downloads land in. */
  downloadsDir: string
}

export type BrowserDownloadState = "started" | "completed" | "failed"

/** `browser://download`: one record, emitted when it starts and when it ends. */
export interface BrowserDownload {
  id: string
  tabId: string
  url: string
  fileName: string
  /** Absolute path the engine writes to; never overwrites an existing file. */
  path: string
  state: BrowserDownloadState
}

export type SurfaceChoice = "auto" | "child" | "window"

export interface BrowserClosedPayload {
  tabId: string
  ownerWindow: string
}

export type PopupPresentation = "adopted" | "denied"

export interface BrowserPopupPayload {
  presentation: PopupPresentation
  openerTabId: string
  tabId: string | null
  url: string
  requestedSize: [number, number] | null
  reason: string | null
}

/** `browser://telemetry`: page-side data forwarded as-is; never trust it. */
export interface BrowserTelemetryPayload {
  tabId: string
  kind: "gesture"
  untrusted: true
  mainFrame: boolean
  top: boolean
  payload: unknown
}

/** Backend → frontend: open this URL as a browser tab (agent tools, deep
 *  links, the dev puppet). The frontend owns the tab records. */
export interface BrowserOpenRequestPayload {
  url: string
  source: string
  activate: boolean
  ownerWindow: string | null
  /** Backend id of the tab the request came from (modifier-click), if any. */
  openerTabId: string | null
}

export const BROWSER_OPEN_REQUEST_EVENT = "browser://open-request"
export const BROWSER_STATE_EVENT = "browser://state"
export const BROWSER_CLOSED_EVENT = "browser://closed"
export const BROWSER_POPUP_EVENT = "browser://popup"
export const BROWSER_TELEMETRY_EVENT = "browser://telemetry"
export const BROWSER_DOWNLOAD_EVENT = "browser://download"
