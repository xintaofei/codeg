"use client"

import { getActiveRemoteConnectionId } from "@/lib/transport"
import type { AppUpdateInfo } from "@/lib/updater"

/**
 * Persistence for the *availability* half of the update flow (is a newer
 * release out there?), kept separate from the backend-owned download/install
 * lifecycle in `update-provider.tsx`.
 *
 * Two facts survive a reload:
 *   * the last check and its result — so reloading a workspace window restores
 *     the badge instantly instead of re-fetching the manifest and going quiet
 *     for the first few seconds. Caching the *result*, not just the timestamp,
 *     is what keeps a throttled reload from claiming "you're up to date" when
 *     the previous check had in fact found something;
 *   * which version the user waved away — so the status-bar badge stops
 *     nagging for that release but comes back for the next one.
 *
 * Keys are scoped per backend: a remote-desktop window is the same browser
 * origin as the local app but reports a *different* server's version, so an
 * unscoped cache would show one backend's answer in the other's window.
 *
 * Every accessor is SSR- and private-mode-safe: a throwing/absent
 * `localStorage` degrades to "nothing remembered", never an exception.
 */

const LAST_CHECK_KEY = "codeg.updateCheck.last"
const DISMISSED_VERSION_KEY = "codeg.updateCheck.dismissedVersion"

export interface CachedUpdateCheck {
  /** Epoch ms when the check completed. */
  at: number
  currentVersion: string
  /** The newer release found, or null if already up to date. */
  info: AppUpdateInfo | null
}

function scoped(key: string): string {
  const remoteId = getActiveRemoteConnectionId()
  return remoteId ? `${key}:remote-${remoteId}` : key
}

/** The key `writeLastCheck` writes to, for `storage`-event listeners. Scoped for
 * the same reason as {@link dismissedVersionStorageKey}. */
export function lastCheckStorageKey(): string {
  return scoped(LAST_CHECK_KEY)
}

export function readLastCheck(): CachedUpdateCheck | null {
  if (typeof window === "undefined") return null
  try {
    const raw = localStorage.getItem(scoped(LAST_CHECK_KEY))
    if (!raw) return null
    const parsed = JSON.parse(raw) as unknown
    if (!parsed || typeof parsed !== "object") return null
    const c = parsed as Record<string, unknown>
    // Reject NaN and nonsense (e.g. written while the clock was wrong): a bad
    // timestamp must not permanently suppress checks.
    if (typeof c.at !== "number" || !Number.isFinite(c.at) || c.at <= 0) {
      return null
    }
    const info =
      c.info && typeof c.info === "object"
        ? (c.info as Record<string, unknown>)
        : null
    return {
      at: c.at,
      currentVersion:
        typeof c.currentVersion === "string" ? c.currentVersion : "",
      info:
        info && typeof info.version === "string"
          ? {
              version: info.version,
              body: typeof info.body === "string" ? info.body : "",
              date: typeof info.date === "string" ? info.date : null,
            }
          : null,
    }
  } catch {
    return null
  }
}

export function writeLastCheck(value: CachedUpdateCheck): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(scoped(LAST_CHECK_KEY), JSON.stringify(value))
  } catch {
    /* ignore */
  }
}

/** Drop the cached answer. Used when the running version no longer matches the
 * one the answer was computed against — most importantly the relaunch right
 * after an update lands, where the cache would otherwise advertise the very
 * release that was just installed. */
export function clearLastCheck(): void {
  if (typeof window === "undefined") return
  try {
    localStorage.removeItem(scoped(LAST_CHECK_KEY))
  } catch {
    /* ignore */
  }
}

/**
 * The key `writeDismissedVersion` writes to, for `storage`-event listeners.
 * Exposed because a dismissal is the one piece of update state a sibling window
 * changes WITHOUT touching the check cache, so listeners need to recognise it
 * on its own. Scoping matters here: a remote-desktop window is the same origin
 * as the local app, and must not react to the other backend's dismissals.
 */
export function dismissedVersionStorageKey(): string {
  return scoped(DISMISSED_VERSION_KEY)
}

/** The version the user dismissed the badge for, if any. */
export function readDismissedVersion(): string | null {
  if (typeof window === "undefined") return null
  try {
    return localStorage.getItem(scoped(DISMISSED_VERSION_KEY)) || null
  } catch {
    return null
  }
}

/** Pass null to clear (the dismissed release is no longer the newest one). */
export function writeDismissedVersion(version: string | null): void {
  if (typeof window === "undefined") return
  try {
    const key = scoped(DISMISSED_VERSION_KEY)
    if (version) localStorage.setItem(key, version)
    else localStorage.removeItem(key)
  } catch {
    /* ignore */
  }
}
