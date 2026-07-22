"use client"

/**
 * VS Code-style right-click menu for a workspace / session file path.
 *
 * Used by the per-conversation message navigator and the per-reply artifacts
 * card so both surfaces share the same copy / open / mention actions.
 *
 * Nested under ConversationDetailPanel's full-surface ContextMenu — that
 * ancestor steals right-clicks unless we (1) stopPropagation on our trigger
 * and (2) mark `[data-file-path-menu]` so the panel can suppress its own open
 * when the event still bubbles (see conversation-detail-panel).
 */

import {
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  useCallback,
  useMemo,
} from "react"
import {
  AppWindow,
  AtSign,
  ClipboardCopy,
  Code2,
  Copy,
  ExternalLink,
  FileCode,
  FileType,
  FolderOpen,
  TextCursorInput,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import { useTabStore } from "@/contexts/tab-context"
import {
  copyPathText,
  openFileWithDefaultApp,
  openFileWithExternalEditor,
  resolveFilePathTargets,
  revealFileInManager,
  systemExplorerLabelKey,
  type ExternalEditorId,
} from "@/lib/file-path-actions"
import { isLocalDesktop } from "@/lib/platform"
import { toErrorMessage } from "@/lib/app-error"
import { emitAttachFileToSession } from "@/lib/session-attachment-events"
import { cn } from "@/lib/utils"

/** Attribute the conversation-panel ContextMenu looks for to yield ownership. */
export const FILE_PATH_MENU_ATTR = "data-file-path-menu"

export interface FilePathContextMenuProps {
  /** Agent-reported path (absolute or workspace-relative). */
  filePath: string
  /** Active workspace folder; needed for absolute-path resolve & relative copy. */
  folderPath?: string
  /**
   * When true, reveal / open-with-external actions are disabled (e.g. a deleted
   * file has no on-disk target). Copy, open-in-Codeg, and add-to-chat stay
   * available (chat still accepts a path mention for deleted files).
   */
  externalOpenDisabled?: boolean
  /** Primary open action (Codeg tab / session diff). Omitted = hide the item. */
  onOpenInCodeg?: () => void
  /**
   * Native HTML tooltip for the trigger surface (e.g. absolute path on a
   * tool-row wrapper). Applied on the trigger element that receives hover.
   */
  title?: string
  /**
   * Merge trigger props onto `children` (must be a single element that accepts
   * a ref). Use for tool headers (`CollapsibleTrigger`) so the whole row owns
   * the right-click without an extra wrapping span.
   */
  asChild?: boolean
  className?: string
  children: ReactNode
}

export function FilePathContextMenu({
  filePath,
  folderPath,
  externalOpenDisabled = false,
  onOpenInCodeg,
  title,
  asChild = false,
  className,
  children,
}: FilePathContextMenuProps) {
  const t = useTranslations("Folder.chat.filePathMenu")
  const localDesktop = isLocalDesktop()

  const activeSessionTabId = useTabStore((s) => {
    const active = s.tabs.find((tab) => tab.id === s.activeTabId)
    if (!active || active.kind !== "conversation") return null
    return active.id
  })

  const { relativePath, absolutePath, fileName } = useMemo(
    () => resolveFilePathTargets(filePath, folderPath),
    [filePath, folderPath]
  )

  const explorerLabel = t(systemExplorerLabelKey())
  const canOpenExternally =
    localDesktop && !!absolutePath && !externalOpenDisabled
  const attachPath = absolutePath ?? relativePath
  const canAddToChat = Boolean(activeSessionTabId && attachPath)

  const notifyCopy = useCallback(
    async (text: string, successKey: "pathCopied" | "fileNameCopied") => {
      const ok = await copyPathText(text)
      if (ok) {
        toast.success(t(successKey))
      } else {
        toast.error(t("copyFailed"))
      }
    },
    [t]
  )

  const runExternal = useCallback(
    async (action: () => Promise<void>) => {
      try {
        await action()
      } catch (error) {
        toast.error(t("openFailed"), {
          description: toErrorMessage(error),
        })
      }
    },
    [t]
  )

  const handleReveal = useCallback(() => {
    if (!absolutePath) return
    void runExternal(() => revealFileInManager(absolutePath))
  }, [absolutePath, runExternal])

  const handleOpenDefault = useCallback(() => {
    if (!absolutePath) return
    void runExternal(() => openFileWithDefaultApp(absolutePath))
  }, [absolutePath, runExternal])

  const handleOpenEditor = useCallback(
    (editor: ExternalEditorId) => {
      if (!absolutePath) return
      void runExternal(() => openFileWithExternalEditor(absolutePath, editor))
    },
    [absolutePath, runExternal]
  )

  const handleAddToChat = useCallback(() => {
    if (!activeSessionTabId || !attachPath) {
      toast.error(t("noActiveConversation"))
      return
    }
    emitAttachFileToSession({
      tabId: activeSessionTabId,
      path: attachPath,
    })
    toast.success(t("addToChatDone", { label: fileName }))
  }, [activeSessionTabId, attachPath, fileName, t])

  // Own the gesture before it reaches ConversationDetailPanel's ContextMenu.
  // Always stamp `data-file-path-menu` so the panel can `preventDefault` its
  // own open via composeEventHandlers when the event still bubbles.
  const claimContextMenu = useCallback((event: ReactMouseEvent) => {
    event.stopPropagation()
  }, [])

  const claimRightPointer = useCallback((event: ReactPointerEvent) => {
    if (event.button === 2) event.stopPropagation()
  }, [])

  // Prefer an explicit title, otherwise the resolved absolute path so hover on
  // the wrapper (not only the inner button) still shows the full path.
  const triggerTitle = title ?? absolutePath ?? relativePath

  return (
    <ContextMenu modal={false}>
      <ContextMenuTrigger
        asChild={asChild}
        data-file-path-menu=""
        title={triggerTitle || undefined}
        className={asChild ? className : cn("block w-full min-w-0", className)}
        onContextMenu={claimContextMenu}
        onPointerDown={claimRightPointer}
      >
        {children}
      </ContextMenuTrigger>
      <ContextMenuContent className="z-[80] min-w-52">
        {onOpenInCodeg && (
          <ContextMenuItem onSelect={() => onOpenInCodeg()}>
            <FileCode className="h-4 w-4" />
            {t("openInCodeg")}
          </ContextMenuItem>
        )}

        <ContextMenuItem
          disabled={!canAddToChat}
          onSelect={() => {
            if (!canAddToChat) return
            handleAddToChat()
          }}
        >
          <AtSign className="h-4 w-4" />
          {t("addToChat")}
        </ContextMenuItem>

        {localDesktop && (
          <ContextMenuItem
            disabled={!canOpenExternally}
            onSelect={() => {
              if (!canOpenExternally) return
              handleReveal()
            }}
          >
            <FolderOpen className="h-4 w-4" />
            {explorerLabel}
          </ContextMenuItem>
        )}

        {localDesktop && (
          <ContextMenuSub>
            <ContextMenuSubTrigger disabled={!canOpenExternally}>
              <ExternalLink className="h-4 w-4" />
              {t("openWith")}
            </ContextMenuSubTrigger>
            <ContextMenuSubContent className="min-w-44">
              <ContextMenuItem
                disabled={!canOpenExternally}
                onSelect={() => {
                  if (!canOpenExternally) return
                  handleOpenDefault()
                }}
              >
                <AppWindow className="h-4 w-4" />
                {t("openWithDefault")}
              </ContextMenuItem>
              <ContextMenuItem
                disabled={!canOpenExternally}
                onSelect={() => {
                  if (!canOpenExternally) return
                  handleOpenEditor("vscode")
                }}
              >
                <Code2 className="h-4 w-4" />
                {t("openWithVsCode")}
              </ContextMenuItem>
              <ContextMenuItem
                disabled={!canOpenExternally}
                onSelect={() => {
                  if (!canOpenExternally) return
                  handleOpenEditor("cursor")
                }}
              >
                <TextCursorInput className="h-4 w-4" />
                {t("openWithCursor")}
              </ContextMenuItem>
            </ContextMenuSubContent>
          </ContextMenuSub>
        )}

        <ContextMenuSeparator />

        <ContextMenuItem
          onSelect={() => {
            void notifyCopy(relativePath, "pathCopied")
          }}
        >
          <Copy className="h-4 w-4" />
          {t("copyRelativePath")}
        </ContextMenuItem>
        <ContextMenuItem
          disabled={!absolutePath}
          onSelect={() => {
            if (!absolutePath) return
            void notifyCopy(absolutePath, "pathCopied")
          }}
        >
          <ClipboardCopy className="h-4 w-4" />
          {t("copyAbsolutePath")}
        </ContextMenuItem>
        <ContextMenuItem
          onSelect={() => {
            void notifyCopy(fileName, "fileNameCopied")
          }}
        >
          <FileType className="h-4 w-4" />
          {t("copyFileName")}
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}
