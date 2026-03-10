"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { getFolderConversation } from "@/lib/tauri"
import type { DbConversationDetail } from "@/lib/types"

// Module-level cache: survives component unmount/remount
const detailCache = new Map<number, DbConversationDetail>()
const detailInFlight = new Map<number, Promise<DbConversationDetail>>()
const detailListeners = new Map<
  number,
  Set<(detail: DbConversationDetail) => void>
>()

function publishDetail(conversationId: number, detail: DbConversationDetail) {
  const listeners = detailListeners.get(conversationId)
  if (!listeners || listeners.size === 0) return
  for (const listener of listeners) {
    listener(detail)
  }
}

function setCachedDetail(conversationId: number, detail: DbConversationDetail) {
  detailCache.set(conversationId, detail)
  publishDetail(conversationId, detail)
}

function subscribeDetail(
  conversationId: number,
  listener: (detail: DbConversationDetail) => void
) {
  let listeners = detailListeners.get(conversationId)
  if (!listeners) {
    listeners = new Set()
    detailListeners.set(conversationId, listeners)
  }
  listeners.add(listener)

  return () => {
    const current = detailListeners.get(conversationId)
    if (!current) return
    current.delete(listener)
    if (current.size === 0) {
      detailListeners.delete(conversationId)
    }
  }
}

/** Invalidate cached detail so the next mount re-fetches from disk. */
export function invalidateDetailCache(conversationId: number) {
  detailCache.delete(conversationId)
}

async function loadAndCacheDetail(
  conversationId: number
): Promise<DbConversationDetail> {
  const existing = detailInFlight.get(conversationId)
  if (existing) return existing

  const promise = getFolderConversation(conversationId)
    .then((detail) => {
      setCachedDetail(conversationId, detail)
      return detail
    })
    .finally(() => {
      detailInFlight.delete(conversationId)
    })

  detailInFlight.set(conversationId, promise)
  return promise
}

export async function refreshDetailCache(
  conversationId: number
): Promise<DbConversationDetail> {
  detailCache.delete(conversationId)
  return loadAndCacheDetail(conversationId)
}

interface State {
  key: number
  detail: DbConversationDetail | null
  loading: boolean
  error: string | null
  fetchSeq: number
}

function isVirtualConversationId(conversationId: number): boolean {
  return !Number.isFinite(conversationId) || conversationId <= 0
}

export function useDbMessageDetail(conversationId: number) {
  const isVirtualId = isVirtualConversationId(conversationId)
  const getCachedState = useCallback((id: number): State => {
    if (isVirtualConversationId(id)) {
      return {
        key: id,
        detail: null,
        loading: false,
        error: null,
        fetchSeq: 0,
      }
    }
    const cached = detailCache.get(id)
    return {
      key: id,
      detail: cached ?? null,
      loading: !cached,
      error: null,
      fetchSeq: 0,
    }
  }, [])

  const [state, setState] = useState<State>(() => {
    return getCachedState(conversationId)
  })

  const derivedState =
    state.key === conversationId ? state : getCachedState(conversationId)

  useEffect(() => {
    if (isVirtualId) return
    return subscribeDetail(conversationId, (detail) => {
      setState((prev) => ({
        key: conversationId,
        detail,
        loading: false,
        error: null,
        fetchSeq: prev.key === conversationId ? prev.fetchSeq : 0,
      }))
    })
  }, [conversationId, isVirtualId])

  const refetch = useCallback(() => {
    if (isVirtualConversationId(conversationId)) {
      setState(getCachedState(conversationId))
      return
    }
    detailCache.delete(conversationId)
    setState((prev) => {
      const base =
        prev.key === conversationId ? prev : getCachedState(conversationId)
      return {
        ...base,
        key: conversationId,
        loading: true,
        error: null,
        fetchSeq: base.fetchSeq + 1,
      }
    })
  }, [conversationId, getCachedState])

  useEffect(() => {
    if (isVirtualId) return
    // Skip fetch if cache already has data
    if (detailCache.has(conversationId)) return

    let cancelled = false
    loadAndCacheDetail(conversationId)
      .then((d) => {
        if (!cancelled) {
          setState((prev) =>
            prev.key === conversationId
              ? { ...prev, detail: d, loading: false, error: null }
              : {
                  key: conversationId,
                  detail: d,
                  loading: false,
                  error: null,
                  fetchSeq: 0,
                }
          )
        }
      })
      .catch((e) => {
        if (!cancelled) {
          setState((prev) =>
            prev.key === conversationId
              ? {
                  ...prev,
                  error: e instanceof Error ? e.message : String(e),
                  loading: false,
                }
              : {
                  key: conversationId,
                  detail: null,
                  loading: false,
                  error: e instanceof Error ? e.message : String(e),
                  fetchSeq: 0,
                }
          )
        }
      })
    return () => {
      cancelled = true
    }
  }, [conversationId, derivedState.fetchSeq, isVirtualId])

  return useMemo(
    () => ({
      detail: derivedState.detail,
      loading: derivedState.loading,
      error: derivedState.error,
      refetch,
    }),
    [derivedState.detail, derivedState.loading, derivedState.error, refetch]
  )
}
