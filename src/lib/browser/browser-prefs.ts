// Built-in browser preferences. Persisted per key in localStorage (one key per
// setting rather than one JSON blob, so the settings window and the workspace
// window never clobber each other's writes); read through a cached snapshot so
// `useSyncExternalStore` gets a stable object between changes. Same shape as
// `office-preview-prefs.ts`: a same-window custom event plus the native
// cross-window `storage` event keep every reader live.

import { useSyncExternalStore } from "react"

/** Where a clicked address came from; each source carries its own default. */
export type LinkSource =
  | "transcript"
  | "toolCard"
  | "terminal"
  | "editor"
  | "notification"

export const LINK_SOURCES: readonly LinkSource[] = [
  "transcript",
  "toolCard",
  "terminal",
  "editor",
  "notification",
]

export type LinkTarget = "builtin" | "system"

/** Runtime escape hatch over the compiled surface choice (§0.3 of the plan):
 *  lets a user on a platform whose child-webview path was never verified fall
 *  back to the owned-window surface without a rebuild. */
export type SurfaceOverride = "auto" | "child" | "window"

export interface BrowserPrefsSnapshot {
  defaultTarget: Readonly<Record<LinkSource, LinkTarget>>
  devtools: boolean
  surfaceOverride: SurfaceOverride
  firstOpenSeen: boolean
  /** Release the native surface of tabs that stay in the background for a
   *  while (they reload when shown again). Off by default: a page's state
   *  is worth more than its memory unless the user says otherwise. */
  suspendBackgroundTabs: boolean
}

export const DEFAULT_BROWSER_PREFS: BrowserPrefsSnapshot = Object.freeze({
  defaultTarget: Object.freeze({
    transcript: "builtin",
    toolCard: "builtin",
    terminal: "builtin",
    editor: "builtin",
    notification: "builtin",
  }),
  devtools: false,
  surfaceOverride: "auto",
  firstOpenSeen: false,
  suspendBackgroundTabs: false,
}) as BrowserPrefsSnapshot

const KEY_PREFIX = "browser:"
const CHANGE_EVENT = "codeg:browser-prefs-changed"

function targetKey(source: LinkSource): string {
  return `${KEY_PREFIX}default-target:${source}`
}
const DEVTOOLS_KEY = `${KEY_PREFIX}devtools`
const SURFACE_KEY = `${KEY_PREFIX}surface-override`
const FIRST_OPEN_KEY = `${KEY_PREFIX}first-open-seen`
const SUSPEND_KEY = `${KEY_PREFIX}suspend-background-tabs`

function readRaw(key: string): string | null {
  if (typeof window === "undefined") return null
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

function parseTarget(raw: string | null): LinkTarget | null {
  return raw === "builtin" || raw === "system" ? raw : null
}

function parseSurface(raw: string | null): SurfaceOverride | null {
  return raw === "auto" || raw === "child" || raw === "window" ? raw : null
}

function read(): BrowserPrefsSnapshot {
  const defaultTarget = {} as Record<LinkSource, LinkTarget>
  for (const source of LINK_SOURCES) {
    defaultTarget[source] =
      parseTarget(readRaw(targetKey(source))) ??
      DEFAULT_BROWSER_PREFS.defaultTarget[source]
  }
  return {
    defaultTarget,
    devtools: readRaw(DEVTOOLS_KEY) === "true",
    surfaceOverride:
      parseSurface(readRaw(SURFACE_KEY)) ??
      DEFAULT_BROWSER_PREFS.surfaceOverride,
    firstOpenSeen: readRaw(FIRST_OPEN_KEY) === "true",
    suspendBackgroundTabs: readRaw(SUSPEND_KEY) === "true",
  }
}

let cached: BrowserPrefsSnapshot | null = null

/** Current preferences. Cached until a write or a cross-window change. */
export function getBrowserPrefs(): BrowserPrefsSnapshot {
  if (cached === null) cached = read()
  return cached
}

function write(key: string, value: string | null): void {
  if (typeof window === "undefined") return
  try {
    if (value === null) localStorage.removeItem(key)
    else localStorage.setItem(key, value)
  } catch {
    /* quota / privacy mode: keep the in-memory value only */
  }
  cached = null
  window.dispatchEvent(new CustomEvent(CHANGE_EVENT))
}

export function setDefaultLinkTarget(
  source: LinkSource,
  target: LinkTarget
): void {
  write(targetKey(source), target)
}

/** The "always use the system browser" toast action: every source at once. */
export function setAllDefaultLinkTargets(target: LinkTarget): void {
  for (const source of LINK_SOURCES) write(targetKey(source), target)
}

export function setBrowserDevtools(enabled: boolean): void {
  write(DEVTOOLS_KEY, enabled ? "true" : "false")
}

export function setBrowserSurfaceOverride(value: SurfaceOverride): void {
  write(SURFACE_KEY, value === "auto" ? null : value)
}

export function markBrowserFirstOpenSeen(): void {
  write(FIRST_OPEN_KEY, "true")
}

export function setBrowserSuspendBackgroundTabs(enabled: boolean): void {
  write(SUSPEND_KEY, enabled ? "true" : null)
}

export function subscribeBrowserPrefs(listener: () => void): () => void {
  if (typeof window === "undefined") return () => {}
  const onChange = () => listener()
  const onStorage = (event: StorageEvent) => {
    // A `null` key is `localStorage.clear()`; anything under our prefix is ours.
    if (event.key === null || event.key.startsWith(KEY_PREFIX)) {
      cached = null
      listener()
    }
  }
  window.addEventListener(CHANGE_EVENT, onChange)
  window.addEventListener("storage", onStorage)
  return () => {
    window.removeEventListener(CHANGE_EVENT, onChange)
    window.removeEventListener("storage", onStorage)
  }
}

function getServerSnapshot(): BrowserPrefsSnapshot {
  return DEFAULT_BROWSER_PREFS
}

/** Reactive read; re-renders on same-window and cross-window changes. */
export function useBrowserPrefs(): BrowserPrefsSnapshot {
  return useSyncExternalStore(
    subscribeBrowserPrefs,
    getBrowserPrefs,
    getServerSnapshot
  )
}

/** Drop the cache and every stored key (tests only). */
export function resetBrowserPrefsForTests(): void {
  cached = null
  if (typeof window === "undefined") return
  try {
    for (const source of LINK_SOURCES)
      localStorage.removeItem(targetKey(source))
    localStorage.removeItem(DEVTOOLS_KEY)
    localStorage.removeItem(SURFACE_KEY)
    localStorage.removeItem(FIRST_OPEN_KEY)
    localStorage.removeItem(SUSPEND_KEY)
  } catch {
    /* ignore */
  }
}
