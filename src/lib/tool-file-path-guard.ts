/**
 * Pure guards for deciding when a tool row should get a filesystem path menu.
 * Kept free of React so unit tests can lock false-positive regressions
 * (WebFetch / Glob / multi-file Edit titles).
 */

/** True for tools that primarily target a single file path. */
export function isFilePathToolName(toolName: string): boolean {
  const name = toolName.toLowerCase()
  return (
    name === "read" ||
    name === "read file" ||
    name === "read_file" ||
    name === "read_text_file" ||
    name === "readfile" ||
    name === "view" ||
    name === "write" ||
    name === "write_file" ||
    name === "writefile" ||
    name === "notebookedit" ||
    name === "edit" ||
    name === "edit_file" ||
    name === "editfile" ||
    name === "apply_patch"
  )
}

/**
 * Strip a tool-title prefix to recover a file path segment.
 * Intentionally strict — never treat URLs, globs, or aggregate titles as paths.
 */
export function pathFromToolTitle(
  title: string | null | undefined
): string | null {
  if (!title) return null
  const trimmed = title.trim()
  const en = trimmed.match(/^(?:Read|Edit|Write|NotebookEdit)\s+(.+)$/i)
  const localized = trimmed.match(
    /^(?:读取|讀取|编辑|編輯|写入|寫入|読み取り|읽기)\s+(.+)$/u
  )
  const target = (en?.[1] ?? localized?.[1])?.trim()
  if (!target) return null

  if (/^\(\d+\s+files?\)$/i.test(target)) return null
  if (/[()]/.test(target)) return null
  if (/^https?:\/\//i.test(target)) return null
  if (/[*?[\]{}]/.test(target)) return null
  // Spaces usually mean prose (allow Windows drive paths with spaces only)
  if (/\s/.test(target) && !/^[A-Za-z]:[\\/]/.test(target)) return null

  return target
}
