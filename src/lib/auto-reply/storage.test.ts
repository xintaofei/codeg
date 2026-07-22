import { afterEach, describe, expect, it } from "vitest"
import {
  AUTO_REPLY_ENABLED_KEY,
  AUTO_REPLY_SETTINGS_KEY,
  createDefaultAutoReplySettings,
  ensureBuiltinRules,
  isAutoReplyEnabled,
  loadAutoReplySettings,
  loadEnabledMap,
  saveAutoReplySettings,
  setAutoReplyEnabled,
} from "./storage"
import type { AutoReplyRule } from "./types"

afterEach(() => {
  window.localStorage.clear()
})

describe("auto-reply storage", () => {
  it("returns builtin defaults when storage is empty", () => {
    const settings = loadAutoReplySettings()
    expect(settings.version).toBe(1)
    expect(settings.rules.map((r) => r.id)).toEqual([
      "builtin-http-429",
      "builtin-http-503",
    ])
  })

  it("returns defaults for corrupt JSON", () => {
    window.localStorage.setItem(AUTO_REPLY_SETTINGS_KEY, "{not-json")
    const settings = loadAutoReplySettings()
    expect(settings).toEqual(createDefaultAutoReplySettings())
  })

  it("round-trips custom rule edits", () => {
    const settings = createDefaultAutoReplySettings()
    settings.rules[0] = {
      ...settings.rules[0],
      replyText: "resume",
      delayMs: 5000,
    }
    settings.rules.push({
      id: "custom-1",
      name: "Custom",
      enabled: true,
      matchKind: "error_text",
      matchValue: "rate limit",
      replyText: "继续",
      delayMs: 1000,
      cooldownMs: 10000,
      maxPerBurst: 2,
    })
    saveAutoReplySettings(settings)
    const loaded = loadAutoReplySettings()
    expect(loaded.rules[0].replyText).toBe("resume")
    expect(loaded.rules[0].delayMs).toBe(5000)
    expect(loaded.rules.some((r) => r.id === "custom-1")).toBe(true)
  })

  it("re-injects missing builtins while keeping user edits", () => {
    const custom: AutoReplyRule = {
      id: "custom-1",
      name: "Custom",
      enabled: true,
      matchKind: "error_text",
      matchValue: "boom",
      replyText: "ok",
      delayMs: 1000,
      cooldownMs: 1000,
      maxPerBurst: 1,
    }
    const edited429: AutoReplyRule = {
      id: "builtin-http-429",
      name: "HTTP 429",
      enabled: false,
      matchKind: "http_status",
      matchValue: "429",
      replyText: "go",
      delayMs: 9000,
      cooldownMs: 1000,
      maxPerBurst: 5,
      builtin: true,
    }
    const merged = ensureBuiltinRules([edited429, custom])
    expect(merged.map((r) => r.id)).toEqual([
      "builtin-http-429",
      "builtin-http-503",
      "custom-1",
    ])
    expect(merged[0].replyText).toBe("go")
    expect(merged[0].enabled).toBe(false)
    expect(merged[1].matchValue).toBe("503")
  })

  it("persists per-conversation enable flags", () => {
    expect(isAutoReplyEnabled("conv-1")).toBe(false)
    setAutoReplyEnabled("conv-1", true)
    expect(isAutoReplyEnabled("conv-1")).toBe(true)
    expect(loadEnabledMap()["conv-1"]).toBe(true)
    setAutoReplyEnabled("conv-1", false)
    expect(isAutoReplyEnabled("conv-1")).toBe(false)
    const raw = window.localStorage.getItem(AUTO_REPLY_ENABLED_KEY)
    expect(raw).toBeTruthy()
  })
})
