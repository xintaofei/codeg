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
 * Resolve the `openWith` argument for Tauri opener across platforms.
 *
 * - **macOS**: full application names for `open -a` (`code`/`Code` fail)
 * - **Windows**: CLI shims on PATH (`code.cmd` / `cursor.cmd` via `code`/`cursor`)
 * - **Linux**: same CLI shims, or desktop-file basenames when available
 */
export function getExternalEditorOpenWith(
  editor: ExternalEditorId,
  platform: PlatformType = detectPlatform()
): string {
  if (platform === "macos") {
    return editor === "vscode" ? "Visual Studio Code" : "Cursor"
  }
  // Windows + Linux + unknown: CLI entry points (must be on PATH)
  return editor === "vscode" ? "code" : "cursor"
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
  await openPath(absolutePath, getExternalEditorOpenWith(editor))
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
