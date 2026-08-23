import { spawn, type ChildProcess } from "node:child_process"
import { constants as fsConstants } from "node:fs"
import { access, mkdir, readFile, rm, stat } from "node:fs/promises"
import { basename, join, resolve } from "node:path"

import { BrowserError } from "./errors.js"

export interface ChromeProcessOptions {
  profileDir: string
  downloadDir: string
  browserPath?: string
  startupTimeoutMs?: number
  headless?: boolean
}

export interface ChromeProcessInfo {
  executable: string
  name: string
  pid: number
  port: number
}

const DEFAULT_STARTUP_TIMEOUT_MS = 20_000
const LAUNCHER_HANDOFF_GRACE_MS = 2_000
const PROCESS_MONITOR_INTERVAL_MS = 1_000
const WINDOWS_PROFILE_PROCESS_QUERY = `
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$profile = [IO.Path]::GetFullPath($env:CODEG_BROWSER_PROFILE_DIR)
$needle = '--user-data-dir=' + $profile
Get-CimInstance Win32_Process -Filter "Name='chrome.exe' OR Name='msedge.exe' OR Name='chromium.exe'" |
  Where-Object {
    $commandLine = ([string]$_.CommandLine).Replace('"', '')
    $index = $commandLine.IndexOf($needle, [StringComparison]::OrdinalIgnoreCase)
    if ($index -lt 0) { return $false }
    $end = $index + $needle.Length
    return $end -eq $commandLine.Length -or [char]::IsWhiteSpace($commandLine[$end])
  } |
  ForEach-Object { "$($_.ProcessId),$($_.ParentProcessId)" }
`.trim()

async function isExecutableFile(path: string): Promise<boolean> {
  try {
    const details = await stat(path)
    if (!details.isFile()) return false
    await access(path, fsConstants.X_OK)
    return true
  } catch {
    return false
  }
}

export async function discoverBrowserPath(
  configuredPath?: string
): Promise<string> {
  if (configuredPath) {
    const path = resolve(configuredPath)
    if (await isExecutableFile(path)) return path
    throw new BrowserError(
      "BROWSER_NOT_FOUND",
      "The configured browser executable does not exist or cannot be executed",
      { recovery: "check_settings" }
    )
  }

  const candidates: string[] = []
  const programFiles = process.env.PROGRAMFILES
  const programFilesX86 = process.env["PROGRAMFILES(X86)"]
  const localAppData = process.env.LOCALAPPDATA
  for (const root of [programFiles, programFilesX86, localAppData]) {
    if (!root) continue
    candidates.push(
      join(root, "Google", "Chrome", "Application", "chrome.exe"),
      join(root, "Microsoft", "Edge", "Application", "msedge.exe"),
      join(root, "Chromium", "Application", "chrome.exe")
    )
  }
  if (process.platform === "darwin") {
    candidates.push(
      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
      "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
      "/Applications/Chromium.app/Contents/MacOS/Chromium"
    )
  }
  if (process.platform === "linux") {
    candidates.push(
      "/usr/bin/google-chrome",
      "/usr/bin/google-chrome-stable",
      "/usr/bin/microsoft-edge",
      "/usr/bin/chromium",
      "/usr/bin/chromium-browser"
    )
  }

  for (const candidate of candidates) {
    if (await isExecutableFile(candidate)) return candidate
  }
  throw new BrowserError(
    "BROWSER_NOT_FOUND",
    "No supported Chrome, Edge, or Chromium installation was found",
    { recovery: "check_settings" }
  )
}

function browserName(executable: string): string {
  const name = basename(executable).toLowerCase()
  if (name.includes("edge")) return "Microsoft Edge"
  if (name.includes("chromium")) return "Chromium"
  return "Google Chrome"
}

async function readDevToolsPort(portFile: string): Promise<number | null> {
  try {
    const [line] = (await readFile(portFile, "utf8")).split(/\r?\n/)
    const port = Number(line)
    return Number.isInteger(port) && port > 0 && port <= 65_535 ? port : null
  } catch {
    return null
  }
}

export async function waitForDevToolsPort(
  portFile: string,
  timeoutMs: number,
  child: Pick<ChildProcess, "exitCode">,
  options: { handoffGraceMs?: number; pollIntervalMs?: number } = {}
): Promise<number> {
  const deadline = Date.now() + timeoutMs
  const handoffGraceMs = options.handoffGraceMs ?? LAUNCHER_HANDOFF_GRACE_MS
  const pollIntervalMs = options.pollIntervalMs ?? 100
  let launcherExitTime: number | null = null

  while (Date.now() < deadline) {
    const port = await readDevToolsPort(portFile)
    if (port !== null) return port

    if (child.exitCode !== null) {
      launcherExitTime ??= Date.now()
      if (Date.now() - launcherExitTime >= handoffGraceMs) {
        throw new BrowserError(
          "RUNTIME_START_FAILED",
          "The browser exited before DevTools became ready",
          { recovery: "check_settings" }
        )
      }
    }
    await new Promise((resolveDelay) =>
      setTimeout(resolveDelay, pollIntervalMs)
    )
  }
  throw new BrowserError(
    "TIMEOUT",
    "Timed out waiting for the browser DevTools endpoint",
    { recovery: "check_settings" }
  )
}

