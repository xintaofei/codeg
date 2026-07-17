"use client"

import { FileText } from "lucide-react"
import { useTranslations } from "next-intl"
import { Switch } from "@/components/ui/switch"
import { useMarkdownPreviewPreferences } from "@/hooks/use-appearance"

export function MarkdownPreviewSettingsSection() {
  const t = useTranslations("AppearanceSettings.markdownPreview")
  const {
    markdownPreviewPreserveLineBreaks,
    setMarkdownPreviewPreserveLineBreaks,
  } = useMarkdownPreviewPreferences()

  return (
    <section className="rounded-xl border bg-card p-4 space-y-4">
      <div className="flex items-center gap-2">
        <FileText className="h-4 w-4 text-muted-foreground" />
        <h2 className="text-sm font-semibold">{t("sectionTitle")}</h2>
      </div>

      <p className="text-xs text-muted-foreground leading-5">
        {t("sectionDescription")}
      </p>

      <label className="flex items-center gap-2">
        <Switch
          checked={markdownPreviewPreserveLineBreaks}
          onCheckedChange={setMarkdownPreviewPreserveLineBreaks}
        />
        <span className="text-xs text-muted-foreground">
          {t("preserveLineBreaks")}
        </span>
      </label>
    </section>
  )
}
