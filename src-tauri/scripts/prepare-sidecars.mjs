#!/usr/bin/env node
//
// Prepare Tauri sidecars before `tauri build` / `tauri dev` consume them.
//
// What it does:
//   1. Resolves the target triple — `--target <triple>` arg, or
//      `TAURI_TARGET_TRIPLE` env, or the host's `rustc -vV` host triple.
//   2. Builds both sidecars for that triple:
//        - `codeg-mcp` via `cargo build --release --bin codeg-mcp --no-default-features`
//        - `codeg-tsnet` via `go build` (pure Go tsnet Funnel control plane)
//   3. Copies each produced binary to
//      `src-tauri/binaries/<name>-<triple>{.exe}` so Tauri's externalBin
//      bundler picks them up under the bare names at install time.
//
// Why a separate script (not inline in beforeBuildCommand / GitHub Actions):
//   - Cross-compile in release.yml passes `--target <triple>` so we honour
//     the matrix triple rather than rebuilding for the host.
//   - Local `pnpm tauri dev` / `pnpm tauri build` invoke it without args and
//     get a host-triple build, so the externalBin lookup still finds a file.
//   - Skippable: set `CODEG_SKIP_SIDECAR=1` when iterating on the frontend
//     and you don't care about delegation / Funnel.
//
// Intentionally Node-only (no shell): runs identically on macOS, Linux,
// Windows GitHub runners.

import { execFileSync } from "node:child_process"
import { existsSync, copyFileSync, mkdirSync, chmodSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import process from "node:process"

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url))
const SRC_TAURI = resolve(SCRIPT_DIR, "..")
const REPO_ROOT = resolve(SRC_TAURI, "..")
const BINARIES_DIR = join(SRC_TAURI, "binaries")
const TSNET_DIR = join(REPO_ROOT, "codeg-tsnet")

const SIDECARS = [
  {
    name: "codeg-mcp",
    kind: "cargo",
  },
  {
    name: "codeg-tsnet",
    kind: "go",
  },
]

function log(msg) {
  console.log(`[prepare-sidecars] ${msg}`)
}

function die(msg) {
  console.error(`[prepare-sidecars][ERROR] ${msg}`)
  process.exit(1)
}

function parseArgs(argv) {
  const args = { target: null }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === "--target" && argv[i + 1]) {
      args.target = argv[++i]
    } else if (a.startsWith("--target=")) {
      args.target = a.slice("--target=".length)
    }
  }
  return args
}

function resolveHostTriple() {
  try {
    const out = execFileSync("rustc", ["-vV"], { encoding: "utf8" })
    const line = out.split(/\r?\n/).find((l) => l.startsWith("host:"))
    if (!line) throw new Error("rustc -vV missing host: line")
    return line.replace(/^host:\s*/, "").trim()
  } catch (e) {
    die(`cannot determine host triple via rustc -vV: ${e.message}`)
  }
}

function tripleToGoEnv(target) {
  const map = {
    "x86_64-apple-darwin": { GOOS: "darwin", GOARCH: "amd64" },
    "aarch64-apple-darwin": { GOOS: "darwin", GOARCH: "arm64" },
    "x86_64-unknown-linux-gnu": { GOOS: "linux", GOARCH: "amd64" },
    "aarch64-unknown-linux-gnu": { GOOS: "linux", GOARCH: "arm64" },
    "x86_64-pc-windows-msvc": { GOOS: "windows", GOARCH: "amd64" },
    "aarch64-pc-windows-msvc": { GOOS: "windows", GOARCH: "arm64" },
  }
  const env = map[target]
  if (!env) {
    die(`unsupported target triple for codeg-tsnet Go build: ${target}`)
  }
  return env
}

function stageBinary(built, dest, isWindows) {
  if (!existsSync(built)) {
    die(`expected ${built} after build, but it does not exist`)
  }
  mkdirSync(BINARIES_DIR, { recursive: true })
  copyFileSync(built, dest)
  if (!isWindows) {
    chmodSync(dest, 0o755)
  }
  log(`sidecar staged at ${dest}`)
}

function buildCargoSidecar(name, target, isWindows) {
  const ext = isWindows ? ".exe" : ""
  log(`building ${name} (--release --no-default-features)`)
  execFileSync(
    "cargo",
    [
      "build",
      "--release",
      "--bin",
      name,
      "--no-default-features",
      "--target",
      target,
    ],
    { stdio: "inherit", cwd: SRC_TAURI }
  )
  const built = join(SRC_TAURI, "target", target, "release", `${name}${ext}`)
  const dest = join(BINARIES_DIR, `${name}-${target}${ext}`)
  stageBinary(built, dest, isWindows)
}

function buildGoSidecar(name, target, isWindows) {
  const ext = isWindows ? ".exe" : ""
  if (!existsSync(TSNET_DIR)) {
    die(`missing ${TSNET_DIR}; cannot build ${name}`)
  }
  const goEnv = tripleToGoEnv(target)
  const outPath = join(TSNET_DIR, `${name}${ext}`)
  log(`building ${name} (go build GOOS=${goEnv.GOOS} GOARCH=${goEnv.GOARCH})`)
  execFileSync("go", ["build", "-o", outPath, "."], {
    stdio: "inherit",
    cwd: TSNET_DIR,
    env: {
      ...process.env,
      GOOS: goEnv.GOOS,
      GOARCH: goEnv.GOARCH,
      CGO_ENABLED: "0",
    },
  })
  const dest = join(BINARIES_DIR, `${name}-${target}${ext}`)
  stageBinary(outPath, dest, isWindows)
}

function main() {
  if (process.env.CODEG_SKIP_SIDECAR === "1") {
    log("CODEG_SKIP_SIDECAR=1 — skipping sidecar preparation")
    return
  }

  const { target: cliTarget } = parseArgs(process.argv.slice(2))
  const target =
    cliTarget || process.env.TAURI_TARGET_TRIPLE || resolveHostTriple()
  const isWindows = target.includes("windows")

  log(`target triple: ${target}`)

  for (const sidecar of SIDECARS) {
    if (sidecar.kind === "cargo") {
      buildCargoSidecar(sidecar.name, target, isWindows)
    } else if (sidecar.kind === "go") {
      buildGoSidecar(sidecar.name, target, isWindows)
    } else {
      die(`unknown sidecar kind for ${sidecar.name}`)
    }
  }
}

main()
