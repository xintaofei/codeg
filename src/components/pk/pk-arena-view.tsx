"use client"

import { memo, useEffect, useMemo, useState } from "react"
import { useLocale, useTranslations } from "next-intl"
import { toast } from "sonner"
import { ExternalLink } from "lucide-react"
import { LiveTranscriptView } from "@/components/message/live-transcript-view"
import { PkDiffView } from "@/components/pk/pk-diff-view"
import { PkJudgePanel } from "@/components/pk/pk-judge-panel"
import { PkScoreboard } from "@/components/pk/pk-scoreboard"
import { PkHistoryPicker } from "@/components/pk/pk-history-picker"
import { getEffortControl } from "@/components/pk/pk-arena-policy"
import { usePkRound } from "@/hooks/use-pk-round"
import { AgentIcon } from "@/components/agent-icon"
import { getAgentLabel } from "@/lib/custom-agents"
import { buildPkReportHtml } from "@/lib/pk-report"
import { preparePkReportData } from "@/lib/pk-report-data"
import { savePkReportHtml } from "@/lib/pk-report-export"
import { openPkRoundWindow } from "@/lib/api"
import type { PkContestant, PkRound } from "@/stores/pk-arena-store"
import { cn } from "@/lib/utils"
import { usePkArenaStore } from "@/stores/pk-arena-store"
import { useTabActions } from "@/stores/tab-store"

/**
 * The arena itself: scoreboard on top, one live transcript column per
 * contestant, a diff tab once the round settles, and a share button that
 * saves a complete self-contained HTML battle report. Round switching comes from the store's
 * The view is keyed by `roundId`, so multiple rounds can stay open in separate
 * workspace tabs or split groups without fighting over one global active id.
 */

