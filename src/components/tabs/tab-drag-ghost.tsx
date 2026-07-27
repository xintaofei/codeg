"use client"

import { useEffect } from "react"
import { createPortal } from "react-dom"
import { useTabStore } from "@/contexts/tab-context"

/**
 * Floating chip that follows the pointer while a conversation tab is dragged
 * over ANOTHER split group. The dragged tab itself is axis-locked inside its
 * own strip (Reorder `drag="x"` + overflow clipping), so this ghost is the
 * only visual that crosses group boundaries — it appears exactly while a
 * foreign drop target is live, alongside that target's highlight.
 *
 * Portal to <body>: ancestors animate with transforms, which would re-anchor
 * `position: fixed` to themselves instead of the viewport.
 */
export function TabDragGhost() {
  const drag = useTabStore((s) => s.tabDrag)
  const dragging = drag != null

  // While ANY tab drag is live, suppress text selection document-wide: the
  // press that starts the drag also starts a browser text selection, and as
  // the pointer sweeps other groups (headers, message bodies) it smears
  // highlight across everything it crosses — noise the user then has to
  // click away after every drop. `select-none` on <body> stops the spread
  // (descendants' `user-select: auto` resolves through it), and clearing the
  // selection on release removes anything that slipped in between the press
  // and the first drag frame.
  useEffect(() => {
    if (!dragging) return
    document.body.classList.add("select-none")
    return () => {
      document.body.classList.remove("select-none")
      window.getSelection()?.removeAllRanges()
    }
  }, [dragging])

  if (!drag || drag.overGroupId == null) return null
  return createPortal(
    <div
      aria-hidden
      className="pointer-events-none fixed z-[100] flex max-w-56 items-center rounded-md border border-border bg-background/95 px-2.5 py-1 text-xs text-foreground shadow-md"
      style={{ left: drag.x + 10, top: drag.y + 12 }}
    >
      <span className="truncate">{drag.title}</span>
    </div>,
    document.body
  )
}
