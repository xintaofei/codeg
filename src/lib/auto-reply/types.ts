export type AutoReplyMatchKind = "http_status" | "error_text"

export interface AutoReplyRule {
  id: string
  name: string
  enabled: boolean
  matchKind: AutoReplyMatchKind
  matchValue: string
  replyText: string
  delayMs: number
  cooldownMs: number
  maxPerBurst: number
  builtin?: boolean
}

export interface AutoReplySettings {
  version: 1
  rules: AutoReplyRule[]
}

export interface AutoReplySignal {
  httpStatus: number | null
  errorText: string
}

export interface AutoReplySafetyInput {
  status: string | null | undefined
  pendingPermission: boolean
  pendingQuestion: boolean
  pendingAskQuestion: boolean
}

export interface AutoReplyScheduleCheckInput {
  rule: AutoReplyRule
  now: number
  lastSentAt: number
  burstCount: number
}

export type AutoReplyScheduleResult =
  | { ok: true }
  | { ok: false; reason: "cooldown" | "max_per_burst" }