export function PkArenaView({
  roundId,
  tabId,
}: {
  roundId: string
  tabId: string
}) {
  const t = useTranslations("PkArena.arena")
  const tWindow = useTranslations("SkillsSettings.actions")
  const locale = useLocale()
  const setPillDismissed = usePkArenaStore((s) => s.setPillDismissed)
  const rounds = usePkArenaStore((s) => s.rounds)
  const { closeTab } = useTabActions()

  const round = useMemo(
    () => rounds.find((r) => r.id === roundId) ?? null,
    [rounds, roundId]
  )

  const {
    cancelRound,
    cleanupRound,
    fetchDiff,
    disconnectFinished,
    startPrompt,
    sendFollowUp,
    applyContestantSelection,
    runJudge,
  } = usePkRound()
  const markRound = usePkArenaStore((s) => s.markRound)
  const [tab, setTab] = useState<"battle" | "diff">("battle")
  const [reportExporting, setReportExporting] = useState(false)
  const [diffLoading, setDiffLoading] = useState(false)

  // Literal keys — next-intl's typed messages reject dynamic concatenation.
  const roundStatusLabel = useMemo(
    () => ({
      ready: t("roundStatus.ready"),
      running: t("roundStatus.running"),
      finished: t("roundStatus.finished"),
      canceled: t("roundStatus.canceled"),
      interrupted: t("roundStatus.interrupted"),
    }),
    [t]
  )
  const tabLabel = useMemo(
    () => ({ battle: t("tabs.battle"), diff: t("tabs.diff") }) as const,
    [t]
  )

  // Diff tab: fetch each contestant's worktree diff once per visit.
  useEffect(() => {
    if (tab !== "diff" || !round) return
    let cancelled = false
    const pending = round.contestants.filter(
      (c) => c.diff == null && c.worktreePath
    )
    if (pending.length === 0) return
    setDiffLoading(true)
    void Promise.allSettled(pending.map((c) => fetchDiff(round, c))).then(
      () => {
        if (!cancelled) setDiffLoading(false)
      }
    )
    return () => {
      cancelled = true
    }
  }, [tab, round, fetchDiff])

  // 状态自愈:任何原因导致回合停在 ready/running 而选手已全部结算
  // (settle 事件漏一帧、重启后回放等),打开竞技场时立即收敛到 finished
  // 并断开残留连接——否则顶部状态永远停在"就绪"。
  useEffect(() => {
    if (!round) return
    const settled = (s: PkContestant["status"]) =>
      s === "done" || s === "error" || s === "canceled"
    if (round.status === "ready" || round.status === "running") {
      if (
        round.contestants.length > 0 &&
        round.contestants.every((c) => settled(c.status))
      ) {
        markRound(round.id, "finished")
        void disconnectFinished(round)
      }
    }
  }, [round, markRound, disconnectFinished])

  const handleExportReport = async () => {
    if (!round || reportExporting) return
    setReportExporting(true)
    try {
      const fresh = usePkArenaStore
        .getState()
        .rounds.find((r) => r.id === round.id)
      const reportData = await preparePkReportData(fresh ?? round)
      const html = buildPkReportHtml(
        reportData.round,
        reportData.artifactsBySlot,
        locale
      )
      const result = await savePkReportHtml(html, round.id)
      if (result === "saved") toast.success(t("reportSaved"))
    } catch (error) {
      toast.error(t("reportFailed", { message: String(error) }))
    } finally {
      setReportExporting(false)
    }
  }

  const roundLive = round != null && round.status === "running"

  return (
    <div className="flex h-full min-h-0 w-full flex-col overflow-hidden bg-background">
      {round ? (
        <div className="flex h-full min-h-0 flex-col">
          <div className="flex items-center gap-3 border-b border-border px-4 py-2 pr-12">
            <span
              className="inline-flex h-5 shrink-0 items-center rounded border border-amber-500/35 bg-amber-500/10 px-1.5 font-mono text-[10px] font-bold tracking-[0.12em] text-amber-700 dark:text-amber-300"
              title={t("title")}
              aria-label={t("title")}
            >
              PK
            </span>
            <div className="min-w-0 flex-1">
              <div className="truncate text-sm font-semibold text-foreground">
                {round.task}
              </div>
              <div className="text-xs text-muted-foreground">
                {roundStatusLabel[round.status]} ·{" "}
                {new Date(round.createdAt).toLocaleString()}
              </div>
            </div>
            <PkHistoryPicker activeRound={round} />
            {round.status === "ready" || roundLive ? (
              <button
                type="button"
                onClick={() => void cancelRound(round)}
                className="rounded-md border border-border px-3 py-1 text-xs text-foreground hover:bg-muted"
              >
                {t("cancelRound")}
              </button>
            ) : round.contestants.some((c) => c.worktreePath) ? (
              <button
                type="button"
                onClick={() =>
                  void cleanupRound(round, true).catch((error) =>
                    toast.error(t("reportFailed", { message: String(error) }))
                  )
                }
                className="rounded-md border border-border px-3 py-1 text-xs text-foreground hover:bg-muted"
                title={t("cleanupHint")}
              >
                {t("cleanupWorktrees")}
              </button>
            ) : null}
            <button
              type="button"
              onClick={() => usePkArenaStore.getState().setLauncherOpen(true)}
              className="rounded-md border border-border px-3 py-1 text-xs text-foreground hover:bg-muted"
            >
              {t("newRound")}
            </button>
            <button
              type="button"
              onClick={() =>
                void openPkRoundWindow(round.id, round.task).catch((error) =>
                  toast.error(String(error))
                )
              }
              className="inline-flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs text-foreground hover:bg-muted"
              title={tWindow("openInWindow")}
              aria-label={tWindow("openInWindow")}
            >
              <ExternalLink className="size-3.5" />
            </button>
            <button
              type="button"
              onClick={() => {
                setPillDismissed(false)
                closeTab(tabId)
              }}
              className="rounded-md border border-border px-3 py-1 text-xs text-foreground hover:bg-muted"
            >
              {t("minimize")}
            </button>
            <button
              type="button"
              onClick={() => void handleExportReport()}
              disabled={reportExporting}
              className="rounded-md bg-primary px-3 py-1 text-xs font-medium text-primary-foreground disabled:opacity-50"
            >
              {reportExporting ? t("exporting") : t("shareReport")}
            </button>
          </div>

          {round.status === "ready" ? (
            <div className="flex items-center gap-3 border-b border-border bg-amber-500/5 px-4 py-2">
              <span className="min-w-0 flex-1 text-xs text-muted-foreground">
                {t("readyNote")}
              </span>
              <button
                type="button"
                onClick={() => void startPrompt(round)}
                disabled={!round.contestants.some((c) => c.status === "ready")}
                className="rounded-md bg-primary px-4 py-1.5 text-sm font-medium text-primary-foreground disabled:opacity-50"
              >
                {t("startMatch")}
              </button>
            </div>
          ) : null}
          <div>
            <PkScoreboard contestants={round.contestants} />

            {/* Judge verdict panel — shown when a judge is configured. */}
            {round.judgeAgent ? (
              <PkJudgePanel
                judgeStatus={round.judgeStatus}
                judgeResult={round.judgeResult}
                judgeAgent={round.judgeAgent}
                contestants={round.contestants}
                onRerun={
                  round.judgeStatus === "done" || round.judgeStatus === "error"
                    ? () => void runJudge(round)
                    : undefined
                }
              />
            ) : null}
          </div>

          <div className="flex items-center gap-1 border-b border-border px-4">
            {(["battle", "diff"] as const).map((key) => (
              <button
                key={key}
                type="button"
                onClick={() => setTab(key)}
                className={cn(
                  "-mb-px border-b-2 px-3 py-1.5 text-sm",
                  tab === key
                    ? "border-primary font-medium text-foreground"
                    : "border-transparent text-muted-foreground hover:text-foreground"
                )}
              >
                {tabLabel[key]}
              </button>
            ))}
          </div>

          <div className="min-h-0 flex-1 overflow-hidden">
            <div className="flex h-full gap-2 overflow-x-auto p-2">
              {tab === "battle"
                ? round.contestants.map((contestant) =>
                    round.status === "ready" ? (
                      <PkReadyPane
                        key={contestant.slot}
                        round={round}
                        contestant={contestant}
                        onSelect={applyContestantSelection}
                      />
                    ) : (
                      <PkBattlePane
                        key={contestant.slot}
                        contestant={contestant}
                        conversationId={contestant.conversationId}
                        connectionId={contestant.connectionId}
                        agentType={contestant.agentType}
                        task={round.task}
                        statusDetail={contestant.statusDetail}
                        preparingLabel={t("preparing")}
                        followUpLabel={t("followUp")}
                        followUpPlaceholder={t("followUpPlaceholder")}
                        onFollowUp={(message) =>
                          void sendFollowUp(round, contestant, message)
                        }
                      />
                    )
                  )
                : round.contestants.map((contestant) => (
                    <PkDiffView
                      key={contestant.slot}
                      agentType={contestant.agentType}
                      diff={contestant.diff}
                      loading={diffLoading && contestant.diff == null}
                    />
                  ))}
            </div>
          </div>
        </div>
      ) : (
        <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
          {t("noRound")}
        </div>
      )}
    </div>
  )
}

