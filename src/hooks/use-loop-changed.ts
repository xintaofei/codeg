import { useEffect, useRef } from "react"

import { subscribe } from "@/lib/platform"
import { LOOP_CHANGED_EVENT, type LoopChanged } from "@/lib/types"

/**
 * Subscribe to the coarse `loop://changed` event. When `spaceId` is given, only
 * events for that space invoke `cb`. The callback is held in a ref so the
 * subscription is set up once and never re-attaches on every render.
 */
export function useLoopChanged(cb: (event: LoopChanged) => void, spaceId?: number) {
  const cbRef = useRef(cb)
  cbRef.current = cb

  useEffect(() => {
    let disposed = false
    let unsub: (() => void) | undefined

    subscribe<LoopChanged>(LOOP_CHANGED_EVENT, (event) => {
      if (disposed) return
      if (spaceId != null && event.space_id !== spaceId) return
      cbRef.current(event)
    }).then((fn) => {
      if (disposed) fn()
      else unsub = fn
    })

    return () => {
      disposed = true
      unsub?.()
    }
  }, [spaceId])
}
