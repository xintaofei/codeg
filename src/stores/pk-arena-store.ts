"use client"

import { create } from "zustand"
import type {
  AgentType,
  DbConversationSummary,
  PkRoundConfig,
  PkRoundInfo,
} from "@/lib/types"
import {
  pkRoundCreate,
  pkRoundUpdateStatus,
  pkRoundDelete,
  pkRoundUpdateJudge,
} from "@/lib/api"

/**
 * Agent PK arena — one task, N agents, isolated worktrees, a scoreboard.
 *
 * Pure data layer: no imports from React contexts so every reducer is
 * unit-testable. The orchestrator (`hooks/use-pk-round`) drives the state
 * machine; the view components only read it.
 *
 * Persistence: round metadata (task, agents, config, status) lives in the DB
 * (`pk_round` table). Live-only fields (connectionId, diff, usage) stay in
 * the Zustand store — they are meaningless across restarts. A round that was
 * still running at shutdown is marked `interrupted` on hydration.
 */

export type PkContestantStatus =
  | "preparing"
  | "connecting"
  | "ready"
  | "running"
  | "done"
  | "error"
  | "canceled"

export interface PkContestantUsage {
  inputTokens: number
  outputTokens: number
  turnCount: number
  /** False when the upstream agent emitted assistant turns but did not report
   * token accounting (Qoder currently returns zeroes for some models). */
  tokensReported: boolean
}

/** Unified reasoning-effort request — "default" means each contestant uses its own default. */
export type PkEffortLevel = "default" | "low" | "medium" | "high" | "max"

export interface PkContestant {
  /** Slot index — unique within a round. Same agent can occupy multiple
   * slots (control-variable PK: e.g. slot 0 = Claude·Sonnet, slot 1 =
   * Claude·Opus). The key for all lookups is (roundId, slot), NOT agentType. */
  slot: number
  /** Wire name — NOT unique within a round when the same agent is picked
   * twice (control-variable PK). Use `slot` for identity. */
  agentType: AgentType
  /** Captured display name for the pinned model/config (e.g. "Sonnet"). */
  label: string | null
  /** Config values pinned in the launcher and applied before the first prompt. */
  configValues: Record<string, string>
  /** Advertised model options (handshake `configOptions`), for the arena pickers. */
  modelOptions: Array<{ value: string; name: string }>
  modelConfigId: string | null
  /** Advertised effort option values, for the arena picker. */
  effortOptions: string[]
  effortConfigId: string | null
  selectedModel: string | null
  selectedEffort: string | null
  /** Connections-context key; null until the orchestrator connects. */
  contextKey: string | null
  connectionId: string | null
  conversationId: number | null
  worktreePath: string | null
  branchName: string | null
  status: PkContestantStatus
  statusDetail: string | null
  startedAt: number | null
  endedAt: number | null
  durationMs: number | null
  usage: PkContestantUsage | null
  /** Populated lazily when the Diff tab opens; never persisted (can be huge). */
  diff: string | null
}

export type PkRoundStatus =
  | "ready"
  | "running"
  | "finished"
  | "canceled"
  | "interrupted"

/** Judge verdict for one contestant. */
export interface PkJudgeScore {
  /** Unique contestant slot. Optional only for verdicts saved before slot
   * identity was added; consumers deterministically backfill legacy rows. */
  slot?: number
  agentType: string
  score: number
  rank: number
  comment: string
}

/** Structured verdict from the judge agent. */
export interface PkJudgeResult {
  scores: PkJudgeScore[]
  summary: string
  /** Raw LLM text (kept for debugging / export). */
  rawText: string
}

export type PkJudgeStatus = "idle" | "running" | "done" | "error" | "skipped"

export type PkPersistenceField = "status" | "judge"

export type PkPersistenceErrors = Record<PkPersistenceField, string | null>

/**
 * Round-level permission policy, applied to every contestant right after
 * connect via `setMode` — the ACP-standard mode ids (Claude Code, Codex and
 * Qoder all advertise these exact spellings). Agents that don't advertise a
 * matching mode keep their default and simply ask, as before.
 */
