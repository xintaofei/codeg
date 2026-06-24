"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { Download, Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  importCcSwitchModelProviders,
  listImportableCcSwitchModelProviders,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import {
  AGENT_LABELS,
  type CcSwitchModelProviderPreviewItem,
  type CcSwitchModelProviderSkipReason,
} from "@/lib/types"

interface CcSwitchModelProviderImporterProps {
  open: boolean
  onClose: () => void
  onDone: () => Promise<void> | void
}

interface ImportSummary {
  imported: number
  skipped: number
}

const SOURCE_APP_LABELS: Record<string, string> = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini",
  opencode: "OpenCode",
  openclaw: "OpenClaw",
  hermes: "Hermes",
}

export function CcSwitchModelProviderImporter({
  open,
  onClose,
  onDone,
}: CcSwitchModelProviderImporterProps) {
  const t = useTranslations("ModelProviderSettings.ccSwitchImport")
  const [items, setItems] = useState<CcSwitchModelProviderPreviewItem[]>([])
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [sourcePath, setSourcePath] = useState("")
  const [available, setAvailable] = useState(false)
  const [loading, setLoading] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [summary, setSummary] = useState<ImportSummary | null>(null)
  const [overwriteSameName, setOverwriteSameName] = useState(false)

  const selectableIds = useMemo(
    () =>
      items
        .filter(
          (item) =>
            item.importable ||
            (overwriteSameName && item.skipReason === "duplicate_name")
        )
        .map((item) => item.sourceId),
    [items, overwriteSameName]
  )

  const refresh = useCallback(
    async (options?: { preserveSummary?: boolean }) => {
      setLoading(true)
      setError(null)
      if (!options?.preserveSummary) {
        setSummary(null)
      }
      try {
        const result = await listImportableCcSwitchModelProviders()
        setItems(result.items)
        setAvailable(result.available)
        setSourcePath(result.sourcePath)
        setSelected(
          new Set(
            result.items
              .filter((item) => item.importable)
              .map((item) => item.sourceId)
          )
        )
        setOverwriteSameName(false)
      } catch (err) {
        setError(toErrorMessage(err))
      } finally {
        setLoading(false)
      }
    },
    []
  )

  useEffect(() => {
    if (open) {
      void refresh()
    }
  }, [open, refresh])

  const toggle = (sourceId: string) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(sourceId)) next.delete(sourceId)
      else next.add(sourceId)
      return next
    })
  }

  const toggleAll = () => {
    setSelected((prev) => {
      if (prev.size === selectableIds.length) return new Set()
      return new Set(selectableIds)
    })
  }

  const handleImport = async () => {
    setSubmitting(true)
    setError(null)
    try {
      const result = await importCcSwitchModelProviders({
        sourceIds: Array.from(selected),
        overwriteSameName,
      })
      setSummary({
        imported: result.importedIds.length,
        skipped: result.skipped.length,
      })
      await onDone()
      await refresh({ preserveSummary: true })
    } catch (err) {
      setError(toErrorMessage(err))
    } finally {
      setSubmitting(false)
    }
  }

  useEffect(() => {
    setSelected((prev) => {
      const next = new Set(
        Array.from(prev).filter((sourceId) => selectableIds.includes(sourceId))
      )
      if (next.size > 0 || selectableIds.length === 0) {
        return next
      }
      return new Set(selectableIds)
    })
  }, [selectableIds])

  const selectedCount = selected.size

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <DialogContent className="flex h-[min(760px,calc(100vh-4rem))] max-w-[min(960px,calc(100vw-2rem))] flex-col gap-0 overflow-hidden p-0 sm:max-w-4xl">
        <DialogHeader className="border-b px-6 py-4">
          <DialogTitle>{t("title")}</DialogTitle>
          <DialogDescription>{t("description")}</DialogDescription>
        </DialogHeader>

        <div className="flex-1 space-y-4 overflow-hidden px-6 py-4">
          <div className="space-y-1">
            <div className="text-xs font-medium text-muted-foreground">
              {t("sourcePathLabel")}
            </div>
            <div className="truncate rounded-md border bg-muted/40 px-3 py-2 font-mono text-xs text-muted-foreground">
              {sourcePath || t("sourcePathPending")}
            </div>
          </div>

          {loading ? (
            <div className="flex h-full items-center justify-center">
              <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
            </div>
          ) : !available ? (
            <div className="rounded-md border border-dashed px-4 py-6 text-sm text-muted-foreground">
              {t("sourceMissing")}
            </div>
          ) : items.length === 0 ? (
            <div className="rounded-md border border-dashed px-4 py-6 text-sm text-muted-foreground">
              {t("empty")}
            </div>
          ) : (
            <>
              <div className="flex items-center justify-between gap-3">
                <div className="text-xs text-muted-foreground">
                  {t("summary", {
                    total: items.length,
                    importable: selectableIds.length,
                    skipped: items.length - selectableIds.length,
                  })}
                </div>
                <div className="flex items-center gap-4">
                  <label className="flex items-center gap-2 text-xs text-muted-foreground">
                    <Checkbox
                      checked={overwriteSameName}
                      aria-label={t("overwriteSameName")}
                      onCheckedChange={(checked) =>
                        setOverwriteSameName(Boolean(checked))
                      }
                    />
                    <span>{t("overwriteSameName")}</span>
                  </label>
                  <Button
                    type="button"
                    size="sm"
                    variant="ghost"
                    onClick={toggleAll}
                    disabled={selectableIds.length === 0}
                  >
                    {selectedCount === selectableIds.length
                      ? t("clearSelection")
                      : t("selectAll")}
                  </Button>
                </div>
              </div>

              <div className="text-xs text-muted-foreground">
                {t("overwriteSameNameHint")}
              </div>

              <div className="flex-1 space-y-2 overflow-auto pr-1">
                {items.map((item) => {
                  const checked = selected.has(item.sourceId)
                  const selectable =
                    item.importable ||
                    (overwriteSameName && item.skipReason === "duplicate_name")
                  const disabled = !selectable || submitting
                  return (
                    <label
                      key={item.sourceId}
                      className={`flex items-start gap-3 rounded-md border px-3 py-3 transition-colors ${
                        disabled
                          ? "cursor-default"
                          : "cursor-pointer hover:bg-accent/40"
                      }`}
                    >
                      <Checkbox
                        checked={checked}
                        disabled={disabled}
                        aria-label={item.name}
                        onCheckedChange={() => {
                          if (!disabled) toggle(item.sourceId)
                        }}
                        className="mt-0.5"
                      />
                      <div className="min-w-0 flex-1 space-y-2">
                        <div className="flex flex-wrap items-center gap-2">
                          <div className="min-w-0 text-sm font-medium">
                            <span className="break-all">{item.name}</span>
                          </div>
                          <Badge variant="secondary" className="text-[10px]">
                            {SOURCE_APP_LABELS[item.sourceAppType] ??
                              item.sourceAppType}
                          </Badge>
                          <Badge variant="outline" className="text-[10px]">
                            {AGENT_LABELS[item.targetAgentType] ??
                              item.targetAgentType}
                          </Badge>
                          {!item.importable && item.skipReason ? (
                            <Badge
                              variant="destructive"
                              className="text-[10px]"
                            >
                              {skipReasonLabel(t, item.skipReason)}
                            </Badge>
                          ) : null}
                        </div>

                        <div className="space-y-1.5 text-xs text-muted-foreground">
                          <div className="truncate font-mono">
                            {item.apiUrl ?? t("missingApiUrl")}
                          </div>
                          <div className="break-all font-mono">
                            {item.model ?? t("missingModel")}
                          </div>
                          <div className="font-mono text-[11px]">
                            {item.sourceId}
                          </div>
                        </div>
                      </div>
                    </label>
                  )
                })}
              </div>
            </>
          )}

          {error ? (
            <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
              {error}
            </div>
          ) : null}
          {summary ? (
            <div className="rounded-md border border-primary/40 bg-primary/10 px-3 py-2 text-xs text-primary">
              {t("result", {
                imported: summary.imported,
                skipped: summary.skipped,
              })}
            </div>
          ) : null}
        </div>

        <DialogFooter className="border-t px-6 py-4">
          <Button type="button" variant="ghost" onClick={onClose}>
            {t("close")}
          </Button>
          <Button
            type="button"
            onClick={handleImport}
            disabled={
              !available || loading || submitting || selectedCount === 0
            }
          >
            {submitting ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <Download className="mr-2 h-4 w-4" />
            )}
            {t("importSelected")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function skipReasonLabel(
  t: ReturnType<typeof useTranslations>,
  reason: CcSwitchModelProviderSkipReason
): string {
  switch (reason) {
    case "unsupported_app_type":
      return t("skipReasons.unsupportedAppType")
    case "missing_name":
      return t("skipReasons.missingName")
    case "missing_api_url":
      return t("skipReasons.missingApiUrl")
    case "missing_api_key":
      return t("skipReasons.missingApiKey")
    case "invalid_model":
      return t("skipReasons.invalidModel")
    case "duplicate_name":
      return t("skipReasons.duplicateName")
    case "duplicate_config":
      return t("skipReasons.duplicateConfig")
    case "malformed_source":
      return t("skipReasons.malformedSource")
  }
}
