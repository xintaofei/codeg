"use client"

import { useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import {
  AlertTriangle,
  Ban,
  Check,
  Code,
  FileCode,
  MessageSquare,
  Send,
  Trash2,
  X,
} from "lucide-react"

import { PipelineDiffFileTree } from "@/components/chat/pipeline-diff-file-tree"
import { UnifiedDiffPreview } from "@/components/diff/unified-diff-preview"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Textarea } from "@/components/ui/textarea"
import {
  addOrUpdateNote,
  formatPipelineNotes,
  getNoteForLine,
  removeNote,
  type PipelineLineNote,
} from "@/lib/pipeline-notes"
import type { PipelineDiff } from "@/lib/types"
import { cn } from "@/lib/utils"

export interface PipelineDiffPanelProps {
  diff: PipelineDiff
  runId?: number
  selectedFilePath?: string | null
  onSelectFilePath?: (path: string) => void
  onRequestChanges?: (notes: string) => void | Promise<void>
  onStopManual?: () => void | Promise<void>
  onApply?: (strategy: "squash" | "no_ff") => void | Promise<void>
  onClose?: () => void
  initialNotes?: PipelineLineNote[]
  onNotesChange?: (notes: PipelineLineNote[]) => void
  isSubmitting?: boolean
  className?: string
}

export interface InteractiveDiffRow {
  key: string
  type: "context" | "added" | "deleted"
  oldLine: number | null
  newLine: number | null
  lineNumber: number
  text: string
}

export interface InteractiveHunk {
  key: string
  header: string
  rows: InteractiveDiffRow[]
}

export function extractFilePatch(patch: string, filePath: string): string {
  if (!patch || !filePath) return patch || ""

  const target = filePath.replace(/\\/g, "/")
  const lines = patch.replace(/\r\n/g, "\n").split("\n")
  const resultLines: string[] = []
  let capturing = false

  for (const line of lines) {
    if (
      line.startsWith("diff --git ") ||
      line.startsWith("*** Update File: ") ||
      line.startsWith("*** Add File: ") ||
      line.startsWith("*** Delete File: ")
    ) {
      const isTarget =
        line.includes(`a/${target}`) ||
        line.includes(`b/${target}`) ||
        line.includes(` ${target}`) ||
        line.includes(`: ${target}`)

      if (isTarget) {
        capturing = true
        resultLines.push(line)
        continue
      } else if (capturing) {
        break
      }
    }

    if (capturing) {
      resultLines.push(line)
    }
  }

  if (resultLines.length === 0) {
    return patch
  }

  return resultLines.join("\n")
}

export function parseFileHunks(patchText: string): InteractiveHunk[] {
  if (!patchText || !patchText.trim()) return []

  const lines = patchText.replace(/\r\n/g, "\n").split("\n")
  const hunks: InteractiveHunk[] = []
  let currentHunk: InteractiveHunk | null = null
  let oldCursor = 1
  let newCursor = 1
  let rowIdx = 0

  for (const line of lines) {
    if (line.startsWith("@@")) {
      if (currentHunk && currentHunk.rows.length > 0) {
        hunks.push(currentHunk)
      }
      const match = line.match(
        /^@@\s*-(\d+)(?:,(\d+))?\s+\+(\d+)(?:,(\d+))?\s*@@/
      )
      if (match && match[1] && match[3]) {
        oldCursor = parseInt(match[1], 10)
        newCursor = parseInt(match[3], 10)
      }
      currentHunk = {
        key: `hunk-${hunks.length}-${line}`,
        header: line,
        rows: [],
      }
      continue
    }

    if (!currentHunk) {
      continue
    }

    if (line.startsWith("+")) {
      rowIdx += 1
      currentHunk.rows.push({
        key: `row-${rowIdx}`,
        type: "added",
        oldLine: null,
        newLine: newCursor,
        lineNumber: newCursor,
        text: line.slice(1),
      })
      newCursor += 1
    } else if (line.startsWith("-")) {
      rowIdx += 1
      currentHunk.rows.push({
        key: `row-${rowIdx}`,
        type: "deleted",
        oldLine: oldCursor,
        newLine: null,
        lineNumber: oldCursor,
        text: line.slice(1),
      })
      oldCursor += 1
    } else if (line.startsWith(" ") || line === "") {
      rowIdx += 1
      const content = line.startsWith(" ") ? line.slice(1) : line
      currentHunk.rows.push({
        key: `row-${rowIdx}`,
        type: "context",
        oldLine: oldCursor,
        newLine: newCursor,
        lineNumber: newCursor,
        text: content,
      })
      oldCursor += 1
      newCursor += 1
    }
  }

  if (currentHunk && currentHunk.rows.length > 0) {
    hunks.push(currentHunk)
  }

  return hunks
}

