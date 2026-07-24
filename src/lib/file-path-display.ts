/**
 * Shared helpers for displaying file paths and classifying diffs in the
 * changed-files UIs (the per-user-message navigator and the per-reply artifacts
 * card). Extracted so both call sites stay in sync.
 */

import { joinFsPath } from "@/lib/path-utils"

/** True when a unified diff represents a file deletion. */
export function isRemovedFileDiff(diff: string | null): boolean {
  if (!diff) return false
  return (
    /^\*\*\* Delete File:\s+/m.test(diff) ||
    /^deleted file mode\b/m.test(diff) ||
    /^\+\+\+\s+\/dev\/null$/m.test(diff)
  )
}

/**
 * True when a unified diff represents a newly created file. Covers the three
 * shapes our diff builders emit: apply_patch `*** Add File:`, git `new file
 * mode`, and the `--- /dev/null` header the `write` op synthesizes. An `edit`
 * with an empty old string does NOT match — `generateUnifiedDiff` still writes
 * an `--- a/<path>` header for those, so this only fires on genuine creations.
 */
export function isAddedFileDiff(diff: string | null): boolean {
  if (!diff) return false
  return (
    /^\*\*\* Add File:\s+/m.test(diff) ||
    /^new file mode\b/m.test(diff) ||
    /^---\s+\/dev\/null$/m.test(diff)
  )
}

/** Normalize backslashes to forward slashes for display/comparison. */
export function normalizeSlashPath(path: string): string {
  return path.replace(/\\/g, "/")
}

const WINDOWS_ABSOLUTE_PATH = /^[a-zA-Z]:[\\/]/

/** True when a path is absolute (POSIX `/…` or Windows `C:\…` / `C:/…`). */
export function isAbsoluteFilePath(path: string): boolean {
  const normalized = normalizeSlashPath(path)
  return normalized.startsWith("/") || WINDOWS_ABSOLUTE_PATH.test(normalized)
}

/**
 * Resolve a session file path to an absolute filesystem path for reveal-in-
 * file-manager. Absolute paths pass through (slash-normalized); relative ones
 * (e.g. Codex's cwd-relative `src/foo.ts`) are joined onto `folderPath`.
 * Returns null when the result cannot be made absolute (relative path with no
 * active folder) so callers can skip the reveal instead of opening a bad path.
 */
export function toAbsoluteFilePath(
  filePath: string,
  folderPath?: string
): string | null {
  const normalized = normalizeSlashPath(filePath)
  if (isAbsoluteFilePath(normalized)) return normalized
  if (!folderPath) return null

  const base = normalizeSlashPath(folderPath).replace(/\/+$/, "")
  if (!base) return null

  return `${base}/${normalized.replace(/^\.\/+/, "")}`
}

/**
 * Render `filePath` relative to `folderPath` when it lives inside that folder;
 * otherwise return the normalized absolute path unchanged.
 */
export function toFolderRelativePath(
  filePath: string,
  folderPath?: string
): string {
  const normalizedFilePath = normalizeSlashPath(filePath)
  if (!folderPath) return normalizedFilePath

  const normalizedFolderPath = normalizeSlashPath(folderPath).replace(
    /\/+$/,
    ""
  )
  if (!normalizedFolderPath) return normalizedFilePath

  const folderPrefix = `${normalizedFolderPath}/`

  // Windows file systems are case-insensitive, and an agent can report a path
  // whose drive-letter / segment casing differs from the stored folder path
  // (e.g. `c:\repo\…` vs `C:\Repo`). Compare case-insensitively when the folder
  // is a Windows drive path so the prefix still strips; the returned slice
  // keeps the ORIGINAL casing. POSIX stays case-sensitive (distinct dirs).
  // Test `folderPrefix` (not the trailing-slash-trimmed folder) so a drive-root
  // workspace like `C:\` — trimmed to `C:`, which fails the regex — is still
  // detected as Windows via its re-added slash (`C:/`).
  const isWindows = WINDOWS_ABSOLUTE_PATH.test(folderPrefix)
  const fileForCompare = isWindows
    ? normalizedFilePath.toLowerCase()
    : normalizedFilePath
  const prefixForCompare = isWindows ? folderPrefix.toLowerCase() : folderPrefix

  if (fileForCompare.startsWith(prefixForCompare)) {
    return normalizedFilePath.slice(folderPrefix.length)
  }

  return normalizedFilePath
}

/** The trailing file name of a (already slash-normalized) display path. */
export function fileNameOf(displayPath: string): string {
  const lastSlash = displayPath.lastIndexOf("/")
  return lastSlash >= 0 ? displayPath.slice(lastSlash + 1) : displayPath
}

/**
 * Absolute path using the OS's native separators — for clipboard copy and for
 * handing paths to the shell / opener. Relative agent paths are joined onto
 * `folderPath` via {@link joinFsPath} so Windows workspaces keep `\`.
 *
 * Unlike {@link toAbsoluteFilePath} (always slash-normalized for UI compare),
 * this preserves Windows backslashes when the workspace path uses them.
 * Returns null when the result cannot be made absolute.
 */
export function toNativeAbsoluteFilePath(
  filePath: string,
  folderPath?: string
): string | null {
  const trimmed = filePath.trim()
  if (!trimmed) return null

  if (isAbsoluteFilePath(trimmed)) {
    // Prefer the folder's separator style when known so clipboard matches
    // Explorer/Finder paths users already copy from the OS.
    if (folderPath?.includes("\\") || WINDOWS_ABSOLUTE_PATH.test(trimmed)) {
      return trimmed.replace(/\//g, "\\")
    }
    return normalizeSlashPath(trimmed)
  }

  if (!folderPath) return null
  const rel = normalizeSlashPath(trimmed).replace(/^\.\/+/, "")
  if (!rel) return null
  return joinFsPath(folderPath, rel)
}
