import { execFileSync } from "node:child_process"
import { mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"

function tryGitCommit() {
  try {
    return execFileSync("git", ["rev-parse", "--short=12", "HEAD"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim()
  } catch {
    return null
  }
}

const pkg = JSON.parse(
  readFileSync(new URL("../package.json", import.meta.url), "utf8")
)
const outPath = join(process.cwd(), "out", "codeg-build.json")

mkdirSync(dirname(outPath), { recursive: true })
writeFileSync(
  outPath,
  `${JSON.stringify(
    {
      version: pkg.version,
      git_commit: tryGitCommit(),
      built_at: new Date().toISOString(),
    },
    null,
    2
  )}\n`
)