export function parseWindowsListenerPid(
  output: string,
  port: number
): number | null {
  const endpoint = `127.0.0.1:${port}`
  for (const line of output.split(/\r?\n/)) {
    const fields = line.trim().split(/\s+/)
    if (fields[0]?.toUpperCase() !== "TCP" || fields[1] !== endpoint) continue
    const value = Number(fields[fields.length - 1])
    if (Number.isInteger(value) && value > 0) return value
  }
  return null
}

async function readCommandOutput(
  executable: string,
  args: string[],
  env?: NodeJS.ProcessEnv
): Promise<string> {
  return await new Promise<string>((resolveOutput, rejectOutput) => {
    const command = spawn(executable, args, {
      env,
      windowsHide: true,
      stdio: ["ignore", "pipe", "ignore"],
    })
    let output = ""
    command.stdout?.setEncoding("utf8")
    command.stdout?.on("data", (chunk: string) => {
      output += chunk
    })
    command.once("error", rejectOutput)
    command.once("close", (code) => {
      if (code === 0) resolveOutput(output)
      else
        rejectOutput(
          new Error(`${executable} exited with code ${String(code)}`)
        )
    })
  })
}

async function readWindowsNetstat(): Promise<string> {
  return await readCommandOutput("netstat.exe", ["-ano", "-p", "tcp"])
}

export function parseWindowsProfileProcessRoots(output: string): number[] {
  const processes = output
    .split(/\r?\n/)
    .map((line) => /^(\d+),(\d+)$/.exec(line.trim()))
    .filter((match): match is RegExpExecArray => match !== null)
    .map((match) => ({
      pid: Number(match[1]),
      parentPid: Number(match[2]),
    }))
    .filter(
      ({ pid, parentPid }) =>
        Number.isInteger(pid) &&
        pid > 0 &&
        Number.isInteger(parentPid) &&
        parentPid >= 0
    )
  const processIds = new Set(processes.map(({ pid }) => pid))
  return processes
    .filter(({ parentPid }) => !processIds.has(parentPid))
    .map(({ pid }) => pid)
}

async function cleanupWindowsProfileProcesses(
  profileDir: string
): Promise<void> {
  if (process.platform !== "win32") return
  try {
    await access(join(profileDir, "lockfile"))
  } catch {
    return
  }

  const env: NodeJS.ProcessEnv = {
    CODEG_BROWSER_PROFILE_DIR: profileDir,
    NODE_ENV: process.env.NODE_ENV,
    PATH: process.env.PATH ?? "",
    SystemRoot: process.env.SystemRoot ?? "C:\\Windows",
    WINDIR: process.env.WINDIR ?? "C:\\Windows",
  }
  let roots: number[]
  try {
    const output = await readCommandOutput(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        WINDOWS_PROFILE_PROCESS_QUERY,
      ],
      env
    )
    roots = parseWindowsProfileProcessRoots(output)
  } catch {
    // A clean profile can still launch when PowerShell/CIM is unavailable.
    return
  }
  await Promise.all(roots.map((pid) => killProcessTree(pid)))
}

function isProcessRunning(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM"
  }
}

async function resolveDevToolsOwnerPid(
  port: number,
  launcherPid: number
): Promise<number | null> {
  if (process.platform !== "win32") return launcherPid

  const deadline = Date.now() + LAUNCHER_HANDOFF_GRACE_MS
  do {
    try {
      const pid = parseWindowsListenerPid(await readWindowsNetstat(), port)
      if (pid !== null) return pid
    } catch {
      break
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 100))
  } while (Date.now() < deadline)

  return isProcessRunning(launcherPid) ? launcherPid : null
}

export class ManagedChromeProcess {
  private child: ChildProcess | null = null
  private infoValue: ChromeProcessInfo | null = null
  private stopping = false
  private starting = false
  private monitorTimer: NodeJS.Timeout | null = null
  private crashListener: ((code: number | null) => void) | undefined

  constructor(private readonly options: ChromeProcessOptions) {}

  get info(): ChromeProcessInfo | null {
    return this.infoValue
  }

  onCrash(listener: (code: number | null) => void): void {
    this.crashListener = listener
  }

