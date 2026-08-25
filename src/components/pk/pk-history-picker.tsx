"use client"

import { useMemo, useState } from "react"
import { Archive, ChevronRight, History, Search, Trophy } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import { cn } from "@/lib/utils"
import { assignJudgeScoreSlots, contestantForJudgeScore } from "@/lib/pk-judge"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useTabActions } from "@/contexts/tab-context"
import { usePkArenaStore, type PkRound } from "@/stores/pk-arena-store"

const STATUS_TONE: Record<PkRound["status"], string> = {
  ready: "bg-amber-500",
  running: "bg-emerald-500",
  finished: "bg-sky-500",
  canceled: "bg-muted-foreground",
  interrupted: "bg-orange-500",
}

export function PkHistoryPicker({ activeRound }: { activeRound: PkRound }) {
  const t = useTranslations("PkArena.history")
  const rounds = usePkArenaStore((s) => s.rounds)
  const setActiveRound = usePkArenaStore((s) => s.setActiveRound)
  const archiveRound = usePkArenaStore((s) => s.archiveRound)
  const refreshConversations = useAppWorkspaceStore(
    (s) => s.refreshConversations
  )
  const conversations = useAppWorkspaceStore((s) => s.conversations)
  const { closeConversationTab, closePkRoundTab, openPkRoundTab } =
    useTabActions()
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState("")
  const [archivingId, setArchivingId] = useState<string | null>(null)

  const filtered = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase()
    if (!needle) return rounds
    return rounds.filter((round) => {
      const agents = round.contestants.map((c) => c.agentType).join(" ")
      return `${round.task} ${agents}`.toLocaleLowerCase().includes(needle)
    })
  }, [query, rounds])

  const statusLabel = (status: PkRound["status"]) =>
    ({
      ready: t("status.ready"),
      running: t("status.running"),
      finished: t("status.finished"),
      canceled: t("status.canceled"),
      interrupted: t("status.interrupted"),
    })[status]

  const handleArchive = async (round: PkRound) => {
    if (!window.confirm(t("archiveConfirm", { task: round.task }))) return
    setArchivingId(round.id)
    try {
      await archiveRound(round.id)
      closePkRoundTab(round.id)
      for (const conversation of conversations) {
        if (conversation.pk_round_id !== Number(round.id)) continue
        closeConversationTab(
          conversation.folder_id,
          conversation.id,
          conversation.agent_type
        )
      }
      await refreshConversations()
      const remaining = usePkArenaStore.getState().rounds
      if (round.id === activeRound.id) {
        if (remaining[0]) setActiveRound(remaining[0].id)
      }
      toast.success(t("archiveSuccess"))
    } catch (error) {
      toast.error(t("archiveFailed", { message: String(error) }))
    } finally {
      setArchivingId(null)
    }
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="inline-flex items-center gap-1.5 rounded-md border border-border bg-background px-2.5 py-1 text-xs text-foreground hover:bg-muted"
          aria-label={t("title")}
        >
          <History className="size-3.5" />
          {t("trigger", { count: rounds.length })}
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        className="w-[min(30rem,calc(100vw-2rem))] p-0"
      >
        <div className="border-b border-border p-3">
          <div className="mb-2">
            <div className="text-sm font-semibold">{t("title")}</div>
            <div className="text-xs text-muted-foreground">{t("hint")}</div>
          </div>
          <label className="flex items-center gap-2 rounded-md border border-border bg-background px-2.5 py-1.5">
            <Search className="size-3.5 text-muted-foreground" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("searchPlaceholder")}
              className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
              autoFocus
            />
          </label>
        </div>
        <div className="max-h-[min(32rem,60vh)] overflow-y-auto p-1.5">
          {filtered.length === 0 ? (
            <div className="px-3 py-8 text-center text-sm text-muted-foreground">
              {t("empty")}
            </div>
          ) : (
            filtered.map((round) => {
              const winner = assignJudgeScoreSlots(
                round.judgeResult?.scores ?? [],
                round.contestants.filter(
                  (contestant) => contestant.status === "done"
                )
              ).find((score) => score.rank === 1)
              const winnerContestant = winner
                ? contestantForJudgeScore(winner, round.contestants)
                : undefined
              const active = round.id === activeRound.id
              return (
                <div
                  key={round.id}
                  className={cn(
                    "group flex items-center gap-2 rounded-lg border border-transparent p-2",
                    active ? "border-border bg-muted/70" : "hover:bg-muted/50"
                  )}
                >
                  <button
                    type="button"
                    onClick={() => {
                      setActiveRound(round.id)
                      openPkRoundTab(round.id, round.folderId, round.task)
                      setOpen(false)
                    }}
                    className="min-w-0 flex flex-1 items-center gap-2 text-left"
                  >
                    <span
                      className={cn(
                        "size-2 shrink-0 rounded-full",
                        STATUS_TONE[round.status]
                      )}
                    />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-sm font-medium">
                        {round.task}
                      </span>
                      <span className="flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground">
                        <span>{statusLabel(round.status)}</span>
                        <span>
                          {new Date(round.createdAt).toLocaleString()}
                        </span>
                        <span>
                          {t("agents", { count: round.contestants.length })}
                        </span>
                        {winner ? (
                          <span className="inline-flex items-center gap-1">
                            <Trophy className="size-3" /> {winner.agentType}
                            {winnerContestant?.label
                              ? ` · ${winnerContestant.label}`
                              : ""}
                          </span>
                        ) : null}
                      </span>
                    </span>
                    <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
                  </button>
                  {round.status !== "running" && round.status !== "ready" ? (
                    <button
                      type="button"
                      onClick={() => void handleArchive(round)}
                      disabled={archivingId === round.id}
                      className="rounded-md p-1.5 text-muted-foreground opacity-0 hover:bg-background hover:text-foreground focus:opacity-100 disabled:opacity-50 group-hover:opacity-100"
                      title={t("archive")}
                    >
                      <Archive className="size-3.5" />
                    </button>
                  ) : null}
                </div>
              )
            })
          )}
        </div>
      </PopoverContent>
    </Popover>
  )
}
