#!/usr/bin/env node

import { resolve } from "node:path"

import { BrowserRuntime } from "./browser-runtime.js"
import { ChromeBackend } from "./chrome-backend.js"
import { BrowserSidecarServer } from "./mcp-server.js"
import { NativeWebViewBackend } from "./native-webview-backend.js"
import type { BrowserBackend } from "./runtime-types.js"

interface CliOptions {
  profileDir: string
  downloadDir: string
  browserPath?: string
  parentPid?: number
  autoStart: boolean
  backend: "embedded" | "external"
}

const VERSION = "0.1.0"

async function main(): Promise<void> {
  if (process.argv.includes("--version")) {
    process.stdout.write(`${VERSION}\n`)
    return
  }

  const options = parseArguments(process.argv.slice(2))
  const token = process.env.CODEG_BROWSER_TOKEN
  if (!token || Buffer.byteLength(token, "utf8") < 32) {
    throw new Error("missing_or_short_control_token")
  }
  const backend = createBackend(options)
  const runtime = new BrowserRuntime(backend)
  const sidecar = new BrowserSidecarServer({
    runtime,
    token,
    onControlEvent: writeControlEvent,
  })

  const port = await sidecar.start()
  writeControlEvent({
    event: "ready",
    version: VERSION,
    pid: process.pid,
    port,
  })

  let parentTimer: ReturnType<typeof setInterval> | undefined
  if (options.parentPid) {
    parentTimer = setInterval(() => {
      if (!processExists(options.parentPid!)) void shutdown("parent_exited")
    }, 2_000)
    parentTimer.unref()
  }

  let shuttingDown = false
  async function shutdown(reason: string): Promise<void> {
    if (shuttingDown) return
    shuttingDown = true
    if (parentTimer) clearInterval(parentTimer)
    await sidecar.stop().catch(() => undefined)
    writeControlEvent({ event: "stopped", reason })
    process.exit(0)
  }

  process.once("SIGINT", () => void shutdown("signal"))
  process.once("SIGTERM", () => void shutdown("signal"))

  if (options.autoStart) {
    const result = await runtime.callTool("admin", "runtime.start", {})
    writeControlEvent({
      event: "browser-status",
      status: runtime.status(),
      ok: result.envelope.ok,
    })
  }
}

function createBackend(options: CliOptions): BrowserBackend {
  if (options.backend === "embedded") {
    const endpoint = process.env.CODEG_BROWSER_NATIVE_BRIDGE_ENDPOINT
    const token = process.env.CODEG_BROWSER_NATIVE_BRIDGE_TOKEN
    if (!endpoint || !token) {
      throw new Error("native_webview_bridge_credentials_missing")
    }
    return new NativeWebViewBackend({ endpoint, token })
  }
  return new ChromeBackend({
    profileDir: options.profileDir,
    downloadDir: options.downloadDir,
    browserPath: options.browserPath,
    headless: false,
  })
}

function parseArguments(args: string[]): CliOptions {
  const values = new Map<string, string>()
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index]!
    if (!argument.startsWith("--")) throw new Error("invalid_argument")
    const equalsAt = argument.indexOf("=")
    if (equalsAt >= 0) {
      values.set(argument.slice(2, equalsAt), argument.slice(equalsAt + 1))
      continue
    }
    const next = args[index + 1]
    if (!next || next.startsWith("--")) {
      values.set(argument.slice(2), "true")
      continue
    }
    values.set(argument.slice(2), next)
    index += 1
  }

  const profileDir = values.get("profile-dir")
  const downloadDir = values.get("download-dir")
  if (!profileDir || !downloadDir) {
    throw new Error("profile_and_download_directories_are_required")
  }
  const parentPidRaw = values.get("parent-pid")
  const parentPid = parentPidRaw ? Number(parentPidRaw) : undefined
  if (
    parentPid !== undefined &&
    (!Number.isInteger(parentPid) || parentPid <= 0)
  ) {
    throw new Error("invalid_parent_pid")
  }
  const backend = values.get("backend") ?? "external"
  if (backend !== "embedded" && backend !== "external") {
    throw new Error("invalid_backend")
  }
  return {
    profileDir: resolve(profileDir),
    downloadDir: resolve(downloadDir),
    browserPath: values.get("browser-path"),
    parentPid,
    autoStart: values.get("auto-start") === "true",
    backend,
  }
}

function processExists(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

function writeControlEvent(event: Record<string, unknown>): void {
  process.stdout.write(`${JSON.stringify(event)}\n`)
}

void main().catch((error: unknown) => {
  const code = error instanceof Error ? error.message : "unknown_fatal_error"
  process.stderr.write(`${JSON.stringify({ event: "fatal", code })}\n`)
  process.exitCode = 1
})
