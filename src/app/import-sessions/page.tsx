"use client"

import { Suspense, useCallback, useEffect } from "react"
import { useSearchParams } from "next/navigation"
import { useTranslations } from "next-intl"
import { AppTitleBar } from "@/components/layout/app-title-bar"
import { AppToaster } from "@/components/ui/app-toaster"
import { ImportSessionsWindow } from "@/components/import-sessions/import-sessions-window"
import { RemoteConnectionGate } from "@/contexts/remote-connection-context"
import { isDesktop } from "@/lib/platform"

const TOAST_DURATION_MS = 6000

function ImportSessionsPageInner() {
  const t = useTranslations("ImportSessions")
  const searchParams = useSearchParams()
  const focusPath = searchParams.get("focusPath")

  const closeWindow = useCallback(async () => {
    if (isDesktop()) {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window")
        await getCurrentWindow().close()
        return
      } catch (err) {
        console.error("[ImportSessionsPage] failed to close window:", err)
      }
    }
    window.close()
  }, [])

  useEffect(() => {
    document.title = `${t("title")} - codeg`
  }, [t])

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-background text-foreground">
      <AppTitleBar
        center={
          <div className="text-sm font-semibold tracking-tight">
            {t("title")}
          </div>
        }
      />
      <main className="min-h-0 flex-1">
        <ImportSessionsWindow
          focusPath={focusPath}
          onClose={() => void closeWindow()}
        />
      </main>
      <AppToaster
        position="bottom-right"
        duration={TOAST_DURATION_MS}
        closeButton
      />
    </div>
  )
}

export default function ImportSessionsPage() {
  return (
    <Suspense>
      <RemoteConnectionGate>
        <ImportSessionsPageInner />
      </RemoteConnectionGate>
    </Suspense>
  )
}