export type PkPermissionMode = "default" | "acceptEdits" | "bypassPermissions"

export interface PkRound {
  /** DB id of the pk_round row, as a string (used in branch names, context keys). */
  id: string
  task: string
  folderId: number
  workingDir: string
  createdAt: number
  status: PkRoundStatus
  permissionMode: PkPermissionMode
  /** Bare mode: contestants are instructed to use no skills at all. */
  bareMode: boolean
  /** Uniform reasoning-effort request, applied to every contestant. */
  effort: PkEffortLevel
  /** Optional judge agent type — reads all diffs and produces a verdict. */
  judgeAgent: string | null
  /** Custom judge evaluation dimensions (null = use defaults). Live copy of
   *  PkRoundConfig.judge_dimensions. */
  judgeDimensions: string[] | null
  /** Git ref each contestant worktree branches from. null = current HEAD.
   *  Set to `X^` when the round sources its task from commit X, so
   *  contestants start before X — they never see X's changes. */
  baseCommit: string | null
  /** Structured judge verdict, persisted with the round for history/report export. */
  judgeResult: PkJudgeResult | null
  /** "idle" → "running" → "done" | "error" | "skipped". Persisted. */
  judgeStatus: PkJudgeStatus
  /** Latest persistence failure per independently saved round field. Live-only. */
  persistenceErrors: PkPersistenceErrors
  contestants: PkContestant[]
}

interface PkArenaState {
  rounds: PkRound[]
  activeRoundId: string | null
  launcherOpen: boolean
  /** The pill was manually dismissed — reset on new round / reopen. */
  pillDismissed: boolean
  /** True while the store is loading rounds from the DB on startup. */
  hydrating: boolean
}

interface PkArenaActions {
  createRound(config: {
    task: string
    folderId: number
    workingDir: string
    agents: Array<{
      agentType: AgentType
      label?: string
      configValues?: Record<string, string>
    }>
    permissionMode?: PkPermissionMode
    bareMode?: boolean
    effort?: PkEffortLevel
    judgeAgent?: string | null
    judgeDimensions?: string[] | null
    baseCommit?: string | null
  }): Promise<PkRound>
  hydrateFromDb(rounds: PkRound[]): void
  updateContestant(
    roundId: string,
    slot: number,
    patch: Partial<PkContestant>
  ): void
  markRound(roundId: string, status: PkRoundStatus): void
  archiveRound(roundId: string): Promise<void>
  retryPersistence(roundId: string): void
  setActiveRound(roundId: string | null): void
  setLauncherOpen(open: boolean): void
  setPillDismissed(dismissed: boolean): void
  updateJudge(
    roundId: string,
    patch: { judgeResult?: PkJudgeResult | null; judgeStatus?: PkJudgeStatus }
  ): void
}

const LAUNCHER_LAST_KEY = "codeg:pk-launcher-last"

const persistenceRevisions = new Map<string, number>()
const persistenceQueues = new Map<string, Promise<void>>()

function persistenceKey(roundId: string, field: PkPersistenceField): string {
  return `${roundId}:${field}`
}

/**
 * Serialize writes for one round field and surface only its latest result.
 * Serialization prevents a slower old request from overwriting a newer DB
 * value; the revision prevents its stale error from replacing newer UI state.
 */
