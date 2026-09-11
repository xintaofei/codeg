// Wire types of the built-in browser — the TypeScript mirror of
// `src-tauri/src/browser/types.rs`. Field names are camelCase and enum values
// kebab-case on both sides; change them together.

export type SurfaceKind = "child" | "window"

/** What a tab shows: a web page, or a local HTML file through the document
 *  guest (`codeg-doc:`), which the file column hosts in place of the inline
 *  HTML preview. */
export type TabKind = "page" | "document"

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

/** What an agent may do with a tab. Nothing is ever granted automatically —
 *  not by address, not by which process is listening, not by an allow-list,
 *  and not for a tab the agent opened itself. The only way in is a person
 *  sharing the tab. Reading counts: an unshared page leaks through a snapshot
 *  exactly as much as through a click. */
export type GrantLevel = "none" | "read" | "control"

/** A grant in force on one tab. Absent means none. */
export interface AgentGrant {
  /** Never `"none"`: a tab with no grant carries no `AgentGrant`. */
  level: GrantLevel
  /** The origin the tab was showing when it was shared. The grant ends the
   *  moment the page leaves it, so one share covers a whole dev loop —
   *  reloads and route changes on the same site — and nothing beyond it. */
  origin: string
  /** Unix milliseconds. */
  grantedAt: number
}

/** Why a tab's grant changed (`browser://agent-grant`). The level itself
 *  travels with the tab on `browser://state`, which stays the one place to
 *  read what it is now; this says what the state cannot — that the change was
 *  the page's doing rather than the user's. */
export type GrantChange = "granted" | "revoked" | "navigated"

export interface AgentGrantPayload {
  tabId: string
  change: GrantChange
  level: GrantLevel
  /** The origin just granted, or the one just lost. */
  origin: string | null
}

/** A page as an agent reads it (`browser_agent_snapshot`). */
export interface PageSnapshot {
  /** Opaque token a later ref must quote. */
  generation: string
  /** The address the page was at when it was walked. */
  url: string
  title: string
  viewport: { width: number; height: number; dpr: number }
  /** The aria tree, in Playwright's `ai` rendering. */
  tree: string
  refsCount: number
  /** The tree stops at `maxChars` rather than at the end of the page. */
  truncated: boolean
}

/** Full per-tab state; every `browser://state` event carries one. */
export interface BrowserTabState {
  tabId: string
  ownerWindow: string
  kind: TabKind
  surface: SurfaceKind
  channel: ChannelKind
  /** Why the page channel could not be installed, in the engine's own words.
   *  Null while it is fine — and also while it is merely still coming up,
   *  which is what `channel: "degraded"` means until the helper says hello. */
  channelError: string | null
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
  /** The browser profile (cookie jar, storage) the tab lives in; a popup
   *  shares its opener's. Null for a document guest. */
  profile: string | null
  /** What an agent may do with this tab. Null is the default and the resting
   *  state. */
  agentGrant: AgentGrant | null
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

/** A site rule as the backend sees it (same shape as `host-rules.ts`). */
export interface WireHostRule {
  pattern: string
  action: "builtin" | "system" | "block"
}

/** The administrator's policy in force. */
export interface BrowserPolicyStatus {
  /** `false`: the built-in browser is turned off machine-wide. */
  enabled: boolean
  /** Rules fixed by the administrator; shown read-only, consulted first. */
  managedRules: WireHostRule[]
  /** Path of the policy file, when one was read. */
  managedSource: string | null
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
  policy: BrowserPolicyStatus
  /** Local HTML files can be shown through the document guest (an embedded
   *  surface with a handler for `codeg-doc:`). */
  docGuest: boolean
  /** More than the default browser profile can exist (macOS 14+, Windows,
   *  Linux); the settings offer to create, clear and delete them. */
  profiles: boolean
  /** Tabs present the sign-in user agent to Google's sign-in hosts when the
   *  preference is on (embedded tabs, and the owned window on Linux). */
  signInUserAgent: boolean
  /** A tab shown in an owned window still answers find, history, stop and
   *  snapshots, and its page still talks to the host — true where the owned
   *  window is the surface the platform shim is written for (Linux). */
  ownedWindowControls: boolean
}

/** How a document guest serves its file: as a picture of itself (no script,
 *  no connection), or with its own scripts running, confined to its folder. */
export type DocMode = "safe" | "dynamic"

export type DocResetReason = "newer" | "changed"

/** Why a guest fell back to safe mode on its own: `path` (relative to the
 *  root) was found newer than the approval, or different from what had been
 *  served since it. */
export interface DocReset {
  path: string
  reason: DocResetReason
}

/** `browser://doc-state`: mode and status of one document guest. */
export interface DocGuestState {
  tabId: string
  mode: DocMode
  /** Absolute directory every request is confined to. */
  root: string
  /** Absolute path of the document. */
  entry: string
  /** The document's URL inside the guest. */
  url: string
  reset: DocReset | null
}

/** The last frame of a page, returned by a hide-with-freeze: the placeholder
 *  paints it while the surface is hidden under an overlay. */
export interface FrozenFrame {
  mime: string
  /** Base64 of the encoded image. */
  data: string
  width: number
  height: number
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
  /** The profile an adopted popup lives in (its opener's, as the backend
   *  knows it); null when denied. */
  profile: string | null
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
  /** The profile the new tab belongs in (the opener's for a modifier-click);
   *  null leaves the choice to the frontend. */
  profile: string | null
}

export const BROWSER_OPEN_REQUEST_EVENT = "browser://open-request"
export const BROWSER_STATE_EVENT = "browser://state"
export const BROWSER_CLOSED_EVENT = "browser://closed"
export const BROWSER_POPUP_EVENT = "browser://popup"
export const BROWSER_TELEMETRY_EVENT = "browser://telemetry"
export const BROWSER_DOWNLOAD_EVENT = "browser://download"
export const BROWSER_SHORTCUT_EVENT = "browser://shortcut"
export const BROWSER_NAVIGATION_BLOCKED_EVENT = "browser://navigation-blocked"
export const BROWSER_DOC_STATE_EVENT = "browser://doc-state"

/** `external` and `download` come from document guests only: a web address
 *  the document pointed at (the user may open it in a tab), and a download
 *  it tried to start (documents do not download). */
export type NavigationBlockReason =
  | "host-rule"
  | "scheme"
  | "external"
  | "download"

/** `browser://navigation-blocked`: a top-level navigation a tab attempted
 *  was refused by policy; the tab itself is unchanged. */
export interface BrowserNavigationBlockedPayload {
  tabId: string
  url: string
  reason: NavigationBlockReason
}

/** A browser shortcut the page had keyboard focus for. */
export interface BrowserShortcutPayload {
  tabId: string
  /** A name from the host's closed set; `find` today. */
  shortcut: string
}
