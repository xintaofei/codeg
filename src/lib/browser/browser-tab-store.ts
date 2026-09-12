// Live state of built-in browser tabs, keyed by the WORKSPACE tab id
// (`browser:<backend id>`). Fed by `BrowserEventsBridge` from the
// `browser://state` stream and by the surface host after `browser_open_tab`;
// read through `useSyncExternalStore` so only components looking at one tab
// re-render when that tab changes. Kept out of the workspace tab record on
// purpose: loading progress and URL changes are frequent and must not churn
// the whole `fileTabs` slice.

import { useSyncExternalStore } from "react"

import { browserClose } from "./browser-api"
import type {
  AgentAction,
  AgentActivityPayload,
  AgentOutcome,
  BrowserTabState,
  DocGuestState,
  NavigationBlockReason,
} from "./types"
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

/** Whether anyone currently holds the claim for this tab. */
export function hasSurfaceClaim(backendTabId: string): boolean {
  return createdSurfaces.has(backendTabId)
}

export function forgetSurfaceCreation(backendTabId: string): void {
  createdSurfaces.delete(backendTabId)
}

// One promise chain per backend tab id, so the create and destroy calls for
// an id happen in the order they were issued.
//
// A backend tab id is reused across generations: a suspended tab is released
// and, when the user comes back to it, created again under the SAME id. Both
// commands are round trips, and without this the backend could run them in
// either order — a close issued for generation 1 arriving after generation 2
// had registered would destroy the live surface and leave a tab that believes
// it is loaded showing nothing.
const surfaceOps = new Map<string, Promise<unknown>>()

export function runSurfaceOp<T>(
  backendTabId: string,
  op: () => Promise<T>
): Promise<T> {
  const previous = surfaceOps.get(backendTabId)
  // With nothing in flight the call goes out now — a decision to close a
  // surface should not wait for a microtask. Otherwise it queues, and runs
  // whether the previous op resolved or rejected: a failed close must not
  // stall every later operation on this tab.
  const next = previous ? previous.then(op, op) : op()
  const settled = next.then(
    () => {},
    () => {}
  )
  surfaceOps.set(backendTabId, settled)
  // Drop the chain once it drains, so a long session does not keep an entry
  // for every tab id it has ever seen.
  void settled.then(() => {
    if (surfaceOps.get(backendTabId) === settled) {
      surfaceOps.delete(backendTabId)
    }
  })
  return next
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
  const hadDoc = docStates.delete(workspaceTabId)
  hiddenAt.delete(workspaceTabId)
  findRequests.delete(workspaceTabId)
  // Counted among the reasons to notify: a strip still mounted over a tab
  // whose state had already gone would otherwise keep showing the lines of
  // the page that left. `hiddenAt` and `findRequests` are not — nothing
  // renders them on their own.
  const hadActivity = agentActivity.delete(workspaceTabId)
  if (states.delete(workspaceTabId) || hadNotice || hadDoc || hadActivity) {
    notify()
  }
}

// Mode and status of document guests (`browser://doc-state`), keyed like the
// tab state. Separate from it because it changes on its own schedule — the
// user's mode choice, a fall-back to safe mode — and carries no page state.
const docStates = new Map<string, DocGuestState>()

export function setDocGuestState(doc: DocGuestState): void {
  const key = browserWorkspaceTabId(doc.tabId)
  const previous = docStates.get(key)
  if (previous && JSON.stringify(previous) === JSON.stringify(doc)) return
  docStates.set(key, doc)
  notify()
}

export function getDocGuestState(workspaceTabId: string): DocGuestState | null {
  return docStates.get(workspaceTabId) ?? null
}

/** Live document-guest state for one tab, or null for a tab that is not a
 *  document (or before its first `browser://doc-state`). */
export function useDocGuestState(
  workspaceTabId: string | null
): DocGuestState | null {
  return useSyncExternalStore(
    subscribeBrowserTabs,
    () => (workspaceTabId ? (docStates.get(workspaceTabId) ?? null) : null),
    getServerSnapshot
  )
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
    void runSurfaceOp(backendId, () => browserClose(backendId)).catch(() => {
      /* already gone */
    })
  }
}