function persistLatest(
  roundId: string,
  field: PkPersistenceField,
  operation: () => Promise<void>
): void {
  const key = persistenceKey(roundId, field)
  const revision = (persistenceRevisions.get(key) ?? 0) + 1
  persistenceRevisions.set(key, revision)
  const previous = persistenceQueues.get(key) ?? Promise.resolve()
  const pending = previous.catch(() => undefined).then(operation)
  persistenceQueues.set(key, pending)

  void pending
    .then(() => {
      if (persistenceRevisions.get(key) !== revision) return
      usePkArenaStore.setState((state) => ({
        rounds: state.rounds.map((round) =>
          round.id === roundId
            ? {
                ...round,
                persistenceErrors: {
                  ...(round.persistenceErrors ?? { status: null, judge: null }),
                  [field]: null,
                },
              }
            : round
        ),
      }))
    })
    .finally(() => {
      if (persistenceQueues.get(key) === pending) {
        persistenceQueues.delete(key)
      }
    })
    .catch((error: unknown) => {
      if (persistenceRevisions.get(key) !== revision) return
      const message = error instanceof Error ? error.message : String(error)
      console.error(
        `[PkArena] failed to persist ${field} for round ${roundId}`,
        error
      )
      usePkArenaStore.setState((state) => ({
        rounds: state.rounds.map((round) =>
          round.id === roundId
            ? {
                ...round,
                persistenceErrors: {
                  ...(round.persistenceErrors ?? { status: null, judge: null }),
                  [field]: message,
                },
              }
            : round
        ),
      }))
    })
}

/** Last launcher config, for one-click prefill on rematch. */
export interface PkLauncherLastConfig {
  agents: Array<{
    agentType: AgentType
    label?: string
    configValues?: Record<string, string>
  }>
  permissionMode: PkPermissionMode
  bareMode: boolean
  effort: PkEffortLevel
  task: string
  judgeAgent?: string | null
  judgeDimensions?: string[] | null
}

export function loadLastLauncherConfig(): PkLauncherLastConfig | null {
  if (typeof window === "undefined") return null
  try {
    const raw = window.localStorage.getItem(LAUNCHER_LAST_KEY)
    if (!raw) return null
    const parsed = JSON.parse(raw) as PkLauncherLastConfig
    // Normalize legacy agents formats. Before the controlled-variable UI (#6),
    // agents were stored as `string[]` (just agent types); normalize each entry
    // to `{ agentType, label? }`. Guard against corrupt/missing agentType.
    if (Array.isArray(parsed.agents)) {
      parsed.agents = parsed.agents
        .map((a) => (typeof a === "string" ? { agentType: a as AgentType } : a))
        .filter(
          (
            a
          ): a is {
            agentType: AgentType
            label?: string
            configValues?: Record<string, string>
          } => a != null && typeof a.agentType === "string"
        )
    }
    return parsed
  } catch {
    return null
  }
}

export function saveLastLauncherConfig(config: PkLauncherLastConfig): void {
  if (typeof window === "undefined") return
  try {
    window.localStorage.setItem(LAUNCHER_LAST_KEY, JSON.stringify(config))
  } catch {
    // Failing to remember the config doesn't affect the round.
  }
}

/** Branch names ride `codeg-pk/<round>/<slot>` — slug-safe and greppable.
 * Uses slot index (not agentType) so the same agent in two slots gets two
 * distinct branches. */
export function contestantBranchName(roundId: string, slot: number): string {
  return `codeg-pk/${roundId}/${slot}`
}

export function contestantContextKey(roundId: string, slot: number): string {
  return `pk:${roundId}:${slot}`
}

/** Convert a DB PkRoundInfo row to a PkRound for the store, reviving
 * interrupted rounds and seeding empty live contestant state. */
