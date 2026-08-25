"use client"

import { Fragment, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { Columns2, Rows3 } from "lucide-react"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { cn } from "@/lib/utils"
import { useDiffViewMode, type DiffViewMode } from "@/lib/diff-view-mode-prefs"
import { ScrollArea } from "@/components/ui/scroll-area"
import { FilePathLink } from "@/components/ai-elements/link-safety"

type RowMarker = "none" | "added" | "deleted" | "modified"
type DiffFileMode = "modified" | "added" | "deleted" | "renamed"

export type { DiffViewMode }

interface RawDiffRow {
  kind: "context" | "add" | "del"
  text: string
  oldLine: number | null
  newLine: number | null
}

export interface ParsedDiffRow {
  type: "context" | "added" | "deleted" | "modified"
  text: string
  sign: " " | "+" | "-"
  oldLine: number | null
  newLine: number | null
}

interface ParsedDiffHunk {
  key: string
  oldStart: number | null
  oldCount: number | null
  newStart: number | null
  newCount: number | null
  rows: ParsedDiffRow[]
}

interface ParsedDiffFile {
  key: string
  path: string
  oldPath: string | null
  newPath: string | null
  mode: DiffFileMode
  additions: number
  deletions: number
  hunks: ParsedDiffHunk[]
}

interface WorkingHunk {
  key: string
  oldStart: number | null
  oldCount: number | null
  newStart: number | null
  newCount: number | null
  rows: RawDiffRow[]
}

interface WorkingFile {
  key: string
  path: string
  oldPath: string | null
  newPath: string | null
  mode: DiffFileMode
  additions: number
  deletions: number
  hunks: WorkingHunk[]
}

const ROW_CLASS: Record<RowMarker, string> = {
  none: "",
  added: "bg-green-500/10 text-green-900 dark:text-green-300",
  deleted: "bg-red-500/10 text-red-900 dark:text-red-300",
  modified: "bg-blue-500/10 text-blue-900 dark:text-blue-300",
}

const SIGN_CLASS: Record<string, string> = {
  "+": "text-green-700 dark:text-green-400",
  "-": "text-red-700 dark:text-red-400",
  " ": "text-muted-foreground/50",
}

function normalizePath(raw: string): string | null {
  const trimmed = raw.trim().replace(/^"|"$/g, "")
  if (!trimmed || trimmed === "/dev/null") return null
  if (trimmed.startsWith("a/") || trimmed.startsWith("b/")) {
    return trimmed.slice(2).replace(/\\/g, "/")
  }
  return trimmed.replace(/\\/g, "/")
}

function parsePathFromDiffGitLine(line: string): string | null {
  const match = line.match(/^diff --git\s+(.+?)\s+(.+)$/)
  if (!match) return null
  return normalizePath(match[2]) ?? normalizePath(match[1])
}

function parseApplyPatchMarker(line: string): {
  path: string | null
  mode: DiffFileMode
} | null {
  if (line.startsWith("*** Update File: ")) {
    return {
      path: normalizePath(line.slice("*** Update File: ".length)),
      mode: "modified",
    }
  }
  if (line.startsWith("*** Add File: ")) {
    return {
      path: normalizePath(line.slice("*** Add File: ".length)),
      mode: "added",
    }
  }
  if (line.startsWith("*** Delete File: ")) {
    return {
      path: normalizePath(line.slice("*** Delete File: ".length)),
      mode: "deleted",
    }
  }
  return null
}

function parseHunkHeader(line: string): {
  oldStart: number | null
  oldCount: number | null
  newStart: number | null
  newCount: number | null
} | null {
  if (!line.startsWith("@@")) return null

  const match = line.match(/^@@\s*-(\d+)(?:,(\d+))?\s+\+(\d+)(?:,(\d+))?\s*@@/)
  if (!match) {
    return {
      oldStart: null,
      oldCount: null,
      newStart: null,
      newCount: null,
    }
  }

  return {
    oldStart: Number(match[1]),
    oldCount: match[2] ? Number(match[2]) : 1,
    newStart: Number(match[3]),
    newCount: match[4] ? Number(match[4]) : 1,
  }
}

function classifyRows(rows: RawDiffRow[]): ParsedDiffRow[] {
  const parsed: ParsedDiffRow[] = []
  let index = 0

  while (index < rows.length) {
    const current = rows[index]
    if (!current) break

    if (current.kind === "context") {
      parsed.push({
        type: "context",
        text: current.text,
        sign: " ",
        oldLine: current.oldLine,
        newLine: current.newLine,
      })
      index += 1
      continue
    }

    if (current.kind === "add") {
      let addEnd = index
      while (addEnd < rows.length && rows[addEnd]?.kind === "add") {
        const row = rows[addEnd]
        if (!row) break
        parsed.push({
          type: "added",
          text: row.text,
          sign: "+",
          oldLine: row.oldLine,
          newLine: row.newLine,
        })
        addEnd += 1
      }
      index = addEnd
      continue
    }

    let delEnd = index
    while (delEnd < rows.length && rows[delEnd]?.kind === "del") {
      delEnd += 1
    }

    let addEnd = delEnd
    while (addEnd < rows.length && rows[addEnd]?.kind === "add") {
      addEnd += 1
    }

    for (let d = index; d < delEnd; d++) {
      const row = rows[d]
      if (!row) continue
      parsed.push({
        type: "deleted",
        text: row.text,
        sign: "-",
        oldLine: row.oldLine,
        newLine: row.newLine,
      })
    }

    for (let a = delEnd; a < addEnd; a++) {
      const row = rows[a]
      if (!row) continue
      parsed.push({
        type: "added",
        text: row.text,
        sign: "+",
        oldLine: row.oldLine,
        newLine: row.newLine,
      })
    }

    index = addEnd
  }

  return parsed
}

function resolveFileMode(file: WorkingFile): DiffFileMode {
  if (file.mode !== "modified") return file.mode
  if (file.oldPath && !file.newPath) return "deleted"
  if (!file.oldPath && file.newPath) return "added"
  if (file.oldPath && file.newPath && file.oldPath !== file.newPath) {
    return "renamed"
  }
  return "modified"
}

function parseUnifiedDiff(diffText: string): ParsedDiffFile[] {
  const lines = diffText.replace(/\r\n/g, "\n").split("\n")
  const files: WorkingFile[] = []

  let fileIndex = 1
  let hunkIndex = 1
  let currentFile: WorkingFile | null = null
  let currentHunk: WorkingHunk | null = null
  let oldLineCursor: number | null = null
  let newLineCursor: number | null = null
  let inferredOldCursorForNextHunk = 1
  let inferredNewCursorForNextHunk = 1

  const getActiveFile = (): WorkingFile | null =>
    currentFile ?? files[files.length - 1] ?? null
  const getActiveHunk = (): WorkingHunk | null => currentHunk
  const getOldLineCursor = (): number | null => oldLineCursor
  const getNewLineCursor = (): number | null => newLineCursor

  const flushHunk = () => {
    const file = getActiveFile()
    if (!file || !currentHunk) return
    file.hunks.push(currentHunk)
    if (oldLineCursor !== null) {
      inferredOldCursorForNextHunk = Math.max(1, oldLineCursor)
    }
    if (newLineCursor !== null) {
      inferredNewCursorForNextHunk = Math.max(1, newLineCursor)
    }
    currentHunk = null
  }

  const startFile = (
    path: string | null,
    mode: DiffFileMode = "modified"
  ): WorkingFile => {
    flushHunk()
    currentFile = {
      key: `file-${fileIndex}`,
      path: path ?? `Diff #${fileIndex}`,
      oldPath: null,
      newPath: null,
      mode,
      additions: 0,
      deletions: 0,
      hunks: [],
    }
    files.push(currentFile)
    fileIndex += 1
    inferredOldCursorForNextHunk = 1
    inferredNewCursorForNextHunk = 1
    return currentFile
  }

  const ensureFile = () => getActiveFile() ?? startFile(null)

  const startHunk = (line: string) => {
    const file = ensureFile()
    flushHunk()

    const parsed = parseHunkHeader(line)
    const resolvedOldStart = parsed?.oldStart ?? inferredOldCursorForNextHunk
    const resolvedNewStart = parsed?.newStart ?? inferredNewCursorForNextHunk
    oldLineCursor = resolvedOldStart
    newLineCursor = resolvedNewStart

    currentHunk = {
      key: `${file.key}:hunk-${hunkIndex}`,
      oldStart: resolvedOldStart,
      oldCount: parsed?.oldCount ?? null,
      newStart: resolvedNewStart,
      newCount: parsed?.newCount ?? null,
      rows: [],
    }
    hunkIndex += 1
  }

  for (const line of lines) {
    if (line.startsWith("diff --git ")) {
      startFile(parsePathFromDiffGitLine(line))
      continue
    }

    const applyPatchMarker = parseApplyPatchMarker(line)
    if (applyPatchMarker) {
      startFile(applyPatchMarker.path, applyPatchMarker.mode)
      continue
    }

    if (line.startsWith("*** Move to: ")) {
      const movedPath = normalizePath(line.slice("*** Move to: ".length))
      const file = getActiveFile()
      if (file && movedPath) {
        file.newPath = movedPath
        file.path = movedPath
        file.mode = "renamed"
      }
      continue
    }

    if (line.startsWith("--- ")) {
      const file = ensureFile()
      const oldPath = normalizePath(line.slice(4))
      file.oldPath = oldPath
      if (!file.newPath && oldPath) file.path = oldPath
      continue
    }

    if (line.startsWith("+++ ")) {
      const file = ensureFile()
      const newPath = normalizePath(line.slice(4))
      file.newPath = newPath
      if (newPath) file.path = newPath
      continue
    }

    if (line.startsWith("@@")) {
      startHunk(line)
      continue
    }

    let hunk = getActiveHunk()
    // Auto-create an implicit hunk for patch formats (e.g. *** Add File)
    // that emit +/- lines without a preceding @@ header.
    if (
      !hunk &&
      (line.startsWith("+") || line.startsWith("-") || line.startsWith(" "))
    ) {
      if (getActiveFile()) {
        startHunk("@@")
        hunk = getActiveHunk()
      }
    }
    if (!hunk) continue

    if (line.startsWith("+") && !line.startsWith("+++")) {
      hunk.rows.push({
        kind: "add",
        text: line.slice(1),
        oldLine: null,
        newLine: newLineCursor,
      })
      const cursor = getNewLineCursor()
      if (cursor !== null) newLineCursor = cursor + 1
      const file = getActiveFile()
      if (file) file.additions += 1
      continue
    }

    if (line.startsWith("-") && !line.startsWith("---")) {
      hunk.rows.push({
        kind: "del",
        text: line.slice(1),
        oldLine: oldLineCursor,
        newLine: null,
      })
      const cursor = getOldLineCursor()
      if (cursor !== null) oldLineCursor = cursor + 1
      const file = getActiveFile()
      if (file) file.deletions += 1
      continue
    }

    if (line.startsWith(" ")) {
      hunk.rows.push({
        kind: "context",
        text: line.slice(1),
        oldLine: oldLineCursor,
        newLine: newLineCursor,
      })
      const nextOldCursor = getOldLineCursor()
      if (nextOldCursor !== null) oldLineCursor = nextOldCursor + 1
      const nextNewCursor = getNewLineCursor()
      if (nextNewCursor !== null) newLineCursor = nextNewCursor + 1
    }
  }

  flushHunk()

  return files
    .map((file) => ({
      ...file,
      mode: resolveFileMode(file),
      hunks: file.hunks
        .filter((hunk) => hunk.rows.length > 0)
        .map((hunk) => ({
          key: hunk.key,
          oldStart: hunk.oldStart,
          oldCount: hunk.oldCount,
          newStart: hunk.newStart,
          newCount: hunk.newCount,
          rows: classifyRows(hunk.rows),
        })),
    }))
    .filter((file) => file.hunks.length > 0)
}

function modeKey(
  mode: DiffFileMode
): "mode.added" | "mode.deleted" | "mode.renamed" | "mode.modified" {
  if (mode === "added") return "mode.added"
  if (mode === "deleted") return "mode.deleted"
  if (mode === "renamed") return "mode.renamed"
  return "mode.modified"
}

function toDisplayPath(filePath: string, folderPath: string | null): string {
  const normalizedPath = filePath.replace(/\\/g, "/")
  if (!folderPath) return normalizedPath

  const normalizedFolder = folderPath.replace(/\\/g, "/").replace(/\/+$/, "")
  if (!normalizedFolder) return normalizedPath

  const prefix = `${normalizedFolder}/`
  if (normalizedPath.startsWith(prefix)) {
    return normalizedPath.slice(prefix.length)
  }

  return normalizedPath
}

function rowMarker(row: ParsedDiffRow): RowMarker {
  if (row.type === "added") return "added"
  if (row.type === "deleted") return "deleted"
  return "none"
}

/** One half of a side-by-side row. `text === null` marks the filler cell a
 *  delete-only or add-only block leaves on the opposite side. */
export interface SplitCell {
  line: number | null
  text: string | null
  marker: RowMarker
}

export interface SplitRow {
  left: SplitCell
  right: SplitCell
}

const EMPTY_CELL: SplitCell = { line: null, text: null, marker: "none" }

/**
 * Re-pair a hunk's unified rows for the side-by-side view: context rows span
 * both sides, and each delete-run followed by its add-run is zipped
 * positionally — the i-th deleted line faces the i-th added line, and the
 * longer run's remainder faces a filler cell (the same alignment GitHub's
 * split view uses; the lines share a row, they are not claimed to be
 * related).
 */
export function toSplitRows(rows: ParsedDiffRow[]): SplitRow[] {
  const out: SplitRow[] = []
  let index = 0

  while (index < rows.length) {
    const row = rows[index]
    if (!row) break

    // Everything that is not part of a delete/add run spans both sides. The
    // branch keys off "not added and not deleted" rather than "is context" so
    // that the loop is guaranteed to consume a row on every pass: a row of the
    // fourth `ParsedDiffRow` type ("modified") would otherwise match neither
    // this branch nor either run below, leaving `index` unmoved and spinning
    // the tab forever.
    if (row.type !== "added" && row.type !== "deleted") {
      out.push({
        left: { line: row.oldLine, text: row.text, marker: "none" },
        right: { line: row.newLine, text: row.text, marker: "none" },
      })
      index += 1
      continue
    }

    const dels: ParsedDiffRow[] = []
    const adds: ParsedDiffRow[] = []
    while (index < rows.length && rows[index]?.type === "deleted") {
      dels.push(rows[index]!)
      index += 1
    }
    while (index < rows.length && rows[index]?.type === "added") {
      adds.push(rows[index]!)
      index += 1
    }

    const pairs = Math.max(dels.length, adds.length)
    for (let p = 0; p < pairs; p++) {
      const del = dels[p]
      const add = adds[p]
      out.push({
        left: del
          ? { line: del.oldLine, text: del.text, marker: "deleted" }
          : EMPTY_CELL,
        right: add
          ? { line: add.newLine, text: add.text, marker: "added" }
          : EMPTY_CELL,
      })
    }
  }

  return out
}

function HunkSeparator({ hunk }: { hunk: ParsedDiffHunk }) {
  const label =
    hunk.oldStart != null && hunk.oldCount != null
      ? `@@ -${hunk.oldStart},${hunk.oldCount} +${hunk.newStart ?? hunk.oldStart},${hunk.newCount ?? hunk.oldCount} @@`
      : "···"
  return (
    <div className="flex items-center gap-2 border-y border-border/50 bg-muted/30 px-3 py-0.5 font-mono text-[11px] text-muted-foreground/60">
      <span className="select-none">{label}</span>
    </div>
  )
}

function HunkLines({ rows }: { rows: ParsedDiffRow[] }) {
  return (
    <div className="font-mono text-[12px] leading-[20px]">
      {rows.map((row, i) => {
        const marker = rowMarker(row)
        return (
          <div key={i} className={cn("flex", ROW_CLASS[marker])}>
            <span className="w-[3.5rem] shrink-0 select-none pr-1 text-right text-muted-foreground/40">
              {row.oldLine ?? ""}
            </span>
            <span className="w-[3.5rem] shrink-0 select-none pr-1 text-right text-muted-foreground/40">
              {row.newLine ?? ""}
            </span>
            <span
              className={cn(
                "w-4 shrink-0 select-none text-center",
                SIGN_CLASS[row.sign] ?? ""
              )}
            >
              {row.sign === " " ? "" : row.sign}
            </span>
            <span className="flex-1 whitespace-pre pr-3">{row.text}</span>
          </div>
        )
      })}
    </div>
  )
}

/** Clean file content view for new files (no diff signs, green highlight as added) */
function NewFileLines({ rows }: { rows: ParsedDiffRow[] }) {
  return (
    <div className="font-mono text-[12px] leading-[20px]">
      {rows.map((row, i) => (
        <div key={i} className={cn("flex", ROW_CLASS.added)}>
          <span className="w-[3.5rem] shrink-0 select-none pr-1 text-right text-muted-foreground/40">
            {row.newLine ?? i + 1}
          </span>
          <span className="flex-1 whitespace-pre pr-3">{row.text}</span>
        </div>
      ))}
    </div>
  )
}

function SplitCellView({
  cell,
  gutterClassName,
}: {
  cell: SplitCell
  gutterClassName: string
}) {
  const empty = cell.text === null
  // Sizing belongs to the grid track, not the cell: a cell that sized itself
  // could end up narrower than its own `whitespace-pre` line and paint it over
  // the neighbouring column.
  return (
    <div className={cn("flex", empty ? "bg-muted/20" : ROW_CLASS[cell.marker])}>
      <span
        className={cn(
          gutterClassName,
          "shrink-0 select-none pr-1 text-right",
          empty ? "text-transparent" : "text-muted-foreground/40"
        )}
      >
        {cell.line ?? ""}
      </span>
      <span className="flex-1 whitespace-pre pr-3">{cell.text}</span>
    </div>
  )
}

/**
 * One grid for the whole hunk, so the two columns are laid out by shared
 * tracks rather than per-row: every row is guaranteed to break at the same
 * x, and no cell can be sized below the line it holds.
 *
 * `w-max` is load-bearing. Without it the grid takes the shrink-to-fit width
 * of its inline-block parent, which resolves to the tracks' MINIMUM sizes;
 * `1fr 1fr` then halves that between the columns, so a row whose two sides
 * differ in length (the normal case — a replaced line rarely keeps its old
 * width) leaves the longer side overflowing its track and painting under the
 * opposite column's background and text. Sizing to `max-content` makes the
 * grid as wide as twice its widest cell, which the enclosing `x="scroll"`
 * ScrollArea then scrolls; `min-w-full` keeps short diffs filling the host
 * instead of huddling on the left.
 */
function HunkSplitLines({ rows }: { rows: ParsedDiffRow[] }) {
  const splitRows = useMemo(() => toSplitRows(rows), [rows])
  return (
    <div className="grid w-max min-w-full grid-cols-[repeat(2,minmax(260px,1fr))] font-mono text-[12px] leading-[20px]">
      {splitRows.map((row, i) => (
        <Fragment key={i}>
          <SplitCellView cell={row.left} gutterClassName="w-[3rem]" />
          <SplitCellView
            cell={row.right}
            gutterClassName="w-[3rem] border-l border-border/40"
          />
        </Fragment>
      ))}
    </div>
  )
}

function isNewFileOnly(file: ParsedDiffFile): boolean {
  return file.mode === "added" && file.deletions === 0
}

/** New files render as plain content in BOTH modes (there is no "before" side
 *  to put anything on), so a diff made only of them has nothing to switch —
 *  offering the toggle there would be a control that visibly does nothing.
 *  Write-tool previews in a transcript are exactly this shape. */
function supportsSplitView(files: ParsedDiffFile[]): boolean {
  return files.some((file) => !isNewFileOnly(file))
}

// Beyond this many rows a single file is rendered as a bounded preview: each
// diff row is its own DOM node (line-number gutters + sign + text), so a
// lockfile or bulk-refactor diff of 5k–20k lines would otherwise build tens of
// thousands of nodes synchronously and freeze the UI for seconds — all just to
// be clipped inside the 420px-tall scroll box. The remainder is one click away.
const MAX_PREVIEW_ROWS = 500

/**
 * Take at most `limit` rows across the file's hunks in order, slicing the hunk
 * that crosses the budget so the preview ends on a clean row. Callers only use
 * this when the file's total row count exceeds `limit`.
 */
function capHunks(hunks: ParsedDiffHunk[], limit: number): ParsedDiffHunk[] {
  const out: ParsedDiffHunk[] = []
  let budget = limit
  for (const hunk of hunks) {
    if (budget <= 0) break
    if (hunk.rows.length <= budget) {
      out.push(hunk)
      budget -= hunk.rows.length
    } else {
      out.push({ ...hunk, rows: hunk.rows.slice(0, budget) })
      budget = 0
    }
  }
  return out
}

function DiffFileSection({
  file,
  view,
  embedded,
  clickableFilePath,
  folderPath,
  unbounded,
}: {
  file: ParsedDiffFile
  view: DiffViewMode
  embedded: boolean
  clickableFilePath: boolean
  folderPath: string | null
  unbounded: boolean
}) {
  const t = useTranslations("Folder.diffPreview")
  const [expanded, setExpanded] = useState(false)

  // Reset the reveal when a new diff reparses to a fresh `file` object at this
  // position: sections are keyed positionally (`file-1`, …), so React reuses
  // this instance across a `diffText` change. `file` is referentially stable
  // while `diffText` is unchanged (it comes from a `useMemo`), so this only
  // fires on a real content change. Resetting during render — not in an effect —
  // guarantees the incoming file is never committed in the previous file's
  // expanded (un-capped) state, which would resurface the exact multi-second
  // freeze the cap exists to prevent (React discards this render without
  // reconciling its children, then re-renders capped).
  const [renderedFile, setRenderedFile] = useState(file)
  if (file !== renderedFile) {
    setRenderedFile(file)
    setExpanded(false)
  }

  const newFile = isNewFileOnly(file)

  const totalRows = useMemo(
    () => file.hunks.reduce((sum, hunk) => sum + hunk.rows.length, 0),
    [file.hunks]
  )
  // A large file is capped until the user opts to render the rest. Small files
  // (the common case) keep rendering exactly as before.
  const capped = !expanded && totalRows > MAX_PREVIEW_ROWS
  const hunks = useMemo(
    () => (capped ? capHunks(file.hunks, MAX_PREVIEW_ROWS) : file.hunks),
    [capped, file.hunks]
  )

  return (
    <section
      className={cn(
        "flex flex-col",
        // Self-capped by default: the section owns a scroll box so a huge file
        // can't stretch its host. `unbounded` hands both back to the host (it
        // supplies its own cap + reveal), which keeps the two from nesting.
        unbounded ? "min-h-0" : "max-h-[420px]",
        embedded
          ? "bg-transparent"
          : "rounded-lg border border-border bg-background"
      )}
    >
      {!embedded && (
        <header className="flex shrink-0 items-center gap-2 border-b border-border bg-muted/40 px-3 py-2 text-[11px]">
          <span className="shrink-0 rounded border border-border bg-background px-1.5 py-0.5 text-[10px] text-muted-foreground">
            {newFile ? "WRITE" : t(modeKey(file.mode))}
          </span>
          {clickableFilePath ? (
            <FilePathLink
              filePath={file.path}
              className="min-w-0 flex-1 font-mono text-foreground"
              title={file.path}
            >
              {toDisplayPath(file.path, folderPath)}
            </FilePathLink>
          ) : (
            <span
              className="min-w-0 flex-1 truncate font-mono text-foreground"
              title={file.path}
            >
              {toDisplayPath(file.path, folderPath)}
            </span>
          )}
          {!newFile && (
            <span className="ml-auto inline-flex shrink-0 items-center gap-2 font-mono">
              <span className="text-green-700 dark:text-green-400">
                +{file.additions}
              </span>
              <span className="text-red-700 dark:text-red-400">
                -{file.deletions}
              </span>
            </span>
          )}
        </header>
      )}

      <ScrollArea x="scroll" y={unbounded ? "hidden" : "scroll"}>
        <div className="inline-block min-w-full">
          {newFile
            ? hunks.map((hunk) => (
                <NewFileLines key={hunk.key} rows={hunk.rows} />
              ))
            : hunks.map((hunk, hunkIdx) => (
                <div key={hunk.key}>
                  {hunkIdx > 0 && <HunkSeparator hunk={hunk} />}
                  {view === "split" ? (
                    <HunkSplitLines rows={hunk.rows} />
                  ) : (
                    <HunkLines rows={hunk.rows} />
                  )}
                </div>
              ))}
        </div>
      </ScrollArea>

      {capped && (
        <button
          type="button"
          onClick={() => setExpanded(true)}
          className={cn(
            "shrink-0 select-none px-3 py-1 text-left font-mono text-[11px] text-muted-foreground hover:bg-muted/40 hover:text-foreground",
            embedded ? "border-t border-border/50" : "border-t border-border"
          )}
        >
          {t("showRemainingLines", { count: totalRows - MAX_PREVIEW_ROWS })}
        </button>
      )}
    </section>
  )
}

export function UnifiedDiffPreview({
  diffText,
  className,
  clickableFilePath = false,
  embedded = false,
  unbounded = false,
}: {
  diffText: string
  /** @deprecated No longer used — kept for API compat */
  modelId?: string
  className?: string
  /** When true, file-name header is clickable and opens the workspace open-file dialog. */
  clickableFilePath?: boolean
  /**
   * When true, render each file's diff WITHOUT its own bordered card + header
   * chrome — just the line grid. For hosts that already frame the diff and
   * label the file (e.g. the reply-artifacts accordion), this avoids a
   * double border and a redundant path/mode header.
   */
  embedded?: boolean
  /**
   * Let each file's diff render at its natural height instead of inside its
   * own 420px scroll box. For hosts that cap and reveal the whole preview
   * themselves (the task diff dialog), which would otherwise nest a vertical
   * scroll inside another one.
   */
  unbounded?: boolean
}) {
  const t = useTranslations("Folder.diffPreview")
  const { activeFolder: folder } = useActiveFolder()
  const files = useMemo(() => parseUnifiedDiff(diffText), [diffText])
  const [view, switchView] = useDiffViewMode()

  if (!diffText.trim()) {
    return (
      <div
        className={cn(
          "h-full flex items-center justify-center text-xs text-muted-foreground",
          className
        )}
      >
        {t("noDiffData")}
      </div>
    )
  }

  if (files.length === 0) {
    const pre = (
      <pre className="font-mono text-[11px] leading-5 whitespace-pre-wrap text-muted-foreground p-3">
        {diffText}
      </pre>
    )
    return unbounded ? (
      <div className={className}>{pre}</div>
    ) : (
      <ScrollArea className={cn("h-full", className)} x="scroll">
        {pre}
      </ScrollArea>
    )
  }

  // Unbounded: the host sizes and scrolls the preview, so the outer viewport
  // is a plain box (each file still scrolls horizontally on its own).
  const Frame = unbounded ? UnboundedFrame : ScrollAreaFrame

  return (
    <Frame className={className}>
      <div className={embedded ? "space-y-2" : "space-y-3"}>
        {supportsSplitView(files) && (
          <div className="flex items-center justify-end gap-1">
            <ViewModeButton
              active={view === "unified"}
              label={t("viewMode.unified")}
              onClick={() => switchView("unified")}
            >
              <Rows3 className="h-3 w-3" />
            </ViewModeButton>
            <ViewModeButton
              active={view === "split"}
              label={t("viewMode.split")}
              onClick={() => switchView("split")}
            >
              <Columns2 className="h-3 w-3" />
            </ViewModeButton>
          </div>
        )}
        {files.map((file) => (
          <DiffFileSection
            key={file.key}
            file={file}
            view={view}
            embedded={embedded}
            clickableFilePath={clickableFilePath}
            folderPath={folder?.path ?? null}
            unbounded={unbounded}
          />
        ))}
      </div>
    </Frame>
  )
}

function ViewModeButton({
  active,
  label,
  onClick,
  children,
}: {
  active: boolean
  label: string
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      aria-pressed={active}
      title={label}
      className={cn(
        "inline-flex items-center gap-1 rounded border border-border bg-background px-2 py-0.5 text-[10px] transition-colors hover:bg-muted",
        active ? "text-foreground" : "text-muted-foreground"
      )}
    >
      {children}
    </button>
  )
}

function ScrollAreaFrame({
  className,
  children,
}: {
  className?: string
  children: React.ReactNode
}) {
  return (
    <ScrollArea className={cn("h-full", className)} x="scroll">
      {children}
    </ScrollArea>
  )
}

function UnboundedFrame({
  className,
  children,
}: {
  className?: string
  children: React.ReactNode
}) {
  return <div className={className}>{children}</div>
}
