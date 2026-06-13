"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  FolderGit2,
  Loader2,
  MoreVertical,
  Pencil,
  Plus,
  Trash2,
  TriangleAlert,
} from "lucide-react"

import { deleteLoopSpace, listLoopSpaces } from "@/lib/loops-api"
import type { LoopSpaceSummary } from "@/lib/types"
import { toErrorMessage } from "@/lib/app-error"
import { useLoopChanged } from "@/hooks/use-loop-changed"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
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
import { SpaceFormDialog } from "@/components/loops/space-form-dialog"
import { SpaceDetail } from "@/components/loops/space-detail"

export function LoopsWorkbench() {
  const t = useTranslations("Loops.workbench")
  const tCommon = useTranslations("Loops.common")
  const tToasts = useTranslations("Loops.toasts")

  const [spaces, setSpaces] = useState<LoopSpaceSummary[]>([])
  const [loading, setLoading] = useState(true)
  const [selectedSpaceId, setSelectedSpaceId] = useState<number | null>(null)
  const [formOpen, setFormOpen] = useState(false)
  const [editing, setEditing] = useState<LoopSpaceSummary | null>(null)
  const [deleting, setDeleting] = useState<LoopSpaceSummary | null>(null)
  const [deleteBusy, setDeleteBusy] = useState(false)

  const refresh = useCallback(async () => {
    try {
      const list = await listLoopSpaces()
      setSpaces(list)
    } catch (err) {
      toast.error(t("loadFailed", { message: toErrorMessage(err) }))
    } finally {
      setLoading(false)
    }
  }, [t])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useLoopChanged(() => {
    void refresh()
  })

  const selectedSpace = useMemo(
    () => spaces.find((s) => s.id === selectedSpaceId) ?? null,
    [spaces, selectedSpaceId]
  )

  // The selected space vanished (deleted elsewhere) — fall back to the grid.
  useEffect(() => {
    if (selectedSpaceId != null && !loading && !selectedSpace) {
      setSelectedSpaceId(null)
    }
  }, [selectedSpaceId, selectedSpace, loading])

  const handleDelete = async () => {
    if (!deleting) return
    setDeleteBusy(true)
    try {
      await deleteLoopSpace(deleting.id)
      if (selectedSpaceId === deleting.id) setSelectedSpaceId(null)
      setDeleting(null)
      await refresh()
    } catch (err) {
      toast.error(
        tToasts("spaceDeleteFailed", { message: toErrorMessage(err) })
      )
    } finally {
      setDeleteBusy(false)
    }
  }

  if (selectedSpace) {
    return (
      <SpaceDetail
        space={selectedSpace}
        onBack={() => setSelectedSpaceId(null)}
      />
    )
  }

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <div className="flex shrink-0 items-start justify-between gap-4 px-6 pt-6 pb-4">
        <div className="min-w-0">
          <h1 className="text-lg font-semibold">{t("title")}</h1>
          <p className="mt-1 max-w-2xl text-sm text-muted-foreground">
            {t("subtitle")}
          </p>
        </div>
        <Button
          onClick={() => {
            setEditing(null)
            setFormOpen(true)
          }}
          className="shrink-0"
        >
          <Plus className="mr-1.5 h-4 w-4" />
          {t("newSpace")}
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-6">
        {loading ? (
          <div className="flex h-40 items-center justify-center text-muted-foreground">
            <Loader2 className="h-5 w-5 animate-spin" />
          </div>
        ) : spaces.length === 0 ? (
          <div className="flex h-40 items-center justify-center rounded-lg border border-dashed text-center text-sm text-muted-foreground">
            <p className="max-w-md px-6">{t("empty")}</p>
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {spaces.map((space) => (
              <SpaceCard
                key={space.id}
                space={space}
                onOpen={() => setSelectedSpaceId(space.id)}
                onEdit={() => {
                  setEditing(space)
                  setFormOpen(true)
                }}
                onDelete={() => setDeleting(space)}
              />
            ))}
          </div>
        )}
      </div>

      <SpaceFormDialog
        open={formOpen}
        onOpenChange={setFormOpen}
        space={editing}
        onSaved={(saved) => {
          void refresh()
          if (!editing) setSelectedSpaceId(saved.id)
        }}
      />

      <AlertDialog
        open={deleting != null}
        onOpenChange={(o) => !o && setDeleting(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("confirmDeleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("confirmDeleteDescription", { name: deleting?.name ?? "" })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteBusy}>
              {tCommon("cancel")}
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                e.preventDefault()
                void handleDelete()
              }}
              disabled={deleteBusy}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              {deleteBusy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {t("deleteSpace")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

function SpaceCard({
  space,
  onOpen,
  onEdit,
  onDelete,
}: {
  space: LoopSpaceSummary
  onOpen: () => void
  onEdit: () => void
  onDelete: () => void
}) {
  const t = useTranslations("Loops.workbench")
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => {
        // Only the card itself activates on Enter/Space — not the menu trigger
        // or other focusable children nested inside it.
        if (e.target !== e.currentTarget) return
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault()
          onOpen()
        }
      }}
      className="group flex flex-col gap-3 rounded-lg border bg-card p-4 text-left outline-none transition-colors hover:border-primary/40 hover:bg-accent/40 focus-visible:ring-2 focus-visible:ring-ring"
    >
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0 flex-1">
          <div className="truncate font-medium">{space.name}</div>
          <div className="mt-0.5 flex items-center gap-1 text-xs text-muted-foreground">
            <FolderGit2 className="h-3 w-3 shrink-0" />
            <span className="truncate">{space.folder_path}</span>
          </div>
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="h-7 w-7 shrink-0 opacity-0 transition-opacity group-hover:opacity-100 data-[state=open]:opacity-100"
              onClick={(e) => e.stopPropagation()}
              aria-label={t("rename")}
            >
              <MoreVertical className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" onClick={(e) => e.stopPropagation()}>
            <DropdownMenuItem onSelect={onEdit}>
              <Pencil className="h-3.5 w-3.5" />
              {t("rename")}
            </DropdownMenuItem>
            <DropdownMenuItem
              onSelect={onDelete}
              className="text-destructive focus:text-destructive"
            >
              <Trash2 className="h-3.5 w-3.5" />
              {t("deleteSpace")}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <div className="flex items-center gap-2 text-xs">
        {space.detached && (
          <Badge variant="destructive" className="gap-1">
            <TriangleAlert className="h-3 w-3" />
            {t("detached")}
          </Badge>
        )}
        <Badge variant="secondary">
          {t("spaceIssues", { count: space.issue_count })}
        </Badge>
        {space.running_count > 0 && (
          <Badge variant="default">
            {t("spaceRunning", { count: space.running_count })}
          </Badge>
        )}
      </div>
    </div>
  )
}
