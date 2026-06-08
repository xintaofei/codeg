"use client"

import { useCallback, useEffect, useState, type RefObject } from "react"
import { ChevronRight, FileIcon, ListTree, X } from "lucide-react"
import { useTranslations } from "next-intl"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useWorkspaceContext } from "@/contexts/workspace-context"
import type { FileChangeStat } from "@/lib/session-files"
import type { MessageScrollContextValue } from "@/components/message/message-scroll-context"
import {
  CommitFileAdditions,
  CommitFileDeletions,
} from "@/components/ai-elements/commit"
import { ScrollArea } from "@/components/ui/scroll-area"
import { cn } from "@/lib/utils"

/** One navigable user message. Present for every user turn, even when it made
 *  no file edits (`hasChanges === false`) so the rail is a complete index. */
export interface MessageNavEntry {
  /** Index into the rendered `threadItems` array — fed to `scrollToIndex`. */
  threadIndex: number
  turnId: string
  /** 1-based position among shown entries. */
  ordinal: number
  label: string
  additions: number
  deletions: number
  files: FileChangeStat[]
  hasChanges: boolean
}

interface ConversationMessageNavProps {
  entries: MessageNavEntry[]
  scrollApiRef: RefObject<MessageScrollContextValue | null>
  /** Thread index nearest the top of the viewport (for active highlight). */
  activeThreadIndex: number | null
  /** Called with the clicked entry's threadIndex so the parent can
   *  optimistically highlight it before the (possibly clamped) scroll settles. */
  onActivate?: (threadIndex: number) => void
}

const STORAGE_KEY = "workspace:message-nav"

function readPersistedExpanded(): boolean {
  if (typeof window === "undefined") return false
  try {
    return window.localStorage.getItem(STORAGE_KEY) === "1"
  } catch {
    return false
  }
}

function isRemovedFileDiff(diff: string | null): boolean {
  if (!diff) return false
  return (
    /^\*\*\* Delete File:\s+/m.test(diff) ||
    /^deleted file mode\b/m.test(diff) ||
    /^\+\+\+\s+\/dev\/null$/m.test(diff)
  )
}

function normalizeSlashPath(path: string): string {
  return path.replace(/\\/g, "/")
}

function toFolderRelativePath(filePath: string, folderPath?: string): string {
  const normalizedFilePath = normalizeSlashPath(filePath)
  if (!folderPath) return normalizedFilePath

  const normalizedFolderPath = normalizeSlashPath(folderPath).replace(
    /\/+$/,
    ""
  )
  if (!normalizedFolderPath) return normalizedFilePath

  const folderPrefix = `${normalizedFolderPath}/`
  if (normalizedFilePath.startsWith(folderPrefix)) {
    return normalizedFilePath.slice(folderPrefix.length)
  }

  return normalizedFilePath
}

function fileNameOf(displayPath: string): string {
  const lastSlash = displayPath.lastIndexOf("/")
  return lastSlash >= 0 ? displayPath.slice(lastSlash + 1) : displayPath
}

/**
 * Per-conversation message navigator docked at the pane's right edge.
 *
 * Collapsed (default): a slim rail with one marker per user message — click to
 * scroll. Expanded: a popout listing each message with `+N/-N` and an
 * expandable file-diff list (clicking a file opens it in the main editor via
 * `openSessionFileDiff`). The rail itself stays mounted so the layout never
 * reflows when toggling.
 */
