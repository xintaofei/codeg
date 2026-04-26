"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { Reorder } from "motion/react"
import { Code, Eye, ExternalLink, FileText, GitCompare, X } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { getSystemOpenTargetSettings, openPathWithTarget } from "@/lib/api"
import { isDesktop, openPath, revealItemInDir } from "@/lib/platform"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useTerminalContext } from "@/contexts/terminal-context"
import { useWorkspaceContext } from "@/contexts/workspace-context"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { toErrorMessage } from "@/lib/app-error"
import { matchShortcutEvent } from "@/lib/keyboard-shortcuts"
import { isWebFileLanguage } from "@/lib/open-targets"
import { joinFsPath } from "@/lib/path-utils"
import { cn } from "@/lib/utils"
import type { SystemOpenTarget, SystemWebFileOpenMethod } from "@/lib/types"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"

function parentDir(filePath: string): string {
  const slashIndex = filePath.lastIndexOf("/")
  const backslashIndex = filePath.lastIndexOf("\\")
  const splitIndex = Math.max(slashIndex, backslashIndex)
  if (splitIndex < 0) return filePath
  if (splitIndex === 0) return filePath.slice(0, 1)
  return filePath.slice(0, splitIndex)
}

function baseName(path: string): string {
  return path.split(/[/\\]/).pop() || path
}

