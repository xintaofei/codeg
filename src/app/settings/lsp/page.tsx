"use client"

import { Suspense } from "react"
import { useTranslations } from "next-intl"
import { LspServerSettings } from "@/components/settings/lsp-server-settings"

export default function SettingsLspPage() {
  const t = useTranslations("SettingsPages")

  return (
    <Suspense
      fallback={
        <div className="h-full flex items-center justify-center text-sm text-muted-foreground">
          {t("lspLoading")}
        </div>
      }
    >
      <LspServerSettings />
    </Suspense>
  )
}
