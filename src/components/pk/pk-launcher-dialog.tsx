"use client"

import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog"
import { AgentIcon } from "@/components/agent-icon"
import { Loader2, RefreshCw, X } from "lucide-react"
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { useAgentOptions } from "@/components/automations/use-agent-options"
import { useAcpAgents } from "@/hooks/use-acp-agents"
import {
  acpGetAgentStatus,
  getFolder,
  getGitBranch,
  gitInit,
  gitLog,
} from "@/lib/api"
import { getAgentLabel } from "@/lib/custom-agents"
import { PK_TEMPLATES } from "@/lib/pk-templates"
import type {
  AgentType,
  GitLogEntry,
  SessionConfigOptionInfo,
} from "@/lib/types"
import { cn } from "@/lib/utils"
import {
  loadLastLauncherConfig,
  saveLastLauncherConfig,
  usePkArenaStore,
  type PkEffortLevel,
  type PkPermissionMode,
} from "@/stores/pk-arena-store"
import { useTabStore } from "@/stores/tab-store"

/**
 * Arena launcher: pick 2-8 installed agents, write the task, start the round.
 * Reads the ACTIVE tab for the target folder (an arena needs a real folder —
 * its git repo provides the per-contestant worktrees; chat mode has none).
 */

const MIN_CONTESTANTS = 2
const MAX_CONTESTANTS = 8

interface LauncherSlot {
  id: number
  agentType: AgentType
  label: string
  configValues: Record<string, string>
}

