import { createBuiltinRules } from "./match"
import type { AutoReplyRule, AutoReplySettings } from "./types"

export const AUTO_REPLY_SETTINGS_KEY = "codeg:auto-reply:settings:v1"
export const AUTO_REPLY_ENABLED_KEY = "codeg:auto-reply:enabled:v1"

const VALID_MATCH_KINDS = new Set(["http_status", "error_text"])

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function sanitizeRule(raw: unknown): AutoReplyRule | null {
  if (!isRecord(raw)) return null
  if (typeof raw.id !== "string" || !raw.id) return null
  if (typeof raw.name !== "string") return null
  if (typeof raw.enabled !== "boolean") return null
  if (typeof raw.matchKind !== "string" || !VALID_MATCH_KINDS.has(raw.matchKind)) {
    return null
  }
  if (typeof raw.matchValue !== "string") return null
  if (typeof raw.replyText !== "string") return null
  if (typeof raw.delayMs !== "number" || !Number.isFinite(raw.delayMs)) return null
  if (typeof raw.cooldownMs !== "number" || !Number.isFinite(raw.cooldownMs)) {
    return null
  }
  if (typeof raw.maxPerBurst !== "number" || !Number.isFinite(raw.maxPerBurst)) {
    return null
  }

  const rule: AutoReplyRule = {
    id: raw.id,
    name: raw.name,
    enabled: raw.enabled,
    matchKind: raw.matchKind as AutoReplyRule["matchKind"],
    matchValue: raw.matchValue,
    replyText: raw.replyText,
    delayMs: Math.max(0, Math.trunc(raw.delayMs)),
    cooldownMs: Math.max(0, Math.trunc(raw.cooldownMs)),
    maxPerBurst: Math.max(1, Math.trunc(raw.maxPerBurst)),
  }
  if (raw.builtin === true) rule.builtin = true
  return rule
}

/** Ensure builtin rules always exist. Keep user edits for known builtin ids. */
export function ensureBuiltinRules(rules: AutoReplyRule[]): AutoReplyRule[] {
  const builtins = createBuiltinRules()
  const byId = new Map(rules.map((rule) => [rule.id, rule]))
  const merged: AutoReplyRule[] = []

  for (const builtin of builtins) {
    const existing = byId.get(builtin.id)
    if (existing) {
      merged.push({
        ...existing,
        id: builtin.id,
        builtin: true,
      })
      byId.delete(builtin.id)
    } else {
      merged.push(builtin)
    }
  }

  for (const rule of rules) {
    if (byId.has(rule.id)) {
      const next = { ...rule }
      if (next.builtin) delete next.builtin
      // Non-seed ids should not pretend to be builtins.
      if (next.id.startsWith("builtin-")) {
        // Keep flag only for known seeds handled above; drop orphans' builtin bit.
        delete next.builtin
      }
      merged.push(next)
      byId.delete(rule.id)
    }
  }

  return merged
}

export function createDefaultAutoReplySettings(): AutoReplySettings {
  return {
    version: 1,
    rules: createBuiltinRules(),
  }
}

export function normalizeAutoReplySettings(
  raw: unknown
): AutoReplySettings {
  if (!isRecord(raw) || raw.version !== 1 || !Array.isArray(raw.rules)) {
    return createDefaultAutoReplySettings()
  }

  const rules: AutoReplyRule[] = []
  const seen = new Set<string>()
  for (const item of raw.rules) {
    const rule = sanitizeRule(item)
    if (!rule) continue
    if (seen.has(rule.id)) continue
    seen.add(rule.id)
    rules.push(rule)
  }

  return {
    version: 1,
    rules: ensureBuiltinRules(rules),
  }
}

export function loadAutoReplySettings(): AutoReplySettings {
  if (typeof window === "undefined") return createDefaultAutoReplySettings()
  try {
    const raw = window.localStorage.getItem(AUTO_REPLY_SETTINGS_KEY)
    if (!raw) return createDefaultAutoReplySettings()
    return normalizeAutoReplySettings(JSON.parse(raw) as unknown)
  } catch {
    return createDefaultAutoReplySettings()
  }
}

export function saveAutoReplySettings(settings: AutoReplySettings): void {
  if (typeof window === "undefined") return
  const normalized = normalizeAutoReplySettings(settings)
  try {
    window.localStorage.setItem(
      AUTO_REPLY_SETTINGS_KEY,
      JSON.stringify(normalized)
    )
  } catch {
    /* ignore quota/permission failures */
  }
}

export type AutoReplyEnabledMap = Record<string, boolean>

export function loadEnabledMap(): AutoReplyEnabledMap {
  if (typeof window === "undefined") return {}
  try {
    const raw = window.localStorage.getItem(AUTO_REPLY_ENABLED_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as unknown
    if (!isRecord(parsed)) return {}
    const out: AutoReplyEnabledMap = {}
    for (const [key, value] of Object.entries(parsed)) {
      if (typeof value === "boolean") out[key] = value
    }
    return out
  } catch {
    return {}
  }
}

export function saveEnabledMap(map: AutoReplyEnabledMap): void {
  if (typeof window === "undefined") return
  try {
    window.localStorage.setItem(AUTO_REPLY_ENABLED_KEY, JSON.stringify(map))
  } catch {
    /* ignore */
  }
}

export function isAutoReplyEnabled(enableKey: string): boolean {
  if (!enableKey) return false
  return loadEnabledMap()[enableKey] === true
}

export function setAutoReplyEnabled(
  enableKey: string,
  enabled: boolean
): void {
  if (!enableKey) return
  const map = loadEnabledMap()
  if (enabled) {
    map[enableKey] = true
  } else {
    delete map[enableKey]
  }
  saveEnabledMap(map)
}
