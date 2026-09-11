#!/usr/bin/env node
//
// Prepare Tauri sidecars before `tauri build` / `tauri dev` consume them.
//
// What it does:
//   1. Resolves the target triple — `--target <triple>` arg, or
//      `TAURI_TARGET_TRIPLE` env, or the host's `rustc -vV` host triple.
//   2. Runs `cargo build --release --bin codeg-mcp --no-default-features`
//      for that triple from `src-tauri/`.
//   3. Copies the produced binary to
//      `src-tauri/binaries/codeg-mcp-<triple>{.exe}` so Tauri's externalBin
//      bundler picks it up under the bare name `codeg-mcp` at install time.
//
// Why a separate script (not inline in beforeBuildCommand / GitHub Actions):
//   - Cross-compile in release.yml passes `--target <triple>` so we honour
//     the matrix triple rather than rebuilding for the host.
//   - Local `pnpm tauri dev` / `pnpm tauri build` invoke it without args and
//     get a host-triple build, so the externalBin lookup still finds a file.
//   - Skippable: set `CODEG_SKIP_SIDECAR=1` when iterating on the frontend
//     and you don't care about delegation.
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
const BINARIES_DIR = join(SRC_TAURI, "binaries")
const BIN_NAME = "codeg-mcp"

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

function main() {
  if (process.env.CODEG_SKIP_SIDECAR === "1") {
    log("CODEG_SKIP_SIDECAR=1 — skipping sidecar preparation")
    return
  }

  const { target: cliTarget } = parseArgs(process.argv.slice(2))
  const target =
    cliTarget || process.env.TAURI_TARGET_TRIPLE || resolveHostTriple()
  const isWindows = target.includes("windows")
  const ext = isWindows ? ".exe" : ""

  log(`target triple: ${target}`)
  log(`building ${BIN_NAME} (--release --no-default-features)`)

  // cargo build needs to run from src-tauri so it resolves the local manifest
  // and shares the swatinem/rust-cache key with other cargo invocations.
  // `--no-default-features` keeps codeg-mcp free of the Tauri runtime deps —
  // the bin's required-features is empty, so this just enables cross-compile
  // without dragging in macOS-private-api / Linux WebKit / Windows WebView2.
  execFileSync(
    "cargo",
    [
      "build",
      "--release",
      "--bin",
      BIN_NAME,
      "--no-default-features",
      "--target",
      target,
    ],
    { stdio: "inherit", cwd: SRC_TAURI }
  )

  const built = join(
    SRC_TAURI,
    "target",
    target,
    "release",
    `${BIN_NAME}${ext}`
  )
  if (!existsSync(built)) {
    die(`expected ${built} after cargo build, but it does not exist`)
  }

  mkdirSync(BINARIES_DIR, { recursive: true })
  const dest = join(BINARIES_DIR, `${BIN_NAME}-${target}${ext}`)
  copyFileSync(built, dest)
  if (!isWindows) {
    // copyFileSync preserves modes on POSIX, but be explicit for tarball
    // sources that may strip the +x bit.
    chmodSync(dest, 0o755)
  }
  log(`sidecar staged at ${dest}`)
  stageTsnet(target, ext, isWindows)
}

function stageTsnet(target, ext, isWindows) {
  const goDir = resolve(SRC_TAURI, "..", "codeg-tsnet")
  if (!existsSync(join(goDir, "main.go"))) {
    log("codeg-tsnet source missing — skip")
    return
  }
  try {
    execFileSync("go", ["version"], { encoding: "utf8" })
  } catch {
    log("go not on PATH — skip codeg-tsnet sidecar")
    return
  }
  log("building codeg-tsnet (latest pinned tailscale.com)")
  execFileSync("go", ["mod", "tidy"], { stdio: "inherit", cwd: goDir })
  const outName = `codeg-tsnet${ext}`
  execFileSync("go", ["build", "-o", outName, "."], {
    stdio: "inherit",
    cwd: goDir,
    env: { ...process.env, CGO_ENABLED: "0" },
  })
  const built = join(goDir, outName)
  if (!existsSync(built)) {
    die(`expected ${built} after go build`)
  }
  mkdirSync(BINARIES_DIR, { recursive: true })
  const dest = join(BINARIES_DIR, `codeg-tsnet-${target}${ext}`)
  copyFileSync(built, dest)
  if (!isWindows) chmodSync(dest, 0o755)
  log(`sidecar staged at ${dest}`)
}

main()
