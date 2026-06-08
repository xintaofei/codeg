"use client"

import { ArrowUpCircle } from "lucide-react"
import { useTranslations } from "next-intl"
import { useAppUpdate } from "@/components/providers/update-provider"

function Spinner({ className }: { className?: string }) {
  return (
    <div
      className={`animate-spin rounded-full border-[1.5px] border-current border-t-transparent ${className}`}
    />
  )
}

/**
 * App-wide upgrade indicator in the status bar. The download/install lifecycle
 * lives in the {@link useAppUpdate} provider, so this stays in sync no matter
 * which page started it — and a "Restart to update" prompt surfaces here even
 * when the settings page is closed (the same pattern as VS Code / JetBrains).
 * Renders nothing while idle or errored.
 */
export function StatusBarUpdate() {
  const t = useTranslations("SystemSettings")
  const update = useAppUpdate()
  if (!update) return null

  const { state, isUpdating, restartCountdown, isRestarting, restart } = update

  // A relaunch is in progress (countdown, the backend `restarting` event, or
  // the brief desktop window right after the click) — show progress, never a
  // re-triggerable button.
  if (
    restartCountdown !== null ||
    state.status === "restarting" ||
    isRestarting
  ) {
    const label =
      restartCountdown !== null && restartCountdown > 0
        ? t("restartingIn", { seconds: restartCountdown })
        : t("restarting")
    return (
      <div className="flex items-center gap-1.5">
        <Spinner className="h-3 w-3" />
        <span>{label}</span>
      </div>
    )
  }

  // Downloaded and waiting — the one actionable state, so make it clickable.
  if (state.status === "ready_to_restart") {
    return (
      <button
        onClick={() => void restart()}
        className="flex items-center gap-1.5 text-primary hover:text-primary/80 transition-colors"
      >
        <ArrowUpCircle className="h-3.5 w-3.5" />
        <span>{t("restartToUpdate")}</span>
      </button>
    )
  }

  if (isUpdating) {
    let label: string
    if (state.status === "downloading") {
      const pct =
        state.total && state.total > 0
          ? Math.min(
              100,
              Math.round(((state.downloaded ?? 0) / state.total) * 100)
            )
          : null
      label = pct !== null ? `${t("downloading")} ${pct}%` : t("downloading")
    } else {
      label = t("updating")
    }
    return (
      <div className="flex items-center gap-1.5">
        <Spinner className="h-3 w-3" />
        <span>{label}</span>
      </div>
    )
  }

  return null
}
