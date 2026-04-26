import { isDesktop } from "@/lib/platform"
import type { SystemOpenTarget } from "@/lib/types"

export type OpenTargetKind = "editor" | "file_manager" | "terminal"

export type OpenTargetSettingsLabelKey =
  | "openTargets.vscode"
  | "openTargets.file_manager"
  | "openTargets.terminal"

export type FileTreeOpenTargetLabelKey =
  | "openInBrowser"
  | "openInEditor"
  | "openInFileManager"
  | "openInFinder"
  | "openInExplorer"
  | "openInTerminal"

export interface OpenTargetRegistryItem {
  id: SystemOpenTarget
  kind: OpenTargetKind
  labelKey: OpenTargetSettingsLabelKey
  fileTreeLabelKey: FileTreeOpenTargetLabelKey
  supportsFiles: boolean
  supportsDirectories: boolean
  desktopOnly: boolean
}

export const OPEN_TARGET_REGISTRY: OpenTargetRegistryItem[] = [
  {
    id: "vscode",
    kind: "editor",
    labelKey: "openTargets.vscode",
    fileTreeLabelKey: "openInEditor",
    supportsFiles: true,
    supportsDirectories: false,
    desktopOnly: true,
  },
  {
    id: "file_manager",
    kind: "file_manager",
    labelKey: "openTargets.file_manager",
    fileTreeLabelKey: "openInFileManager",
    supportsFiles: true,
    supportsDirectories: true,
    desktopOnly: true,
  },
  {
    id: "terminal",
    kind: "terminal",
    labelKey: "openTargets.terminal",
    fileTreeLabelKey: "openInTerminal",
    supportsFiles: true,
    supportsDirectories: true,
    desktopOnly: false,
  },
]

export const OPEN_TARGET_VALUES: SystemOpenTarget[] = OPEN_TARGET_REGISTRY.map(
  (target) => target.id
)

export function isSystemOpenTarget(value: string): value is SystemOpenTarget {
  return OPEN_TARGET_VALUES.includes(value as SystemOpenTarget)
}

export function isWebFilePath(path: string): boolean {
  const cleanPath = path.split(/[?#]/, 1)[0] ?? path
  const extension = cleanPath.split(".").pop()?.toLowerCase()
  return extension === "html" || extension === "htm"
}

export function isWebFileLanguage(language?: string | null): boolean {
  return language === "html"
}

export function getOpenTargetRegistryItems(): OpenTargetRegistryItem[] {
  return OPEN_TARGET_REGISTRY.filter(
    (target) => !target.desktopOnly || isDesktop()
  )
}

export function getOpenTargetRegistryItem(
  target: SystemOpenTarget
): OpenTargetRegistryItem | null {
  return OPEN_TARGET_REGISTRY.find((item) => item.id === target) ?? null
}

export function getFileTreeOpenTargetItems(
  nodeKind: "file" | "dir"
): OpenTargetRegistryItem[] {
  return getOpenTargetRegistryItems().filter((target) =>
    nodeKind === "file" ? target.supportsFiles : target.supportsDirectories
  )
}

export function getPlatformFileManagerLabelKey():
  | "openInFileManager"
  | "openInFinder"
  | "openInExplorer" {
  if (typeof navigator === "undefined") return "openInFileManager"

  const platform = `${navigator.platform} ${navigator.userAgent}`.toLowerCase()
  if (platform.includes("mac")) return "openInFinder"
  if (platform.includes("win")) return "openInExplorer"
  return "openInFileManager"
}

export function getFileTreeOpenTargetLabelKey(
  target: OpenTargetRegistryItem
): FileTreeOpenTargetLabelKey {
  if (target.id === "file_manager") return getPlatformFileManagerLabelKey()
  return target.fileTreeLabelKey
}
