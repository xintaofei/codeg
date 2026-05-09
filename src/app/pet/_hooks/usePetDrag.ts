"use client"

import { useCallback, useEffect, useRef } from "react"
import { isDesktop } from "@/lib/transport"
import { recordPetWindowPosition } from "@/lib/pet/api"
import type { PetState } from "@/lib/pet/animation"

const CLICK_THRESHOLD_PX = 5
const DIRECTION_IDLE_MS = 160
const DIRECTION_THRESHOLD_PX = 1
const PERSIST_DEBOUNCE_MS = 220

export interface UsePetDragOptions {
  /**
   * Fired during drag with the directional running state, or `null` when
   * the cursor stalls (held-still) so the parent can revert to the
   * default animation.
   */
  onDragDirection: (state: PetState | null) => void
  /**
   * Fired on pointer-up when total movement was below the click threshold.
   * Parents typically map this to a one-shot waving animation.
   */
  onClick: () => void
}

export interface UsePetDragResult {
  onPointerDown: (event: React.PointerEvent) => void
}

/**
 * Custom (non-native) drag for the floating pet window.
 *
 * Why not `WebviewWindow.startDragging()`?
 * - On macOS, native window drag enters a modal run-loop that throttles JS
 *   timers — the sprite animation freezes mid-drag.
 * - We can't observe the cursor delta during native drag, so we can't
 *   switch the sprite to the directional `running-right` / `running-left`
 *   row, which is the whole point of those Codex animation rows.
 *
 * Approach:
 * 1. On pointerdown, snapshot the screen pointer position and the window's
 *    current outer position (converted to logical pixels via the active
 *    monitor's scale factor).
 * 2. On each pointermove, compute the logical delta and `setPosition` to
 *    `start + delta`, scheduled through `requestAnimationFrame` to coalesce
 *    move events at display refresh rate.
 * 3. Track horizontal direction; emit `running_right` / `running_left` to
 *    the parent. After `DIRECTION_IDLE_MS` of no horizontal motion, clear
 *    the directional state so the pet returns to its base animation while
 *    the user holds the window still.
 * 4. On pointerup with cumulative travel < `CLICK_THRESHOLD_PX`, treat as a
 *    click and fire `onClick`. Otherwise clear the directional state and
 *    debounce-persist the final position to the DB.
 */
