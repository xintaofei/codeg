"use client"

import { useCallback, useRef, useState } from "react"
import type { PointerEvent as ReactPointerEvent, RefObject } from "react"
import { cn } from "@/lib/utils"
import type { HandleRect } from "@/lib/tab-group-layout"

interface GroupSplitHandleProps {
  handle: HandleRect
  /** The relative container the percentage rects are computed against. */
  containerRef: RefObject<HTMLDivElement | null>
  /** Forwarded to the store's `resizeGroupSplit`. */
  onResize: (splitId: string, index: number, boundaryFraction: number) => void
}

/**
 * Divider between two adjacent split groups: a 1px `bg-border` line (the
 * groups themselves draw no borders) with a centered ~9px pointer hit area,
 * thickening on hover/drag like the workspace `ResizableHandle`. Dragging maps
 * the cursor to a boundary fraction within the owning split node — the rects
 * are plain percentages of the shared container, so the math is a straight
 * projection against the container's bounding box.
 */
export function GroupSplitHandle({
  handle,
  containerRef,
  onResize,
}: GroupSplitHandleProps) {
  const [dragging, setDragging] = useState(false)
  const draggingRef = useRef(false)
  // Vertical divider between side-by-side children; horizontal when stacked.
  const verticalLine = handle.orientation === "horizontal"

  const resizeToPointer = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      const container = containerRef.current
      if (!container) return
      const bounds = container.getBoundingClientRect()
      // Degenerate geometry (a collapsed container mid-drag) would divide to
      // NaN/Infinity and poison the persisted ratios — bail instead.
      const axis = verticalLine ? bounds.width : bounds.height
      if (!(axis > 0) || handle.nodeExtent <= 0) return
      const cursorPct = verticalLine
        ? ((event.clientX - bounds.left) / axis) * 100
        : ((event.clientY - bounds.top) / axis) * 100
      const fraction = (cursorPct - handle.nodeStart) / handle.nodeExtent
      if (!Number.isFinite(fraction)) return
      onResize(handle.splitId, handle.index, fraction)
    },
    [containerRef, handle, onResize, verticalLine]
  )

  const handlePointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (event.button !== 0) return
      event.preventDefault()
      event.currentTarget.setPointerCapture(event.pointerId)
      draggingRef.current = true
      setDragging(true)
    },
    []
  )

  const handlePointerMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (!draggingRef.current) return
      resizeToPointer(event)
    },
    [resizeToPointer]
  )

  const endDrag = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return
    draggingRef.current = false
    setDragging(false)
    event.currentTarget.releasePointerCapture(event.pointerId)
  }, [])

  return (
    <div
      role="separator"
      aria-orientation={verticalLine ? "vertical" : "horizontal"}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      className={cn(
        "group/split-handle absolute z-20 flex touch-none select-none items-center justify-center",
        verticalLine ? "cursor-col-resize" : "cursor-row-resize"
      )}
      style={
        verticalLine
          ? {
              left: `calc(${handle.x}% - 4px)`,
              top: `${handle.y}%`,
              width: "9px",
              height: `${handle.length}%`,
            }
          : {
              left: `${handle.x}%`,
              top: `calc(${handle.y}% - 4px)`,
              width: `${handle.length}%`,
              height: "9px",
            }
      }
    >
      <div
        className={cn(
          "bg-border transition-[width,height,background-color] duration-150 ease-out",
          verticalLine
            ? cn(
                "h-full",
                dragging ? "w-[3px]" : "w-px group-hover/split-handle:w-[3px]"
              )
            : cn(
                "w-full",
                dragging ? "h-[3px]" : "h-px group-hover/split-handle:h-[3px]"
              ),
          dragging
            ? "bg-foreground/60"
            : "group-hover/split-handle:bg-foreground/40"
        )}
      />
    </div>
  )
}
