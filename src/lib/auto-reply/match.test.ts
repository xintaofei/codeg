import { describe, expect, it } from "vitest"
import {
  buildAutoReplyDraft,
  buildBurstKey,
  canScheduleAutoReply,
  createBuiltinRules,
  findMatchingRule,
  isSafeToAutoReply,
  normalizeErrorText,
  signalFromSources,
} from "./match"
import type { AutoReplyRule } from "./types"

const CONTINUE = "\u7ee7\u7eed"

function rule(
  partial: Partial<AutoReplyRule> & Pick<AutoReplyRule, "id">
): AutoReplyRule {
  return {
    name: partial.name ?? partial.id,
    enabled: partial.enabled ?? true,
    matchKind: partial.matchKind ?? "http_status",
    matchValue: partial.matchValue ?? "429",
    replyText: partial.replyText ?? CONTINUE,
    delayMs: partial.delayMs ?? 3000,
    cooldownMs: partial.cooldownMs ?? 15000,
    maxPerBurst: partial.maxPerBurst ?? 3,
    builtin: partial.builtin,
    ...partial,
  }
}

describe("auto-reply match helpers", () => {
  it("seeds 429 and 503 builtins that reply continue", () => {
    const rules = createBuiltinRules()
    expect(rules).toHaveLength(2)
    expect(rules.map((r) => r.matchValue)).toEqual(["429", "503"])
    expect(rules.every((r) => r.replyText === CONTINUE && r.builtin)).toBe(true)
  })

  it("matches http_status exactly and ignores disabled rules", () => {
    const rules = [
      rule({ id: "off", enabled: false, matchValue: "429" }),
      rule({ id: "on", matchValue: "429" }),
    ]
    expect(findMatchingRule(rules, { httpStatus: 429, errorText: "" })?.id).toBe(
      "on"
    )
    expect(findMatchingRule(rules, { httpStatus: 503, errorText: "" })).toBeNull()
  })

  it("matches error_text as case-sensitive substring; first wins", () => {
    const rules = [
      rule({
        id: "first",
        matchKind: "error_text",
        matchValue: "Too Many",
      }),
      rule({
        id: "second",
        matchKind: "error_text",
        matchValue: "Too Many Requests",
      }),
    ]
    expect(
      findMatchingRule(rules, {
        httpStatus: null,
        errorText: "Error: Too Many Requests",
      })?.id
    ).toBe("first")
    expect(
      findMatchingRule(rules, {
        httpStatus: null,
        errorText: "too many requests",
      })
    ).toBeNull()
  })

  it("builds stable burst keys and normalizes whitespace", () => {
    expect(normalizeErrorText("  a   b\n c ")).toBe("a b c")
    expect(
      buildBurstKey({ httpStatus: 429, errorText: "  rate  limit " })
    ).toBe("429|rate limit")
    expect(buildBurstKey({ httpStatus: null, errorText: "" })).toBe("none|")
  })

  it("applies cooldown and maxPerBurst", () => {
    const r = rule({ id: "r", cooldownMs: 1000, maxPerBurst: 2 })
    expect(
      canScheduleAutoReply({
        rule: r,
        now: 5000,
        lastSentAt: 4500,
        burstCount: 0,
      })
    ).toEqual({ ok: false, reason: "cooldown" })
    expect(
      canScheduleAutoReply({
        rule: r,
        now: 6000,
        lastSentAt: 0,
        burstCount: 2,
      })
    ).toEqual({ ok: false, reason: "max_per_burst" })
    expect(
      canScheduleAutoReply({
        rule: r,
        now: 6000,
        lastSentAt: 0,
        burstCount: 1,
      })
    ).toEqual({ ok: true })
  })

  it("only allows safe connection states", () => {
    expect(
      isSafeToAutoReply({
        status: "connected",
        pendingPermission: false,
        pendingQuestion: false,
        pendingAskQuestion: false,
      })
    ).toBe(true)
    expect(
      isSafeToAutoReply({
        status: "prompting",
        pendingPermission: false,
        pendingQuestion: false,
        pendingAskQuestion: false,
      })
    ).toBe(false)
    expect(
      isSafeToAutoReply({
        status: "connected",
        pendingPermission: true,
        pendingQuestion: false,
        pendingAskQuestion: false,
      })
    ).toBe(false)
  })

  it("prefers claudeApiRetry error over connection error", () => {
    expect(
      signalFromSources({
        claudeApiRetry: {
          errorStatus: 429,
          error: "Too Many Requests",
        },
        connectionError: "other",
      })
    ).toEqual({ httpStatus: 429, errorText: "Too Many Requests" })
  })

  it("builds a plain-text PromptDraft", () => {
    expect(buildAutoReplyDraft(CONTINUE)).toEqual({
      blocks: [{ type: "text", text: CONTINUE }],
      displayText: CONTINUE,
    })
  })
})