/**
 * One battle column. Memoized on stable props: the dialog re-renders on
 * every contestant store update (status/usage/diff of ANY contestant), and
 * an unmemoized pane re-rendered four streaming markdown transcripts each
 * time — the field-reported arena lag.
 */
const PkBattlePane = memo(function PkBattlePane({
  contestant,
  conversationId,
  connectionId,
  agentType,
  task,
  statusDetail,
  preparingLabel,
  followUpLabel,
  followUpPlaceholder,
  onFollowUp,
}: {
  contestant: PkContestant
  conversationId: number | null
  connectionId: string | null
  agentType: PkContestant["agentType"]
  task: string
  statusDetail: string | null
  preparingLabel: string
  followUpLabel: string
  followUpPlaceholder: string
  onFollowUp: (message: string) => void
}) {
  // The follow-up box shows when the contestant finished its last turn AND
  // its connection is still alive (contextKey set). A disconnected contestant
  // can't receive a new prompt.
  const canFollowUp =
    contestant.status === "done" && contestant.contextKey != null
  const [followUpText, setFollowUpText] = useState("")
  const [sending, setSending] = useState(false)

  const handleSend = async () => {
    const trimmed = followUpText.trim()
    if (!trimmed || sending) return
    setSending(true)
    setFollowUpText("")
    try {
      await onFollowUp(trimmed)
    } finally {
      setSending(false)
    }
  }

  return (
    <div className="flex h-full min-w-80 flex-1 flex-col overflow-hidden rounded-lg border border-border">
      {conversationId != null ? (
        <LiveTranscriptView
          conversationId={conversationId}
          connectionId={connectionId}
          agentType={agentType}
          kickoffText={task}
        />
      ) : (
        <div className="flex flex-1 items-center justify-center px-3 text-xs text-muted-foreground">
          {statusDetail ?? preparingLabel}
        </div>
      )}
      {canFollowUp ? (
        <div className="border-t border-border bg-muted/30 p-2">
          <div className="mb-1 text-[11px] font-medium text-muted-foreground">
            {followUpLabel}
          </div>
          <textarea
            value={followUpText}
            onChange={(e) => setFollowUpText(e.target.value)}
            placeholder={followUpPlaceholder}
            rows={2}
            disabled={sending}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault()
                void handleSend()
              }
            }}
            className="w-full resize-none rounded-md border border-border bg-background px-2 py-1.5 text-xs text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring disabled:opacity-50"
          />
          <div className="mt-1 flex items-center justify-between">
            <span className="text-[10px] text-muted-foreground">⌘↩</span>
            <button
              type="button"
              onClick={() => void handleSend()}
              disabled={!followUpText.trim() || sending}
              className="rounded-md bg-primary px-3 py-1 text-xs font-medium text-primary-foreground disabled:opacity-50"
            >
              {sending ? "…" : followUpLabel}
            </button>
          </div>
        </div>
      ) : null}
    </div>
  )
})

