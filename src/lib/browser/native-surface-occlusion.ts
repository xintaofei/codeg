// Occlusion leases for native browser surfaces.
//
// A child webview is a native view: it paints above every DOM element of the
// workspace window, so a dialog, command palette, context menu or drawer that
// opens over a browser tab would be hidden behind the page. Overlays therefore
// hold a lease while they are open; while any lease exists the surface hosts
// hide their webviews (handing keyboard focus back to the main webview first,
// so Esc / Tab reach the overlay) and show them again when the last lease is
// released. Coarse on purpose — any overlay hides every surface — because the
// rect-intersection refinement buys little and costs a layout read per
// overlay open.
//
// Zero cost when no browser tab is showing: the store has no subscribers and
// acquire/release is a counter bump.

import { useEffect, useSyncExternalStore } from "react"

const holders = new Map<string, number>()
const listeners = new Set<() => void>()
let count = 0

function notify(): void {
  for (const listener of [...listeners]) listener()
}

/** Take a lease; the returned function releases it exactly once. */
export function acquireNativeSurfaceOcclusion(reason: string): () => void {
  holders.set(reason, (holders.get(reason) ?? 0) + 1)
  count += 1
  if (count === 1) notify()
  let released = false
  return () => {
    if (released) return
    released = true
    const remaining = (holders.get(reason) ?? 1) - 1
    if (remaining <= 0) holders.delete(reason)
    else holders.set(reason, remaining)
    count = Math.max(0, count - 1)
    if (count === 0) notify()
  }
}

export function isNativeSurfaceOccluded(): boolean {
  return count > 0
}

/** Diagnostic: who is holding leases right now. */
export function nativeSurfaceOcclusionHolders(): Record<string, number> {
  return Object.fromEntries(holders)
}

export function subscribeNativeSurfaceOcclusion(
  listener: () => void
): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

function getServerSnapshot(): boolean {
  return false
}

/** True while any overlay holds a lease. */
export function useNativeSurfaceOccluded(): boolean {
  return useSyncExternalStore(
    subscribeNativeSurfaceOcclusion,
    isNativeSurfaceOccluded,
    getServerSnapshot
  )
}

/**
 * For overlay components: hold a lease for as long as the component is
 * mounted (Radix content mounts only while open) and `active` is true.
 */
export function useNativeSurfaceOcclusion(reason: string, active = true): void {
  useEffect(() => {
    if (!active) return
    return acquireNativeSurfaceOcclusion(reason)
  }, [reason, active])
}

export function resetNativeSurfaceOcclusionForTests(): void {
  holders.clear()
  listeners.clear()
  count = 0
}

// ---------------------------------------------------------------------------
// Fallback detector for overlays that do not hold a lease (third-party
// portals, components that predate the lease). Watches the document for open
// dialog / menu roles. Only consulted when no lease is held, and only
// observing while some surface host is mounted.

const FALLBACK_SELECTOR =
  '[role="dialog"][data-state="open"], [role="alertdialog"][data-state="open"], [role="menu"][data-state="open"]'

const fallbackListeners = new Set<() => void>()
let fallbackObserver: MutationObserver | null = null
let fallbackOpen = false

function evaluateFallback(): void {
  const next =
    typeof document !== "undefined" &&
    document.querySelector(FALLBACK_SELECTOR) !== null
  if (next === fallbackOpen) return
  fallbackOpen = next
  for (const listener of [...fallbackListeners]) listener()
}

function subscribeFallback(listener: () => void): () => void {
  fallbackListeners.add(listener)
  if (
    !fallbackObserver &&
    typeof MutationObserver !== "undefined" &&
    typeof document !== "undefined"
  ) {
    fallbackObserver = new MutationObserver(evaluateFallback)
    fallbackObserver.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["data-state"],
    })
    evaluateFallback()
  }
  return () => {
    fallbackListeners.delete(listener)
    if (fallbackListeners.size === 0 && fallbackObserver) {
      fallbackObserver.disconnect()
      fallbackObserver = null
      fallbackOpen = false
    }
  }
}

/** True while an open dialog / menu is in the document (lease-less path). */
export function useFallbackOverlayOpen(): boolean {
  return useSyncExternalStore(
    subscribeFallback,
    () => fallbackOpen,
    getServerSnapshot
  )
}
