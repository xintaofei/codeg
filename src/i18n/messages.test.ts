import { createTranslator } from "next-intl"
import { describe, expect, it } from "vitest"

import ar from "./messages/ar.json"
import de from "./messages/de.json"
import en from "./messages/en.json"
import es from "./messages/es.json"
import fr from "./messages/fr.json"
import ja from "./messages/ja.json"
import ko from "./messages/ko.json"
import pt from "./messages/pt.json"
import zhCN from "./messages/zh-CN.json"
import zhTW from "./messages/zh-TW.json"

type MessageNode = string | { [key: string]: MessageNode }

function collectKeys(node: MessageNode, prefix = ""): string[] {
  if (typeof node === "string") {
    return [prefix]
  }
  const out: string[] = []
  for (const [key, value] of Object.entries(node)) {
    const next = prefix ? `${prefix}.${key}` : key
    out.push(...collectKeys(value, next))
  }
  return out
}

const reference = new Set(collectKeys(en as MessageNode))

// `en.json` is the source of truth. Any missing key in another locale fails
// the test with the exact dotted path, making translation gaps grep-able.
describe("i18n locale key parity vs en.json", () => {
  it.each([
    ["ar", ar],
    ["de", de],
    ["es", es],
    ["fr", fr],
    ["ja", ja],
    ["ko", ko],
    ["pt", pt],
    ["zh-CN", zhCN],
    ["zh-TW", zhTW],
  ] as const)("%s has the same key set as en", (_locale, messages) => {
    const localeKeys = new Set(collectKeys(messages as MessageNode))
    const missing = [...reference].filter((k) => !localeKeys.has(k))
    const extra = [...localeKeys].filter((k) => !reference.has(k))
    expect({ missing, extra }).toEqual({ missing: [], extra: [] })
  })
})

const ALL_LOCALES = [
  ["en", en],
  ["ar", ar],
  ["de", de],
  ["es", es],
  ["fr", fr],
  ["ja", ja],
  ["ko", ko],
  ["pt", pt],
  ["zh-CN", zhCN],
  ["zh-TW", zhTW],
] as const

const SKILL_AVAILABILITY_MESSAGES = [
  ["SkillsSettings.availability.enabled", []],
  ["SkillsSettings.availability.disabled", []],
  ["SkillsSettings.availability.toggleAria", ["agent", "skill"]],
  ["SkillsSettings.availability.readOnly", []],
  ["SkillsSettings.availability.cannotIsolate", []],
  ["SkillsSettings.toasts.enabled", []],
  ["SkillsSettings.toasts.disabled", []],
  ["SkillsSettings.toasts.toggleFailed", []],
] as const

function getMessage(node: MessageNode, path: string): string | undefined {
  let current: MessageNode | undefined = node
  for (const segment of path.split(".")) {
    if (typeof current === "string") return undefined
    current = current?.[segment]
  }
  return typeof current === "string" ? current : undefined
}

describe("skill availability message contract", () => {
  it.each(ALL_LOCALES)(
    "%s includes the required keys and placeholders",
    (_locale, messages) => {
      for (const [key, expectedPlaceholders] of SKILL_AVAILABILITY_MESSAGES) {
        const message = getMessage(messages as MessageNode, key)
        expect(message, key).toBeTypeOf("string")
        const placeholders = [
          ...(message?.matchAll(/\{([a-zA-Z][\w]*)\}/g) ?? []),
        ]
          .map((match) => match[1])
          .sort()
        expect(placeholders, key).toEqual(expectedPlaceholders)
      }
    }
  )
})

// Every message goes through ICU MessageFormat, which reserves `<tag>`, `{`,
// `}` and `#`. A string like `<QODER_CONFIG_DIR>/settings.json` parses as an
// unclosed tag and falls back to rendering the KEY — visible in the UI as
// `qoder.configDescription`, and only in the one locale that has it.
//
// The check runs the real production path (`createTranslator`) rather than a
// regex, so it fails on exactly what users would see fail, and it fails with
// the dotted path so the offending string is grep-able.
describe("i18n messages are valid ICU", () => {
  it.each(ALL_LOCALES)("%s renders every message", (locale, messages) => {
    const broken: string[] = []
    const t = createTranslator({
      locale,
      messages: messages as Record<string, MessageNode>,
      onError: () => {},
      // A message that fails to parse reaches here with its own key; anything
      // that needs real ICU arguments renders as its key too, which is fine —
      // we only care that the parse itself succeeded.
      getMessageFallback: ({ key, error }) => {
        if (error?.code === "INVALID_MESSAGE") broken.push(key)
        return key
      },
    })
    for (const key of collectKeys(messages as MessageNode)) {
      // Placeholders are supplied loosely: an unknown-argument error is not an
      // ICU syntax problem, and `getMessageFallback` only records the syntax one.
      t(key as never, { count: 1, name: "x", value: "x" } as never)
    }
    expect(broken).toEqual([])
  })
})