/** A transient, dismissible message shown between the toolbar and the page. */
export type BrowserTabNotice =
  | { kind: "popup-denied"; url: string; reason: string | null }
  /** A top-level navigation the tab attempted was refused by policy. */
  | { kind: "navigation-blocked"; url: string; reason: NavigationBlockReason }
  /** The page left the origin its grant was bound to, so agents lost access
   *  to this tab without the user doing anything. The one grant transition
   *  worth interrupting for — the other two the user just performed. */
  | { kind: "agent-grant-lost"; origin: string }

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

/** A run of identical attempts an agent made on one tab. */
export interface BrowserAgentActivity {
  action: AgentAction
  outcome: AgentOutcome
  /** Unix milliseconds of the most recent one. */
  at: number
  /** How many identical attempts this line stands for. */
  count: number
}

// What agents have done to each tab, newest first. Lives here rather than in
// the tab state because the backend does not keep it: it announces each
// attempt once and forgets, which is the right division — a record with no
// reader is a leak, and the reader is a pane that exists for as long as the
// tab does.
//
// Runs of the same (action, outcome) collapse into one line with a count. An
// agent working through a page reads it dozens of times; forty lines saying
// "read the page" hide the one that says something else, which is the only
// line worth having a strip for.
const AGENT_ACTIVITY_LIMIT = 50
const NO_ACTIVITY: readonly BrowserAgentActivity[] = []
const agentActivity = new Map<string, readonly BrowserAgentActivity[]>()

export function recordBrowserAgentActivity(
  payload: AgentActivityPayload
): void {
  const key = browserWorkspaceTabId(payload.tabId)
  const previous = agentActivity.get(key) ?? NO_ACTIVITY
  const head = previous[0]
  const next =
    head && head.action === payload.action && head.outcome === payload.outcome
      ? [
          { ...head, at: payload.at, count: head.count + 1 },
          ...previous.slice(1),
        ]
      : [
          {
            action: payload.action,
            outcome: payload.outcome,
            at: payload.at,
            count: 1,
          },
          ...previous.slice(0, AGENT_ACTIVITY_LIMIT - 1),
        ]
  agentActivity.set(key, next)
  notify()
}

/** What agents have done to this tab, newest first. Empty until something
 *  has. Not a log: nothing persists it, and it dies with the tab. */
export function useBrowserAgentActivity(
  workspaceTabId: string | null
): readonly BrowserAgentActivity[] {
  return useSyncExternalStore(
    subscribeBrowserTabs,
    () =>
      workspaceTabId
        ? (agentActivity.get(workspaceTabId) ?? NO_ACTIVITY)
        : NO_ACTIVITY,
    getNoActivity
  )
}

function getNoActivity(): readonly BrowserAgentActivity[] {
  return NO_ACTIVITY
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
  docStates.clear()
  listeners.clear()
  createdSurfaces.clear()
  surfaceOps.clear()
  hiddenAt.clear()
  findRequests.clear()
  agentActivity.clear()
}

function shallowEqualState(a: BrowserTabState, b: BrowserTabState): boolean {
  const keys = Object.keys(a) as (keyof BrowserTabState)[]
  if (keys.length !== Object.keys(b).length) return false
  for (const key of keys) {
    const x = a[key]
    const y = b[key]
    if (x === y) continue
    // Nested fields (`error`, `agentGrant`) are rebuilt by the deserializer
    // on every event, so identity says nothing about them; compare them
    // structurally. Deliberately by shape rather than by naming the fields:
    // the third one to be added would otherwise make every `browser://state`
    // look like a change, and a loading page emits a lot of them.
    if (x && y && typeof x === "object" && typeof y === "object") {
      if (JSON.stringify(x) === JSON.stringify(y)) continue
    }
    return false
  }
  return true
}
