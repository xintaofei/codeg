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
  if (states.delete(workspaceTabId)) notify()
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
 */
export function releaseBrowserTab(workspaceTabId: string): void {
  removeBrowserTabState(workspaceTabId)
  const backendId = workspaceTabId.startsWith("browser:")
    ? decodeURIComponent(workspaceTabId.slice("browser:".length))
    : null
  if (backendId && isDesktop()) {
    void browserClose(backendId).catch(() => {
      /* already gone */
    })
  }
}

export function resetBrowserTabStoreForTests(): void {
  states.clear()
  listeners.clear()
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
