"use client"

import { useEffect } from "react"
import { useConversationRuntime } from "@/contexts/conversation-runtime-context"
import type { DbConversationDetail } from "@/lib/types"

function isVirtualConversationId(conversationId: number): boolean {
  return !Number.isFinite(conversationId) || conversationId <= 0
}

export function useConversationDetail(
  conversationId: number,
  options?: {
    /**
     * Gate the built-in auto-fetch. Defaults to `true`. Pass `false` when the
     * caller drives fetching itself and must prevent a fetch from landing at
     * the wrong moment — e.g. the sub-agent session dialog, which must not load
     * the child's persisted detail while it is mid-stream (the parser surfaces
     * the in-progress turn as a normal turn, which would then duplicate the
     * live stream).
     */
    enabled?: boolean
  }
): {
  detail: DbConversationDetail | null
  loading: boolean
  error: string | null
  acpLoadError: string | null
} {
  const enabled = options?.enabled ?? true
  const { getSession, fetchDetail } = useConversationRuntime()
  const session = getSession(conversationId)
  const isVirtual = isVirtualConversationId(conversationId)

  useEffect(() => {
    if (!enabled) return
    if (isVirtual) return
    if (session?.detail || session?.detailLoading) return
    fetchDetail(conversationId)
  }, [
    enabled,
    conversationId,
    isVirtual,
    session?.detail,
    session?.detailLoading,
    fetchDetail,
  ])

  return {
    detail: session?.detail ?? null,
    loading: session ? session.detailLoading : !isVirtual,
    error: session?.detailError ?? null,
    acpLoadError: session?.acpLoadError ?? null,
  }
}
