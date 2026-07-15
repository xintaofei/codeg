"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import type { ClaudeApiRetryState } from "@/contexts/acp-connections-context"
import type { ConnectionStatus, PromptDraft } from "@/lib/types"
import {
  buildAutoReplyDraft,
  buildBurstKey,
  canScheduleAutoReply,
  findMatchingRule,
  isSafeToAutoReply,
  signalFromSources,
} from "@/lib/auto-reply/match"
import { useAutoReplySettings } from "@/lib/auto-reply/settings-store"
import type { AutoReplyRule } from "@/lib/auto-reply/types"

export interface AutoReplyPendingState {
  ruleId: string
  replyText: string
  fireAt: number
  remainingMs: number
  matchedLabel: string
  burstKey: string
}

export interface AutoReplyStopNotice {
  reason: "max_per_burst"
  matchedLabel: string
}

export interface UseAutoReplyEngineArgs {
  enabled: boolean
  status: ConnectionStatus | null
  error: string | null
  claudeApiRetry: ClaudeApiRetryState | null
  pendingPermission: boolean
  pendingQuestion: boolean
  pendingAskQuestion: boolean
  onSend: (draft: PromptDraft) => void
}

export interface UseAutoReplyEngineResult {
  pending: AutoReplyPendingState | null
  stopNotice: AutoReplyStopNotice | null
  cancelPending: () => void
  notifyManualSend: () => void
  dismissStopNotice: () => void
}

function matchedLabelFor(rule: AutoReplyRule): string {
  if (rule.matchKind === "http_status") {
    return `HTTP ${rule.matchValue}`
  }
  return rule.name || rule.matchValue
}

function hasSignalText(text: string): boolean {
  return text.trim().length > 0
}

