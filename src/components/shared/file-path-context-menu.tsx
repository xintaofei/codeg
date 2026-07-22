"use client"

/**
 * VS Code-style right-click menu for a workspace / session file path.
 *
 * Used by the per-conversation message navigator and the per-reply artifacts
 * card so both surfaces share the same copy / open / mention actions.
 *
 * Menu shape (items appear only when applicable):
 *  - Open in Codeg (optional `onOpenInCodeg`)
 *  - Add to chat (insert `@file` badge into the active composer)
 *  - Reveal in Finder / Explorer (local desktop)
 *  - Open with → Default app / VS Code / Cursor (local desktop)
 *  - Copy Relative Path
 *  - Copy Absolute Path
 *  - Copy File Name
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
  children: ReactNode
}

export function FilePathContextMenu({
  filePath,
  folderPath,
  externalOpenDisabled = false,
  onOpenInCodeg,
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
  // Composer attach expects a resolvable filesystem path (absolute preferred).
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

  /**
   * Insert an inline `@file` reference badge into the active conversation
   * composer — same event path as the file-tree "Add to session" action.
   */
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

  // ConversationDetailPanel wraps the whole chat surface in its own
  // ContextMenu (copy selection / export / …). Nested Radix context menus
  // both listen on bubble — without stopPropagation the outer menu steals
  // the right-click and this file menu never appears. Stop the event on the
  // trigger so only this menu opens (same pattern as chat-input).
  const stopOuterContextMenu = useCallback((event: ReactMouseEvent) => {
    event.stopPropagation()
  }, [])

  const stopOuterRightPointer = useCallback((event: ReactPointerEvent) => {
    // Parent panel also intercepts right-button pointerdown when text is
    // selected; keep the event local so nested file rows stay responsive.
    if (event.button === 2) event.stopPropagation()
  }, [])

  return (
    <ContextMenu modal={false}>
      <ContextMenuTrigger
        asChild
        onContextMenu={stopOuterContextMenu}
        onPointerDown={stopOuterRightPointer}
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