export function dbRoundToStoreRound(
  info: PkRoundInfo,
  workingDir: string,
  linkedConversations: readonly DbConversationSummary[] = []
): PkRound {
  const wasLive = info.status === "ready" || info.status === "running"
  const status: PkRoundStatus = wasLive ? "interrupted" : info.status
  const contestantConversations = linkedConversations
    .filter(
      (conversation) =>
        conversation.pk_round_id === info.id &&
        !conversation.title?.startsWith("PK Judge ·")
    )
    .sort(
      (a, b) =>
        new Date(a.created_at).getTime() - new Date(b.created_at).getTime() ||
        a.id - b.id
    )
  return {
    id: String(info.id),
    task: info.task,
    folderId: info.folder_id,
    workingDir,
    createdAt: new Date(info.created_at).getTime(),
    status,
    permissionMode:
      (info.config.permission_mode as PkPermissionMode) ?? "default",
    bareMode: info.config.bare_mode ?? false,
    effort: (info.config.effort as PkEffortLevel) ?? "default",
    judgeAgent: info.config.judge_agent ?? null,
    judgeDimensions: info.config.judge_dimensions?.length
      ? info.config.judge_dimensions
      : null,
    baseCommit: info.config.base_commit ?? null,
    judgeResult:
      info.judge_result != null
        ? {
            scores: (info.judge_result.scores ?? []).map((s) => ({
              slot: s.slot,
              agentType: s.agentType,
              score: s.score,
              rank: s.rank,
              comment: s.comment,
            })),
            summary: info.judge_result.summary ?? "",
            rawText: info.judge_result.rawText ?? "",
          }
        : null,
    judgeStatus: (info.judge_status as PkJudgeStatus) ?? "idle",
    persistenceErrors: { status: null, judge: null },
    contestants: info.config.agents.map((entry, slot) => {
      const linked = contestantConversations[slot]
      const agentType = typeof entry === "string" ? entry : entry.agent
      const label = typeof entry === "string" ? null : (entry.label ?? null)
      const configValues =
        typeof entry === "string" ? {} : (entry.config_values ?? {})
      const persistedStatus = linked?.status
      const contestantStatus: PkContestantStatus = wasLive
        ? "canceled"
        : persistedStatus === "cancelled" || persistedStatus === "in_progress"
          ? "canceled"
          : "done"
      return {
        slot,
        agentType: agentType as AgentType,
        label,
        configValues,
        modelOptions: [],
        modelConfigId: null,
        effortOptions: [],
        effortConfigId: null,
        selectedModel: linked?.model ?? configValues.model ?? null,
        selectedEffort: null,
        contextKey: null,
        connectionId: null,
        conversationId: linked?.id ?? null,
        worktreePath: null,
        branchName: null,
        status: contestantStatus,
        statusDetail:
          wasLive || persistedStatus === "in_progress"
            ? "interrupted"
            : persistedStatus === "cancelled"
              ? "conversation cancelled"
              : null,
        startedAt: null,
        endedAt: null,
        durationMs: null,
        usage: null,
        diff: null,
      }
    }),
  }
}

