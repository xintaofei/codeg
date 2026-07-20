"use client"

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent,
  type ReactNode,
} from "react"
import {
  GripVertical,
  Loader2,
  Plus,
  Save,
  SquareTerminal,
  Trash2,
} from "lucide-react"
import { Reorder, useDragControls } from "motion/react"
import { useTranslations } from "next-intl"
import {
  createFolderCommand,
  deleteFolderCommand,
  listFolderCommands,
  reorderFolderCommands,
  updateFolderCommand,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import type { FolderCommand } from "@/lib/types"
import { Button } from "@/components/ui/button"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { useIsMobile } from "@/hooks/use-mobile"
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable"
import { cn } from "@/lib/utils"

const LEFT_MIN_WIDTH = 260
const RIGHT_MIN_WIDTH = 380

interface CommandManageDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  folderId: number
  onChanged: () => void
}

interface Draft {
  id: number | null
  name: string
  command: string
}

interface CommandReorderItemProps {
  command: FolderCommand
  selected: boolean
  disabled: boolean
  onSelect: (id: number) => void
  onDragEnd: () => void
  children: (
    startDrag: (event: PointerEvent<HTMLButtonElement>) => void
  ) => ReactNode
}

const EMPTY_DRAFT: Draft = {
  id: null,
  name: "",
  command: "",
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value))
}

function toPercent(pixels: number, totalPixels: number): number {
  if (totalPixels <= 0) return 0
  return (pixels / totalPixels) * 100
}

function CommandReorderItem({
  command,
  selected,
  disabled,
  onSelect,
  onDragEnd,
  children,
}: CommandReorderItemProps) {
  const dragControls = useDragControls()

  const startDrag = useCallback(
    (event: PointerEvent<HTMLButtonElement>) => {
      event.preventDefault()
      event.stopPropagation()
      if (!disabled) {
        dragControls.start(event)
      }
    },
    [disabled, dragControls]
  )

  return (
    <Reorder.Item
      as="section"
      value={command}
      data-folder-command-id={command.id}
      drag={disabled ? false : "y"}
      dragListener={false}
      dragControls={dragControls}
      dragMomentum={false}
      layout="position"
      className={cn(
        "min-h-12 cursor-pointer rounded-lg border bg-card p-2.5 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40 sm:min-h-0",
        selected && "border-primary/60 bg-primary/5"
      )}
      tabIndex={0}
      onDragEnd={onDragEnd}
      onClick={() => onSelect(command.id)}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget) return
        if (event.key !== "Enter" && event.key !== " ") return
        event.preventDefault()
        onSelect(command.id)
      }}
    >
      {children(startDrag)}
    </Reorder.Item>
  )
}