export function PkLauncherDialog() {
  const t = useTranslations("PkArena.launcher")
  const open = usePkArenaStore((s) => s.launcherOpen)
  const setLauncherOpen = usePkArenaStore((s) => s.setLauncherOpen)
  const createRound = usePkArenaStore((s) => s.createRound)
  const openPkRoundTab = useTabStore((s) => s.openPkRoundTab)
  const { agents: rawAgents } = useAcpAgents()
  const nextSlotId = useRef(0)
  const activeTab = useTabStore((s) =>
    s.activeTabId
      ? (s.tabs.find((tab) => tab.id === s.activeTabId) ?? null)
      : null
  )

  const [slots, setSlots] = useState<LauncherSlot[]>([])
  const [task, setTask] = useState("")
  const [workingDir, setWorkingDir] = useState<string | null>(null)
  const [folderId, setFolderId] = useState<number | null>(null)
  // null = unknown (still checking); false disables Start — worktrees need a
  // real git repo, and `git worktree add` in a plain folder fails instantly.
  const [isGitRepo, setIsGitRepo] = useState<boolean | null>(null)
  const [initializing, setInitializing] = useState(false)
  const [permissionMode, setPermissionMode] =
    useState<PkPermissionMode>("default")
  const [bareMode, setBareMode] = useState(false)
  const [effort, setEffort] = useState<PkEffortLevel>("default")
  const [judgeAgent, setJudgeAgent] = useState<string | null>(null)
  const [judgeDimensions, setJudgeDimensions] = useState<string>("")
  const [startError, setStartError] = useState<string | null>(null)
  const [commitPickerOpen, setCommitPickerOpen] = useState(false)
  const [commits, setCommits] = useState<GitLogEntry[]>([])
  const [commitsLoading, setCommitsLoading] = useState(false)
  const [commitSkip, setCommitSkip] = useState(0)
  const [commitsExhausted, setCommitsExhausted] = useState(false)
  /** The commit chosen as the task source. null = start from current HEAD.
   * When set, the worktree branches from `<hash>^` (one commit before), so
   * contestants never see this commit's changes — only its message as the
   * task. */
  const [selectedCommit, setSelectedCommit] = useState<GitLogEntry | null>(null)

  const checkGitRepo = (dir: string, cancelledRef: { current: boolean }) => {
    setIsGitRepo(null)
    getGitBranch(dir)
      .then((branch) => {
        if (!cancelledRef.current) setIsGitRepo(branch != null)
      })
      .catch(() => {
        if (!cancelledRef.current) setIsGitRepo(false)
      })
  }

  useEffect(() => {
    if (!open) return
    setSlots([])
    setTask("")
    setWorkingDir(null)
    setFolderId(null)
    setIsGitRepo(null)
    setPermissionMode("default")
    setBareMode(false)
    setEffort("default")
    setJudgeAgent(null)
    setJudgeDimensions("")
    setStartError(null)
    setCommitPickerOpen(false)
    setSelectedCommit(null)
    // 复赛预填:上次配置的选手若仍可参与则沿用。
    const last = loadLastLauncherConfig()
    if (last && last.agents.length > 0) {
      setSlots((prev) =>
        prev.length > 0
          ? prev
          : last.agents.map((a) => ({
              id: ++nextSlotId.current,
              agentType: a.agentType,
              label: a.label ?? "",
              configValues: a.configValues ?? {},
            }))
      )
      setTask(last.task ?? "")
      setPermissionMode(last.permissionMode)
      setBareMode(last.bareMode)
      setEffort(last.effort)
      setJudgeAgent(last.judgeAgent ?? null)
      setJudgeDimensions(last.judgeDimensions?.join("\n") ?? "")
    }
    // The active tab decides where the arena runs. Draft tabs may lack a
    // workingDir; fall back to the folder's own path.
    if (activeTab?.folderId == null || activeTab.folderId < 0) return
    const cancelled = { current: false }
    const resolve = (id: number, dir: string) => {
      setFolderId(id)
      setWorkingDir(dir)
      checkGitRepo(dir, cancelled)
    }
    if (activeTab.kind === "conversation" && activeTab.workingDir) {
      resolve(activeTab.folderId, activeTab.workingDir)
    } else {
      getFolder(activeTab.folderId)
        .then((folder) => {
          if (!cancelled.current) resolve(folder.id, folder.path)
        })
        .catch(() => undefined)
    }
    return () => {
      cancelled.current = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  const handleInitGit = async () => {
    if (!workingDir || initializing) return
    setInitializing(true)
    try {
      await gitInit(workingDir)
      setIsGitRepo(true)
    } catch {
      setIsGitRepo(false)
    } finally {
      setInitializing(false)
    }
  }

  // 只列真正能跑的:安装到位(installed_version) + 未禁用 + 可用。
  // 未安装的 agent 在 connect 的 preflight 会被拦,但那时回合/会话已创建,
  // 留下的宿主会话会一直空转——所以 PK 干脆只收已就绪的选手。
  const agents = useMemo(
    () =>
      rawAgents.filter(
        (a) => a.enabled && a.available && a.installed_version != null
      ),
    [rawAgents]
  )

  const noFolder = open && folderId == null && activeTab != null
  const taskValid = task.trim().length > 0
  const selectionValid =
    slots.length >= MIN_CONTESTANTS && slots.length <= MAX_CONTESTANTS
  const canStart =
    taskValid &&
    selectionValid &&
    folderId != null &&
    workingDir != null &&
    isGitRepo === true

  const addSlot = (agentType: AgentType) => {
    setSlots((prev) =>
      prev.length >= MAX_CONTESTANTS
        ? prev
        : [
            ...prev,
            {
              id: ++nextSlotId.current,
              agentType,
              label: "",
              configValues: {},
            },
          ]
    )
  }

  const removeSlot = (index: number) => {
    setSlots((prev) => prev.filter((_, i) => i !== index))
  }

  const updateSlotModel = (
    index: number,
    configId: string,
    value: string,
    label: string
  ) => {
    setSlots((prev) =>
      prev.map((slot, i) =>
        i === index
          ? {
              ...slot,
              label,
              configValues: { ...slot.configValues, [configId]: value },
            }
          : slot
      )
    )
  }

  const handleStart = async () => {
    if (!canStart || folderId == null || workingDir == null) return
    // 开赛前预检:任何选手不可用就中止,不建回合、不留残留会话。
    // Deduplicate agent types — the same agent in two slots only needs one check.
    const uniqueAgents = Array.from(new Set(slots.map((s) => s.agentType)))
    for (const agentType of uniqueAgents) {
      try {
        const status = await acpGetAgentStatus(agentType)
        if (!status.enabled || !status.available || !status.installed_version) {
          setStartError(t("agentNotReady", { agent: getAgentLabel(agentType) }))
          return
        }
      } catch {
        setStartError(
          t("agentCheckFailed", { agent: getAgentLabel(agentType) })
        )
        return
      }
    }
    setStartError(null)
    const parsedDimensions = judgeDimensions
      .split("\n")
      .map((d) => d.trim())
      .filter(Boolean)
    const agentsPayload = slots.map((s) =>
      s.label.trim() || Object.keys(s.configValues).length > 0
        ? {
            agentType: s.agentType,
            ...(s.label.trim() ? { label: s.label.trim() } : {}),
            configValues: s.configValues,
          }
        : { agentType: s.agentType }
    )
    saveLastLauncherConfig({
      agents: agentsPayload,
      permissionMode,
      bareMode,
      effort,
      task: task.trim(),
      judgeAgent,
      judgeDimensions: parsedDimensions.length > 0 ? parsedDimensions : null,
    })
    // Selected a commit → worktree branches from its PARENT, so contestants
    // start before that commit and never see its changes. null = current HEAD.
    const baseCommit = selectedCommit ? `${selectedCommit.hash}^` : null
    const round = await createRound({
      task: task.trim(),
      folderId,
      workingDir,
      agents: agentsPayload,
      permissionMode,
      bareMode,
      effort,
      judgeAgent,
      judgeDimensions: parsedDimensions.length > 0 ? parsedDimensions : null,
      baseCommit,
    })
    setLauncherOpen(false)
    openPkRoundTab(round.id, round.folderId, round.task)
    // The orchestrator (in PkArenaHost) picks the new round up from the store.
  }

  return (
    <Dialog open={open} onOpenChange={setLauncherOpen}>
      <DialogContent className="flex max-h-[calc(100dvh-2rem)] w-full max-w-2xl flex-col gap-0 overflow-hidden rounded-2xl p-0">
        <DialogTitle className="border-b border-border px-6 py-4 text-base font-semibold">
          {t("title")}
        </DialogTitle>
        <DialogDescription className="sr-only">
          {t("description")}
        </DialogDescription>
        <div className="min-h-0 flex-1 space-y-6 overflow-y-auto px-6 py-5">
          <section className="rounded-xl border border-border bg-muted/20 p-4">
            <div className="mb-3 flex items-center justify-between gap-3">
              <div className="text-sm font-medium text-foreground">
                {t("contestantsLabel", {
                  min: MIN_CONTESTANTS,
                  max: MAX_CONTESTANTS,
                })}
              </div>
              <span className="text-xs tabular-nums text-muted-foreground">
                {slots.length}/{MAX_CONTESTANTS}
              </span>
            </div>
            {noFolder ? (
              <div className="text-xs text-muted-foreground">
                {t("noFolderHint")}
              </div>
            ) : null}
            {workingDir != null && isGitRepo === false ? (
              <div className="flex items-center gap-2 text-xs text-muted-foreground">
                <span className="min-w-0 flex-1">{t("notAGitRepo")}</span>
                <button
                  type="button"
                  onClick={() => void handleInitGit()}
                  disabled={initializing}
                  className="shrink-0 rounded-md border border-border px-2 py-1 text-xs text-foreground hover:bg-muted disabled:opacity-50"
                >
                  {initializing ? t("initializing") : t("initGitRepo")}
                </button>
              </div>
            ) : null}
            <div className="flex flex-wrap gap-2">
              {agents.map((agent) => {
                const slotsFull = slots.length >= MAX_CONTESTANTS
                return (
                  <button
                    key={agent.agent_type}
                    type="button"
                    onClick={() => addSlot(agent.agent_type)}
                    disabled={slotsFull}
                    aria-label={t("addContestant", { agent: agent.name })}
                    className={cn(
                      "inline-flex items-center gap-1.5 rounded-full border px-3 py-1.5 text-sm transition-colors",
                      "border-border text-muted-foreground hover:bg-muted",
                      "disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent"
                    )}
                  >
                    <AgentIcon
                      agentType={agent.agent_type}
                      className="size-4"
                    />
                    {agent.name}
                    <span className="text-xs text-muted-foreground">+</span>
                  </button>
                )
              })}
            </div>
            {slots.length > 0 ? (
              <div className="mt-4 flex flex-col gap-2">
                {slots.map((slot, index) => {
                  if (!slot.agentType) return null
                  const agentName =
                    agents.find((a) => a.agent_type === slot.agentType)?.name ??
                    slot.agentType
                  return (
                    <div
                      key={slot.id}
                      className="flex items-center gap-3 rounded-xl border border-border bg-background px-3 py-2.5 shadow-xs"
                    >
                      <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted">
                        <AgentIcon
                          agentType={slot.agentType}
                          className="size-4"
                        />
                      </span>
                      <div className="w-32 min-w-0 shrink-0">
                        <div className="truncate text-sm font-medium text-foreground">
                          {agentName}
                        </div>
                        <div className="text-[11px] text-muted-foreground">
                          {t("slotNumber", { number: index + 1 })}
                        </div>
                      </div>
                      <SlotModelSelect
                        agentType={slot.agentType}
                        workingDir={workingDir}
                        configValues={slot.configValues}
                        onChange={(configId, value, label) =>
                          updateSlotModel(index, configId, value, label)
                        }
                        loadingLabel={t("modelLoading")}
                        unavailableLabel={t("modelUnavailable")}
                        failedLabel={t("modelLoadFailed")}
                        retryLabel={t("retryModelLoad")}
                      />
                      <button
                        type="button"
                        onClick={() => removeSlot(index)}
                        aria-label={t("removeSlot")}
                        className="shrink-0 rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                      >
                        <X className="size-3.5" />
                      </button>
                    </div>
                  )
                })}
              </div>
            ) : null}
            {slots.length > 0 && slots.length < MIN_CONTESTANTS ? (
              <div className="mt-2 text-xs text-muted-foreground">
                {t("needMore", { count: MIN_CONTESTANTS })}
              </div>
            ) : null}
          </section>
          <section>
            <label
              htmlFor="pk-task"
              className="mb-2 text-sm font-medium text-foreground"
            >
              {t("taskLabel")}
            </label>
            {/* ── 创意 PK:一键模板 ── */}
            <div className="mb-1 text-xs font-medium text-muted-foreground">
              {t("creativeTemplates")}
            </div>
            <div className="mb-3 flex flex-wrap items-center gap-1.5">
              {PK_TEMPLATES.map((tpl) => (
                <button
                  key={tpl.id}
                  type="button"
                  title={tpl.task}
                  onClick={() => setTask(tpl.task)}
                  className="inline-flex items-center gap-1 rounded-full border border-border px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                >
                  <span>{tpl.emoji}</span>
                  {t(`templates.${tpl.labelKey}` as "templates.pelican")}
                </button>
              ))}
            </div>

            {/* ── 真实工程 PK:起点选择 ── */}
            {workingDir != null && isGitRepo === true ? (
              <>
                <div className="mb-1 text-xs font-medium text-muted-foreground">
                  {t("realEngineering")}
                </div>
                <div className="mb-2 flex flex-wrap items-center gap-1.5">
                  <button
                    type="button"
                    onClick={async () => {
                      if (commitPickerOpen) {
                        setCommitPickerOpen(false)
                        return
                      }
                      setCommitSkip(0)
                      setCommitsExhausted(false)
                      setCommitsLoading(true)
                      setCommitPickerOpen(true)
                      try {
                        const result = await gitLog(workingDir, 10)
                        setCommits(result.entries)
                        setCommitsExhausted(result.entries.length < 10)
                      } catch {
                        setCommits([])
                      } finally {
                        setCommitsLoading(false)
                      }
                    }}
                    className={cn(
                      "inline-flex items-center gap-1 rounded-full border px-2.5 py-1 text-xs transition-colors",
                      commitPickerOpen || selectedCommit
                        ? "border-primary bg-primary/10 text-foreground"
                        : "border-border text-muted-foreground hover:bg-muted hover:text-foreground"
                    )}
                  >
                    {t("startPoint")}
                  </button>
                  {selectedCommit ? (
                    <button
                      type="button"
                      onClick={() => {
                        setSelectedCommit(null)
                        setCommitPickerOpen(false)
                      }}
                      className="inline-flex items-center gap-1 rounded-full border border-border px-2 py-1 text-xs text-muted-foreground hover:bg-muted"
                    >
                      <span className="font-mono">
                        {selectedCommit.hash.slice(0, 7)}
                      </span>
                      <span className="max-w-[180px] truncate">
                        {selectedCommit.message.split("\n")[0]}
                      </span>
                      <X className="size-3 shrink-0" />
                    </button>
                  ) : (
                    <span className="text-xs text-muted-foreground">
                      {t("fromHead")}
                    </span>
                  )}
                </div>
                {commitPickerOpen ? (
                  <div className="mb-2 max-h-48 overflow-auto rounded-lg border border-border bg-background">
                    {commitsLoading ? (
                      <div className="px-3 py-2 text-xs text-muted-foreground">
                        {t("loadingCommits")}
                      </div>
                    ) : commits.length === 0 ? (
                      <div className="px-3 py-2 text-xs text-muted-foreground">
                        {t("noCommits")}
                      </div>
                    ) : (
                      <>
                        {commits.map((commit) => (
                          <button
                            key={commit.hash}
                            type="button"
                            onClick={() => {
                              // Use the commit message as the task so the
                              // contestant gets a clear goal. The worktree will
                              // branch from <hash>^ — one commit BEFORE this —
                              // so the contestant never sees these changes.
                              setTask(commit.message.split("\n")[0])
                              setSelectedCommit(commit)
                              setCommitPickerOpen(false)
                            }}
                            className="block w-full px-3 py-2 text-left hover:bg-muted"
                            title={commit.message}
                          >
                            <div className="flex items-center gap-2">
                              <span className="font-mono text-xs text-muted-foreground">
                                {commit.hash.slice(0, 7)}
                              </span>
                              <span className="truncate text-xs text-foreground">
                                {commit.message.split("\n")[0]}
                              </span>
                            </div>
                            <div className="mt-0.5 flex items-center gap-3 text-[11px] text-muted-foreground">
                              <span>{commit.author}</span>
                              <span>
                                {new Date(commit.date).toLocaleDateString()}
                              </span>
                              {commit.files.length > 0 ? (
                                <span>📄 {commit.files.length} 文件</span>
                              ) : null}
                            </div>
                          </button>
                        ))}
                        {!commitsExhausted ? (
                          <button
                            type="button"
                            onClick={async () => {
                              const nextSkip = commitSkip + 10
                              setCommitsLoading(true)
                              try {
                                const result = await gitLog(
                                  workingDir,
                                  10,
                                  undefined,
                                  undefined,
                                  nextSkip
                                )
                                setCommits((prev) => [
                                  ...prev,
                                  ...result.entries,
                                ])
                                setCommitSkip(nextSkip)
                                setCommitsExhausted(result.entries.length < 10)
                              } catch {
                                // ignore
                              } finally {
                                setCommitsLoading(false)
                              }
                            }}
                            disabled={commitsLoading}
                            className="block w-full border-t border-border px-3 py-2 text-center text-xs text-muted-foreground hover:bg-muted disabled:opacity-50"
                          >
                            {commitsLoading ? "…" : t("loadMore")}
                          </button>
                        ) : null}
                      </>
                    )}
                  </div>
                ) : null}
                {selectedCommit ? (
                  <p className="mb-2 text-[11px] leading-relaxed text-muted-foreground">
                    {t("startPointHint")}
                  </p>
                ) : null}
              </>
            ) : null}
            <textarea
              id="pk-task"
              value={task}
              onChange={(event) => setTask(event.target.value)}
              placeholder={t("taskPlaceholder")}
              rows={5}
              className="w-full resize-none rounded-lg border border-border bg-background px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring"
            />
          </section>
          <section>
            <div className="mb-2 text-sm font-medium text-foreground">
              {t("permissionLabel")}
            </div>
            <div className="flex flex-col gap-1.5">
              {(["default", "acceptEdits", "bypassPermissions"] as const).map(
                (mode) => (
                  <label
                    key={mode}
                    className="flex cursor-pointer items-center gap-2 text-sm text-foreground"
                  >
                    <input
                      type="radio"
                      name="pk-permission"
                      checked={permissionMode === mode}
                      onChange={() => setPermissionMode(mode)}
                      className="accent-foreground"
                    />
                    <span className="font-medium">
                      {t(`permissionOptions.${mode}`)}
                    </span>
                    <span className="text-xs text-muted-foreground">
                      {t(`permissionHints.${mode}`)}
                    </span>
                  </label>
                )
              )}
            </div>
            <div className="mt-1.5 text-xs text-muted-foreground">
              {t("permissionNote")}
            </div>
          </section>
          <section>
            <div className="mb-2 text-sm font-medium text-foreground">
              {t("effortLabel")}
            </div>
            <div className="flex flex-wrap gap-1.5">
              {(["default", "low", "medium", "high", "max"] as const).map(
                (level) => (
                  <button
                    key={level}
                    type="button"
                    onClick={() => setEffort(level)}
                    aria-pressed={effort === level}
                    className={cn(
                      "rounded-full border px-3 py-1 text-xs transition-colors",
                      effort === level
                        ? "border-primary bg-primary/10 text-foreground"
                        : "border-border text-muted-foreground hover:bg-muted"
                    )}
                  >
                    {t(`effortOptions.${level}`)}
                  </button>
                )
              )}
            </div>
            <div className="mt-1.5 text-xs text-muted-foreground">
              {t("effortNote")}
            </div>
          </section>
          <label className="flex cursor-pointer items-start gap-2 text-sm">
            <input
              type="checkbox"
              checked={bareMode}
              onChange={(event) => setBareMode(event.target.checked)}
              className="mt-0.5 accent-foreground"
            />
            <span>
              <span className="font-medium text-foreground">
                {t("bareModeLabel")}
              </span>
              <span className="block text-xs text-muted-foreground">
                {t("bareModeHint")}
              </span>
            </span>
          </label>
          <section>
            <div className="mb-2 text-sm font-medium text-foreground">
              {t("judgeLabel")}
            </div>
            <div className="flex flex-wrap gap-2">
              {/* "No judge" chip */}
              <button
                type="button"
                onClick={() => setJudgeAgent(null)}
                aria-pressed={judgeAgent === null}
                className={cn(
                  "inline-flex items-center gap-1.5 rounded-full border px-3 py-1.5 text-sm transition-colors",
                  judgeAgent === null
                    ? "border-primary bg-primary/10 text-foreground"
                    : "border-border text-muted-foreground hover:bg-muted"
                )}
              >
                {t("judgeNone")}
              </button>
              {agents.map((agent) => {
                const isSelected = judgeAgent === agent.agent_type
                const isContestant = slots.some(
                  (s) => s.agentType === agent.agent_type
                )
                return (
                  <button
                    key={agent.agent_type}
                    type="button"
                    onClick={() =>
                      setJudgeAgent(
                        judgeAgent === agent.agent_type
                          ? null
                          : agent.agent_type
                      )
                    }
                    aria-pressed={isSelected}
                    className={cn(
                      "inline-flex items-center gap-1.5 rounded-full border px-3 py-1.5 text-sm transition-colors",
                      isSelected
                        ? "border-primary bg-primary/10 text-foreground"
                        : "border-border text-muted-foreground hover:bg-muted",
                      isContestant && !isSelected && "opacity-40"
                    )}
                  >
                    <AgentIcon
                      agentType={agent.agent_type}
                      className="size-4"
                    />
                    {agent.name}
                  </button>
                )
              })}
            </div>
            <div className="mt-1.5 text-xs text-muted-foreground">
              {t("judgeHint")}
            </div>
            {judgeAgent != null && (
              <div className="mt-2.5">
                <div className="mb-1.5 text-xs font-medium text-foreground">
                  {t("judgeDimensionsLabel")}
                </div>
                <textarea
                  value={judgeDimensions}
                  onChange={(e) => setJudgeDimensions(e.target.value)}
                  placeholder={t("judgeDimensionsPlaceholder")}
                  rows={3}
                  className="w-full resize-none rounded-lg border border-border bg-background px-3 py-2 text-xs text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring"
                />
                <div className="mt-1 text-xs text-muted-foreground">
                  {t("judgeDimensionsHint")}
                </div>
              </div>
            )}
          </section>
        </div>
        {startError != null ? (
          <div className="border-t border-border px-5 py-2 text-xs text-red-600 dark:text-red-400">
            {startError}
          </div>
        ) : null}
        <div className="flex items-center justify-between gap-3 border-t border-border bg-background px-6 py-3.5">
          <span className="text-xs text-muted-foreground">
            {t("selectedCount", {
              selected: slots.length,
              min: MIN_CONTESTANTS,
              max: MAX_CONTESTANTS,
            })}
          </span>
          <div className="flex gap-2">
            <button
              type="button"
              onClick={() => setLauncherOpen(false)}
              className="rounded-md px-3 py-1.5 text-sm text-muted-foreground hover:bg-muted"
            >
              {t("cancel")}
            </button>
            <button
              type="button"
              disabled={!canStart}
              onClick={() => void handleStart()}
              className="rounded-md bg-primary px-4 py-1.5 text-sm font-medium text-primary-foreground disabled:opacity-50"
            >
              {t("start")}
            </button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}

function findModelOption(
  options: SessionConfigOptionInfo[]
): SessionConfigOptionInfo | null {
  return (
    options.find(
      (option) =>
        option.kind.type === "select" &&
        (option.id === "model" ||
          option.id === "model_id" ||
          option.category === "model")
    ) ?? null
  )
}

function modelValueLabel(
  option: SessionConfigOptionInfo,
  value: string
): string {
  if (option.kind.type !== "select") return value
  for (const group of option.kind.groups) {
    const match = group.options.find((item) => item.value === value)
    if (match) return match.name
  }
  return option.kind.options.find((item) => item.value === value)?.name ?? value
}

function hasModelValue(
  option: SessionConfigOptionInfo,
  value: string
): boolean {
  if (option.kind.type !== "select") return false
  return (
    option.kind.options.some((item) => item.value === value) ||
    option.kind.groups.some((group) =>
      group.options.some((item) => item.value === value)
    )
  )
}

function SlotModelSelect({
  agentType,
  workingDir,
  configValues,
  onChange,
  loadingLabel,
  unavailableLabel,
  failedLabel,
  retryLabel,
}: {
  agentType: AgentType
  workingDir: string | null
  configValues: Record<string, string>
  onChange: (configId: string, value: string, label: string) => void
  loadingLabel: string
  unavailableLabel: string
  failedLabel: string
  retryLabel: string
}) {
  const { snapshot, loading, error, reload } = useAgentOptions(
    agentType,
    workingDir
  )
  const modelOption = useMemo(
    () => findModelOption(snapshot?.config_options ?? []),
    [snapshot]
  )
  const configuredValue = modelOption ? configValues[modelOption.id] : null
  const currentValue =
    modelOption?.kind.type === "select"
      ? configuredValue && hasModelValue(modelOption, configuredValue)
        ? configuredValue
        : modelOption.kind.current_value
      : null

  useEffect(() => {
    if (
      modelOption?.kind.type !== "select" ||
      !currentValue ||
      configValues[modelOption.id] === currentValue
    ) {
      return
    }
    onChange(
      modelOption.id,
      currentValue,
      modelValueLabel(modelOption, currentValue)
    )
  }, [configValues, currentValue, modelOption, onChange])

  if (loading) {
    return (
      <div className="flex min-w-0 flex-1 items-center gap-2 text-xs text-muted-foreground">
        <Loader2 className="size-3.5 animate-spin" aria-hidden />
        <span className="truncate">{loadingLabel}</span>
      </div>
    )
  }
  if (error) {
    return (
      <div className="flex min-w-0 flex-1 items-center gap-2 text-xs text-destructive">
        <span className="min-w-0 flex-1 truncate" title={error}>
          {failedLabel}
        </span>
        <button
          type="button"
          onClick={reload}
          aria-label={retryLabel}
          title={retryLabel}
          className="rounded-md p-1.5 hover:bg-muted"
        >
          <RefreshCw className="size-3.5" />
        </button>
      </div>
    )
  }
  if (!modelOption || modelOption.kind.type !== "select") {
    return (
      <div className="min-w-0 flex-1 text-xs text-muted-foreground">
        {unavailableLabel}
      </div>
    )
  }

  return (
    <Select
      value={currentValue ?? undefined}
      onValueChange={(value) =>
        onChange(modelOption.id, value, modelValueLabel(modelOption, value))
      }
    >
      <SelectTrigger
        size="sm"
        className="min-w-0 flex-1 rounded-lg bg-background"
        aria-label={modelOption.name}
      >
        <SelectValue placeholder={modelOption.name} />
      </SelectTrigger>
      <SelectContent position="popper" align="start">
        {modelOption.kind.groups.length > 0
          ? modelOption.kind.groups.map((group) => (
              <SelectGroup key={group.group}>
                <SelectLabel>{group.name}</SelectLabel>
                {group.options.map((item) => (
                  <SelectItem
                    key={`${group.group}-${item.value}`}
                    value={item.value}
                  >
                    {item.name}
                  </SelectItem>
                ))}
              </SelectGroup>
            ))
          : modelOption.kind.options.map((item) => (
              <SelectItem key={item.value} value={item.value}>
                {item.name}
              </SelectItem>
            ))}
      </SelectContent>
    </Select>
  )
}
