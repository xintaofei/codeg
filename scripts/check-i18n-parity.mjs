#!/usr/bin/env node
/**
 * Validates that all locale files under src/i18n/messages/ share the
 * exact same set of message keys as en.json (the source of truth).
 *
 * Exits non-zero with a diff-style report if any locale is missing keys
 * or carries stale extras. Designed to be wired into CI / lint flows.
 */

import fs from "node:fs"
import path from "node:path"
import url from "node:url"

const __dirname = path.dirname(url.fileURLToPath(import.meta.url))
const MESSAGES_DIR = path.resolve(__dirname, "..", "src", "i18n", "messages")
const SOURCE_LOCALE = "en.json"

function flatten(obj, prefix = "") {
  const out = new Set()
  for (const [k, v] of Object.entries(obj)) {
    const key = prefix ? `${prefix}.${k}` : k
    if (v !== null && typeof v === "object" && !Array.isArray(v)) {
      for (const inner of flatten(v, key)) out.add(inner)
    } else {
      out.add(key)
    }
  }
  return out
}

function main() {
  if (!fs.existsSync(MESSAGES_DIR)) {
    console.error(`[i18n-parity] missing dir: ${MESSAGES_DIR}`)
    process.exit(2)
  }
  const sourcePath = path.join(MESSAGES_DIR, SOURCE_LOCALE)
  if (!fs.existsSync(sourcePath)) {
    console.error(`[i18n-parity] source locale not found: ${sourcePath}`)
    process.exit(2)
  }

  const source = JSON.parse(fs.readFileSync(sourcePath, "utf8"))
  const sourceKeys = flatten(source)

  let failed = false
  const files = fs
    .readdirSync(MESSAGES_DIR)
    .filter((f) => f.endsWith(".json") && f !== SOURCE_LOCALE)
    .sort()

  for (const fn of files) {
    const data = JSON.parse(
      fs.readFileSync(path.join(MESSAGES_DIR, fn), "utf8")
    )
    const keys = flatten(data)
    const missing = [...sourceKeys].filter((k) => !keys.has(k))
    const extra = [...keys].filter((k) => !sourceKeys.has(k))
    if (missing.length || extra.length) {
      failed = true
      console.error(
        `\n[i18n-parity] ${fn}: -${missing.length} +${extra.length}`
      )
      for (const k of missing.slice(0, 20)) console.error(`  missing: ${k}`)
      for (const k of extra.slice(0, 20)) console.error(`  extra:   ${k}`)
      if (missing.length > 20)
        console.error(`  …and ${missing.length - 20} more missing`)
      if (extra.length > 20)
        console.error(`  …and ${extra.length - 20} more extra`)
    }
  }

  if (failed) {
    console.error("\n[i18n-parity] FAIL")
    process.exit(1)
  }
  console.log(
    `[i18n-parity] OK — ${files.length} locales × ${sourceKeys.size} keys`
  )
}

main()
