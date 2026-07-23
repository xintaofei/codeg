/**
 * Shared actions for VS Code-style path context menus on changed-file rows
 * (message navigator + reply artifacts, and any future call sites).
 */

import {
  fileNameOf,
  toFolderRelativePath,
  toNativeAbsoluteFilePath,
} from "@/lib/file-path-display"
import { isLocalDesktop, openPath, revealItemInDir } from "@/lib/platform"
import { detectPlatform, type PlatformType } from "@/hooks/use-platform"
import { copyTextFromMenu } from "@/lib/utils"

export type ExternalEditorId = "vscode" | "cursor"

/**
 * Resolve a single preferred `openWith` name for the platform (macOS app name
 * or CLI shim). Windows should prefer {@link resolveExternalEditorOpenWithCandidates}
 * so we open `Cursor.exe` / `Code.exe` instead of a bare name that ShellExecute
 * can resolve to the install directory.
 */
export function getExternalEditorOpenWith(
  editor: ExternalEditorId,
  platform: PlatformType = detectPlatform()
): string {
  if (platform === "macos") {
    return editor === "vscode" ? "Visual Studio Code" : "Cursor"
  }
  // Windows + Linux: CLI entry points (must be on PATH). On Windows bare
  // "cursor" is unreliable — see resolveExternalEditorOpenWithCandidates.
  return editor === "vscode" ? "code" : "cursor"
}

/**
 * Ordered openWith candidates for the current platform.
 *
 * On Windows, `open::with(path, "cursor")` often launches the Cursor *install
 * folder* (ShellExecute name lookup) instead of opening `path` in the editor.
 * Prefer the real `.exe` under `%LOCALAPPDATA%\\Programs\\...` first, then
 * `.cmd` shims that forward arguments correctly.
 */
export async function resolveExternalEditorOpenWithCandidates(
  editor: ExternalEditorId,
  platform: PlatformType = detectPlatform()
): Promise<string[]> {
  if (platform === "macos") {
    return [getExternalEditorOpenWith(editor, "macos")]
  }

  if (platform === "linux") {
    // CLI shims first; some distros also register desktop ids.
    return editor === "vscode"
      ? ["code", "code-insiders"]
      : ["cursor", "cursor.AppImage"]
  }

  // Windows
  let localAppData = ""
  try {
    const { localDataDir } = await import("@tauri-apps/api/path")
    localAppData = await localDataDir()
  } catch {
    // Fall through to PATH-only candidates
  }

  const join = (base: string, ...parts: string[]) =>
    [base.replace(/[\\/]+$/, ""), ...parts].join("\\")

  if (editor === "vscode") {
    const candidates: string[] = []
    if (localAppData) {
      candidates.push(
        join(localAppData, "Programs", "Microsoft VS Code", "Code.exe"),
        join(localAppData, "Programs", "Microsoft VS Code", "bin", "code.cmd")
      )
    }
    candidates.push("code.cmd", "code")
    return candidates
  }

  // Cursor — install layout varies slightly by version/channel
  const candidates: string[] = []
  if (localAppData) {
    candidates.push(
      join(localAppData, "Programs", "cursor", "Cursor.exe"),
      join(localAppData, "Programs", "Cursor", "Cursor.exe"),
      join(
        localAppData,
        "Programs",
        "cursor",
        "resources",
        "app",
        "bin",
        "cursor.cmd"
      ),
      join(
        localAppData,
        "Programs",
        "Cursor",
        "resources",
        "app",
        "bin",
        "cursor.cmd"
      )
    )
  }
  candidates.push("cursor.cmd", "cursor")
  return candidates
}

/** CLI shim defaults (non-macOS). Prefer {@link getExternalEditorOpenWith}. */
export const EXTERNAL_EDITOR_OPEN_WITH: Record<ExternalEditorId, string> = {
  vscode: "code",
  cursor: "cursor",
}

export function resolveFilePathTargets(
  filePath: string,
  folderPath?: string
): {
  relativePath: string
  absolutePath: string | null
  fileName: string
} {
  const relativePath = toFolderRelativePath(filePath, folderPath)
  const absolutePath = toNativeAbsoluteFilePath(filePath, folderPath)
  return {
    relativePath,
    absolutePath,
    fileName: fileNameOf(relativePath),
  }
}

export async function copyPathText(text: string): Promise<boolean> {
  return copyTextFromMenu(text)
}

export async function revealFileInManager(absolutePath: string): Promise<void> {
  if (!isLocalDesktop()) return
  await revealItemInDir(absolutePath)
}

export async function openFileWithDefaultApp(
  absolutePath: string
): Promise<void> {
  if (!isLocalDesktop()) return
  await openPath(absolutePath)
}

export async function openFileWithExternalEditor(
  absolutePath: string,
  editor: ExternalEditorId
): Promise<void> {
  if (!isLocalDesktop()) return
  const candidates = await resolveExternalEditorOpenWithCandidates(editor)
  let lastError: unknown
  for (const app of candidates) {
    try {
      await openPath(absolutePath, app)
      return
    } catch (error) {
      lastError = error
    }
  }
  throw lastError instanceof Error
    ? lastError
    : new Error(`Failed to open with ${editor}`)
}

export function systemExplorerLabelKey(
  platformHint?: string
): "openInFinder" | "openInExplorer" | "openInFileManager" {
  const platform = (
    platformHint ??
    (typeof navigator !== "undefined"
      ? `${navigator.platform} ${navigator.userAgent}`
      : "")
  ).toLowerCase()
  if (platform.includes("mac")) return "openInFinder"
  if (platform.includes("win")) return "openInExplorer"
  return "openInFileManager"
}
