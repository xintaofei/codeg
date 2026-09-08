// Live state of built-in browser tabs, keyed by the WORKSPACE tab id
// (`browser:<backend id>`). Fed by `BrowserEventsBridge` from the
// `browser://state` stream and by the surface host after `browser_open_tab`;
// read through `useSyncExternalStore` so only components looking at one tab
// re-render when that tab changes. Kept out of the workspace tab record on
// purpose: loading progress and URL changes are frequent and must not churn
// the whole `fileTabs` slice.

import { useSyncExternalStore } from "react"

import { browserClose } from "./browser-api"
import type { BrowserTabState } from "./types"
import { buildFileTabId } from "@/lib/file-tab-id"
import { isDesktop } from "@/lib/transport"

type Listener = () => void

const states = new Map<string, BrowserTabState>()
const listeners = new Set<Listener>()

// Backend ids whose surface this document has asked the backend to create,
// each with the token of the claim that asked. The surface host consults it
// so a StrictMode double effect or a re-mount of the same tab never asks
// twice: the webview lives as long as the tab record, not as long as the host
// component. Released together with the state, which is what lets a suspended
// tab be brought back through the same host.
//
// The token exists because `browser_open_tab` is a round trip: by the time it
// answers, the tab may have been closed (claim released) or shown again
// (claim re-taken). A holder whose token is no longer the current one owns a
// surface nobody is going to use, and must close it — otherwise the native
// webview stays painted over the workspace with no tab behind it.
const createdSurfaces = new Map<string, number>()
let nextClaimToken = 0

/** Claim the right to create a surface; null when someone already holds it. */
export function claimSurfaceCreation(backendTabId: string): number | null {
  if (createdSurfaces.has(backendTabId)) return null
  nextClaimToken += 1
  createdSurfaces.set(backendTabId, nextClaimToken)
  return nextClaimToken
}

/** Whether `token` is still the live claim for this tab. */
export function surfaceClaimIsCurrent(
  backendTabId: string,
  token: number
): boolean {
  return createdSurfaces.get(backendTabId) === token
}

export function forgetSurfaceCreation(backendTabId: string): void {
  createdSurfaces.delete(backendTabId)
}

// When each tab's surface host last went away (`null` while one is mounted).
// A host is mounted exactly while the tab is on screen — the active tab of a
// pane or the viewer drawer — so this is "how long has this page been in the
// background", which the optional background unload is based on.
const hiddenAt = new Map<string, number | null>()

export function markBrowserTabShown(workspaceTabId: string): void {
  hiddenAt.set(workspaceTabId, null)
}

export function markBrowserTabHidden(workspaceTabId: string): void {
  hiddenAt.set(workspaceTabId, Date.now())
}

/** Milliseconds-since-epoch the tab left the screen; `null` while it is on
 *  screen; `undefined` for a tab that was never shown in this document. */
export function browserTabHiddenAt(
  workspaceTabId: string
): number | null | undefined {
  return hiddenAt.get(workspaceTabId)
}

function notify(): void {
  for (const listener of [...listeners]) listener()
}

/** Workspace tab id for a backend tab id. */
export function browserWorkspaceTabId(backendTabId: string): string {
  return buildFileTabId({ kind: "browser", id: backendTabId })
}

export function getBrowserTabState(
  workspaceTabId: string
): BrowserTabState | null {
  return states.get(workspaceTabId) ?? null
}

/** Replace a tab's state (identity changes only when the payload does). */
export function setBrowserTabState(state: BrowserTabState): void {
  const key = browserWorkspaceTabId(state.tabId)
  const previous = states.get(key)
  if (previous && shallowEqualState(previous, state)) return
  states.set(key, state)
  notify()
}

export function removeBrowserTabState(workspaceTabId: string): void {
  const hadNotice = notices.delete(workspaceTabId)
  hiddenAt.delete(workspaceTabId)
  findRequests.delete(workspaceTabId)
  if (states.delete(workspaceTabId) || hadNotice) notify()
}

export function subscribeBrowserTabs(listener: Listener): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

function getServerSnapshot(): null {
  return null
}

/** Live state for one tab, or null before its first `browser://state`. */
export function useBrowserTabState(
  workspaceTabId: string | null
): BrowserTabState | null {
  return useSyncExternalStore(
    subscribeBrowserTabs,
    () => (workspaceTabId ? (states.get(workspaceTabId) ?? null) : null),
    getServerSnapshot
  )
}

/**
 * Tear down a tab's native surface and forget its state. Idempotent on the
 * backend side, so calling it for a tab that never got a surface is fine.
 * Afterwards the tab record is back to "not loaded": a surface host mounting
 * for it creates a fresh surface (that is how a suspended tab resumes).
 */
export function releaseBrowserTab(workspaceTabId: string): void {
  removeBrowserTabState(workspaceTabId)
  const backendId = workspaceTabId.startsWith("browser:")
    ? decodeURIComponent(workspaceTabId.slice("browser:".length))
    : null
  if (!backendId) return
  forgetSurfaceCreation(backendId)
  if (isDesktop()) {
    void browserClose(backendId).catch(() => {
      /* already gone */
    })
  }
}

/** A transient, dismissible message shown between the toolbar and the page. */
export interface BrowserTabNotice {
  kind: "popup-denied"
  url: string
  reason: string | null
}

const notices = new Map<string, BrowserTabNotice>()

export function setBrowserTabNotice(
  workspaceTabId: string,
  notice: BrowserTabNotice | null
): void {
  if (notice) notices.set(workspaceTabId, notice)
  else if (!notices.delete(workspaceTabId)) return
  notify()
}

export function useBrowserTabNotice(
  workspaceTabId: string | null
): BrowserTabNotice | null {
  return useSyncExternalStore(
    subscribeBrowserTabs,
    () => (workspaceTabId ? (notices.get(workspaceTabId) ?? null) : null),
    getServerSnapshot
  )
}

// Per-tab counter of "open the find bar" requests. The page owns ⌘F while it
// has keyboard focus (the app's DOM never sees that keystroke), so the host
// forwards it as `browser://shortcut` and it arrives here; the tab view
// watches the counter rather than a boolean, so a second ⌘F on an already
// open bar still re-focuses it.
const findRequests = new Map<string, number>()

export function requestBrowserFind(workspaceTabId: string): void {
  findRequests.set(workspaceTabId, (findRequests.get(workspaceTabId) ?? 0) + 1)
  notify()
}

export function useBrowserFindRequest(workspaceTabId: string | null): number {
  return useSyncExternalStore(
    subscribeBrowserTabs,
    () => (workspaceTabId ? (findRequests.get(workspaceTabId) ?? 0) : 0),
    getServerZero
  )
}

function getServerZero(): number {
  return 0
}

export function resetBrowserTabStoreForTests(): void {
  states.clear()
  notices.clear()
  listeners.clear()
  createdSurfaces.clear()
  hiddenAt.clear()
  findRequests.clear()
}

function shallowEqualState(a: BrowserTabState, b: BrowserTabState): boolean {
  const keys = Object.keys(a) as (keyof BrowserTabState)[]
  if (keys.length !== Object.keys(b).length) return false
  for (const key of keys) {
    const x = a[key]
    const y = b[key]
    if (x === y) continue
    // `error` is the only nested object; compare it structurally.
    if (
      key === "error" &&
      x &&
      y &&
      typeof x === "object" &&
      typeof y === "object"
    ) {
      if (JSON.stringify(x) === JSON.stringify(y)) continue
    }
    return false
  }
  return true
}
