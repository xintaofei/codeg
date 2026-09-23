export interface PipelineLineNote {
  id?: string
  path: string
  line: number
  note: string
}

export type PipelineNote = PipelineLineNote

/**
 * Formats a list of line notes into a single string formatted as `path:line: note`.
 * Notes are sorted alphabetically by path, then by line number ascending.
 * Empty or whitespace-only notes are omitted.
 */
export function formatPipelineNotes(notes: PipelineLineNote[]): string {
  if (!Array.isArray(notes)) return ""

  const validNotes = notes.filter(
    (n) =>
      Boolean(n) &&
      typeof n.path === "string" &&
      n.path.trim().length > 0 &&
      typeof n.line === "number" &&
      n.line > 0 &&
      typeof n.note === "string" &&
      n.note.trim().length > 0
  )

  if (validNotes.length === 0) return ""

  const sorted = [...validNotes].sort((a, b) => {
    const pathCmp = a.path.localeCompare(b.path)
    if (pathCmp !== 0) return pathCmp
    return a.line - b.line
  })

  return sorted.map((n) => `${n.path}:${n.line}: ${n.note.trim()}`).join("\n")
}

/**
 * Parses formatted `path:line: note` strings into PipelineLineNote objects.
 */
export function parsePipelineNotes(text: string): PipelineLineNote[] {
  if (!text || typeof text !== "string" || !text.trim()) return []

  const lines = text.split("\n")
  const results: PipelineLineNote[] = []

  for (const rawLine of lines) {
    const trimmed = rawLine.trim()
    if (!trimmed) continue

    const match = trimmed.match(/^([^:]+):(\d+):\s*(.*)$/)
    if (match && match[1] && match[2] && match[3]) {
      const path = match[1].trim()
      const line = parseInt(match[2], 10)
      const note = match[3].trim()
      if (path && !Number.isNaN(line) && line > 0 && note) {
        results.push({
          id: `${path}:${line}`,
          path,
          line,
          note,
        })
      }
    }
  }

  return results
}

/**
 * Adds or updates a note for a given path and line.
 * If the note text is empty, any existing note for that path and line is removed.
 */
export function addOrUpdateNote(
  notes: PipelineLineNote[],
  item: { path: string; line: number; note: string; id?: string }
): PipelineLineNote[] {
  const trimmed = item.note ? item.note.trim() : ""
  const filtered = (notes || []).filter(
    (n) =>
      !(
        (item.id && n.id === item.id) ||
        (n.path === item.path && n.line === item.line)
      )
  )

  if (!trimmed) {
    return filtered
  }

  const newNote: PipelineLineNote = {
    id: item.id || `${item.path}:${item.line}`,
    path: item.path,
    line: item.line,
    note: trimmed,
  }

  return [...filtered, newNote]
}

/**
 * Removes a note matching by id or by { path, line }.
 */
export function removeNote(
  notes: PipelineLineNote[],
  target: string | { path: string; line: number }
): PipelineLineNote[] {
  if (!Array.isArray(notes)) return []

  if (typeof target === "string") {
    return notes.filter(
      (n) => n.id !== target && `${n.path}:${n.line}` !== target
    )
  }

  return notes.filter(
    (n) => !(n.path === target.path && n.line === target.line)
  )
}

/**
 * Gets all notes for a specific file path, sorted by line number ascending.
 */
export function getNotesForFile(
  notes: PipelineLineNote[],
  path: string
): PipelineLineNote[] {
  if (!Array.isArray(notes)) return []
  return notes
    .filter(
      (n) =>
        n.path === path &&
        typeof n.note === "string" &&
        n.note.trim().length > 0
    )
    .sort((a, b) => a.line - b.line)
}

/**
 * Gets a note for a specific file path and line number.
 */
export function getNoteForLine(
  notes: PipelineLineNote[],
  path: string,
  line: number
): PipelineLineNote | undefined {
  if (!Array.isArray(notes)) return undefined
  return notes.find(
    (n) =>
      n.path === path &&
      n.line === line &&
      typeof n.note === "string" &&
      n.note.trim().length > 0
  )
}

/**
 * Returns a map of path -> count of active notes.
 */
export function countNotesByFile(
  notes: PipelineLineNote[]
): Record<string, number> {
  const counts: Record<string, number> = {}
  if (!Array.isArray(notes)) return counts

  for (const n of notes) {
    if (n && n.path && typeof n.note === "string" && n.note.trim()) {
      counts[n.path] = (counts[n.path] || 0) + 1
    }
  }

  return counts
}