export function CommandManageDialog({
  open,
  onOpenChange,
  folderId,
  onChanged,
}: CommandManageDialogProps) {
  const t = useTranslations("Folder.commandDropdown.manageDialog")
  const tCommon = useTranslations("Folder.common")
  const [commands, setCommands] = useState<FolderCommand[]>([])
  const [selectedId, setSelectedId] = useState<number | null>(null)
  const [draft, setDraft] = useState<Draft>(EMPTY_DRAFT)
  const [searchQuery, setSearchQuery] = useState("")
  const [loading, setLoading] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [formError, setFormError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [deleteTargetId, setDeleteTargetId] = useState<number | null>(null)
  const [reordering, setReordering] = useState(false)
  const isMobile = useIsMobile()
  const pendingOrderRef = useRef<number[] | null>(null)
  const panelContainerRef = useRef<HTMLDivElement | null>(null)
  const [panelContainerWidth, setPanelContainerWidth] = useState(0)
  // Monotonic load token: a folder switch (or reopen) bumps it so a slower
  // listFolderCommands response for the previous folder can't overwrite the
  // newer folder's state.
  const loadSeqRef = useRef(0)

  const refresh = useCallback(async () => {
    if (!folderId) {
      loadSeqRef.current += 1
      setCommands([])
      setSelectedId(null)
      return
    }
    const seq = (loadSeqRef.current += 1)
    setLoading(true)
    setLoadError(null)
    try {
      const list = await listFolderCommands(folderId)
      if (seq !== loadSeqRef.current) return
      setCommands(list)
      setSelectedId((prev) => {
        if (prev === null) {
          return list[0]?.id ?? null
        }
        if (list.some((item) => item.id === prev)) {
          return prev
        }
        return list[0]?.id ?? null
      })
    } catch (err) {
      if (seq !== loadSeqRef.current) return
      setLoadError(`${t("loadFailed")}: ${toErrorMessage(err)}`)
      setCommands([])
    } finally {
      if (seq === loadSeqRef.current) setLoading(false)
    }
  }, [folderId, t])

  // Reloads on open and whenever folderId changes while open (refresh is keyed
  // on folderId), resetting transient UI so a folder switch never shows stale
  // commands.
  useEffect(() => {
    if (open) {
      setFormError(null)
      setSearchQuery("")
      void refresh()
    }
  }, [open, refresh])

  useEffect(() => {
    const container = panelContainerRef.current
    if (!container) return
    const updateWidth = (next: number) => {
      setPanelContainerWidth((prev) =>
        Math.abs(prev - next) < 1 ? prev : next
      )
    }
    updateWidth(container.getBoundingClientRect().width)
    const observer = new ResizeObserver((entries) => {
      updateWidth(
        entries[0]?.contentRect.width ?? container.getBoundingClientRect().width
      )
    })
    observer.observe(container)
    return () => {
      observer.disconnect()
    }
  }, [open])

  const selected = useMemo(
    () => commands.find((item) => item.id === selectedId) ?? null,
    [commands, selectedId]
  )
  const deleteTarget = useMemo(
    () =>
      deleteTargetId === null
        ? null
        : (commands.find((item) => item.id === deleteTargetId) ?? null),
    [commands, deleteTargetId]
  )

  useEffect(() => {
    setFormError(null)
    if (!selected) {
      setDraft(EMPTY_DRAFT)
      return
    }
    setDraft({
      id: selected.id,
      name: selected.name,
      command: selected.command,
    })
  }, [selected])

  const filteredCommands = useMemo(() => {
    const query = searchQuery.trim().toLowerCase()
    if (!query) return commands
    return commands.filter(
      (command) =>
        command.name.toLowerCase().includes(query) ||
        command.command.toLowerCase().includes(query)
    )
  }, [commands, searchQuery])

  const searchActive = searchQuery.trim().length > 0
  const safeContainerWidth = panelContainerWidth > 0 ? panelContainerWidth : 900
  const leftMinSize = clamp(
    toPercent(LEFT_MIN_WIDTH, safeContainerWidth),
    5,
    95
  )
  const rightMinSize = clamp(
    toPercent(RIGHT_MIN_WIDTH, safeContainerWidth),
    5,
    95
  )
  const leftMaxSize = Math.max(leftMinSize, 100 - rightMinSize)

  const updateDraft = useCallback((patch: Partial<Draft>) => {
    setFormError(null)
    setDraft((prev) => ({ ...prev, ...patch }))
  }, [])

  const startNew = useCallback(() => {
    setSelectedId(null)
    setFormError(null)
    setDraft(EMPTY_DRAFT)
  }, [])

  const persistReorder = useCallback(
    async (ids: number[]) => {
      if (ids.length === 0) return
      setReordering(true)
      setFormError(null)
      try {
        await reorderFolderCommands(folderId, ids)
        onChanged()
      } catch (err) {
        setFormError(`${t("orderFailed")}: ${toErrorMessage(err)}`)
        await refresh()
      } finally {
        setReordering(false)
      }
    },
    [folderId, onChanged, refresh, t]
  )

  const handleReorder = useCallback(
    (next: FolderCommand[]) => {
      if (searchActive) return
      const reordered = next.map((command, index) => ({
        ...command,
        sort_order: index,
      }))
      setCommands(reordered)
      pendingOrderRef.current = reordered.map((command) => command.id)
    },
    [searchActive]
  )

  const handleSave = useCallback(async () => {
    setSaving(true)
    setFormError(null)
    try {
      const name = draft.name.trim()
      const command = draft.command.trim()
      const saved =
        draft.id === null
          ? await createFolderCommand(folderId, name, command)
          : await updateFolderCommand(draft.id, name, command)
      setCommands((prev) => {
        const exists = prev.some((item) => item.id === saved.id)
        if (exists) {
          return prev.map((item) => (item.id === saved.id ? saved : item))
        }
        return [...prev, saved]
      })
      setSelectedId(saved.id)
      setDraft({
        id: saved.id,
        name: saved.name,
        command: saved.command,
      })
      onChanged()
    } catch (err) {
      setFormError(`${t("saveFailed")}: ${toErrorMessage(err)}`)
    } finally {
      setSaving(false)
    }
  }, [draft, folderId, onChanged, t])

  const handleDelete = useCallback(async () => {
    if (deleteTargetId === null) return
    const target = deleteTargetId
    setDeleting(true)
    setFormError(null)
    try {
      await deleteFolderCommand(target)
      setCommands((prev) => {
        const next = prev.filter((item) => item.id !== target)
        setSelectedId((current) =>
          current === target ? (next[0]?.id ?? null) : current
        )
        return next
      })
      onChanged()
      setDeleteTargetId(null)
    } catch (err) {
      setFormError(`${t("deleteFailed")}: ${toErrorMessage(err)}`)
      setDeleteTargetId(null)
    } finally {
      setDeleting(false)
    }
  }, [deleteTargetId, onChanged, t])

  const saveDisabled =
    saving || deleting || loading || !draft.name.trim() || !draft.command.trim()

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent
          closeButtonClassName="top-2 right-3 size-10 rounded-xl sm:top-4 sm:right-4 sm:size-8"
          className="flex h-[calc(100dvh-5rem)] max-h-[calc(100dvh-5rem)] max-w-[calc(100vw-2rem)] flex-col gap-0 overflow-hidden rounded-[24px] p-0 sm:h-[min(760px,calc(100vh-4rem))] sm:max-w-5xl sm:rounded-4xl"
        >
          <DialogHeader className="flex min-h-14 shrink-0 justify-center border-b px-5 py-2.5 sm:min-h-0 sm:px-4 sm:py-3">
            <DialogTitle className="text-lg sm:text-lg">
              {t("title")}
            </DialogTitle>
          </DialogHeader>

          <div
            ref={panelContainerRef}
            className="min-h-0 min-w-0 flex-1 p-2 sm:p-3"
          >
            <ResizablePanelGroup
              direction={isMobile ? "vertical" : "horizontal"}
              className="h-full min-h-0 min-w-0"
            >
              <ResizablePanel
                defaultSize={isMobile ? 38 : 36}
                minSize={isMobile ? 28 : leftMinSize}
                maxSize={isMobile ? 50 : leftMaxSize}
              >
                <div className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden rounded-xl border bg-card lg:rounded-r-none">
                  <div className="space-y-2 border-b p-2.5 sm:space-y-2.5 sm:p-3">
                    <div className="grid gap-2 sm:flex sm:items-center">
                      <Input
                        className="h-11 w-full min-w-0 sm:h-9 sm:flex-1"
                        value={searchQuery}
                        onChange={(event) => setSearchQuery(event.target.value)}
                        placeholder={t("searchPlaceholder")}
                      />
                      <Button
                        size="sm"
                        className="h-11 w-full shrink-0 justify-center px-3 sm:h-8 sm:w-auto"
                        onClick={startNew}
                      >
                        <Plus className="h-4 w-4 sm:h-3.5 sm:w-3.5" />
                        {t("newCommand")}
                      </Button>
                    </div>
                  </div>

                  {loadError ? (
                    <div className="m-3 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
                      {loadError}
                    </div>
                  ) : loading ? (
                    <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      {tCommon("loading")}
                    </div>
                  ) : filteredCommands.length === 0 ? (
                    <div className="flex flex-1 flex-col items-center justify-center gap-2 px-4 text-center text-xs text-muted-foreground">
                      <span className="flex size-9 items-center justify-center rounded-full bg-muted/70">
                        <SquareTerminal className="size-4" />
                      </span>
                      <span>
                        {commands.length === 0 ? t("empty") : t("noResults")}
                      </span>
                    </div>
                  ) : (
                    <Reorder.Group
                      as="div"
                      axis="y"
                      values={filteredCommands}
                      onReorder={handleReorder}
                      className="min-h-0 flex-1 space-y-2 overflow-y-auto p-2"
                    >
                      {filteredCommands.map((command) => {
                        const dragDisabled =
                          reordering ||
                          searchActive ||
                          filteredCommands.length < 2
                        return (
                          <CommandReorderItem
                            key={command.id}
                            command={command}
                            selected={selectedId === command.id}
                            disabled={dragDisabled}
                            onSelect={setSelectedId}
                            onDragEnd={() => {
                              const order = pendingOrderRef.current
                              pendingOrderRef.current = null
                              if (order && !reordering) {
                                persistReorder(order).catch((err) => {
                                  console.error(
                                    "[CommandManage] reorder failed:",
                                    err
                                  )
                                })
                              }
                            }}
                          >
                            {(startDrag) => (
                              <div className="flex items-center gap-2 overflow-hidden">
                                <button
                                  type="button"
                                  className="cursor-grab rounded p-0.5 text-muted-foreground hover:bg-muted active:cursor-grabbing disabled:cursor-default disabled:opacity-40"
                                  title={t("dragSort")}
                                  aria-label={t("dragSortCommand", {
                                    name: command.name,
                                  })}
                                  onPointerDown={startDrag}
                                  onClick={(event) => event.stopPropagation()}
                                  disabled={dragDisabled}
                                >
                                  <GripVertical className="h-3.5 w-3.5" />
                                </button>
                                <div className="min-w-0 flex-1">
                                  <div className="truncate text-sm font-medium">
                                    {command.name}
                                  </div>
                                  <div className="mt-0.5 truncate font-mono text-[11px] text-muted-foreground">
                                    {command.command}
                                  </div>
                                </div>
                              </div>
                            )}
                          </CommandReorderItem>
                        )
                      })}
                    </Reorder.Group>
                  )}
                </div>
              </ResizablePanel>

              <ResizableHandle
                withHandle
                className={cn(isMobile && "my-1.5")}
              />

              <ResizablePanel
                defaultSize={isMobile ? 62 : 64}
                minSize={isMobile ? 48 : rightMinSize}
              >
                <div className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden rounded-xl border bg-card lg:rounded-l-none lg:border-l-0">
                  <div className="min-h-0 flex-1 space-y-3.5 overflow-y-auto p-3 sm:space-y-4 sm:p-4">
                    <div className="space-y-1.5">
                      <Label htmlFor="folder-command-name" className="text-xs">
                        {t("nameLabel")}
                      </Label>
                      <Input
                        id="folder-command-name"
                        className="h-11 text-base sm:h-9 sm:text-sm"
                        value={draft.name}
                        onChange={(event) =>
                          updateDraft({ name: event.target.value })
                        }
                      />
                    </div>
                    <div className="space-y-1.5">
                      <Label
                        htmlFor="folder-command-command"
                        className="text-xs"
                      >
                        {t("commandLabel")}
                      </Label>
                      <Input
                        id="folder-command-command"
                        className="h-11 font-mono text-base sm:h-9 sm:text-sm"
                        value={draft.command}
                        placeholder="pnpm dev"
                        onChange={(event) =>
                          updateDraft({ command: event.target.value })
                        }
                      />
                    </div>
                  </div>

                  <div className="shrink-0 space-y-3 border-t px-3.5 py-3 sm:px-4">
                    {formError ? (
                      <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
                        {formError}
                      </div>
                    ) : null}
                    <div className="grid grid-cols-2 gap-2 sm:flex sm:items-center sm:justify-between">
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => setDeleteTargetId(draft.id)}
                        disabled={
                          deleting || saving || loading || draft.id === null
                        }
                        className="h-11 text-red-500 hover:text-red-500 sm:h-8"
                      >
                        {deleting ? (
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        ) : (
                          <Trash2 className="h-3.5 w-3.5" />
                        )}
                        {tCommon("delete")}
                      </Button>
                      <Button
                        size="sm"
                        className="h-11 sm:h-8"
                        onClick={() => {
                          handleSave().catch((err) => {
                            console.error("[CommandManage] save failed:", err)
                          })
                        }}
                        disabled={saveDisabled}
                      >
                        {saving ? (
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        ) : (
                          <Save className="h-3.5 w-3.5" />
                        )}
                        {tCommon("save")}
                      </Button>
                    </div>
                  </div>
                </div>
              </ResizablePanel>
            </ResizablePanelGroup>
          </div>
        </DialogContent>
      </Dialog>

      <AlertDialog
        open={deleteTargetId !== null}
        onOpenChange={(nextOpen) => {
          if (!nextOpen && !deleting) setDeleteTargetId(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("confirmDelete.title")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("confirmDelete.message", {
                name: deleteTarget?.name ?? "",
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleting}>
              {tCommon("cancel")}
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={(event) => {
                event.preventDefault()
                handleDelete().catch((err) => {
                  console.error("[CommandManage] delete failed:", err)
                })
              }}
              disabled={deleting}
            >
              {deleting ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : null}
              {tCommon("delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
