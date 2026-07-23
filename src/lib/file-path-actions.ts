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
 * Resolve the `openWith` argument for Tauri opener.
 *
 * macOS `open -a` matches application names (short CLI shims like `code`
 * fail). Windows/Linux keep CLI shims when those are on PATH.
 */
export function getExternalEditorOpenWith(
  editor: ExternalEditorId,
  platform: PlatformType = detectPlatform()
): string {
  if (platform === "macos") {
    return editor === "vscode" ? "Visual Studio Code" : "Cursor"
  }
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