export function useAutoReplyEngine(
  args: UseAutoReplyEngineArgs
): UseAutoReplyEngineResult {
  const settings = useAutoReplySettings()
  const [pending, setPending] = useState<AutoReplyPendingState | null>(null)
  const [stopNotice, setStopNotice] = useState<AutoReplyStopNotice | null>(
    null
  )
  const [nowTick, setNowTick] = useState(() => Date.now())

  const pendingRef = useRef<AutoReplyPendingState | null>(null)
  const fireTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const tickTimerRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const lastSentAtByRuleRef = useRef<Map<string, number>>(new Map())
  const burstCountByKeyRef = useRef<Map<string, number>>(new Map())
  const stopNoticeBurstKeysRef = useRef<Set<string>>(new Set())
  // After cancel / manual send, do not auto-reschedule the same burst until
  // the interruption signal changes (or clears).
  const suppressedBurstKeyRef = useRef<string | null>(null)
  const onSendRef = useRef(args.onSend)
  onSendRef.current = args.onSend

  const clearTimers = useCallback(() => {
    if (fireTimerRef.current !== null) {
      clearTimeout(fireTimerRef.current)
      fireTimerRef.current = null
    }
    if (tickTimerRef.current !== null) {
      clearInterval(tickTimerRef.current)
      tickTimerRef.current = null
    }
  }, [])

  const clearPending = useCallback(() => {
    clearTimers()
    pendingRef.current = null
    setPending(null)
  }, [clearTimers])

  const cancelPending = useCallback(() => {
    if (pendingRef.current) {
      suppressedBurstKeyRef.current = pendingRef.current.burstKey
    }
    clearPending()
  }, [clearPending])

  const notifyManualSend = useCallback(() => {
    if (pendingRef.current) {
      suppressedBurstKeyRef.current = pendingRef.current.burstKey
    }
    clearPending()
  }, [clearPending])

  const dismissStopNotice = useCallback(() => {
    setStopNotice(null)
  }, [])

  const signal = useMemo(
    () =>
      signalFromSources({
        claudeApiRetry: args.claudeApiRetry
          ? {
              errorStatus: args.claudeApiRetry.errorStatus,
              error: args.claudeApiRetry.error,
            }
          : null,
        connectionError: args.error,
      }),
    [args.claudeApiRetry, args.error]
  )

  const safety = useMemo(
    () => ({
      status: args.status,
      pendingPermission: args.pendingPermission,
      pendingQuestion: args.pendingQuestion,
      pendingAskQuestion: args.pendingAskQuestion,
    }),
    [
      args.status,
      args.pendingPermission,
      args.pendingQuestion,
      args.pendingAskQuestion,
    ]
  )

  const firePending = useCallback(() => {
    const current = pendingRef.current
    if (!current) return

    const stillSafe = isSafeToAutoReply({
      status: args.status,
      pendingPermission: args.pendingPermission,
      pendingQuestion: args.pendingQuestion,
      pendingAskQuestion: args.pendingAskQuestion,
    })
    if (!args.enabled || !stillSafe) {
      clearPending()
      return
    }

    const liveSignal = signalFromSources({
      claudeApiRetry: args.claudeApiRetry
        ? {
            errorStatus: args.claudeApiRetry.errorStatus,
            error: args.claudeApiRetry.error,
          }
        : null,
      connectionError: args.error,
    })
    const liveRule = findMatchingRule(settings.rules, liveSignal)
    if (!liveRule || liveRule.id !== current.ruleId) {
      clearPending()
      return
    }
    if (buildBurstKey(liveSignal) !== current.burstKey) {
      clearPending()
      return
    }

    const now = Date.now()
    const lastSentAt = lastSentAtByRuleRef.current.get(liveRule.id) ?? 0
    const burstCount = burstCountByKeyRef.current.get(current.burstKey) ?? 0
    const gate = canScheduleAutoReply({
      rule: liveRule,
      now,
      lastSentAt,
      burstCount,
    })
    if (!gate.ok) {
      clearPending()
      if (gate.reason === "max_per_burst") {
        setStopNotice({
          reason: "max_per_burst",
          matchedLabel: matchedLabelFor(liveRule),
        })
      }
      return
    }

    clearPending()
    // Suppress re-schedule of this same burst until the signal changes.
    suppressedBurstKeyRef.current = current.burstKey
    lastSentAtByRuleRef.current.set(liveRule.id, now)
    burstCountByKeyRef.current.set(current.burstKey, burstCount + 1)
    onSendRef.current(buildAutoReplyDraft(liveRule.replyText))
  }, [
    args.claudeApiRetry,
    args.enabled,
    args.error,
    args.pendingAskQuestion,
    args.pendingPermission,
    args.pendingQuestion,
    args.status,
    clearPending,
    settings.rules,
  ])

  const schedule = useCallback(
    (rule: AutoReplyRule, burstKey: string) => {
      clearTimers()
      const fireAt = Date.now() + Math.max(0, rule.delayMs)
      const next: AutoReplyPendingState = {
        ruleId: rule.id,
        replyText: rule.replyText,
        fireAt,
        remainingMs: Math.max(0, fireAt - Date.now()),
        matchedLabel: matchedLabelFor(rule),
        burstKey,
      }
      pendingRef.current = next
      setPending(next)
      setNowTick(Date.now())

      fireTimerRef.current = setTimeout(() => {
        firePending()
      }, Math.max(0, rule.delayMs))

      tickTimerRef.current = setInterval(() => {
        setNowTick(Date.now())
      }, 250)
    },
    [clearTimers, firePending]
  )

  useEffect(() => {
    if (!pendingRef.current) return
    const current = pendingRef.current
    const remainingMs = Math.max(0, current.fireAt - nowTick)
    setPending((prev) => {
      if (
        !prev ||
        prev.ruleId !== current.ruleId ||
        prev.fireAt !== current.fireAt
      ) {
        return prev
      }
      if (prev.remainingMs === remainingMs) return prev
      const next = { ...prev, remainingMs }
      pendingRef.current = next
      return next
    })
  }, [nowTick])

  useEffect(() => {
    if (!args.enabled) {
      clearPending()
      return
    }

    if (!isSafeToAutoReply(safety)) {
      if (pendingRef.current) clearPending()
      return
    }

    const activeSignal =
      signal.httpStatus !== null || hasSignalText(signal.errorText)
    if (!activeSignal) {
      suppressedBurstKeyRef.current = null
      if (pendingRef.current) clearPending()
      return
    }

    const rule = findMatchingRule(settings.rules, signal)
    if (!rule) {
      if (pendingRef.current) clearPending()
      return
    }

    const burstKey = buildBurstKey(signal)

    // If the signal changed away from a previously suppressed burst, allow again.
    if (
      suppressedBurstKeyRef.current &&
      suppressedBurstKeyRef.current !== burstKey
    ) {
      suppressedBurstKeyRef.current = null
    }

    if (pendingRef.current) {
      if (
        pendingRef.current.ruleId === rule.id &&
        pendingRef.current.burstKey === burstKey
      ) {
        return
      }
      clearPending()
    }

    if (suppressedBurstKeyRef.current === burstKey) {
      return
    }

    const now = Date.now()
    const lastSentAt = lastSentAtByRuleRef.current.get(rule.id) ?? 0
    const burstCount = burstCountByKeyRef.current.get(burstKey) ?? 0
    const gate = canScheduleAutoReply({
      rule,
      now,
      lastSentAt,
      burstCount,
    })
    if (!gate.ok) {
      if (
        gate.reason === "max_per_burst" &&
        !stopNoticeBurstKeysRef.current.has(burstKey)
      ) {
        stopNoticeBurstKeysRef.current.add(burstKey)
        setStopNotice({
          reason: "max_per_burst",
          matchedLabel: matchedLabelFor(rule),
        })
      }
      return
    }

    schedule(rule, burstKey)
  }, [args.enabled, clearPending, safety, schedule, settings.rules, signal])

  useEffect(() => {
    return () => {
      clearTimers()
      pendingRef.current = null
    }
  }, [clearTimers])

  return {
    pending,
    stopNotice,
    cancelPending,
    notifyManualSend,
    dismissStopNotice,
  }
}