  async start(): Promise<ChromeProcessInfo> {
    if (this.infoValue) return this.infoValue
    const executable = await discoverBrowserPath(this.options.browserPath)
    const profileDir = resolve(this.options.profileDir)
    const downloadDir = resolve(this.options.downloadDir)
    await Promise.all([
      mkdir(profileDir, { recursive: true }),
      mkdir(downloadDir, { recursive: true }),
    ])

    const portFile = join(profileDir, "DevToolsActivePort")
    await cleanupWindowsProfileProcesses(profileDir)
    await rm(portFile, { force: true })
    const chromeArgs = [
      ...(this.options.headless
        ? ["--headless=new", "--window-size=1440,900"]
        : []),
      "--remote-debugging-address=127.0.0.1",
      "--remote-debugging-port=0",
      `--user-data-dir=${profileDir}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-background-networking",
      "--disable-component-update",
      "--disable-sync",
      "--metrics-recording-only",
      "about:blank",
    ]
    const child = spawn(executable, chromeArgs, {
      detached: false,
      windowsHide: true,
      stdio: "ignore",
    })
    if (!child.pid) {
      throw new BrowserError(
        "RUNTIME_START_FAILED",
        "The browser process did not return a process identifier",
        { recovery: "check_settings" }
      )
    }
    this.child = child
    this.stopping = false
    this.starting = true
    child.once("exit", (code) => {
      if (this.child !== child) return
      this.child = null
      if (this.stopping || this.starting) return
      if (this.infoValue?.pid && this.infoValue.pid !== child.pid) return
      this.handleUnexpectedExit(code, child.pid)
    })

    try {
      const port = await waitForDevToolsPort(
        portFile,
        this.options.startupTimeoutMs ?? DEFAULT_STARTUP_TIMEOUT_MS,
        child
      )
      const pid = await resolveDevToolsOwnerPid(port, child.pid)
      if (pid === null) {
        throw new BrowserError(
          "RUNTIME_START_FAILED",
          "The browser DevTools endpoint has no live owning process",
          { recovery: "check_settings" }
        )
      }
      this.infoValue = {
        executable,
        name: browserName(executable),
        pid,
        port,
      }
      this.starting = false
      if (pid !== child.pid || child.exitCode !== null) {
        this.monitorProcess(pid)
      }
      return this.infoValue
    } catch (error) {
      this.starting = false
      await this.stop()
      if (error instanceof BrowserError) throw error
      throw new BrowserError(
        "RUNTIME_START_FAILED",
        "The browser did not expose its DevTools endpoint",
        { cause: error, recovery: "check_settings" }
      )
    }
  }

  async stop(): Promise<void> {
    const child = this.child
    const info = this.infoValue
    this.stopping = true
    this.starting = false
    this.child = null
    this.infoValue = null
    this.clearMonitor()

    const pids = new Set<number>()
    if (info?.pid) pids.add(info.pid)
    if (child?.pid && child.exitCode === null) pids.add(child.pid)
    try {
      await Promise.all([...pids].map((pid) => killProcessTree(pid)))
    } finally {
      this.stopping = false
    }
  }

  private monitorProcess(pid: number): void {
    this.clearMonitor()
    this.monitorTimer = setInterval(() => {
      if (this.stopping || this.infoValue?.pid !== pid) return
      if (!isProcessRunning(pid)) this.handleUnexpectedExit(null, pid)
    }, PROCESS_MONITOR_INTERVAL_MS)
    this.monitorTimer.unref()
  }

  private clearMonitor(): void {
    if (this.monitorTimer) clearInterval(this.monitorTimer)
    this.monitorTimer = null
  }

  private handleUnexpectedExit(code: number | null, pid?: number): void {
    if (this.stopping) return
    if (pid && this.infoValue?.pid && this.infoValue.pid !== pid) return
    this.child = null
    this.infoValue = null
    this.clearMonitor()
    this.crashListener?.(code)
  }
}

async function killProcessTree(pid: number): Promise<void> {
  if (!Number.isInteger(pid) || pid <= 0) return
  if (process.platform === "win32") {
    await new Promise<void>((resolveDone) => {
      const killer = spawn("taskkill.exe", ["/PID", String(pid), "/T", "/F"], {
        windowsHide: true,
        stdio: "ignore",
      })
      killer.once("error", () => resolveDone())
      killer.once("exit", () => resolveDone())
    })
    return
  }
  try {
    process.kill(pid, "SIGTERM")
  } catch {
    return
  }
  await new Promise((resolveDone) => setTimeout(resolveDone, 750))
  try {
    process.kill(pid, "SIGKILL")
  } catch {
    // The process already exited.
  }
}