export const usePkArenaStore = create<PkArenaState & PkArenaActions>((set) => ({
  rounds: [],
  activeRoundId: null,
  launcherOpen: false,
  pillDismissed: false,
  hydrating: true,

  hydrateFromDb: (dbRounds) => {
    set((state) => {
      const activeRoundStillExists = dbRounds.some(
        (round) => round.id === state.activeRoundId
      )
      return {
        rounds: dbRounds,
        hydrating: false,
        activeRoundId: activeRoundStillExists ? state.activeRoundId : null,
      }
    })
  },

  createRound: async ({
    task,
    folderId,
    workingDir,
    agents,
    permissionMode,
    bareMode,
    effort,
    judgeAgent,
    judgeDimensions,
    baseCommit,
  }) => {
    const config: PkRoundConfig = {
      agents: agents.map((a) => {
        const configValues = a.configValues ?? {}
        if (!a.label && Object.keys(configValues).length === 0) {
          return a.agentType
        }
        return {
          agent: a.agentType,
          ...(a.label ? { label: a.label } : {}),
          ...(Object.keys(configValues).length > 0
            ? { config_values: configValues }
            : {}),
        }
      }),
      permission_mode: permissionMode ?? "default",
      bare_mode: bareMode ?? false,
      effort: effort ?? "default",
      judge_agent: judgeAgent ?? undefined,
      judge_dimensions: judgeDimensions?.filter((d) => d.trim()) ?? [],
      base_commit: baseCommit ?? undefined,
    }
    const info = await pkRoundCreate(folderId, task, config)
    const round: PkRound = {
      id: String(info.id),
      task,
      folderId,
      workingDir,
      createdAt: new Date(info.created_at).getTime(),
      status: "ready",
      permissionMode: permissionMode ?? "default",
      bareMode: bareMode ?? false,
      effort: effort ?? "default",
      judgeAgent: judgeAgent ?? null,
      judgeDimensions: judgeDimensions?.filter((d) => d.trim()) ?? null,
      baseCommit: baseCommit ?? null,
      judgeResult: null,
      judgeStatus: "idle",
      persistenceErrors: { status: null, judge: null },
      contestants: agents.map((a, slot) => ({
        slot,
        agentType: a.agentType,
        label: a.label ?? null,
        configValues: a.configValues ?? {},
        modelOptions: [],
        modelConfigId: null,
        effortOptions: [],
        effortConfigId: null,
        selectedModel: null,
        selectedEffort: null,
        contextKey: null,
        connectionId: null,
        conversationId: null,
        worktreePath: null,
        branchName: null,
        status: "preparing",
        statusDetail: null,
        startedAt: null,
        endedAt: null,
        durationMs: null,
        usage: null,
        diff: null,
      })),
    }
    set((state) => ({
      rounds: [round, ...state.rounds],
      activeRoundId: round.id,
    }))
    return round
  },

  updateContestant: (roundId, slot, patch) => {
    set((state) => ({
      rounds: state.rounds.map((round) =>
        round.id !== roundId
          ? round
          : {
              ...round,
              contestants: round.contestants.map((c) =>
                c.slot !== slot ? c : { ...c, ...patch }
              ),
            }
      ),
    }))
  },

  markRound: (roundId, status) => {
    set((state) => ({
      rounds: state.rounds.map((round) =>
        round.id === roundId ? { ...round, status } : round
      ),
    }))
    persistLatest(roundId, "status", () =>
      pkRoundUpdateStatus(Number(roundId), status)
    )
  },

  archiveRound: async (roundId) => {
    await pkRoundDelete(Number(roundId))
    persistenceRevisions.delete(persistenceKey(roundId, "status"))
    persistenceRevisions.delete(persistenceKey(roundId, "judge"))
    persistenceQueues.delete(persistenceKey(roundId, "status"))
    persistenceQueues.delete(persistenceKey(roundId, "judge"))
    set((state) => ({
      rounds: state.rounds.filter((round) => round.id !== roundId),
      activeRoundId:
        state.activeRoundId === roundId ? null : state.activeRoundId,
    }))
  },

  retryPersistence: (roundId) => {
    const round = usePkArenaStore
      .getState()
      .rounds.find((candidate) => candidate.id === roundId)
    if (!round) return
    if (round.persistenceErrors?.status) {
      persistLatest(roundId, "status", () =>
        pkRoundUpdateStatus(Number(roundId), round.status)
      )
    }
    if (round.persistenceErrors?.judge) {
      persistLatest(roundId, "judge", () =>
        pkRoundUpdateJudge(
          Number(roundId),
          round.judgeResult,
          round.judgeStatus
        )
      )
    }
  },

  setActiveRound: (roundId) => set({ activeRoundId: roundId }),
  setLauncherOpen: (open) => set({ launcherOpen: open }),
  setPillDismissed: (dismissed) => set({ pillDismissed: dismissed }),

  updateJudge: (roundId, patch) => {
    set((state) => ({
      rounds: state.rounds.map((round) =>
        round.id !== roundId
          ? round
          : {
              ...round,
              judgeResult: patch.judgeResult ?? round.judgeResult,
              judgeStatus: patch.judgeStatus ?? round.judgeStatus,
            }
      ),
    }))
    // Persist judge verdict + status to DB so it survives refresh/restart.
    const round = usePkArenaStore
      .getState()
      .rounds.find((r) => r.id === roundId)
    if (round) {
      const merged = patch.judgeResult ?? round.judgeResult ?? null
      persistLatest(roundId, "judge", () =>
        pkRoundUpdateJudge(
          Number(roundId),
          merged,
          patch.judgeStatus ?? round.judgeStatus
        )
      )
    }
  },
}))
