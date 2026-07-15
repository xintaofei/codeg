import type { PromptDraft } from "@/lib/types"
import type {
  AutoReplyRule,
  AutoReplySafetyInput,
  AutoReplyScheduleCheckInput,
  AutoReplyScheduleResult,
  AutoReplySignal,
} from "./types"

const DEFAULT_REPLY = "继续"

export function createBuiltinRules(): AutoReplyRule[] {
  return [
    {
      id: "builtin-http-429",
      name: "HTTP 429",
      enabled: true,
      matchKind: "http_status",
      matchValue: "429",
      replyText: DEFAULT_REPLY,
      delayMs: 3000,
      cooldownMs: 15000,
      maxPerBurst: 3,
      builtin: true,
    },
    {
      id: "builtin-http-503",
      name: "HTTP 503",
      enabled: true,
      matchKind: "http_status",
      matchValue: "503",
      replyText: DEFAULT_REPLY,
      delayMs: 3000,
      cooldownMs: 15000,
      maxPerBurst: 3,
      builtin: true,
    },
  ]
}

export function normalizeErrorText(text: string): string {
  return text.trim().replace(/\s+/g, " ")
}

export function buildBurstKey(signal: AutoReplySignal): string {
  const status =
    signal.httpStatus === null || signal.httpStatus === undefined
      ? "none"
      : String(signal.httpStatus)
  return status + "|" + normalizeErrorText(signal.errorText)
}

function matchesRule(rule: AutoReplyRule, signal: AutoReplySignal): boolean {
  if (rule.matchKind === "http_status") {
    if (signal.httpStatus === null || signal.httpStatus === undefined) {
      return false
    }
    const expected = Number(rule.matchValue)
    if (!Number.isFinite(expected)) return false
    return expected === signal.httpStatus
  }

  if (rule.matchKind === "error_text") {
    if (!rule.matchValue) return false
    return signal.errorText.includes(rule.matchValue)
  }

  return false
}

export function findMatchingRule(
  rules: AutoReplyRule[],
  signal: AutoReplySignal
): AutoReplyRule | null {
  for (const rule of rules) {
    if (!rule.enabled) continue
    if (matchesRule(rule, signal)) return rule
  }
  return null
}

export function canScheduleAutoReply(
  input: AutoReplyScheduleCheckInput
): AutoReplyScheduleResult {
  const { rule, now, lastSentAt, burstCount } = input
  if (lastSentAt > 0 && now < lastSentAt + rule.cooldownMs) {
    return { ok: false, reason: "cooldown" }
  }
  if (burstCount >= rule.maxPerBurst) {
    return { ok: false, reason: "max_per_burst" }
  }
  return { ok: true }
}

export function isSafeToAutoReply(input: AutoReplySafetyInput): boolean {
  if (input.status !== "connected") return false
  if (input.pendingPermission) return false
  if (input.pendingQuestion) return false
  if (input.pendingAskQuestion) return false
  return true
}

export function signalFromSources(input: {
  claudeApiRetry: { errorStatus: number | null; error: string | null } | null
  connectionError: string | null | undefined
}): AutoReplySignal {
  const httpStatus = input.claudeApiRetry?.errorStatus ?? null
  const errorText = input.claudeApiRetry?.error ?? input.connectionError ?? ""
  return {
    httpStatus,
    errorText,
  }
}

export function buildAutoReplyDraft(text: string): PromptDraft {
  return {
    blocks: [{ type: "text", text }],
    displayText: text,
  }
}