export function usePetDrag(opts: UsePetDragOptions): UsePetDragResult {
  const draggingRef = useRef(false)
  const startScreenRef = useRef<{ x: number; y: number } | null>(null)
  const startWinLogicalRef = useRef<{ x: number; y: number } | null>(null)
  const scaleFactorRef = useRef(1)
  const totalMovedRef = useRef(0)
  const lastScreenXRef = useRef<number | null>(null)
  const lastDirRef = useRef<"left" | "right" | null>(null)
  const dirIdleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const persistTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const rafRef = useRef<number | null>(null)
  const pendingPosRef = useRef<{ x: number; y: number } | null>(null)
  const setPositionFnRef = useRef<
    ((x: number, y: number) => Promise<void>) | null
  >(null)

  // Keep the latest opts in a ref so the global pointer listeners (set up
  // once) always call into the current closure, without re-binding on
  // every render.
  const optsRef = useRef(opts)
  useEffect(() => {
    optsRef.current = opts
  }, [opts])

  const persistPosition = useCallback(async () => {
    if (!isDesktop()) return
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window")
      const win = getCurrentWindow()
      const pos = await win.outerPosition()
      await recordPetWindowPosition(pos.x, pos.y)
    } catch (err) {
      console.warn("[Pet] failed to persist window position:", err)
    }
  }, [])

  useEffect(() => {
    function flushPosition() {
      const p = pendingPosRef.current
      pendingPosRef.current = null
      rafRef.current = null
      if (!p || !setPositionFnRef.current) return
      void setPositionFnRef.current(p.x, p.y)
    }

    function onPointerMove(e: PointerEvent) {
      if (!draggingRef.current) return
      const start = startScreenRef.current
      const winStart = startWinLogicalRef.current
      if (!start || !winStart) return

      const dxLog = e.screenX - start.x
      const dyLog = e.screenY - start.y
      const dist = Math.sqrt(dxLog * dxLog + dyLog * dyLog)
      if (dist > totalMovedRef.current) totalMovedRef.current = dist

      // Direction tracks the *instant* frame-to-frame motion, not cumulative
      // displacement from drag start. With cumulative, after a long left-drag
      // the user has to overshoot back past the origin before the pet flips
      // to running_right — feels sticky. Frame-delta flips as soon as motion
      // reverses.
      const lastX = lastScreenXRef.current ?? e.screenX
      const instantDx = e.screenX - lastX
      lastScreenXRef.current = e.screenX

      if (Math.abs(instantDx) > DIRECTION_THRESHOLD_PX) {
        const dir: "left" | "right" = instantDx > 0 ? "right" : "left"
        if (dir !== lastDirRef.current) {
          lastDirRef.current = dir
          optsRef.current.onDragDirection(
            dir === "right" ? "running_right" : "running_left"
          )
        }
      }

      // Refresh idle timer on *every* pointermove, not just frames that
      // cross the direction threshold. Otherwise a slow drag (sub-pixel
      // per frame) never refreshes the timer and falls back to the
      // default animation while the user is still actively dragging.
      if (dirIdleTimerRef.current) clearTimeout(dirIdleTimerRef.current)
      dirIdleTimerRef.current = setTimeout(() => {
        if (draggingRef.current) {
          lastDirRef.current = null
          optsRef.current.onDragDirection(null)
        }
      }, DIRECTION_IDLE_MS)

      // Update window position. Compose in logical coordinates for
      // monitor-DPR independence, then convert to physical for the API.
      const physX = (winStart.x + dxLog) * scaleFactorRef.current
      const physY = (winStart.y + dyLog) * scaleFactorRef.current
      pendingPosRef.current = { x: Math.round(physX), y: Math.round(physY) }
      if (rafRef.current === null) {
        rafRef.current = requestAnimationFrame(flushPosition)
      }
    }

    function onPointerUp() {
      if (!draggingRef.current) return
      draggingRef.current = false
      if (dirIdleTimerRef.current) {
        clearTimeout(dirIdleTimerRef.current)
        dirIdleTimerRef.current = null
      }
      const moved = totalMovedRef.current
      totalMovedRef.current = 0
      lastDirRef.current = null
      lastScreenXRef.current = null

      if (moved < CLICK_THRESHOLD_PX) {
        optsRef.current.onClick()
      } else {
        optsRef.current.onDragDirection(null)
      }

      if (persistTimerRef.current) clearTimeout(persistTimerRef.current)
      persistTimerRef.current = setTimeout(() => {
        void persistPosition()
      }, PERSIST_DEBOUNCE_MS)
    }

    window.addEventListener("pointermove", onPointerMove)
    window.addEventListener("pointerup", onPointerUp)
    window.addEventListener("pointercancel", onPointerUp)
    return () => {
      window.removeEventListener("pointermove", onPointerMove)
      window.removeEventListener("pointerup", onPointerUp)
      window.removeEventListener("pointercancel", onPointerUp)
      if (dirIdleTimerRef.current) clearTimeout(dirIdleTimerRef.current)
      if (persistTimerRef.current) clearTimeout(persistTimerRef.current)
      if (rafRef.current !== null) cancelAnimationFrame(rafRef.current)
    }
  }, [persistPosition])

  const onPointerDown = useCallback((event: React.PointerEvent) => {
    if (event.button !== 0) return
    if (!isDesktop()) return
    // Prevent the default drag-region behaviour and text selection.
    event.preventDefault()

    void (async () => {
      try {
        const [{ getCurrentWindow }, dpiModule] = await Promise.all([
          import("@tauri-apps/api/window"),
          import("@tauri-apps/api/dpi"),
        ])
        const win = getCurrentWindow()
        const scale = await win.scaleFactor()
        const pos = await win.outerPosition()

        scaleFactorRef.current = scale
        startScreenRef.current = { x: event.screenX, y: event.screenY }
        startWinLogicalRef.current = { x: pos.x / scale, y: pos.y / scale }
        totalMovedRef.current = 0
        lastScreenXRef.current = event.screenX
        lastDirRef.current = null
        draggingRef.current = true

        const PhysicalPosition = dpiModule.PhysicalPosition
        setPositionFnRef.current = async (x, y) => {
          try {
            await win.setPosition(new PhysicalPosition(x, y))
          } catch (err) {
            console.warn("[Pet] setPosition failed:", err)
          }
        }
      } catch (err) {
        console.warn("[Pet] drag start failed:", err)
        draggingRef.current = false
      }
    })()
  }, [])

  return { onPointerDown }
}
