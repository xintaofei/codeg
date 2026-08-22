"use client"

import { useEffect, useRef } from "react"

const RECOVERY_MARKER = "codeg:chunk-load-recovery"
const HEALTHY_WINDOW_MS = 10_000
const CHUNK_LOAD_PATTERN =
  /chunkloaderror|failed to load chunk|loading chunk .+ failed|failed to fetch dynamically imported module|importing a module script failed/i

function errorText(value: unknown): string {
  if (typeof value === "string") return value
  if (value == null || typeof value !== "object") return ""
  const record = value as { name?: unknown; message?: unknown }
  return [record.name, record.message]
    .filter((part): part is string => typeof part === "string")
    .join(": ")
}

export function isChunkLoadError(value: unknown): boolean {
  return CHUNK_LOAD_PATTERN.test(errorText(value))
}

function reloadWindow(): void {
  window.location.reload()
}

/**
 * Recover from a stale Next.js runtime after a deploy or dev-server rebuild.
 * Lazy chunks surface only when their feature is first used (for example,
 * opening a generated file), so the initial page can look healthy while its
 * chunk graph is already invalid. One guarded reload obtains a coherent graph;
 * sessionStorage prevents a broken deployment from entering a reload loop.
 */
export function ChunkLoadRecovery({
  reloadPage = reloadWindow,
}: {
  reloadPage?: () => void
}) {
  const attemptedRef = useRef(false)

  useEffect(() => {
    const pageKey = `${window.location.pathname}${window.location.search}`

    const recover = (reason: unknown) => {
      if (attemptedRef.current || !isChunkLoadError(reason)) return

      try {
        if (window.sessionStorage.getItem(RECOVERY_MARKER) === pageKey) return
        window.sessionStorage.setItem(RECOVERY_MARKER, pageKey)
      } catch {
        // A local in-memory guard still prevents repeated reload attempts in
        // this document when storage is unavailable.
      }

      attemptedRef.current = true
      reloadPage()
    }

    const onError = (event: ErrorEvent) => {
      recover(event.error ?? event.message)
    }
    const onUnhandledRejection = (event: PromiseRejectionEvent) => {
      recover(event.reason)
    }

    window.addEventListener("error", onError)
    window.addEventListener("unhandledrejection", onUnhandledRejection)
    const healthyTimer = window.setTimeout(() => {
      try {
        if (window.sessionStorage.getItem(RECOVERY_MARKER) === pageKey) {
          window.sessionStorage.removeItem(RECOVERY_MARKER)
        }
      } catch {
        // Storage is optional; there is nothing to clear when it is blocked.
      }
    }, HEALTHY_WINDOW_MS)

    return () => {
      window.removeEventListener("error", onError)
      window.removeEventListener("unhandledrejection", onUnhandledRejection)
      window.clearTimeout(healthyTimer)
    }
  }, [reloadPage])

  return null
}
