"use client"

import { useEffect } from "react"
import { subscribe, onTransportReconnect } from "@/lib/platform"
import { CANVAS_CHANGED_EVENT, type CanvasChange } from "@/lib/types"
import { useCanvasStore } from "@/stores/canvas-store"

/**
 * Wires the canvas store to the backend while the canvas page is mounted:
 * subscribe FIRST, fetch the snapshot only once the listener is live (the
 * tab-context handshake) — a mutation committed between a snapshot read and
 * the subscription going live would otherwise be dropped by the broadcaster
 * and only surface as a gap much later. A transport reconnect refetches, which
 * also covers events lost while the socket was down.
 *
 * Mounted per canvas visit (not app-wide): the canvas is a full-page route,
 * so keeping the node set hot while it's closed buys nothing.
 */
export function useCanvasData(): void {
  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    void (async () => {
      const dispose = await subscribe<CanvasChange>(
        CANVAS_CHANGED_EVENT,
        (change) => useCanvasStore.getState().handleCanvasChanged(change)
      )
      if (disposed) {
        dispose()
        return
      }
      unlisten = dispose
      void useCanvasStore.getState().refetch()
    })()
    const offReconnect = onTransportReconnect(() =>
      useCanvasStore.getState().refetch()
    )
    return () => {
      disposed = true
      unlisten?.()
      offReconnect?.()
    }
  }, [])
}
