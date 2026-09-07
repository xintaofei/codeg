"use client"

import { useCallback, useEffect, useRef, useState } from "react"

import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import { useWorkspaceView } from "@/contexts/workspace-context"
import { useOptionalWorkbenchRoute } from "@/contexts/workbench-route-context"
import { useOverlayHostHidden } from "@/components/ui/overlay-host-hidden"
import {
  browserOpenTab,
  browserSetBounds,
  browserSetVisible,
} from "@/lib/browser/browser-api"
import {
  getBrowserTabState,
  setBrowserTabState,
} from "@/lib/browser/browser-tab-store"
import {
  useFallbackOverlayOpen,
  useNativeSurfaceOccluded,
} from "@/lib/browser/native-surface-occlusion"
import type { Bounds } from "@/lib/browser/types"
import { browserTabBackendId } from "@/lib/file-tab-id"
import { cn } from "@/lib/utils"

/** How often the host re-checks CSS visibility it cannot observe otherwise
 *  (a `visibility: hidden` ancestor toggled by the layout). One
 *  `checkVisibility()` call per tick. */
const VISIBILITY_POLL_MS = 500

// Backend ids whose surface this window has asked to create. Guards the
// StrictMode double-effect and re-mounts of the same tab: the webview lives
// as long as the tab record, not as long as this component.
const created = new Set<string>()

function measure(el: HTMLElement): Bounds {
  const rect = el.getBoundingClientRect()
  return { x: rect.left, y: rect.top, width: rect.width, height: rect.height }
}

function sameBounds(a: Bounds | null, b: Bounds): boolean {
  if (!a) return false
  return (
    Math.abs(a.x - b.x) < 0.5 &&
    Math.abs(a.y - b.y) < 0.5 &&
    Math.abs(a.width - b.width) < 0.5 &&
    Math.abs(a.height - b.height) < 0.5
  )
}

function elementVisible(el: HTMLElement): boolean {
  if (typeof el.checkVisibility === "function") {
    return el.checkVisibility({ visibilityProperty: true } as never)
  }
  return true
}

/**
 * The placeholder a browser tab's native webview is fitted to.
 *
 * The webview is a native view painted by the OS above the DOM at this
 * element's rect, so this component never renders page content itself. It
 * (1) creates the surface on first mount, (2) keeps the webview's bounds equal
 * to its own rect, and (3) hides the webview whenever the rect is not really
 * visible — the file column is CSS-hidden in conversation mode, an overlay
 * holds an occlusion lease, the tab is showing an error page — handing
 * keyboard focus back to the main webview first so the overlay gets Esc/Tab.
 */
export function BrowserSurfaceHost({
  tab,
  hidden = false,
  className,
}: {
  tab: BrowserWorkspaceTab
  /** Force-hide (e.g. while a DOM error page replaces the page). */
  hidden?: boolean
  className?: string
}) {
  const backendId = browserTabBackendId(tab.id)
  const ref = useRef<HTMLDivElement | null>(null)
  const lastBoundsRef = useRef<Bounds | null>(null)
  const lastVisibleRef = useRef<boolean | null>(null)
  const [createError, setCreateError] = useState<string | null>(null)
  const occluded = useNativeSurfaceOccluded()
  const fallbackOverlay = useFallbackOverlayOpen()
  const view = useWorkspaceView()
  const route = useOptionalWorkbenchRoute()
  const routeVisible = route ? route.isConversations : true
  // The whole workspace surface is CSS-hidden under a full-page route.
  const hostHidden = useOverlayHostHidden()
  const shouldShow =
    !hidden && !occluded && !fallbackOverlay && routeVisible && !hostHidden

  // One pass: measure, push bounds if they moved, push visibility if it
  // flipped. Called from every signal that could change either.
  const sync = useCallback(() => {
    const el = ref.current
    if (!el || !backendId) return
    const bounds = measure(el)
    const visible =
      shouldShow && bounds.width > 0 && bounds.height > 0 && elementVisible(el)
    if (visible && !sameBounds(lastBoundsRef.current, bounds)) {
      lastBoundsRef.current = bounds
      void browserSetBounds(backendId, bounds).catch(() => {})
    }
    if (lastVisibleRef.current !== visible) {
      lastVisibleRef.current = visible
      void browserSetVisible(backendId, visible, !visible).catch(() => {})
    }
  }, [backendId, shouldShow])

  // Create the surface once per tab record; adopted popups and re-mounts
  // already have one (the store knows about it).
  useEffect(() => {
    const el = ref.current
    if (!el || !backendId) return
    if (created.has(backendId) || getBrowserTabState(tab.id)) {
      lastBoundsRef.current = null
      lastVisibleRef.current = null
      sync()
      return
    }
    created.add(backendId)
    const bounds = measure(el)
    lastBoundsRef.current = bounds
    lastVisibleRef.current = true
    browserOpenTab({
      tabId: backendId,
      url: tab.browser.initialUrl,
      bounds,
      folderId: tab.folderId,
    })
      .then((next) => {
        setBrowserTabState(next)
        sync()
      })
      .catch((error: unknown) => {
        created.delete(backendId)
        setCreateError(String(error))
      })
    // Intentionally not re-run on `sync` identity changes: creation is a
    // one-shot per mount, the effect below handles every later sync.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [backendId, tab.id, tab.browser.initialUrl, tab.folderId])

  // Geometry and visibility tracking for the life of the mount.
  useEffect(() => {
    const el = ref.current
    if (!el || !backendId) return
    let frame = 0
    const schedule = () => {
      if (frame) return
      frame = requestAnimationFrame(() => {
        frame = 0
        sync()
      })
    }
    schedule()
    const observer =
      typeof ResizeObserver !== "undefined"
        ? new ResizeObserver(schedule)
        : null
    observer?.observe(el)
    window.addEventListener("resize", schedule)
    const poll = window.setInterval(sync, VISIBILITY_POLL_MS)
    return () => {
      if (frame) cancelAnimationFrame(frame)
      observer?.disconnect()
      window.removeEventListener("resize", schedule)
      window.clearInterval(poll)
    }
  }, [backendId, sync])

  // Layout-driven visibility flips (pane / mode / route) re-sync at once
  // instead of waiting for the poll.
  useEffect(() => {
    sync()
  }, [sync, view.mode, view.activePane, view.filesMaximized, routeVisible])

  // Unmount: the tab is no longer on screen (another tab took the pane, the
  // drawer closed, the panel went away). Hide, never destroy — the record
  // owns the surface.
  useEffect(() => {
    if (!backendId) return
    return () => {
      lastVisibleRef.current = null
      void browserSetVisible(backendId, false, false).catch(() => {})
    }
  }, [backendId])

  return (
    <div
      ref={ref}
      data-browser-surface={backendId ?? undefined}
      className={cn(
        "relative h-full w-full min-h-0 min-w-0 bg-background",
        className
      )}
      aria-hidden
    >
      {createError ? (
        <div className="absolute inset-0 flex items-center justify-center p-6 text-center text-sm text-destructive">
          {createError}
        </div>
      ) : null}
    </div>
  )
}
