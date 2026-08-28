"use client"

import { useCallback, useMemo, useState } from "react"
import { ArrowRightLeft, Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { DirectoryPathInput } from "@/components/shared/directory-path-input"
import { FolderSelect } from "@/components/shared/folder-select"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { extractAppCommandError, toErrorMessage } from "@/lib/app-error"
import { moveConversation } from "@/lib/api"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useTabStore } from "@/stores/tab-store"

export interface ConversationMoveTarget {
  conversationId: number
  folderId: number
  folderPath?: string
  title: string
}

interface ConversationMoveDialogProps {
  target: ConversationMoveTarget
  onClose: () => void
}

/**
 * Rebind a persisted top-level conversation to another working folder.
 *
 * The target is a snapshot captured by the menu that opened the dialog. This
 * matters in the detail header, whose single instance can switch from tab A to
 * tab B while the modal remains open. The backend owns the authoritative
 * transaction and idle-connection detach; this surface only registers an
 * arbitrary directory when needed and immediately converges local stores from
 * the returned summary.
 */
export function ConversationMoveDialog({
  target,
  onClose,
}: ConversationMoveDialogProps) {
  const t = useTranslations("Folder.conversationCard")
  const allFolders = useAppWorkspaceStore((state) => state.allFolders)
  const openFolder = useAppWorkspaceStore((state) => state.openFolder)
  const [targetFolderId, setTargetFolderId] = useState<number | null>(null)
  const [directoryPath, setDirectoryPath] = useState("")
  const [openingDirectory, setOpeningDirectory] = useState(false)
  const [moving, setMoving] = useState(false)
  const busy = openingDirectory || moving

  const sourceFolder = allFolders.find(
    (folder) => folder.id === target.folderId
  )
  const sourcePath =
    target.folderPath ?? sourceFolder?.path ?? `#${target.folderId}`
  const targetFolders = useMemo(
    () =>
      allFolders.filter(
        (folder) => folder.kind !== "chat" && folder.id !== target.folderId
      ),
    [allFolders, target.folderId]
  )

  const handleOpenDirectory = useCallback(async () => {
    const path = directoryPath.trim()
    if (!path || busy) return
    setOpeningDirectory(true)
    try {
      const folder = await openFolder(path)
      if (folder.id === target.folderId) {
        toast.error(t("moveSameFolder"))
        return
      }
      setTargetFolderId(folder.id)
    } catch (error) {
      toast.error(t("moveOpenFolderFailed"), {
        description: toErrorMessage(error),
      })
    } finally {
      setOpeningDirectory(false)
    }
  }, [busy, directoryPath, openFolder, t, target.folderId])

  const handleMove = useCallback(async () => {
    if (targetFolderId == null || busy) return
    const destination = useAppWorkspaceStore
      .getState()
      .allFolders.find((folder) => folder.id === targetFolderId)
    if (!destination || destination.kind === "chat") {
      toast.error(t("moveDestinationUnavailable"))
      return
    }

    setMoving(true)
    try {
      const summary = await moveConversation(
        target.conversationId,
        destination.id
      )
      useAppWorkspaceStore.getState().applyConversationUpsert(summary)
      useTabStore
        .getState()
        .moveConversationTab(summary.id, destination.id, destination.path)
      toast.success(t("moveSuccess"))
      onClose()
    } catch (error) {
      const commandError = extractAppCommandError(error)
      toast.error(t("moveFailed"), {
        description:
          commandError?.code === "turn_in_progress"
            ? t("moveTurnInProgress")
            : toErrorMessage(error),
      })
    } finally {
      setMoving(false)
    }
  }, [busy, onClose, t, target.conversationId, targetFolderId])

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose()
      }}
    >
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("moveConversationTitle")}</DialogTitle>
          <DialogDescription>
            {t("moveConversationDescription", { title: target.title })}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-1.5">
            <span className="text-xs font-medium text-muted-foreground">
              {t("moveCurrentFolder")}
            </span>
            <div
              dir="ltr"
              title={sourcePath}
              className="truncate rounded-md border bg-muted/40 px-3 py-2 text-xs"
            >
              {sourcePath}
            </div>
          </div>

          <div className="space-y-1.5">
            <span className="text-xs font-medium text-muted-foreground">
              {t("moveDestinationFolder")}
            </span>
            <FolderSelect
              folders={targetFolders}
              value={targetFolderId}
              onChange={setTargetFolderId}
              placeholder={t("moveSelectFolder")}
              variant="field"
              disabled={busy}
              className="w-full max-w-none justify-between"
            />
          </div>

          <div className="space-y-1.5">
            <span className="text-xs font-medium text-muted-foreground">
              {t("moveOtherDirectory")}
            </span>
            <div className="flex items-center gap-2">
              <DirectoryPathInput
                value={directoryPath}
                onValueChange={setDirectoryPath}
                placeholder={t("moveDirectoryPlaceholder")}
                browseLabel={t("moveBrowseDirectory")}
                browserTitle={t("moveBrowseDirectory")}
                initialPath={
                  sourcePath.startsWith("#") ? undefined : sourcePath
                }
                disabled={busy}
                className="min-w-0 flex-1"
              />
              <Button
                type="button"
                variant="outline"
                onClick={handleOpenDirectory}
                disabled={!directoryPath.trim() || busy}
              >
                {openingDirectory ? (
                  <Loader2 className="size-4 animate-spin" aria-hidden />
                ) : null}
                {openingDirectory ? t("moveOpeningFolder") : t("moveUseFolder")}
              </Button>
            </div>
          </div>
        </div>

        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={onClose}
            disabled={busy}
          >
            {t("cancel")}
          </Button>
          <Button
            type="button"
            onClick={handleMove}
            disabled={targetFolderId == null || busy}
          >
            {moving ? (
              <Loader2 className="size-4 animate-spin" aria-hidden />
            ) : (
              <ArrowRightLeft className="size-4" aria-hidden />
            )}
            {moving ? t("movingConversation") : t("moveConversation")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
