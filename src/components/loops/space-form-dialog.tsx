"use client"

import { useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { FolderOpen, Loader2 } from "lucide-react"

import { openFolder } from "@/lib/api"
import { createLoopSpace, updateLoopSpace } from "@/lib/loops-api"
import type { LoopSpaceSummary } from "@/lib/types"
import { toErrorMessage } from "@/lib/app-error"
import { isDesktop, openFileDialog } from "@/lib/platform"
import { getActiveRemoteConnectionId } from "@/lib/transport"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Button } from "@/components/ui/button"
import { Label } from "@/components/ui/label"
import { DirectoryBrowserDialog } from "@/components/shared/directory-browser-dialog"

interface SpaceFormDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  /** Present = edit (rename only); absent = create. */
  space?: LoopSpaceSummary | null
  onSaved: (space: LoopSpaceSummary) => void
}

/**
 * Create a loop space (name + a bound git-repo folder) or rename an existing
 * one. The folder is resolved to a registered folder id only on submit, so a
 * cancelled flow never registers anything. Backend rejects non-git folders with
 * `NotGitRepo`, surfaced as a toast.
 */
export function SpaceFormDialog({
  open,
  onOpenChange,
  space,
  onSaved,
}: SpaceFormDialogProps) {
  const t = useTranslations("Loops.spaceForm")
  const tToasts = useTranslations("Loops.toasts")
  const isEdit = space != null

  const [name, setName] = useState("")
  const [folderPath, setFolderPath] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [browserOpen, setBrowserOpen] = useState(false)

  useEffect(() => {
    if (open) {
      setName(space?.name ?? "")
      setFolderPath(space?.folder_path ?? null)
      setBusy(false)
    }
  }, [open, space])

  const handleBrowse = async () => {
    if (isDesktop() && getActiveRemoteConnectionId() === null) {
      const selected = await openFileDialog({
        directory: true,
        multiple: false,
      })
      if (selected) {
        setFolderPath(Array.isArray(selected) ? selected[0] : selected)
      }
    } else {
      setBrowserOpen(true)
    }
  }

  const canSubmit = isEdit
    ? name.trim().length > 0
    : name.trim().length > 0 && folderPath != null

  const handleSubmit = async () => {
    if (!canSubmit || busy) return
    setBusy(true)
    try {
      if (isEdit) {
        const updated = await updateLoopSpace(space.id, name.trim())
        onSaved(updated)
      } else {
        // Register the folder to obtain its id, then bind it to the space.
        const folder = await openFolder(folderPath as string)
        const created = await createLoopSpace(name.trim(), folder.id)
        toast.success(tToasts("spaceCreated", { name: created.name }))
        onSaved(created)
      }
      onOpenChange(false)
    } catch (err) {
      const msg = toErrorMessage(err)
      const isNotGit = /not a git repository|NotGitRepo/i.test(msg)
      toast.error(
        isNotGit
          ? tToasts("notGitRepo")
          : tToasts(isEdit ? "spaceUpdateFailed" : "spaceCreateFailed", {
              message: msg,
            })
      )
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>
              {isEdit ? t("editTitle") : t("createTitle")}
            </DialogTitle>
          </DialogHeader>
          <div className="space-y-4 py-2">
            <div className="space-y-2">
              <Label htmlFor="loop-space-name">{t("nameLabel")}</Label>
              <Input
                id="loop-space-name"
                placeholder={t("namePlaceholder")}
                value={name}
                onChange={(e) => setName(e.target.value)}
                disabled={busy}
                autoFocus
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="loop-space-folder">{t("folderLabel")}</Label>
              <div className="flex gap-2">
                <Input
                  id="loop-space-folder"
                  value={folderPath ?? ""}
                  placeholder={t("noFolder")}
                  readOnly
                  disabled={isEdit || busy}
                  className="flex-1"
                />
                {!isEdit && (
                  <Button
                    variant="outline"
                    size="icon"
                    onClick={handleBrowse}
                    disabled={busy}
                    title={t("chooseFolder")}
                    aria-label={t("chooseFolder")}
                    type="button"
                  >
                    <FolderOpen className="h-4 w-4" />
                  </Button>
                )}
              </div>
              {!isEdit && (
                <p className="text-xs text-muted-foreground">
                  {t("folderHint")}
                </p>
              )}
            </div>
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => onOpenChange(false)}
              disabled={busy}
              type="button"
            >
              {t("cancel")}
            </Button>
            <Button
              onClick={handleSubmit}
              disabled={!canSubmit || busy}
              type="button"
            >
              {busy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {isEdit ? t("save") : t("create")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <DirectoryBrowserDialog
        open={browserOpen}
        onOpenChange={setBrowserOpen}
        onSelect={(path) => setFolderPath(path)}
      />
    </>
  )
}
