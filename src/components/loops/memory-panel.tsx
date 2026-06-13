"use client"

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { Archive, ArchiveRestore, Loader2, Plus, Trash2 } from "lucide-react"

import {
  createLoopMemory,
  deleteLoopMemory,
  listLoopMemory,
  updateLoopMemory,
} from "@/lib/loops-api"
import type { LoopMemoryKind, LoopMemoryRow } from "@/lib/types"
import { toErrorMessage } from "@/lib/app-error"
import { useLoopChanged } from "@/hooks/use-loop-changed"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

const MEMORY_KINDS: LoopMemoryKind[] = [
  "constitution",
  "constraint",
  "decision",
  "preference",
  "pitfall",
]

/**
 * Space memory: the durable constraints, decisions and pitfalls fed into every
 * issue's briefing. Each entry shows its source — `human` (curated) or `agent`
 * (proposed by a loop iteration) — and can be added, archived/restored or
 * deleted. The engine reads only `active` entries, so archiving retires an entry
 * without losing it.
 */
export function MemoryPanel({ spaceId }: { spaceId: number }) {
  const t = useTranslations("Loops.memory")
  const tKind = useTranslations("Loops.memoryKind")
  const tActor = useTranslations("Loops.actorKind")
  const tCommon = useTranslations("Loops.common")
  const tToasts = useTranslations("Loops.toasts")

  const [items, setItems] = useState<LoopMemoryRow[]>([])
  const [loading, setLoading] = useState(true)
  const [busyId, setBusyId] = useState<number | null>(null)
  const [addOpen, setAddOpen] = useState(false)
  const [kind, setKind] = useState<LoopMemoryKind>("decision")
  const [title, setTitle] = useState("")
  const [content, setContent] = useState("")
  const [creating, setCreating] = useState(false)

  const refresh = useCallback(async () => {
    try {
      setItems(await listLoopMemory(spaceId))
    } catch {
      // non-fatal; the empty state covers it
    } finally {
      setLoading(false)
    }
  }, [spaceId])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useLoopChanged(() => {
    void refresh()
  }, spaceId)

  const create = async () => {
    if (!title.trim()) return
    setCreating(true)
    try {
      await createLoopMemory({
        spaceId,
        kind,
        title: title.trim(),
        content: content.trim(),
      })
      toast.success(tToasts("memorySaved"))
      setAddOpen(false)
      setTitle("")
      setContent("")
      setKind("decision")
      await refresh()
    } catch (err) {
      toast.error(tToasts("actionFailed", { message: toErrorMessage(err) }))
    } finally {
      setCreating(false)
    }
  }

  const run = async (id: number, fn: () => Promise<void>, ok: string) => {
    setBusyId(id)
    try {
      await fn()
      toast.success(ok)
      await refresh()
    } catch (err) {
      toast.error(tToasts("actionFailed", { message: toErrorMessage(err) }))
    } finally {
      setBusyId(null)
    }
  }

  const setArchived = (item: LoopMemoryRow, archived: boolean) =>
    run(
      item.id,
      () =>
        updateLoopMemory({
          spaceId,
          id: item.id,
          title: item.title,
          content: item.content,
          status: archived ? "archived" : "active",
        }),
      tToasts("memorySaved")
    )

  const remove = (item: LoopMemoryRow) =>
    run(
      item.id,
      () => deleteLoopMemory(spaceId, item.id),
      tToasts("memoryDeleted")
    )

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center justify-between px-1 pb-2">
        <span className="text-xs text-muted-foreground">{t("subtitle")}</span>
        <Button
          size="sm"
          variant="outline"
          className="h-7"
          onClick={() => setAddOpen(true)}
        >
          <Plus className="mr-1 h-3.5 w-3.5" />
          {t("add")}
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {loading ? (
          <div className="flex h-24 items-center justify-center text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
          </div>
        ) : items.length === 0 ? (
          <p className="px-1 py-6 text-center text-xs text-muted-foreground">
            {t("empty")}
          </p>
        ) : (
          <ul className="space-y-2">
            {items.map((m) => {
              const busy = busyId === m.id
              const archived = m.status === "archived"
              return (
                <li
                  key={m.id}
                  className={`rounded-md border p-2.5 ${archived ? "opacity-60" : ""}`}
                >
                  <div className="flex items-center gap-1.5">
                    <Badge variant="outline">{tKind(m.kind)}</Badge>
                    <Badge
                      variant={m.source === "agent" ? "secondary" : "outline"}
                    >
                      {tActor(m.source)}
                    </Badge>
                    {archived && <Badge variant="ghost">{t("archived")}</Badge>}
                    <span className="ml-1 min-w-0 flex-1 truncate text-sm font-medium">
                      {m.title}
                    </span>
                    <Button
                      size="icon"
                      variant="ghost"
                      className="h-7 w-7 shrink-0"
                      disabled={busy}
                      onClick={() => void setArchived(m, !archived)}
                      aria-label={archived ? t("restore") : t("archive")}
                    >
                      {archived ? (
                        <ArchiveRestore className="h-3.5 w-3.5" />
                      ) : (
                        <Archive className="h-3.5 w-3.5" />
                      )}
                    </Button>
                    <Button
                      size="icon"
                      variant="ghost"
                      className="h-7 w-7 shrink-0 text-destructive hover:text-destructive"
                      disabled={busy}
                      onClick={() => void remove(m)}
                      aria-label={tCommon("delete")}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                  {m.content.trim() && (
                    <p className="mt-1 whitespace-pre-wrap break-words text-xs text-muted-foreground">
                      {m.content}
                    </p>
                  )}
                </li>
              )
            })}
          </ul>
        )}
      </div>

      <Dialog
        open={addOpen}
        onOpenChange={(o) => {
          if (!o) setAddOpen(false)
        }}
      >
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t("addTitle")}</DialogTitle>
          </DialogHeader>
          <div className="space-y-3">
            <div className="space-y-1.5">
              <Label htmlFor="memory-kind">{t("kindLabel")}</Label>
              <div id="memory-kind">
                <Select
                  value={kind}
                  onValueChange={(v) => setKind(v as LoopMemoryKind)}
                >
                  <SelectTrigger className="h-8">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {MEMORY_KINDS.map((k) => (
                      <SelectItem key={k} value={k}>
                        {tKind(k)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="memory-title">{t("titleLabel")}</Label>
              <Input
                id="memory-title"
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder={t("titlePlaceholder")}
                autoFocus
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="memory-content">{t("contentLabel")}</Label>
              <Textarea
                id="memory-content"
                value={content}
                onChange={(e) => setContent(e.target.value)}
                placeholder={t("contentPlaceholder")}
                rows={4}
              />
            </div>
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              type="button"
              onClick={() => setAddOpen(false)}
              disabled={creating}
            >
              {tCommon("cancel")}
            </Button>
            <Button
              type="button"
              onClick={create}
              disabled={creating || !title.trim()}
            >
              {creating && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {t("create")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
