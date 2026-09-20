import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)

const MESSAGES_DIR = path.resolve(__dirname, "../src/i18n/messages")
const EN_FILE = path.join(MESSAGES_DIR, "en.json")

function collectKeys(node, prefix = "") {
  if (typeof node !== "object" || node === null) {
    return [prefix]
  }
  const out = []
  for (const [key, value] of Object.entries(node)) {
    const next = prefix ? `${prefix}.${key}` : key
    out.push(...collectKeys(value, next))
  }
  return out
}

function run() {
  if (!fs.existsSync(EN_FILE)) {
    console.error(`en.json not found at ${EN_FILE}`)
    process.exit(1)
  }

  let enContent
  try {
    enContent = JSON.parse(fs.readFileSync(EN_FILE, "utf8"))
  } catch (err) {
    console.error(`Failed to parse en.json: ${err.message}`)
    process.exit(1)
  }

  const enKeys = new Set(collectKeys(enContent))
  console.log(`Reference (en.json): ${enKeys.size} recursive keys`)

  const files = fs
    .readdirSync(MESSAGES_DIR)
    .filter((f) => f.endsWith(".json") && f !== "en.json")
    .sort()

  let hasDiscrepancy = false

  for (const file of files) {
    const fullPath = path.join(MESSAGES_DIR, file)
    let content
    try {
      content = JSON.parse(fs.readFileSync(fullPath, "utf8"))
    } catch (err) {
      console.error(`[FAIL] ${file}: Failed to parse JSON - ${err.message}`)
      hasDiscrepancy = true
      continue
    }

    const localeKeys = new Set(collectKeys(content))
    const missing = [...enKeys].filter((k) => !localeKeys.has(k))
    const extra = [...localeKeys].filter((k) => !enKeys.has(k))

    if (missing.length === 0 && extra.length === 0) {
      console.log(`[PASS] ${file}: ${localeKeys.size} keys match en.json`)
    } else {
      hasDiscrepancy = true
      console.error(
        `[FAIL] ${file}: Discrepancies found (${localeKeys.size} keys vs ${enKeys.size} reference)`
      )
      if (missing.length > 0) {
        console.error(`  Missing in ${file} (${missing.length}):`)
        for (const k of missing) {
          console.error(`    - ${k}`)
        }
      }
      if (extra.length > 0) {
        console.error(`  Extra in ${file} (${extra.length}):`)
        for (const k of extra) {
          console.error(`    + ${k}`)
        }
      }
    }
  }

  if (hasDiscrepancy) {
    console.error("\ni18n parity check failed.")
    process.exit(1)
  }

  console.log("\nAll locales are in perfect parity with en.json.")
  process.exit(0)
}

run()
