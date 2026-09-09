"use client"

import { useCallback, useEffect, useState } from "react"

/**
 * Report whether the observed element is inside (or within a generous margin
 * of) the viewport readiness to do expensive work — here, firing a translation.
 *
 * The hook does not read `document` at module scope, so it is safe on the
 * server; a jsdom/test environment without IntersectionObserver resolves to
 * "load now", which keeps default-off translation (and unit tests) from
 * relying on a browser API that may not be present.
 */
export function useNearViewport<T extends Element>(): {
  ref: (node: T | null) => void
  shouldLoad: boolean
} {
  const [node, setNode] = useState<T | null>(null)
  const [near, setNear] = useState(
    () => typeof IntersectionObserver === "undefined"
  )

  useEffect(() => {
    if (typeof IntersectionObserver === "undefined" || near) return
    if (!node) return

    // A wide margin is deliberate: the default-only-in-viewport rule is meant
    // to keep translations from firing for messages far off-screen, not to
    // wait until the text is already under the cursor.
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            setNear(true)
            observer.disconnect()
            break
          }
        }
      },
      { rootMargin: "1000px 0px" }
    )
    observer.observe(node)
    return () => observer.disconnect()
  }, [near, node])

  const ref = useCallback((next: T | null) => setNode(next), [])

  return { ref, shouldLoad: near }
}