export function FileWorkspaceTabBar() {
  const t = useTranslations("Folder.fileWorkspace")
  const tFileTree = useTranslations("Folder.fileTreeTab")
  const {
    mode,
    activePane,
    fileTabs,
    activeFileTabId,
    switchFileTab,
    closeFileTab,
    closeOtherFileTabs,
    closeAllFileTabs,
    reorderFileTabs,
    previewFileTabIds,
    toggleFileTabPreview,
  } = useWorkspaceContext()
  const { activeFolder: folder } = useActiveFolder()
  const { createTerminalInDirectory } = useTerminalContext()
  const { shortcuts } = useShortcutSettings()
  const scrollRef = useRef<HTMLDivElement>(null)
  const [isHovered, setIsHovered] = useState(false)
  const [webFileOpenMethod, setWebFileOpenMethod] =
    useState<SystemWebFileOpenMethod>("browser")

  const handleWheel = useCallback((e: React.WheelEvent<HTMLDivElement>) => {
    if (e.deltaY !== 0 && scrollRef.current) {
      e.preventDefault()
      scrollRef.current.scrollLeft += e.deltaY
    }
  }, [])

  useEffect(() => {
    if (!activeFileTabId || !scrollRef.current) return
    const el = scrollRef.current.querySelector(
      `[data-file-tab-id="${activeFileTabId}"]`
    )
    el?.scrollIntoView({ block: "nearest", inline: "nearest" })
  }, [activeFileTabId])

  useEffect(() => {
    if (!isDesktop()) return

    let cancelled = false
    getSystemOpenTargetSettings()
      .then((settings) => {
        if (!cancelled) {
          setWebFileOpenMethod(settings.web_file_open_method ?? "browser")
        }
      })
      .catch(() => {})

    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const shouldHandleShortcut =
        mode === "files" || (mode === "fusion" && activePane === "files")
      if (!shouldHandleShortcut) return
      if (matchShortcutEvent(event, shortcuts.close_all_file_tabs)) {
        event.preventDefault()
        closeAllFileTabs()
        return
      }
      if (!matchShortcutEvent(event, shortcuts.close_current_tab)) return

      if (!activeFileTabId) return
      event.preventDefault()
      closeFileTab(activeFileTabId)
    }

    window.addEventListener("keydown", onKeyDown)
    return () => {
      window.removeEventListener("keydown", onKeyDown)
    }
  }, [
    activeFileTabId,
    closeAllFileTabs,
    closeFileTab,
    mode,
    activePane,
    shortcuts.close_all_file_tabs,
    shortcuts.close_current_tab,
  ])

  const activeTab = fileTabs.find((tab) => tab.id === activeFileTabId)
  const activeFilePath = activeTab?.path ?? null
  const activeFolderPath = folder?.path ?? null
  const canPreview =
    activeTab?.kind === "file" && activeTab.language === "markdown"
  const canOpenWebFile =
    isDesktop() &&
    activeTab?.kind === "file" &&
    !activeTab.loading &&
    isWebFileLanguage(activeTab.language)
  const canOpenInEditor =
    isDesktop() &&
    activeTab?.kind === "file" &&
    !activeTab.loading &&
    !canOpenWebFile &&
    Boolean(activeFilePath && activeFolderPath)
  const isPreviewActive =
    canPreview && activeFileTabId
      ? previewFileTabIds.has(activeFileTabId)
      : false

  const loadLatestOpenTargetSettings = useCallback(async () => {
    const settings = await getSystemOpenTargetSettings()
    const nextTarget = settings.target ?? "file_manager"
    const nextWebFileOpenMethod = settings.web_file_open_method ?? "browser"
    setWebFileOpenMethod(nextWebFileOpenMethod)
    return {
      target: nextTarget,
      webFileOpenMethod: nextWebFileOpenMethod,
    }
  }, [])

  const openFileWithTarget = useCallback(
    async (params: {
      folderPath: string
      relativePath: string
      target: SystemOpenTarget
    }) => {
      const fullPath = joinFsPath(params.folderPath, params.relativePath)

      switch (params.target) {
        case "vscode":
          await openPathWithTarget({
            folderPath: params.folderPath,
            relativePath: params.relativePath,
            target: "vscode",
          })
          return
        case "file_manager":
          await revealItemInDir(fullPath)
          return
        case "terminal": {
          const terminalId = await createTerminalInDirectory(
            parentDir(fullPath),
            tFileTree("terminalTitle", { name: baseName(params.relativePath) })
          )
          if (!terminalId) {
            throw new Error(tFileTree("toasts.openBuiltinTerminalFailed"))
          }
          return
        }
        default: {
          const exhaustive: never = params.target
          return exhaustive
        }
      }
    },
    [createTerminalInDirectory, tFileTree]
  )

  const handleOpenInEditor = useCallback(async () => {
    if (!activeFilePath || !activeFolderPath) return

    try {
      const { target } = await loadLatestOpenTargetSettings()
      await openFileWithTarget({
        folderPath: activeFolderPath,
        relativePath: activeFilePath,
        target,
      })
    } catch (err) {
      toast.error(t("openInEditorFailed", { message: toErrorMessage(err) }))
    }
  }, [
    activeFilePath,
    activeFolderPath,
    loadLatestOpenTargetSettings,
    openFileWithTarget,
    t,
  ])

  const handleOpenWebFile = useCallback(async () => {
    if (!activeFilePath || !activeFolderPath) return

    let settings: Awaited<ReturnType<typeof loadLatestOpenTargetSettings>>
    try {
      settings = await loadLatestOpenTargetSettings()
    } catch (err) {
      toast.error(t("openInEditorFailed", { message: toErrorMessage(err) }))
      return
    }

    const fullPath = joinFsPath(activeFolderPath, activeFilePath)

    if (settings.webFileOpenMethod === "editor") {
      try {
        await openFileWithTarget({
          folderPath: activeFolderPath,
          relativePath: activeFilePath,
          target: settings.target,
        })
      } catch (err) {
        toast.error(t("openInEditorFailed", { message: toErrorMessage(err) }))
      }
      return
    }

    try {
      await openPath(fullPath)
    } catch (err) {
      toast.error(t("openInBrowserFailed", { message: toErrorMessage(err) }))
    }
  }, [
    activeFilePath,
    activeFolderPath,
    loadLatestOpenTargetSettings,
    openFileWithTarget,
    t,
  ])

  if (fileTabs.length === 0) {
    return (
      <div className="h-10 px-3 flex items-center border-b border-border text-xs text-muted-foreground">
        {t("files")}
      </div>
    )
  }

  return (
    <div className="flex items-stretch">
      <Reorder.Group
        as="div"
        ref={scrollRef}
        role="tablist"
        axis="x"
        values={fileTabs}
        onReorder={reorderFileTabs}
        onWheel={handleWheel}
        onMouseEnter={() => setIsHovered(true)}
        onMouseLeave={() => setIsHovered(false)}
        className={cn(
          "h-10 pt-1.5 px-1.5 flex-1 min-w-0 flex items-stretch gap-1.5 border-b border-border",
          "overflow-x-scroll",
          isHovered
            ? [
                "pb-0.5",
                "[&::-webkit-scrollbar]:h-1",
                "[&::-webkit-scrollbar-track]:bg-transparent",
                "[&::-webkit-scrollbar-thumb]:rounded-full",
                "[&::-webkit-scrollbar-thumb]:bg-border",
              ]
            : ["pb-1.5", "[&::-webkit-scrollbar]:h-0"]
        )}
      >
        {fileTabs.map((tab) => {
          const active = tab.id === activeFileTabId
          const isDiff = tab.kind === "diff" || tab.kind === "rich-diff"
          const isDirty = tab.kind === "file" && Boolean(tab.isDirty)

          return (
            <Reorder.Item
              key={tab.id}
              as="div"
              value={tab}
              data-file-tab-id={tab.id}
              className="shrink-0 rounded-full cursor-grab active:cursor-grabbing"
            >
              <ContextMenu>
                <ContextMenuTrigger asChild>
                  <div
                    role="tab"
                    aria-selected={active}
                    onClick={() => switchFileTab(tab.id)}
                    className={cn(
                      "group/filetab relative flex items-center h-full gap-1.5 px-3 text-xs rounded-full",
                      "cursor-pointer select-none shrink-0 hover:bg-primary/8 transition-colors",
                      active
                        ? "bg-primary/10 text-foreground"
                        : "text-muted-foreground"
                    )}
                    title={tab.description ?? tab.title}
                  >
                    {isDiff ? (
                      <GitCompare className="h-3.5 w-3.5" />
                    ) : (
                      <FileText className="h-3.5 w-3.5" />
                    )}
                    <span className="truncate max-w-[180px]">
                      {tab.title}
                      {isDirty ? " *" : ""}
                    </span>
                    <button
                      type="button"
                      className={cn(
                        "rounded-full p-0.5 hover:bg-muted",
                        active
                          ? "opacity-100"
                          : "opacity-0 group-hover/filetab:opacity-100"
                      )}
                      onClick={(event) => {
                        event.stopPropagation()
                        closeFileTab(tab.id)
                      }}
                      aria-label={t("closeFileTab")}
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </div>
                </ContextMenuTrigger>
                <ContextMenuContent>
                  <ContextMenuItem onSelect={() => closeFileTab(tab.id)}>
                    {t("close")}
                  </ContextMenuItem>
                  <ContextMenuItem onSelect={() => closeOtherFileTabs(tab.id)}>
                    {t("closeOthers")}
                  </ContextMenuItem>
                  <ContextMenuSeparator />
                  <ContextMenuItem onSelect={closeAllFileTabs}>
                    {t("closeAll")}
                  </ContextMenuItem>
                </ContextMenuContent>
              </ContextMenu>
            </Reorder.Item>
          )
        })}
      </Reorder.Group>
      {canPreview && activeFileTabId && (
        <button
          type="button"
          onClick={() => toggleFileTabPreview(activeFileTabId)}
          className={cn(
            "shrink-0 flex items-center justify-center w-10 border-b border-border hover:bg-primary/8 transition-colors",
            isPreviewActive && "text-primary"
          )}
          aria-label={isPreviewActive ? t("editSource") : t("preview")}
          title={isPreviewActive ? t("editSource") : t("preview")}
        >
          {isPreviewActive ? (
            <Code className="h-4 w-4" />
          ) : (
            <Eye className="h-4 w-4" />
          )}
        </button>
      )}
      {canOpenInEditor && (
        <button
          type="button"
          onClick={handleOpenInEditor}
          className="shrink-0 flex items-center justify-center w-10 border-b border-border hover:bg-primary/8 transition-colors"
          aria-label={t("openInEditor")}
          title={t("openInEditor")}
        >
          <ExternalLink className="h-4 w-4" />
        </button>
      )}
      {canOpenWebFile && activeFilePath && activeFolderPath && (
        <button
          type="button"
          onClick={handleOpenWebFile}
          className="shrink-0 flex items-center justify-center w-10 border-b border-border hover:bg-primary/8 transition-colors"
          aria-label={
            webFileOpenMethod === "editor" ? t("openInEditor") : t("preview")
          }
          title={
            webFileOpenMethod === "editor" ? t("openInEditor") : t("preview")
          }
        >
          <ExternalLink className="h-4 w-4" />
        </button>
      )}
    </div>
  )
}
