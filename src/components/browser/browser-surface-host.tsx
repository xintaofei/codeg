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
  browserProfileExists,
  DEFAULT_BROWSER_PROFILE_ID,
  getBrowserPrefs,
} from "@/lib/browser/browser-prefs"
import { browserClose } from "@/lib/browser/browser-api"
import {
  claimSurfaceCreation,
  forgetSurfaceCreation,
  getBrowserTabState,
  hasSurfaceClaim,
  markBrowserTabHidden,
  markBrowserTabShown,
  releaseBrowserTab,
  runSurfaceOp,
  setBrowserTabState,
  surfaceClaimIsCurrent,
  useBrowserTabState,
} from "@/lib/browser/browser-tab-store"
import {
  useFallbackOverlayOpen,
  useNativeSurfaceOccluded,
} from "@/lib/browser/native-surface-occlusion"
import type { Bounds, BrowserTabState } from "@/lib/browser/types"
import { browserTabBackendId } from "@/lib/file-tab-id"
import { cn } from "@/lib/utils"

/** How often the host re-checks CSS visibility it cannot observe otherwise
 *  (a `visibility: hidden` ancestor toggled by the layout). One
 *  `checkVisibility()` call per tick. */
const VISIBILITY_POLL_MS = 500

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

export interface NativeSurfaceHostProps {
  /** The backend's id of the surface (label-safe). */
  backendId: string
  /** Where the surface's state lives in the tab store: `browser:<backendId>`
   *  (the workspace tab id for a browser tab; a key of its own for a
   *  document guest hosted by a file tab). */
  storeKey: string
  /** Create the surface at these bounds; resolves to its first state. Keep
   *  the identity stable for the life of the mount (memoize it): a new
   *  identity only re-syncs, never re-creates. */
  create: (bounds: Bounds) => Promise<BrowserTabState>
  /** Tear the surface down when this host unmounts. A browser tab's surface
   *  belongs to its tab record and merely hides (the record outlives every
   *  host); a document guest belongs to the preview on screen and goes with
   *  it — the document comes back the same from disk. */
  destroyOnUnmount?: boolean
  /** Force-hide (e.g. while a DOM error page replaces the page). */
  hidden?: boolean
  className?: string
}

/**
 * The placeholder a native webview is fitted to.
 *
 * The webview is a native view painted by the OS above the DOM at this
 * element's rect, so this component never renders page content itself. It
 * (1) creates the surface on first mount, (2) keeps the webview's bounds equal
 * to its own rect, and (3) hides the webview whenever the rect is not really
 * visible — the file column is CSS-hidden in conversation mode, an overlay
 * holds an occlusion lease, the tab is showing an error page — handing
 * keyboard focus back to the main webview first so the overlay gets Esc/Tab.
 */
