"use client"

import { useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { Swords, X } from "lucide-react"
import { getArenaPillRound } from "@/components/pk/pk-arena-policy"
import { cn } from "@/lib/utils"
import { usePkArenaStore } from "@/stores/pk-arena-store"
import { useTabStore } from "@/stores/tab-store"

/**
 * 竞技场最小化胶囊。大窗关闭后,只要还有进行中(ready/running)的回合,
 * 右下角常驻这个小胶囊:显示 ⚔ + 已完成/总数,随时点开回到全屏——
 * 比赛在后台继续,用户该干嘛干嘛。手动 ✕ 只把它藏起来,回合不受影响;
 * 左上角 ⚔ 或新回合会自动把它唤回来。
 */
export function PkMinimizedPill() {
  const t = useTranslations("PkArena.minimized")
  const rounds = usePkArenaStore((s) => s.rounds)
  const activeRoundId = usePkArenaStore((s) => s.activeRoundId)
  const pillDismissed = usePkArenaStore((s) => s.pillDismissed)
  const setPillDismissed = usePkArenaStore((s) => s.setPillDismissed)
  const activeTabId = useTabStore((s) => s.activeTabId)
  const openPkRoundTab = useTabStore((s) => s.openPkRoundTab)

  const round = getArenaPillRound(rounds, activeRoundId)
  const isRoundTabActive =
    round != null && activeTabId === `pk-round-${round.id}`
  const roundLive = round?.status === "ready" || round?.status === "running"
  const tStatus = useTranslations("PkArena.arena.roundStatus")
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!round || !roundLive || isRoundTabActive) return
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [round, roundLive, isRoundTabActive])

  const visible =
    round != null && roundLive && !isRoundTabActive && !pillDismissed
  if (!visible || !round) {
    return null
  }

  const done = round.contestants.filter(
    (c) =>
      c.status === "done" || c.status === "error" || c.status === "canceled"
  ).length
  const total = round.contestants.length
  const elapsed = Math.round((now - round.createdAt) / 1000)

  return (
    <div
      className="fixed bottom-16 right-4 z-50 flex items-center gap-2 rounded-full border border-border bg-card px-3 py-1.5 shadow-lg"
      data-testid="pk-minimized-pill"
    >
      <button
        type="button"
        onClick={() => {
          setPillDismissed(false)
          openPkRoundTab(round.id, round.folderId, round.task)
        }}
        className="flex items-center gap-2 text-sm text-foreground hover:opacity-80"
        title={t("restore")}
      >
        <Swords className="size-4 text-primary" />
        <span className="tabular-nums font-medium">
          {done}/{total}
        </span>
        <span className="hidden text-xs text-muted-foreground sm:inline">
          {tStatus(round.status)}
          {roundLive && elapsed > 0 ? ` · ${elapsed}s` : ""}
        </span>
        <span
          className={cn(
            "size-2 rounded-full",
            round.status === "ready" && "bg-amber-500",
            round.status === "running" && "animate-pulse bg-emerald-500",
            round.status === "finished" && "bg-emerald-500",
            round.status === "canceled" && "bg-muted-foreground/60",
            round.status === "interrupted" && "bg-orange-500"
          )}
          aria-hidden
        />
      </button>
      <button
        type="button"
        onClick={() => setPillDismissed(true)}
        className="text-muted-foreground hover:text-foreground"
        title={t("dismiss")}
        aria-label={t("dismiss")}
      >
        <X className="size-3.5" />
      </button>
    </div>
  )
}
