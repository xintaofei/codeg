"use client"

import { useCallback, useEffect, useState } from "react"
import { Loader2, Save } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { getChatCommandPrefix, setChatCommandPrefix } from "@/lib/api"

const BUILT_IN_COMMANDS = [
  { name: "recent", descKey: "recentDesc" },
  { name: "search <keyword>", descKey: "searchDesc" },
  { name: "detail <id>", descKey: "detailDesc" },
  { name: "today", descKey: "todayDesc" },
  { name: "status", descKey: "statusDesc" },
  { name: "help", descKey: "helpDesc" },
] as const

export function ChannelCommandsTab() {
  const t = useTranslations("ChatChannelSettings.commands")
  const [prefix, setPrefix] = useState("/")
  const [inputPrefix, setInputPrefix] = useState("/")
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    getChatCommandPrefix()
      .then((p) => {
        setPrefix(p)
        setInputPrefix(p)
      })
      .catch(() => {})
      .finally(() => setLoading(false))
  }, [])

  const handleSavePrefix = useCallback(async () => {
    const trimmed = inputPrefix.trim()
    if (
      trimmed.length === 0 ||
      trimmed.length > 3 ||
      /[a-zA-Z0-9]/.test(trimmed)
    ) {
      toast.error(t("prefixInvalid"))
      return
    }
    setSaving(true)
    try {
      await setChatCommandPrefix(trimmed)
      setPrefix(trimmed)
      toast.success(t("prefixSaved"))
    } catch {
      toast.error(t("prefixSaveFailed"))
    } finally {
      setSaving(false)
    }
  }, [inputPrefix, t])

  const dirty = inputPrefix !== prefix

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center text-sm text-muted-foreground gap-2">
        <Loader2 className="h-4 w-4 animate-spin" />
      </div>
    )
  }

  return (
    <div className="space-y-6">
      <section className="space-y-2">
        <h3 className="text-sm font-medium">{t("prefixLabel")}</h3>
        <p className="text-xs text-muted-foreground">
          {t("prefixDescription")}
        </p>
        <div className="flex items-center gap-2">
          <Input
            value={inputPrefix}
            onChange={(e) => setInputPrefix(e.target.value)}
            className="w-20 text-center font-mono"
            maxLength={3}
          />
          <Button
            size="sm"
            disabled={!dirty || saving}
            onClick={handleSavePrefix}
          >
            {saving ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Save className="h-3.5 w-3.5 mr-1" />
            )}
            {t("save")}
          </Button>
        </div>
      </section>

      <section className="space-y-2">
        <h3 className="text-sm font-medium">{t("title")}</h3>
        <p className="text-xs text-muted-foreground">{t("description")}</p>
        <div className="space-y-1">
          {BUILT_IN_COMMANDS.map((cmd) => (
            <div
              key={cmd.name}
              className="flex items-center justify-between rounded-lg border bg-card px-4 py-3"
            >
              <code className="text-sm font-mono">
                {prefix}
                {cmd.name}
              </code>
              <span className="text-xs text-muted-foreground">
                {t(cmd.descKey)}
              </span>
            </div>
          ))}
        </div>
      </section>
    </div>
  )
}
