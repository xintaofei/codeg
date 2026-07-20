"use client"

import { useEffect, useRef, type ReactNode } from "react"
import {
  useOverlayScrollbars,
  type UseOverlayScrollbarsParams,
} from "overlayscrollbars-react"

// Mirrors ui/scroll-area.tsx's ScrollArea with x="scroll" y="hidden".
const TILE_SCROLL_OPTIONS: UseOverlayScrollbarsParams["options"] = {
  scrollbars: {
    theme: "os-theme-codeg",
    autoHide: "leave",
    clickScroll: true,
  },
  overflow: { x: "scroll", y: "hidden" },
}

/**
 * Horizontal scroller for the tiled conversation row that only pays for
 * OverlayScrollbars while tiling is on. Untiled (the common case) nothing can
 * scroll here — each tab scrolls internally — yet a permanently initialized
 * instance kept reacting to every streaming DOM mutation with a
 * MutationObserver update that forces reflow (`scrollWidth` reads on dirty
 * layout). The host/contents pair renders unconditionally so flipping
 * `canTile` mid-stream never remounts the children (which would tear down a
 * live response); only the OverlayScrollbars decoration is created/destroyed
 * around them.
 */
export function TileScrollContainer({
  canTile,
  children,
}: {
  canTile: boolean
  children: ReactNode
}) {
  const hostRef = useRef<HTMLDivElement | null>(null)
  const contentsRef = useRef<HTMLDivElement | null>(null)
  // No `defer`: a deferred creation could fire after `canTile` flipped back
  // off (nothing short of unmounting cancels it) and leave an orphaned live
  // instance on the untiled wrapper. Initialization happens on an explicit
  // mode switch here, not in the first-paint path `defer` protects.
  const [initialize, instance] = useOverlayScrollbars({
    options: TILE_SCROLL_OPTIONS,
  })

  useEffect(() => {
    if (!canTile) return
    const host = hostRef.current
    const contents = contentsRef.current
    if (!host || !contents) return
    initialize({
      target: host,
      elements: { viewport: contents, content: contents },
    })
    return () => {
      instance()?.destroy()
    }
  }, [canTile, initialize, instance])

  // data-overlayscrollbars-initialize only while tiled: pre-init it hides
  // native scrollbars, but its stylesheet also forces `overflow: auto` on an
  // uninitialized host, which would override `overflow-hidden` for the
  // untiled state (plain-CSS rules beat layered Tailwind utilities).
  return (
    <div
      ref={hostRef}
      data-overlayscrollbars-initialize={canTile ? "" : undefined}
      className="h-full w-full overflow-hidden"
    >
      {/* h-full carries the height chain while uninitialized; once
          initialized, the OverlayScrollbars stylesheet's flex sizing takes
          over. */}
      <div
        ref={contentsRef}
        data-overlayscrollbars-contents=""
        className="h-full w-full"
      >
        {children}
      </div>
    </div>
  )
}
