"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { Loader2, Pencil, Plus, Server, Trash2 } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
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
import { listModelProviders, deleteModelProvider } from "@/lib/api"
import {
  ALL_AGENT_TYPES,
  AGENT_LABELS,
  type AgentType,
  type ModelProviderInfo,
} from "@/lib/types"
import { AddModelProviderDialog } from "./add-model-provider-dialog"
import { EditModelProviderDialog } from "./edit-model-provider-dialog"

export function ModelProviderSettings() {
  const t = useTranslations("ModelProviderSettings")
  const [providers, setProviders] = useState<ModelProviderInfo[]>([])
  const [loading, setLoading] = useState(true)
  const [filter, setFilter] = useState<AgentType | null>(null)
  const [addDialogOpen, setAddDialogOpen] = useState(false)
  const [editTarget, setEditTarget] = useState<ModelProviderInfo | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<ModelProviderInfo | null>(
    null
  )

  const loadProviders = useCallback(async () => {
    try {
      const rows = await listModelProviders()
      setProviders(rows)
    } catch {
      toast.error(t("loadFailed"))
    } finally {
      setLoading(false)
    }
  }, [t])

  useEffect(() => {
    loadProviders().catch(console.error)
  }, [loadProviders])

  const filteredProviders = useMemo(() => {
    if (!filter) return providers
    return providers.filter((p) => p.agent_types.includes(filter))
  }, [providers, filter])

  const handleDelete = useCallback(async () => {
    if (!deleteTarget) return
    try {
      await deleteModelProvider(deleteTarget.id)
      toast.success(t("deleteSuccess"))
      setDeleteTarget(null)
      await loadProviders()
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      toast.error(msg)
    }
  }, [deleteTarget, loadProviders, t])

  return (
    <div className="h-full overflow-auto">
      <section className="space-y-3">
        <div>
          <h1 className="text-sm font-semibold">{t("sectionTitle")}</h1>
          <p className="text-sm text-muted-foreground">
            {t("sectionDescription")}
          </p>
        </div>
      </section>

      <section className="mt-4 space-y-2">
        <div className="flex items-center justify-between gap-2">
          <Select
            value={filter ?? "__all__"}
            onValueChange={(v) =>
              setFilter(v === "__all__" ? null : (v as AgentType))
            }
          >
            <SelectTrigger className="h-8 w-40 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__all__">{t("filterAll")}</SelectItem>
              {ALL_AGENT_TYPES.map((at) => (
                <SelectItem key={at} value={at}>
                  {AGENT_LABELS[at]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            size="sm"
            className="h-8 text-xs"
            onClick={() => setAddDialogOpen(true)}
          >
            <Plus className="h-3.5 w-3.5 mr-1" />
            {t("addProvider")}
          </Button>
        </div>

        {loading ? (
          <div className="flex items-center justify-center py-8">
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        ) : filteredProviders.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-8 text-muted-foreground">
            <Server className="h-8 w-8 mb-2 opacity-40" />
            <span className="text-xs">{t("noProviders")}</span>
          </div>
        ) : (
          <div className="space-y-2">
            {filteredProviders.map((p) => (
              <div
                key={p.id}
                className="flex items-center justify-between gap-3 rounded-md border px-3 py-2.5"
              >
                <div className="min-w-0 flex-1 space-y-1">
                  <div className="flex items-center gap-1.5 flex-wrap">
                    <span className="text-sm font-medium">{p.name}</span>
                    {p.agent_types.map((at) => (
                      <Badge
                        key={at}
                        variant="secondary"
                        className="text-[10px] px-1.5 py-0"
                      >
                        {AGENT_LABELS[at as AgentType] ?? at}
                      </Badge>
                    ))}
                  </div>
                  <div className="truncate text-xs text-muted-foreground font-mono">
                    {p.api_url}
                  </div>
                </div>
                <div className="flex shrink-0 gap-1">
                  <Button
                    size="icon"
                    variant="ghost"
                    className="h-7 w-7"
                    onClick={() => setEditTarget(p)}
                  >
                    <Pencil className="h-3.5 w-3.5" />
                  </Button>
                  <Button
                    size="icon"
                    variant="ghost"
                    className="h-7 w-7 text-destructive"
                    onClick={() => setDeleteTarget(p)}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </div>
              </div>
            ))}
          </div>
        )}
      </section>

      <AddModelProviderDialog
        open={addDialogOpen}
        onOpenChange={setAddDialogOpen}
        onProviderAdded={loadProviders}
      />

      <EditModelProviderDialog
        provider={editTarget}
        onOpenChange={(open) => {
          if (!open) setEditTarget(null)
        }}
        onProviderUpdated={loadProviders}
      />

      <AlertDialog
        open={!!deleteTarget}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("deleteConfirmTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("deleteConfirmMessage", { name: deleteTarget?.name ?? "" })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={handleDelete}>
              {t("delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
