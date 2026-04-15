"use client"

import { useCallback, useEffect, useMemo, useState } from "react"

import { acpListAgentSkills } from "@/lib/api"
import type { AgentSkillItem, AgentType } from "@/lib/types"

// Cache/inflight keyed by `${agentType}|${workspacePath ?? ""}` so different
// folders keep their own skill list, and switching folders never serves stale
// entries from a previous workspace.
const cache = new Map<string, AgentSkillItem[]>()
const inflight = new Map<string, Promise<AgentSkillItem[]>>()

const EMPTY: AgentSkillItem[] = []

function makeKey(agentType: AgentType, workspacePath: string | null): string {
  return `${agentType}|${workspacePath ?? ""}`
}

function fetchSkills(
  agentType: AgentType,
  workspacePath: string | null
): Promise<AgentSkillItem[]> {
  const key = makeKey(agentType, workspacePath)
  let promise = inflight.get(key)
  if (!promise) {
    promise = acpListAgentSkills({ agentType, workspacePath })
      .then((result) => {
        const skills = result.supported ? result.skills : EMPTY
        cache.set(key, skills)
        inflight.delete(key)
        return skills
      })
      .catch((err) => {
        inflight.delete(key)
        console.warn("[useAgentSkills] failed:", err)
        return EMPTY
      })
    inflight.set(key, promise)
  }
  return promise
}

export function useAgentSkills(
  agentType: AgentType | null,
  workspacePath?: string | null
): AgentSkillItem[] {
  const normalizedPath = workspacePath ?? null
  const cacheKey = useMemo(
    () => (agentType ? makeKey(agentType, normalizedPath) : null),
    [agentType, normalizedPath]
  )
  const cached = useMemo(
    () => (cacheKey ? (cache.get(cacheKey) ?? null) : null),
    [cacheKey]
  )
  // Track which (agentType, workspacePath) the fetched result belongs to so
  // stale data from a previous key is never returned after a switch.
  const [fetched, setFetched] = useState<{
    key: string
    skills: AgentSkillItem[]
  } | null>(null)

  const doFetch = useCallback(() => {
    if (!agentType || !cacheKey || cache.has(cacheKey)) return
    let cancelled = false
    fetchSkills(agentType, normalizedPath).then((list) => {
      if (!cancelled) setFetched({ key: cacheKey, skills: list })
    })
    return () => {
      cancelled = true
    }
  }, [agentType, cacheKey, normalizedPath])

  // Initial fetch
  useEffect(() => doFetch(), [doFetch])

  // Re-fetch when window regains focus (covers cross-window cache
  // invalidation — e.g. settings window creates/removes skills while the
  // conversation window stays mounted). Only invalidate the current key to
  // avoid clobbering caches for other folders.
  useEffect(() => {
    const onFocus = () => {
      if (!cacheKey) return
      cache.delete(cacheKey)
      inflight.delete(cacheKey)
      doFetch()
    }
    window.addEventListener("focus", onFocus)
    return () => window.removeEventListener("focus", onFocus)
  }, [cacheKey, doFetch])

  if (!agentType || !cacheKey) return EMPTY
  if (cached) return cached
  if (fetched && fetched.key === cacheKey) return fetched.skills
  return EMPTY
}

export function invalidateAgentSkillsCache(agentType?: AgentType) {
  if (agentType) {
    const prefix = `${agentType}|`
    for (const key of Array.from(cache.keys())) {
      if (key.startsWith(prefix)) cache.delete(key)
    }
    for (const key of Array.from(inflight.keys())) {
      if (key.startsWith(prefix)) inflight.delete(key)
    }
  } else {
    cache.clear()
    inflight.clear()
  }
}