/**
 * 准备阶段的面板:模型 + 思考等级选择器(选项来自握手通告的 configOptions)。
 * 只读竞技场 store 的选项表,变更经 onSelect 直接下发给后端连接。
 */
const PkReadyPane = memo(function PkReadyPane({
  round,
  contestant,
  onSelect,
}: {
  round: PkRound
  contestant: PkContestant
  onSelect: (
    round: PkRound,
    contestant: PkContestant,
    configId: string,
    value: string
  ) => Promise<void>
}) {
  const t = useTranslations("PkArena.arena")
  const effortControl = getEffortControl(
    contestant.effortOptions,
    contestant.effortConfigId
  )
  const effortOptionLabels = {
    off: t("effortOptions.off"),
    minimal: t("effortOptions.minimal"),
    low: t("effortOptions.low"),
    medium: t("effortOptions.medium"),
    high: t("effortOptions.high"),
    max: t("effortOptions.max"),
  } as const
  return (
    <div className="flex h-full min-w-80 flex-1 flex-col overflow-hidden rounded-lg border border-border">
      <div className="flex items-center gap-2.5 border-b border-border bg-muted/30 px-4 py-3">
        <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border bg-background">
          <AgentIcon agentType={contestant.agentType} className="size-5" />
        </span>
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold text-foreground">
            {getAgentLabel(contestant.agentType)}
            {contestant.label ? ` · ${contestant.label}` : ""}
          </div>
          <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <span className="size-1.5 rounded-full bg-amber-400" aria-hidden />
            {t("readyTag")}
          </div>
        </div>
      </div>
      <div className="flex flex-col gap-4 overflow-auto px-4 py-4">
        {contestant.modelOptions.length > 0 && contestant.modelConfigId ? (
          <label className="block">
            <span className="mb-1.5 block text-xs font-medium text-muted-foreground">
              {t("modelLabel")}
            </span>
            <select
              value={contestant.selectedModel ?? ""}
              onChange={(event) =>
                void onSelect(
                  round,
                  contestant,
                  contestant.modelConfigId as string,
                  event.target.value
                )
              }
              className="w-full rounded-md border border-border bg-background px-2 py-1 text-xs text-foreground"
            >
              {contestant.modelOptions.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.name}
                </option>
              ))}
            </select>
          </label>
        ) : null}
        {effortControl.kind === "select" ? (
          <label className="block">
            <span className="mb-1.5 block text-xs font-medium text-muted-foreground">
              {t("effortLabel")}
            </span>
            <select
              value={contestant.selectedEffort ?? ""}
              onChange={(event) =>
                void onSelect(
                  round,
                  contestant,
                  effortControl.configId,
                  event.target.value
                )
              }
              className="w-full rounded-md border border-border bg-background px-2 py-1 text-xs text-foreground"
            >
              {effortControl.options.map((value) => (
                <option key={value} value={value}>
                  {effortOptionLabels[
                    value as keyof typeof effortOptionLabels
                  ] ?? value}
                </option>
              ))}
            </select>
          </label>
        ) : (
          <div>
            <span className="mb-1.5 block text-xs font-medium text-muted-foreground">
              {t("effortLabel")}
            </span>
            <div className="rounded-md border border-dashed border-border bg-muted/30 px-2 py-1.5 text-xs text-muted-foreground">
              {t("effortUnsupported")}
            </div>
          </div>
        )}
      </div>
    </div>
  )
})