export function NativeSurfaceHost({
  backendId,
  storeKey,
  create,
  destroyOnUnmount = false,
  hidden = false,
  className,
}: NativeSurfaceHostProps) {
  const ref = useRef<HTMLDivElement | null>(null)
  const lastBoundsRef = useRef<Bounds | null>(null)
  const lastVisibleRef = useRef<boolean | null>(null)
  const [createError, setCreateError] = useState<string | null>(null)
  // The page's last frame, painted here while the surface is hidden under
  // an overlay (a data URL). Cleared once the surface is showing again — not
  // before, or the pane would flash blank between the two.
  const [frozen, setFrozen] = useState<string | null>(null)
  // Which hide a frame belongs to: one that lands after a later show or hide
  // is dropped rather than painted over the wrong state.
  const hideSeqRef = useRef(0)
  // The error page replacing the surface also replaces any frame: a frame
  // kept through an error would resurface, stale, when the error clears
  // under a still-open overlay. Adjusted during render on the prop change.
  const [wasHidden, setWasHidden] = useState(hidden)
  if (wasHidden !== hidden) {
    setWasHidden(hidden)
    if (hidden) setFrozen(null)
  }
  const occluded = useNativeSurfaceOccluded()
  const fallbackOverlay = useFallbackOverlayOpen()
  const view = useWorkspaceView()
  const route = useOptionalWorkbenchRoute()
  const routeVisible = route ? route.isConversations : true
  // The whole workspace surface is CSS-hidden under a full-page route.
  const hostHidden = useOverlayHostHidden()
  const shouldShow =
    !hidden && !occluded && !fallbackOverlay && routeVisible && !hostHidden
  // Hidden ONLY because an overlay is open over it: the placeholder stays on
  // screen, so it is worth a freeze frame. The other reasons (error page,
  // full-page route, hidden column) take the placeholder off screen too.
  const overlayHide =
    !hidden && routeVisible && !hostHidden && (occluded || fallbackOverlay)

  // One pass: measure, push bounds if they moved, push visibility if it
  // flipped. Called from every signal that could change either.
  const sync = useCallback(() => {
    const el = ref.current
    if (!el) return
    // An owned window is not fitted to this element — the placeholder is
    // invisible by design — so only the "is this tab on screen" signals
    // apply, and there are no bounds to push.
    if (getBrowserTabState(storeKey)?.surface === "window") {
      if (lastVisibleRef.current !== shouldShow) {
        lastVisibleRef.current = shouldShow
        void browserSetVisible(backendId, shouldShow, !shouldShow).catch(
          () => {}
        )
      }
      return
    }
    const bounds = measure(el)
    const visible =
      shouldShow && bounds.width > 0 && bounds.height > 0 && elementVisible(el)
    if (visible && !sameBounds(lastBoundsRef.current, bounds)) {
      lastBoundsRef.current = bounds
      void browserSetBounds(backendId, bounds).catch(() => {})
    }
    if (lastVisibleRef.current !== visible) {
      lastVisibleRef.current = visible
      hideSeqRef.current += 1
      const seq = hideSeqRef.current
      if (visible) {
        // The frame goes once the native view is back — and only if this
        // show is still the latest request: an overlay closed and reopened
        // at once has a newer hide in flight, whose frame this answer must
        // not wipe from under it.
        void browserSetVisible(backendId, true, false)
          .catch(() => {})
          .finally(() => {
            if (hideSeqRef.current === seq) setFrozen(null)
          })
      } else {
        const freeze =
          overlayHide &&
          bounds.width > 0 &&
          bounds.height > 0 &&
          elementVisible(el)
        void browserSetVisible(backendId, false, true, freeze)
          .then((frame) => {
            if (frame && hideSeqRef.current === seq) {
              setFrozen(`data:${frame.mime};base64,${frame.data}`)
            }
          })
          .catch(() => {})
      }
    }
  }, [backendId, overlayHide, shouldShow, storeKey])

  // Whether this tab currently has a live surface. Also the re-creation
  // signal: if the state goes away while this host is mounted — the tab was
  // released by the background unload just as the user switched to it — the
  // effect below runs again and loads the page instead of leaving a blank
  // pane behind.
  const loaded = useBrowserTabState(storeKey) !== null

  // Create the surface once per tab record; adopted popups and re-mounts
  // already have one (the store knows about it). A record whose surface was
  // released (background unload) is "not loaded" again and gets a new one
  // here, at the URL the record was updated to.
  useEffect(() => {
    const el = ref.current
    if (!el) return
    if (getBrowserTabState(storeKey)) {
      lastBoundsRef.current = null
      lastVisibleRef.current = null
      sync()
      return
    }
    const token = claimSurfaceCreation(backendId)
    if (token === null) {
      sync()
      return
    }
    const bounds = measure(el)
    lastBoundsRef.current = bounds
    lastVisibleRef.current = true
    // Queued per tab id: a close issued for an earlier generation must reach
    // the backend before this create, never after it.
    runSurfaceOp(backendId, () => create(bounds))
      .then((next) => {
        if (!surfaceClaimIsCurrent(backendId, token)) {
          // Someone else claimed this id meanwhile: their own create is
          // already queued behind us, and closing here would land after it
          // and destroy THEIR surface. Only clean up when the id is
          // ownerless — the tab was closed while this was in flight.
          if (!hasSurfaceClaim(backendId)) {
            void runSurfaceOp(backendId, () => browserClose(backendId)).catch(
              () => {}
            )
          }
          return
        }
        setBrowserTabState(next)
        sync()
      })
      .catch((error: unknown) => {
        if (!surfaceClaimIsCurrent(backendId, token)) return
        forgetSurfaceCreation(backendId)
        setCreateError(String(error))
      })
    // Intentionally not re-run on `sync` identity changes: creation is a
    // one-shot per mount, the effect below handles every later sync.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [backendId, storeKey, create, loaded])

  // Geometry and visibility tracking for the life of the mount.
  useEffect(() => {
    const el = ref.current
    if (!el) return
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

  // Mount = the tab is on screen; unmount = it is no longer (another tab took
  // the pane, the drawer closed, the panel went away). A browser tab's
  // surface hides, never dies — the record owns it, and the store keeps the
  // timestamps so the optional background unload knows how long a page has
  // been off screen. A document guest is torn down: it is the preview's, and
  // the preview is gone.
  useEffect(() => {
    markBrowserTabShown(storeKey)
    return () => {
      lastVisibleRef.current = null
      hideSeqRef.current += 1
      markBrowserTabHidden(storeKey)
      if (destroyOnUnmount) {
        releaseBrowserTab(storeKey)
      } else {
        void browserSetVisible(backendId, false, false).catch(() => {})
      }
    }
  }, [backendId, destroyOnUnmount, storeKey])

  return (
    <div
      ref={ref}
      data-browser-surface={backendId}
      className={cn(
        "relative h-full w-full min-h-0 min-w-0 bg-background",
        className
      )}
      aria-hidden
    >
      {frozen ? (
        // A data URL the backend just produced, shown for the life of an
        // overlay: nothing for next/image to optimise, load or cache.
        // eslint-disable-next-line @next/next/no-img-element
        <img
          src={frozen}
          alt=""
          draggable={false}
          data-browser-frozen-frame=""
          className="pointer-events-none absolute inset-0 h-full w-full select-none object-cover object-left-top"
        />
      ) : null}
      {createError ? (
        <div className="absolute inset-0 flex items-center justify-center p-6 text-center text-sm text-destructive">
          {createError}
        </div>
      ) : null}
    </div>
  )
}

/**
 * The surface host of a browser tab: the workspace tab record names the
 * backend id and the URL, and the surface is created with the preferences
 * read at that moment (a surface cannot change its inspector or its kind
 * after it exists, so a settings change applies to new tabs).
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
  const initialUrl = tab.browser.initialUrl
  const profile = tab.browser.profile
  const folderId = tab.folderId
  const create = useCallback(
    (bounds: Bounds) => {
      const prefs = getBrowserPrefs()
      return browserOpenTab({
        tabId: backendId ?? "",
        url: initialUrl,
        bounds,
        folderId,
        surface: prefs.surfaceOverride,
        devtools: prefs.devtools,
        // The record is moved to the default profile when its own is
        // deleted; should a create run before that reaches this window, the
        // backend still must not recreate the deleted store.
        profile: browserProfileExists(prefs, profile)
          ? profile
          : DEFAULT_BROWSER_PROFILE_ID,
      })
    },
    [backendId, folderId, initialUrl, profile]
  )
  // A browser tab always has a backend id; anything else is not a browser
  // tab and gets no surface.
  if (!backendId) return null
  return (
    <NativeSurfaceHost
      backendId={backendId}
      storeKey={tab.id}
      create={create}
      hidden={hidden}
      className={className}
    />
  )
}