export function PipelineDiffPanel({
  diff,
  selectedFilePath,
  onSelectFilePath,
  onRequestChanges,
  onStopManual,
  onApply,
  onClose,
  initialNotes = [],
  onNotesChange,
  isSubmitting = false,
  className,
}: PipelineDiffPanelProps) {
  const t = useTranslations("Pipeline")

  const [internalSelectedFile, setInternalSelectedFile] = useState<
    string | null
  >(() => {
    return selectedFilePath ?? diff.files[0]?.path ?? null
  })

  const activeFilePath = selectedFilePath ?? internalSelectedFile

  const handleSelectFile = (path: string) => {
    setInternalSelectedFile(path)
    onSelectFilePath?.(path)
  }

  const [notes, setNotes] = useState<PipelineLineNote[]>(initialNotes)
  const [activeCommentRowKey, setActiveCommentRowKey] = useState<string | null>(
    null
  )
  const [noteDraftText, setNoteDraftText] = useState("")

  const updateNotes = (nextNotes: PipelineLineNote[]) => {
    setNotes(nextNotes)
    onNotesChange?.(nextNotes)
  }

  const handleSaveNote = (path: string, line: number, text: string) => {
    const next = addOrUpdateNote(notes, { path, line, note: text })
    updateNotes(next)
    setActiveCommentRowKey(null)
    setNoteDraftText("")
  }

  const handleDeleteNote = (path: string, line: number) => {
    const next = removeNote(notes, { path, line })
    updateNotes(next)
    setActiveCommentRowKey(null)
    setNoteDraftText("")
  }

  const handleOpenCommentInput = (rowKey: string, existingText?: string) => {
    if (activeCommentRowKey === rowKey) {
      setActiveCommentRowKey(null)
      setNoteDraftText("")
    } else {
      setActiveCommentRowKey(rowKey)
      setNoteDraftText(existingText ?? "")
    }
  }

  const handleRequestChanges = () => {
    if (!onRequestChanges) return
    const formatted = formatPipelineNotes(notes)
    onRequestChanges(formatted)
  }

  const activeFilePatch = useMemo(() => {
    if (!activeFilePath) return diff.patch
    return extractFilePatch(diff.patch, activeFilePath)
  }, [diff.patch, activeFilePath])

  const hunks = useMemo(() => {
    return parseFileHunks(activeFilePatch)
  }, [activeFilePatch])

  const activeFileObj = useMemo(() => {
    return diff.files.find((f) => f.path === activeFilePath)
  }, [diff.files, activeFilePath])

  return (
    <div
      className={cn(
        "flex flex-col h-full bg-background border border-border rounded-lg shadow-sm overflow-hidden",
        className
      )}
      data-testid="pipeline-diff-panel"
    >
      {/* Header bar */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-border bg-muted/40 shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <Code className="h-4 w-4 text-primary shrink-0" />
          <span className="text-xs font-semibold truncate">{t("title")}</span>
          {activeFilePath && (
            <span className="text-xs text-muted-foreground font-mono truncate max-w-[240px]">
              / {activeFilePath}
            </span>
          )}
        </div>

        <div className="flex items-center gap-1.5 shrink-0">
          {onClose && (
            <Button
              variant="ghost"
              size="icon"
              className="h-7 w-7 text-muted-foreground hover:text-foreground"
              onClick={onClose}
              aria-label="Close"
            >
              <X className="h-4 w-4" />
            </Button>
          )}
        </div>
      </div>

      {/* Truncated notice banner */}
      {diff.truncated && (
        <div
          data-testid="diff-truncated-banner"
          className="flex items-center gap-2 px-3 py-2 bg-amber-500/10 border-b border-amber-500/30 text-amber-900 dark:text-amber-200 text-xs shrink-0"
        >
          <AlertTriangle className="h-4 w-4 shrink-0 text-amber-600 dark:text-amber-400" />
          <span>{t("diffTruncated")}</span>
        </div>
      )}

      {/* Main body: File tree + Diff area */}
      <div className="flex-1 flex min-h-0 divide-x divide-border">
        {/* Left: Changed files tree */}
        <div className="w-64 max-w-[40%] min-w-[180px] shrink-0 h-full">
          <PipelineDiffFileTree
            files={diff.files}
            selectedPath={activeFilePath}
            onSelectFile={handleSelectFile}
            notes={notes}
          />
        </div>

        {/* Right: Selected file diff view */}
        <div className="flex-1 flex flex-col h-full min-w-0 overflow-hidden bg-background">
          {diff.files.length === 0 && !diff.patch.trim() ? (
            <div className="flex-1 flex flex-col items-center justify-center p-8 text-center text-muted-foreground">
              <FileCode className="h-10 w-10 mb-2 opacity-30" />
              <p className="text-xs" data-testid="diff-empty-message">
                {t("diffEmpty")}
              </p>
            </div>
          ) : hunks.length === 0 ? (
            <div className="flex-1 p-2 overflow-auto">
              <UnifiedDiffPreview
                diffText={activeFilePatch || diff.patch}
                embedded
                unbounded
              />
            </div>
          ) : (
            <ScrollArea className="flex-1" x="scroll">
              <div
                dir="ltr"
                className="font-mono text-xs leading-[1.25rem] min-w-full pb-6"
              >
                {/* File Header */}
                {activeFileObj && (
                  <div className="sticky top-0 z-10 flex items-center justify-between px-3 py-1.5 bg-muted/90 backdrop-blur-xs border-b border-border text-2xs">
                    <span className="font-semibold text-foreground truncate">
                      {activeFileObj.path}
                    </span>
                    <span className="inline-flex items-center gap-1.5 shrink-0">
                      {activeFileObj.additions > 0 && (
                        <span className="text-green-600 dark:text-green-400 font-bold">
                          +{activeFileObj.additions}
                        </span>
                      )}
                      {activeFileObj.additions === 0 &&
                        activeFileObj.deletions === 0 && (
                          <span className="text-muted-foreground">0</span>
                        )}
                      {activeFileObj.deletions > 0 && (
                        <span className="text-red-600 dark:text-red-400 font-bold">
                          -{activeFileObj.deletions}
                        </span>
                      )}
                    </span>
                  </div>
                )}

                {/* Hunks and lines */}
                {hunks.map((hunk) => (
                  <div key={hunk.key} className="flex flex-col">
                    {/* Hunk Header */}
                    <div className="sticky left-0 bg-muted/40 border-y border-border/40 px-3 py-0.5 text-2xs text-muted-foreground select-none">
                      {hunk.header}
                    </div>

                    {/* Hunk Rows */}
                    {hunk.rows.map((row) => {
                      const effectiveLine =
                        row.newLine ?? row.oldLine ?? row.lineNumber
                      const rowNote = activeFilePath
                        ? getNoteForLine(notes, activeFilePath, effectiveLine)
                        : undefined
                      // Only display note on the primary row for this line number (added or context prefered over deleted)
                      const isPrimaryRowForLine =
                        row.type !== "deleted" ||
                        hunk.rows.every(
                          (r) =>
                            r === row ||
                            (r.newLine ?? r.oldLine) !== effectiveLine
                        )
                      const shouldShowNote = rowNote && isPrimaryRowForLine
                      const isEditingComment = activeCommentRowKey === row.key

                      const isAdded = row.type === "added"
                      const isDeleted = row.type === "deleted"

                      const rowBg = isAdded
                        ? "bg-green-500/10 text-green-950 dark:text-green-200 hover:bg-green-500/15"
                        : isDeleted
                          ? "bg-red-500/10 text-red-950 dark:text-red-200 hover:bg-red-500/15"
                          : "hover:bg-muted/30 text-foreground"

                      const signColor = isAdded
                        ? "text-green-600 dark:text-green-400 font-bold"
                        : isDeleted
                          ? "text-red-600 dark:text-red-400 font-bold"
                          : "text-muted-foreground/40"

                      return (
                        <div
                          key={row.key}
                          className="flex flex-col border-b border-border/20 group"
                        >
                          <div
                            className={cn(
                              "flex items-stretch min-h-[1.35rem]",
                              rowBg
                            )}
                          >
                            {/* Gutter: line numbers + comment trigger */}
                            <div className="flex items-center shrink-0 select-none bg-muted/20 border-r border-border/50 text-2xs text-muted-foreground/60">
                              <span className="w-8 px-1 text-right">
                                {row.oldLine ?? ""}
                              </span>
                              <span className="w-8 px-1 text-right">
                                {row.newLine ?? ""}
                              </span>
                              <button
                                type="button"
                                onClick={() =>
                                  handleOpenCommentInput(row.key, rowNote?.note)
                                }
                                title={t("addComment")}
                                aria-label={`${t("addComment")} ${row.lineNumber}`}
                                data-testid={`line-comment-btn-${row.key}`}
                                className={cn(
                                  "w-5 h-full flex items-center justify-center transition-opacity",
                                  rowNote || isEditingComment
                                    ? "opacity-100 text-amber-600 dark:text-amber-400 font-bold"
                                    : "opacity-0 group-hover:opacity-100 hover:text-foreground"
                                )}
                              >
                                <MessageSquare className="h-3 w-3" />
                              </button>
                            </div>

                            {/* Sign */}
                            <span
                              className={cn(
                                "w-4 text-center select-none",
                                signColor
                              )}
                            >
                              {isAdded ? "+" : isDeleted ? "-" : " "}
                            </span>

                            {/* Code text */}
                            <span className="flex-1 whitespace-pre pr-3 font-mono">
                              {row.text}
                            </span>
                          </div>

                          {/* Existing note display when not actively editing */}
                          {shouldShowNote && rowNote && !isEditingComment && (
                            <div
                              data-testid={`line-note-display-${activeFilePath}-${effectiveLine}`}
                              onClick={() =>
                                handleOpenCommentInput(row.key, rowNote.note)
                              }
                              className="mx-8 my-1 p-2 bg-amber-500/10 border border-amber-500/30 text-foreground rounded-md text-xs font-sans flex items-start gap-2 cursor-pointer hover:bg-amber-500/20"
                            >
                              <MessageSquare className="h-3.5 w-3.5 text-amber-600 dark:text-amber-400 shrink-0 mt-0.5" />
                              <div className="flex-1 min-w-0">
                                <div className="text-2xs font-mono font-medium text-amber-800 dark:text-amber-300">
                                  {activeFilePath}:{row.lineNumber}
                                </div>
                                <div className="text-xs break-words">
                                  {rowNote.note}
                                </div>
                              </div>
                              <button
                                type="button"
                                onClick={(e) => {
                                  e.stopPropagation()
                                  if (activeFilePath) {
                                    handleDeleteNote(
                                      activeFilePath,
                                      row.lineNumber
                                    )
                                  }
                                }}
                                aria-label="Delete note"
                                className="text-muted-foreground hover:text-destructive p-1 rounded"
                              >
                                <Trash2 className="h-3.5 w-3.5" />
                              </button>
                            </div>
                          )}

                          {/* Active comment inline editing input */}
                          {isEditingComment && activeFilePath && (
                            <div
                              data-testid={`line-comment-box-${row.lineNumber}`}
                              className="mx-8 my-1.5 p-2 bg-background border border-amber-500/40 rounded-md shadow-xs flex flex-col gap-2 font-sans"
                            >
                              <div className="flex items-center justify-between text-2xs font-mono text-muted-foreground">
                                <span>
                                  {activeFilePath}:{row.lineNumber}
                                </span>
                                {rowNote && (
                                  <Button
                                    variant="ghost"
                                    size="xs"
                                    onClick={() =>
                                      handleDeleteNote(
                                        activeFilePath,
                                        row.lineNumber
                                      )
                                    }
                                    className="h-5 px-1.5 text-destructive hover:bg-destructive/10"
                                  >
                                    <Trash2 className="h-3 w-3 mr-1" />
                                    Delete
                                  </Button>
                                )}
                              </div>
                              <Textarea
                                value={noteDraftText}
                                onChange={(e) =>
                                  setNoteDraftText(e.target.value)
                                }
                                placeholder={t("commentPlaceholder")}
                                aria-label={t("commentPlaceholder")}
                                data-testid="line-comment-input"
                                className="text-xs min-h-[50px] resize-y"
                                autoFocus
                              />
                              <div className="flex items-center justify-end gap-1.5">
                                <Button
                                  variant="ghost"
                                  size="xs"
                                  onClick={() => {
                                    setActiveCommentRowKey(null)
                                    setNoteDraftText("")
                                  }}
                                  className="h-6 text-2xs"
                                >
                                  Cancel
                                </Button>
                                <Button
                                  size="xs"
                                  onClick={() =>
                                    handleSaveNote(
                                      activeFilePath,
                                      row.lineNumber,
                                      noteDraftText
                                    )
                                  }
                                  data-testid="save-comment-btn"
                                  className="h-6 text-2xs"
                                >
                                  {t("addComment")}
                                </Button>
                              </div>
                            </div>
                          )}
                        </div>
                      )
                    })}
                  </div>
                ))}
              </div>
            </ScrollArea>
          )}
        </div>
      </div>

      {/* Notes summary bar if any notes exist */}
      {notes.length > 0 && (
        <div
          data-testid="notes-summary-bar"
          className="px-3 py-1.5 border-t border-border bg-amber-500/5 flex flex-col gap-1 shrink-0"
        >
          <div className="flex items-center justify-between text-2xs text-muted-foreground font-medium">
            <span className="flex items-center gap-1.5 text-amber-700 dark:text-amber-400">
              <MessageSquare className="h-3 w-3" />
              {t("notesForCoder")} ({notes.length})
            </span>
          </div>
          <div className="flex flex-wrap gap-1 max-h-16 overflow-y-auto">
            {notes.map((n) => (
              <Badge
                key={`${n.path}:${n.line}`}
                variant="outline"
                className="text-2xs font-mono py-0.5 px-1.5 bg-background border-amber-500/30 flex items-center gap-1"
              >
                <span>
                  {n.path}:{n.line}
                </span>
                <button
                  type="button"
                  onClick={() => handleDeleteNote(n.path, n.line)}
                  className="hover:text-destructive"
                  aria-label={`Remove note for ${n.path}:${n.line}`}
                >
                  <X className="h-2.5 w-2.5" />
                </button>
              </Badge>
            ))}
          </div>
        </div>
      )}

      {/* Footer Actions */}
      <div className="flex items-center justify-between px-3 py-2 border-t border-border bg-muted/30 shrink-0 gap-2">
        <div className="flex items-center gap-2">
          {onStopManual && (
            <Button
              variant="outline"
              size="sm"
              onClick={onStopManual}
              disabled={isSubmitting}
              data-testid="stop-manual-button"
              className="gap-1.5 h-8 text-xs"
            >
              <Ban className="h-3.5 w-3.5" />
              {t("fixMyself")}
            </Button>
          )}
        </div>

        <div className="flex items-center gap-2">
          {onRequestChanges && (
            <Button
              variant="default"
              size="sm"
              onClick={handleRequestChanges}
              disabled={isSubmitting}
              data-testid="request-changes-button"
              className="gap-1.5 h-8 text-xs bg-amber-600 hover:bg-amber-700 text-white"
            >
              <Send className="h-3.5 w-3.5" />
              {t("sendToCoder")}
              {notes.length > 0 && (
                <Badge
                  variant="secondary"
                  className="ml-0.5 h-4 px-1 text-2xs bg-black/20 text-white font-mono"
                >
                  {notes.length}
                </Badge>
              )}
            </Button>
          )}

          {onApply && (
            <Button
              variant="default"
              size="sm"
              disabled={isSubmitting}
              onClick={() => onApply("squash")}
              data-testid="apply-button"
              className="gap-1.5 h-8 text-xs bg-emerald-600 hover:bg-emerald-700 text-white dark:bg-emerald-600 dark:hover:bg-emerald-700"
            >
              <Check className="h-3.5 w-3.5" />
              {t("apply")}
            </Button>
          )}
        </div>
      </div>
    </div>
  )
}
