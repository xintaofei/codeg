"use client"

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { getSearchIndexStatus, setSearchSettings } from "@/lib/api"
import { toast } from "sonner"
import type { SearchIndexStatus } from "@/lib/types"

export function SearchSettingsSection() {
  const t = useTranslations("SettingsPages")
  const [status, setStatus] = useState<SearchIndexStatus | null>(null)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    let cancelled = false
    getSearchIndexStatus()
      .then((next) => {
        if (!cancelled) setStatus(next)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [t])

  const save = useCallback(
    async (enabled: boolean, mode: "auto" | "scan" | "fts") => {
      setSaving(true)
      try {
        await setSearchSettings(enabled, mode)
        const next = await getSearchIndexStatus()
        setStatus(next)
      } catch {
        toast.error(t("searchSaveFailed"))
      } finally {
        setSaving(false)
      }
    },
    [t]
  )

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div>
          <div className="text-sm font-medium">{t("searchEnabled")}</div>
          <div className="text-xs text-muted-foreground">
            {status?.indexed_conversation_count ?? 0} /{" "}
            {status?.visible_conversation_count ?? 0}
          </div>
        </div>
        <Switch
          checked={status?.user_enabled ?? true}
          disabled={saving || !status}
          onCheckedChange={(enabled) => {
            void save(enabled, status?.user_mode ?? "auto")
          }}
        />
      </div>

      <Select
        value={status?.user_mode ?? "auto"}
        disabled={saving || !status}
        onValueChange={(value) => {
          void save(
            status?.user_enabled ?? true,
            value as "auto" | "scan" | "fts"
          )
        }}
      >
        <SelectTrigger className="w-full">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="auto">{t("searchModeAuto")}</SelectItem>
          <SelectItem value="scan">{t("searchModeScan")}</SelectItem>
          <SelectItem value="fts">{t("searchModeFts")}</SelectItem>
        </SelectContent>
      </Select>
      {saving && (
        <div className="text-xs text-muted-foreground">{t("searchSaving")}</div>
      )}
    </div>
  )
}
