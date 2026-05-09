import { getTransport, isDesktop } from "./transport"

// All updater imports are dynamic to avoid crashing in non-Tauri browsers.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Update = any

export type DownloadEvent =
  | { event: "Started"; data: { contentLength?: number } }
  | { event: "Progress"; data: { chunkLength: number } }
  | { event: "Finished" }

export interface AppUpdateCheckResult {
  currentVersion: string
  update: Update | null
}

export type AppUpdateErrorKind =
  | "source_unreachable"
  | "network"
  | "download_failed"
  | "install_failed"
  | "unknown"

export interface AppUpdateErrorInfo {
  kind: AppUpdateErrorKind
  rawMessage: string
}

export async function getCurrentAppVersion(): Promise<string> {
  if (!isDesktop()) {
    const result =
      await getTransport().call<AppUpdateCheckResult>("check_app_update")
    return result.currentVersion
  }
  try {
    const { getVersion } = await import("@tauri-apps/api/app")
    return await getVersion()
  } catch {
    return "unknown"
  }
}

export async function checkAppUpdate(): Promise<AppUpdateCheckResult> {
  if (!isDesktop()) {
    return getTransport().call<AppUpdateCheckResult>("check_app_update")
  }
  const { getVersion } = await import("@tauri-apps/api/app")
  const { check } = await import("@tauri-apps/plugin-updater")
  const [currentVersion, update] = await Promise.all([getVersion(), check()])
  return { currentVersion, update }
}

export async function installAppUpdate(
  update: NonNullable<Update>,
  onEvent?: (progress: DownloadEvent) => void
): Promise<void> {
  // Web mode: server returns metadata only; downloadAndInstall is unavailable.
  // The browser-side user can't trigger a server-side install, so the caller
  // is expected to surface a "view release" affordance instead.
  if (typeof update?.downloadAndInstall !== "function") return
  await update.downloadAndInstall(onEvent)
}

export async function relaunchApp(): Promise<void> {
  const { relaunch } = await import("@tauri-apps/plugin-process")
  await relaunch()
}

export async function closeAppUpdate(
  update: NonNullable<Update>
): Promise<void> {
  if (typeof update?.close !== "function") return
  await update.close()
}

function toErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return String(error)
}

export function normalizeAppUpdateError(error: unknown): AppUpdateErrorInfo {
  const rawMessage = toErrorMessage(error)
  const normalized = rawMessage.toLowerCase()

  if (
    normalized.includes("latest.json") ||
    normalized.includes("/releases/latest/download/")
  ) {
    return { kind: "source_unreachable", rawMessage }
  }

  if (
    normalized.includes("error sending request for url") ||
    normalized.includes("failed to send request") ||
    normalized.includes("network") ||
    normalized.includes("timed out") ||
    normalized.includes("dns") ||
    normalized.includes("connection refused")
  ) {
    return { kind: "network", rawMessage }
  }

  if (
    normalized.includes("download") ||
    normalized.includes("checksum") ||
    normalized.includes("content-length")
  ) {
    return { kind: "download_failed", rawMessage }
  }

  if (
    normalized.includes("install") ||
    normalized.includes("installer") ||
    normalized.includes("permission denied")
  ) {
    return { kind: "install_failed", rawMessage }
  }

  return { kind: "unknown", rawMessage }
}