export function ConversationMessageNav({
  entries,
  scrollApiRef,
  activeThreadIndex,
  onActivate,
}: ConversationMessageNavProps) {
  const t = useTranslations("Folder.chat.messageNav")
  const { openSessionFileDiff } = useWorkspaceContext()
  const { activeFolder: folder } = useActiveFolder()
  // Deterministic initial state so the prerendered (static export) HTML matches
  // the first client render; hydrate the persisted value after mount — mirrors
  // the AuxPanel pattern in aux-panel-context.tsx.
  const [expanded, setExpanded] = useState(false)
  const [openGroups, setOpenGroups] = useState<Record<string, boolean>>({})

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setExpanded(readPersistedExpanded())
  }, [])

  const setExpandedPersist = useCallback((next: boolean) => {
    setExpanded(next)
    try {
      window.localStorage.setItem(STORAGE_KEY, next ? "1" : "0")
    } catch {
      // ignore (private mode / disabled storage)
    }
  }, [])

  const jump = useCallback(
    (threadIndex: number) => {
      onActivate?.(threadIndex)
      scrollApiRef.current?.scrollToIndex(threadIndex, {
        align: "start",
        smooth: true,
      })
    },
    [onActivate, scrollApiRef]
  )

  const handleFileClick = useCallback(
    (
      filePath: string,
      diff: string | null,
      ordinal: number,
      changeIndex: number
    ) => {
      openSessionFileDiff(
        filePath,
        diff ?? t("noDiffDataAvailable", { filePath }),
        `msg-${ordinal}-chg-${changeIndex + 1}`
      )
    },
    [openSessionFileDiff, t]
  )

  if (entries.length === 0) return null

  // Float near the conversation's right edge. The rail spans the full height
  // (pointer-events only on the dots/toggle) so the expanded popout can be
  // height-capped to the conversation area and never slip under the tabs above.
  return (
    <div className="group pointer-events-none absolute inset-y-0 right-2 z-20 flex flex-col items-center justify-center">
      <button
        type="button"
        aria-label={expanded ? t("collapse") : t("expand")}
        aria-expanded={expanded}
        title={t("title")}
        onClick={() => setExpandedPersist(!expanded)}
        className="pointer-events-auto flex size-4 shrink-0 items-center justify-center rounded-md text-muted-foreground opacity-60 transition-colors hover:bg-accent/40 hover:text-foreground hover:opacity-100 focus-visible:opacity-100"
      >
        <ListTree className="h-2.5 w-2.5" />
      </button>

      <ScrollArea className="pointer-events-auto max-h-[80%] min-h-0">
        <div className="flex flex-col items-center py-1">
          {entries.map((entry) => {
            const active = activeThreadIndex === entry.threadIndex
            return (
              <button
                key={entry.turnId}
                type="button"
                aria-label={t("jumpToMessage", { label: entry.label })}
                aria-current={active ? "true" : undefined}
                title={`#${entry.ordinal} ${entry.label}`}
                onClick={() => jump(entry.threadIndex)}
                className="group/marker flex size-4 items-center justify-center"
              >
                <span
                  className={cn(
                    "rounded-full border transition-all",
                    active ? "size-2" : "size-1.5",
                    entry.hasChanges
                      ? active
                        ? "border-primary bg-primary"
                        : "border-primary/50 bg-transparent group-hover/marker:border-primary"
                      : active
                        ? "border-foreground/60 bg-foreground/60"
                        : "border-muted-foreground/40 bg-transparent group-hover/marker:border-muted-foreground/70"
                  )}
                />
              </button>
            )
          })}
        </div>
      </ScrollArea>

      {expanded && (
        <div className="pointer-events-auto absolute right-full top-1/2 z-30 mr-1 flex max-h-[calc(100%_-_1rem)] w-72 max-w-[80vw] -translate-y-1/2 flex-col rounded-lg border border-border bg-popover text-popover-foreground shadow-xl">
          <div className="flex h-9 shrink-0 items-center justify-between border-b border-border px-3">
            <span className="text-xs font-medium">{t("title")}</span>
            <button
              type="button"
              aria-label={t("collapse")}
              onClick={() => setExpandedPersist(false)}
              className="inline-flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>

          <ScrollArea className="flex-1 min-h-0">
            <div className="space-y-1.5 p-2">
              {entries.map((entry) => {
                const isOpen = openGroups[entry.turnId] ?? false
                const active = activeThreadIndex === entry.threadIndex
                const uniqueFileCount = new Set(
                  entry.files.map((file) => normalizeSlashPath(file.path))
                ).size

                return (
                  <div
                    key={entry.turnId}
                    className={cn(
                      "overflow-hidden rounded-lg border bg-card text-card-foreground",
                      active ? "border-primary/40" : "border-border"
                    )}
                  >
                    <div className="flex items-stretch">
                      <button
                        type="button"
                        onClick={() => jump(entry.threadIndex)}
                        className={cn(
                          "flex min-w-0 flex-1 items-start gap-2 px-2.5 py-2 text-left transition-colors hover:bg-accent/40",
                          active && "bg-accent/30"
                        )}
                      >
                        <span className="mt-0.5 shrink-0 rounded-md border border-border bg-muted/40 px-1 text-[10px] tabular-nums text-muted-foreground">
                          #{entry.ordinal}
                        </span>
                        <span className="min-w-0 flex-1">
                          <span className="line-clamp-2 text-xs leading-5 text-foreground">
                            {entry.label}
                          </span>
                          {entry.hasChanges && (
                            <span className="mt-1.5 flex flex-wrap items-center gap-1.5">
                              <span className="rounded-md border border-border bg-muted/40 px-1.5 py-0.5 text-[10px] text-muted-foreground">
                                {t("fileCount", { count: uniqueFileCount })}
                              </span>
                              {/* Always render BOTH counts (incl. zeros) so a
                                  one-sided change still shows its +N and -N. */}
                              <span className="inline-flex items-center gap-1.5 rounded-md border border-border bg-muted/40 px-1.5 py-0.5 font-mono text-[10px]">
                                <span className="text-green-600 dark:text-green-400">
                                  +{entry.additions}
                                </span>
                                <span className="text-red-600 dark:text-red-400">
                                  -{entry.deletions}
                                </span>
                              </span>
                            </span>
                          )}
                        </span>
                      </button>

                      {entry.hasChanges && (
                        <button
                          type="button"
                          aria-label={t("fileCount", {
                            count: uniqueFileCount,
                          })}
                          aria-expanded={isOpen}
                          onClick={() =>
                            setOpenGroups((prev) => ({
                              ...prev,
                              [entry.turnId]: !isOpen,
                            }))
                          }
                          className="flex w-7 shrink-0 items-center justify-center border-l border-border text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground"
                        >
                          <ChevronRight
                            className={cn(
                              "h-3.5 w-3.5 transition-transform",
                              isOpen && "rotate-90"
                            )}
                          />
                        </button>
                      )}
                    </div>

                    {entry.hasChanges && isOpen && (
                      <ul className="space-y-1 border-t border-border p-2">
                        {entry.files.map((file, fileIndex) => {
                          const displayPath = toFolderRelativePath(
                            file.path,
                            folder?.path
                          )
                          const isRemoved = isRemovedFileDiff(file.diff)

                          return (
                            <li key={file.id}>
                              <button
                                type="button"
                                onClick={() =>
                                  handleFileClick(
                                    file.path,
                                    file.diff,
                                    entry.ordinal,
                                    fileIndex
                                  )
                                }
                                title={displayPath}
                                className={cn(
                                  "flex w-full min-w-0 items-center gap-2 rounded-md border px-2 py-1.5 text-left transition-colors",
                                  isRemoved
                                    ? "border-destructive/30 bg-destructive/10 hover:bg-destructive/20"
                                    : "border-border bg-card hover:bg-accent/40"
                                )}
                              >
                                <FileIcon
                                  className={cn(
                                    "h-3.5 w-3.5 shrink-0",
                                    isRemoved
                                      ? "text-destructive"
                                      : "text-muted-foreground"
                                  )}
                                />
                                <span
                                  className={cn(
                                    "min-w-0 flex-1 truncate text-xs",
                                    isRemoved
                                      ? "text-destructive"
                                      : "text-foreground"
                                  )}
                                >
                                  {fileNameOf(displayPath)}
                                </span>
                                {isRemoved ? (
                                  <span className="inline-flex shrink-0 items-center rounded-md border border-destructive/30 bg-destructive/10 px-1.5 py-0.5 font-mono text-[10px] text-destructive">
                                    {t("remove")}
                                  </span>
                                ) : (
                                  <span className="inline-flex shrink-0 items-center gap-1 rounded-md border border-border bg-muted/40 px-1.5 py-0.5 font-mono text-[10px] text-foreground">
                                    <CommitFileAdditions
                                      count={file.additions}
                                      className="text-[10px]"
                                    />
                                    <CommitFileDeletions
                                      count={file.deletions}
                                      className="text-[10px]"
                                    />
                                  </span>
                                )}
                              </button>
                            </li>
                          )
                        })}
                      </ul>
                    )}
                  </div>
                )
              })}
            </div>
          </ScrollArea>
        </div>
      )}
    </div>
  )
}
