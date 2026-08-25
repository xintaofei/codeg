"use client"

import { useCallback, useEffect, useRef } from "react"
import { useLocale } from "next-intl"
import {
  acpRespondPermission,
  createPkConversation,
  getFolderConversation,
  getGitBranch,
  gitDiff,
  gitDiffWithBranch,
  gitRemoveWorktree,
  gitWorktreeAdd,
  updateConversationStatus,
} from "@/lib/api"
import type { PromptInputBlock, SessionConfigOptionInfo } from "@/lib/types"
import { assignJudgeScoreSlots } from "@/lib/pk-judge"
import { preparePkReportData } from "@/lib/pk-report-data"
import {
  useAcpActions,
  useAcpEvent,
  useConnectionStore,
} from "@/contexts/acp-connections-context"
import {
  contestantBranchName,
  contestantContextKey,
  usePkArenaStore,
  type PkContestant,
  type PkContestantUsage,
  type PkEffortLevel,
  type PkJudgeResult,
  type PkJudgeScore,
  type PkPermissionMode,
  type PkRound,
} from "@/stores/pk-arena-store"

/**
 * Arena orchestrator — drives one round's contestants through the existing
 * connection machinery (no broker, no parent agent):
 *
 *   worktree → conversation row → connect(own contextKey) → same prompt
 *
 * Completion is detected from `status_changed` events per connection
 * (`prompting` → a settled state means the contestant's single turn ended),
 * which is exactly the signal the live transcript uses. Duration is measured
 * client-side between the prompt send and that transition; token/turn stats
 * are summed from the persisted conversation afterwards.
 */

/** 公平竞技规则块。裸机模式下追加到任务提示词——软约束(模型仍会看到
 * 全局技能目录的内容),但统一施加于所有选手,对比保持 apples-to-apples。 */
const BARE_MODE_RULES = [
  "FAIR-PLAY RULES (mandatory):",
  "This is a fair competition. Do NOT use any skills, slash commands, plugins,",
  "custom agents, or custom instructions — including anything from",
  "~/.claude/skills, ~/.codex/skills, ~/.agents/skills, or any other global",
  "skill store, and any .claude/skills / .agents/skills / .codex/skills",
  "directories in the repository. Use only your built-in capabilities",
  "(file read/write, running commands, web access).",
].join("\n")

/** 裁判提示词——在所有选手完成后发送给裁判 agent。要求结构化 JSON 输出
 * (每个选手:分数、排名、点评;以及总体总结)。裁判不需要 worktree,
 * 只读取各选手的 diff 文本。 */
/** Default judge evaluation dimensions, used when the round has no custom
 * `judge_dimensions` configured. */
const DEFAULT_JUDGE_DIMENSIONS = [
  "Correctness — does it fulfill the task?",
  "Code quality — readability, structure, edge cases",
  "Completeness — how much of the task is done?",
  "Efficiency — token count and time are NOT factors here; judge code-level efficiency only",
]

export function buildJudgePrompt(
  task: string,
  contestants: Array<{
    slot: number
    agentType: string
    label?: string | null
    diff: string
  }>,
  dimensions?: string[] | null,
  outputLocale = "en"
): PromptInputBlock[] {
  const sections = contestants.map(
    (c) =>
      `--- Contestant slot ${c.slot}: ${c.agentType}${c.label ? ` · ${c.label}` : ""} ---\n${c.diff}\n--- End slot ${c.slot} ---`
  )
  const dims =
    dimensions && dimensions.length > 0 ? dimensions : DEFAULT_JUDGE_DIMENSIONS
  const numbered = dims.map((d, i) => `${i + 1}. ${d}`).join("\n")
  const text = [
    `You are the JUDGE of a coding PK arena.`,
    "",
    `Task given to all contestants:`,
    `"${task}"`,
    "",
    `Below are the git diffs from each contestant. Evaluate each one on:`,
    numbered,
    "",
    "Score each contestant 0-100. Rank them (1 = best).",
    `Write every human-readable comment and summary in the language identified by locale ${outputLocale}. Keep JSON property names unchanged.`,
    "",
    "Respond with ONLY a JSON block (no markdown fences, no prose before or after):",
    "Return exactly one score row for every contestant slot listed below. Preserve each numeric slot exactly.",
    '{"scores":[{"slot":<number>,"agentType":"<agent>","score":<number>,"rank":<number>,"comment":"<one-line>"}],"summary":"<overall verdict in 1-2 sentences>"}',
    "",
    "Here are the diffs:",
    "",
    ...sections,
  ].join("\n")
  return [{ type: "text", text }]
}

/** 解析裁判 LLM 的文本输出,提取结构化 JSON 评分。
 * 容忍 markdown 围栏和前后文本。 */
function parseJudgeResult(
  rawText: string,
  contestants: readonly PkContestant[]
): PkJudgeResult | null {
  // Strip markdown code fences if present.
  const cleaned = rawText
    .replace(/^```(?:json)?\s*/i, "")
    .replace(/\s*```\s*$/i, "")
    .trim()
  // Find the first { and last } — the JSON blob.
  const start = cleaned.indexOf("{")
  const end = cleaned.lastIndexOf("}")
  if (start === -1 || end === -1 || end <= start) return null
  const jsonStr = cleaned.slice(start, end + 1)
  try {
    const parsed = JSON.parse(jsonStr) as {
      scores?: Array<{
        slot?: number
        agentType?: string
        score?: number
        rank?: number
        comment?: string
      }>
      summary?: string
    }
    if (!parsed.scores || !Array.isArray(parsed.scores)) return null
    const scores: PkJudgeScore[] = parsed.scores
      .filter((s) => s.agentType != null)
      .map((s) => ({
        slot: typeof s.slot === "number" ? s.slot : undefined,
        agentType: String(s.agentType),
        score: typeof s.score === "number" ? s.score : 0,
        rank: typeof s.rank === "number" ? s.rank : 0,
        comment: s.comment ?? "",
      }))
    if (scores.length === 0) return null
    return {
      scores: assignJudgeScoreSlots(scores, contestants),
      summary: parsed.summary ?? "",
      rawText,
    }
  } catch {
    return null
  }
}

function taskPromptBlocks(
  task: string,
  worktreePath: string,
  bareMode: boolean
): PromptInputBlock[] {
  return [
    {
      type: "text",
      text: [
        task,
        "",
        `Work inside this directory: ${worktreePath}`,
        "It is a fresh git worktree created for you — this is your isolated arena, no other agent writes here. Commit your work when done.",
        ...(bareMode ? ["", BARE_MODE_RULES] : []),
      ].join("\n"),
    },
  ]
}

/**
 * Apply the round's permission policy via `session/set_mode` once the agent
 * has advertised its modes. The requested mode id is only sent when the
 * agent actually advertises it: forcing an unknown id on an agent that would
 * reject it would fail the whole connect sequence, and an agent without the
 * mode simply keeps asking, exactly as before. "default" needs no call.
 *
 * The modes arrive as a `session_modes` EVENT shortly after session/new —
 * not in connect()'s resolution. The arena attaches the contestant as a
 * by-id delegation child right after connect, and the attach RE-ROUTES the
 * reverseMap to the by-id entry, so the event lands there, never on the
 * owner (contextKey) entry. Polling only the owner entry therefore times
 * out and the mode is silently skipped (field report: presets "did not
 * apply"). Poll both entries: pre-attach events land on the owner, post-
 * attach on the by-id entry.
 */
type ModesStore = {
  getConnection(key: string):
    | {
        modes?: { available_modes?: Array<{ id: string }> } | null
        configOptions?: SessionConfigOptionInfo[] | null
      }
    | undefined
}

/** 统一的思考等级 → 各 agent 通告值的最近匹配。词表各不相同
 * (claude: low/medium/high; codex: minimal/low/medium/high/max;
 * deepseek: off/low/medium/high),按规范序取最近邻,平局取更高档
 * (公平竞技下宁高勿低)。 */
const EFFORT_RANK: Record<string, number> = {
  off: 0,
  minimal: 1,
  low: 2,
  medium: 3,
  high: 4,
  max: 5,
}

function nearestEffort(requested: string, advertised: string[]): string | null {
  if (advertised.length === 0) return null
  const exact = advertised.find((v) => v === requested)
  if (exact) return exact
  const target = EFFORT_RANK[requested] ?? 3
  let best: string | null = null
  let bestDist = Number.POSITIVE_INFINITY
  for (const value of advertised) {
    const rank = EFFORT_RANK[value]
    if (rank === undefined) continue
    const dist = Math.abs(rank - target)
    if (
      dist < bestDist ||
      (dist === bestDist && best !== null && rank > (EFFORT_RANK[best] ?? 0))
    ) {
      best = value
      bestDist = dist
    }
  }
  return best
}

/** 把通告的 configOptions 折成竞技场需要的两份选项表。 */
function selectOptions(configOptions: SessionConfigOptionInfo[] | null): {
  modelOptions: Array<{ value: string; name: string }>
  effortOptions: string[]
} {
  const modelOptions: Array<{ value: string; name: string }> = []
  const effortOptions: string[] = []
  for (const option of configOptions ?? []) {
    if (option.kind?.type !== "select") continue
    if (option.id === "model" || option.id === "model_id") {
      for (const item of option.kind.options) {
        modelOptions.push({ value: item.value, name: item.name ?? item.value })
      }
    } else if (/effort|reasoning/i.test(option.id)) {
      for (const item of option.kind.options) {
        if (EFFORT_RANK[item.value] !== undefined)
          effortOptions.push(item.value)
      }
    }
  }
  return { modelOptions, effortOptions }
}

/** 双条目轮询:modes/configOptions 都走 attach 后的 by-id 路由
 * (见 applyPermissionMode 的注释)。按需等**特定字段**——统一等「任一字段」
 * 会在 modes 先到时立即返回,此时 configOptions 往往还没到,消费方拿到
 * null 就静默放弃(实测:模型/思考等级选择器永远不出现)。 */
function waitForField(
  connectionStore: ModesStore,
  contextKey: string,
  connectionId: string | null,
  field: "modes" | "configOptions",
  timeoutMs = 10000
): Promise<{
  modes?: { available_modes?: Array<{ id: string }> } | null
  configOptions?: SessionConfigOptionInfo[] | null
} | null> {
  return new Promise((resolve) => {
    const startedAt = Date.now()
    const poll = () => {
      const owner = connectionStore.getConnection(contextKey)
      const byId =
        connectionId != null
          ? connectionStore.getConnection(connectionId)
          : undefined
      // 字段优先,不是条目优先:`owner ?? byId` 会在 owner 存在但缺该字段时
      // 永远选 owner,把带字段的 byId 晾在一边——实测 owner=none byId=N,
      // 选择器与权限预设全部静默丢失(同一个根).两个条目都查字段,谁有谁算。
      if (owner != null && owner[field] != null) {
        resolve(owner)
        return
      }
      if (byId != null && byId[field] != null) {
        resolve(byId)
        return
      }
      if (Date.now() - startedAt >= timeoutMs) {
        resolve(null)
        return
      }
      setTimeout(poll, 200)
    }
    poll()
  })
}

/**
 * Apply the round's permission policy via `session/set_mode` once the agent
 * has advertised its modes. The requested mode id is only sent when the
 * agent actually advertises it: forcing an unknown id on an agent that would
 * reject it would fail the whole connect sequence, and an agent without the
 * mode simply keeps asking, exactly as before. "default" needs no call.
 *
 * The modes arrive as a `session_modes` EVENT shortly after session/new —
 * not in connect()'s resolution. The arena attaches the contestant as a
 * by-id delegation child right after connect, and the attach RE-ROUTES the
 * reverseMap to the by-id entry, so the event lands there, never on the
 * owner (contextKey) entry. Polling only the owner entry therefore times
 * out and the mode is silently skipped (field report: presets "did not
 * apply"). `waitForOptions` reads whichever entry the event landed on.
 */
export function mapPermissionToAgentMode(
  mode: PkPermissionMode,
  availableModes: string[]
): string | null {
  if (mode === "default") return null
  if (availableModes.includes(mode)) return mode

  if (mode === "bypassPermissions") {
    const candidate = [
      "agent-full-access",
      "danger-full-access",
      "full-access",
      "auto",
      "acceptEdits",
      "agent",
    ].find((m) => availableModes.includes(m))
    return candidate ?? null
  }

  if (mode === "acceptEdits") {
    const candidate = ["acceptEdits", "agent", "auto"].find((m) =>
      availableModes.includes(m)
    )
    return candidate ?? null
  }

  return null
}

async function applyPermissionMode(
  connectionStore: ModesStore,
  setMode: (contextKey: string, modeId: string) => Promise<void>,
  setConfigOption: (
    contextKey: string,
    configId: string,
    valueId: string
  ) => Promise<void>,
  contextKey: string,
  connectionId: string | null,
  mode: PkPermissionMode
): Promise<void> {
  if (mode === "default") return
  const entry = await waitForField(
    connectionStore,
    contextKey,
    connectionId,
    "modes"
  )
  const advertised = entry?.modes?.available_modes?.map((m) => m.id) ?? []
  const targetMode = mapPermissionToAgentMode(mode, advertised)
  if (targetMode) {
    try {
      await setMode(contextKey, targetMode)
    } catch {
      // A rejected mode switch must not kill the round
    }
  }

  // Also check if the agent (such as Codex) exposes an approval preset via configOptions
  const configEntry = await waitForField(
    connectionStore,
    contextKey,
    connectionId,
    "configOptions"
  )
  const modeOption = configEntry?.configOptions?.find(
    (o) =>
      o.id === "mode" ||
      o.id === "permission_mode" ||
      o.id === "approval_policy"
  )
  if (modeOption && modeOption.kind?.type === "select") {
    const optValues = modeOption.kind.options.map((o) => o.value)
    if (mode === "bypassPermissions") {
      const targetVal = [
        "agent-full-access",
        "danger-full-access",
        "never",
        "auto",
      ].find((v) => optValues.includes(v))
      if (targetVal) {
        try {
          await setConfigOption(contextKey, modeOption.id, targetVal)
        } catch {
          // ignore
        }
      }
    } else if (mode === "acceptEdits") {
      const targetVal = ["agent", "acceptEdits", "auto"].find((v) =>
        optValues.includes(v)
      )
      if (targetVal) {
        try {
          await setConfigOption(contextKey, modeOption.id, targetVal)
        } catch {
          // ignore
        }
      }
    }
  }
}

async function applyPreparedOptions(
  connectionStore: ModesStore,
  setConfigOption: (
    contextKey: string,
    configId: string,
    valueId: string
  ) => Promise<void>,
  contextKey: string,
  connectionId: string | null,
  effort: PkEffortLevel
): Promise<{
  modelOptions: Array<{ value: string; name: string }>
  modelConfigId: string | null
  effortOptions: string[]
  effortConfigId: string | null
  selectedModel: string | null
  selectedEffort: string | null
  diagnostic: string
}> {
  const entry = await waitForField(
    connectionStore,
    contextKey,
    connectionId,
    "configOptions"
  )
  const options = entry?.configOptions ?? null
  const { modelOptions, effortOptions } = selectOptions(options)
  let selectedModel: string | null = null
  let selectedEffort: string | null = null
  const effortConfigId =
    (options ?? []).find(
      (o) => o.kind?.type === "select" && /effort|reasoning/i.test(o.id)
    )?.id ?? null
  if (effortConfigId) {
    const option = (options ?? []).find((o) => o.id === effortConfigId)
    const current =
      option?.kind?.type === "select" ? option.kind.current_value : null
    selectedEffort = current ?? null
  }
  const modelConfigId =
    (options ?? []).find(
      (o) =>
        o.kind?.type === "select" && (o.id === "model" || o.id === "model_id")
    )?.id ?? null
  if (modelConfigId) {
    const option = (options ?? []).find((o) => o.id === modelConfigId)
    const current =
      option?.kind?.type === "select" ? option.kind.current_value : null
    selectedModel = current ?? null
  }
  if (effort !== "default" && effortConfigId) {
    const target = nearestEffort(effort, effortOptions)
    if (target) {
      try {
        await setConfigOption(contextKey, effortConfigId, target)
        selectedEffort = target
      } catch {
        // 拒绝不致命——选手保持当前档位。
      }
    }
  }
  return {
    modelOptions,
    modelConfigId,
    effortOptions,
    effortConfigId,
    selectedModel,
    selectedEffort,
    diagnostic:
      options === null
        ? "no configOptions advertised"
        : `arrived (${options.length} options)`,
  }
}

export async function fetchUsage(
  conversationId: number
): Promise<PkContestantUsage | null> {
  try {
    const detail = await getFolderConversation(conversationId)
    let inputTokens = 0
    let outputTokens = 0
    let turnCount = 0
    let tokensReported = false
    for (const turn of detail.turns ?? []) {
      if (turn.role !== "assistant") continue
      turnCount += 1
      inputTokens += turn.usage?.input_tokens ?? 0
      outputTokens += turn.usage?.output_tokens ?? 0
      if (
        (turn.usage?.input_tokens ?? 0) > 0 ||
        (turn.usage?.output_tokens ?? 0) > 0
      ) {
        tokensReported = true
      }
    }
    return { inputTokens, outputTokens, turnCount, tokensReported }
  } catch {
    return null
  }
}

export function usePkRound(): {
  startRound: (round: PkRound) => Promise<void>
  startPrompt: (round: PkRound) => Promise<void>
  sendFollowUp: (
    round: PkRound,
    contestant: PkContestant,
    message: string
  ) => Promise<void>
  applyContestantSelection: (
    round: PkRound,
    contestant: PkContestant,
    configId: string,
    value: string
  ) => Promise<void>
  cancelRound: (round: PkRound) => Promise<void>
  disconnectFinished: (round: PkRound) => Promise<void>
  cleanupRound: (round: PkRound, keepBranches: boolean) => Promise<void>
  fetchDiff: (round: PkRound, contestant: PkContestant) => Promise<void>
  runJudge: (round: PkRound) => Promise<void>
} {
  const locale = useLocale()
  const {
    connect,
    sendPrompt,
    cancel,
    disconnect,
    setMode,
    setConfigOption,
    touchActivity,
    respondPermission,
    attachDelegationChild,
    detachDelegationChild,
  } = useAcpActions()
  const connectionStore = useConnectionStore()
  const updateContestant = usePkArenaStore((s) => s.updateContestant)
  const markRound = usePkArenaStore((s) => s.markRound)
  const roundsRef = useRef(usePkArenaStore.getState().rounds)
  useEffect(() => {
    const unsub = usePkArenaStore.subscribe((state) => {
      roundsRef.current = state.rounds
    })
    return unsub
  }, [])

  // Map connectionId → {roundId, slot} so the event subscription can
  // resolve envelopes without re-subscribing as rounds change.
  // `isJudge: true` marks the judge agent connection — it uses the same
  // event pipeline but settles into judgeResult instead of contestant state.
  const contestantsByConnection = useRef(
    new Map<
      string,
      {
        roundId: string
        slot: number
        isJudge?: boolean
      }
    >()
  )

  const disconnectFinished = useCallback(
    async (round: PkRound | null | undefined) => {
      if (!round) return
      await Promise.allSettled(
        round.contestants
          .filter((c) => c.connectionId != null)
          .map(async (contestant) => {
            if (contestant.connectionId) {
              detachDelegationChild(contestant.connectionId)
            }
            if (contestant.contextKey) {
              await disconnect(contestant.contextKey).catch(() => undefined)
            }
          })
      )
    },
    [detachDelegationChild, disconnect]
  )

  // 裁判 settled 时的处理:从裁判的 conversation 轮次里提取最后一条
  // assistant 消息文本,解析 JSON 评分。裁判连接用独立 contextKey,不跟
  // 选手混在一起。
  const updateJudge = usePkArenaStore((s) => s.updateJudge)

  const settleJudge = useCallback(
    async (roundId: string) => {
      const round = roundsRef.current.find((r) => r.id === roundId)
      if (!round || !round.judgeAgent) return
      // 裁判的 conversationId 存在 store 里的 judgeResult 临时字段——但
      // 我们没地方存 conversationId。改用 contextKey 从 connection store
      // 拿状态,但文本只能从 conversation 轮次读。这里用 contextKey 去
      // 读 conversationId——不,contextKey 不映射到 conversationId。
      //
      // 方案:裁判的 conversationId 在 runJudge 里创建后存入 ref。
      const judgeConvId = judgeConvIdRef.current.get(roundId)
      if (judgeConvId != null) {
        try {
          const detail = await getFolderConversation(judgeConvId)
          const lastAssistant = [...(detail.turns ?? [])]
            .reverse()
            .find((turn) => turn.role === "assistant")
          const rawText =
            lastAssistant?.blocks
              ?.filter(
                (b): b is { type: "text"; text: string } => b.type === "text"
              )
              .map((b) => b.text)
              .join("\n") ?? ""
          const result = parseJudgeResult(
            rawText,
            round.contestants.filter(
              (contestant) => contestant.status === "done"
            )
          )
          updateJudge(roundId, {
            judgeStatus: "done",
            judgeResult: result ?? {
              scores: [],
              summary: "Judge response could not be parsed.",
              rawText,
            },
          })
        } catch {
          updateJudge(roundId, {
            judgeStatus: "error",
            judgeResult: {
              scores: [],
              summary: "Failed to read judge response.",
              rawText: "",
            },
          })
        }
        // Judge sessions are system-owned terminal work, not user work waiting
        // for review. Explicitly settle the linked conversation so the sidebar
        // does not keep rendering an in-progress spinner after the verdict is
        // already available.
        await updateConversationStatus(judgeConvId, "completed").catch(
          () => undefined
        )
      } else {
        updateJudge(roundId, { judgeStatus: "error" })
      }
      for (const [connectionId, entry] of contestantsByConnection.current) {
        if (entry.isJudge && entry.roundId === roundId) {
          contestantsByConnection.current.delete(connectionId)
        }
      }
      // 断开裁判连接。
      const judgeCtxKey = `pk:${roundId}:judge`
      void disconnect(judgeCtxKey).catch(() => undefined)
    },
    [disconnect, updateJudge]
  )

  const settleJudgeError = useCallback(
    (roundId: string, message: string) => {
      updateJudge(roundId, {
        judgeStatus: "error",
        judgeResult: {
          scores: [],
          summary: message,
          rawText: "",
        },
      })
      const judgeConvId = judgeConvIdRef.current.get(roundId)
      if (judgeConvId != null) {
        void updateConversationStatus(judgeConvId, "cancelled").catch(
          () => undefined
        )
      }
      for (const [connectionId, entry] of contestantsByConnection.current) {
        if (entry.isJudge && entry.roundId === roundId) {
          contestantsByConnection.current.delete(connectionId)
        }
      }
      const judgeCtxKey = `pk:${roundId}:judge`
      void disconnect(judgeCtxKey).catch(() => undefined)
    },
    [disconnect, updateJudge]
  )

  // Store judge conversationId per round — needed by settleJudge to read the
  // conversation turns after the judge finishes.
  const judgeConvIdRef = useRef(new Map<string, number>())

  const fetchDiff = useCallback(
    async (round: PkRound, contestant: PkContestant) => {
      if (!contestant.worktreePath) return
      try {
        // 对比基准分支而不是选手自身分支:worktree 里 `git diff <自身分支>`
        // 在选手提交后为空,而 diff 的意义是"比起跑点改了什么"。先取回合
        // 仓库当前分支(main 等),再在 worktree 里对它 diff——既含已提交
        // 也含未提交的工作区改动。
        const base = (await getGitBranch(round.workingDir)) ?? null
        const diff =
          base == null
            ? // 取不到基准分支名时退回工作区 diff(仅未提交改动)。
              await gitDiff(contestant.worktreePath)
            : await gitDiffWithBranch(contestant.worktreePath, base)
        updateContestant(round.id, contestant.slot, {
          diff: diff.trim() === "" ? "（无可比较内容:选手未改动工作区）" : diff,
        })
      } catch (error) {
        updateContestant(round.id, contestant.slot, {
          diff: `diff unavailable: ${String(error)}`,
        })
      }
    },
    [updateContestant]
  )

  const runJudge = useCallback(
    async (round: PkRound) => {
      if (!round.judgeAgent) return
      const roundId = round.id
      updateJudge(roundId, { judgeStatus: "running" })

      // 收集所有选手的 diff(未加载的先加载)。
      const contestantsWithDiffs = await Promise.all(
        round.contestants
          .filter((c) => c.status === "done")
          .map(async (contestant) => {
            if (contestant.diff == null && contestant.worktreePath) {
              await fetchDiff(round, contestant)
            }
            const freshRound = usePkArenaStore
              .getState()
              .rounds.find((r) => r.id === roundId)
            const fresh = freshRound?.contestants.find(
              (c) => c.slot === contestant.slot
            )
            return {
              agentType: contestant.agentType,
              slot: contestant.slot,
              label: contestant.label,
              diff: fresh?.diff ?? "(no diff available)",
            }
          })
      )

      if (contestantsWithDiffs.length === 0) {
        updateJudge(roundId, {
          judgeStatus: "skipped",
          judgeResult: {
            scores: [],
            summary: "No completed contestants to judge.",
            rawText: "",
          },
        })
        return
      }

      const contextKey = `pk:${roundId}:judge`
      try {
        const connectResult = await connect(
          contextKey,
          round.judgeAgent,
          round.workingDir
        )
        const connectionId =
          connectResult ??
          connectionStore.getConnection(contextKey)?.connectionId ??
          null
        if (connectionId) {
          contestantsByConnection.current.set(connectionId, {
            roundId,
            slot: -1,
            isJudge: true,
          })
        }

        // Create a conversation for the judge so its transcript persists.
        let conversationId: number | null = null
        try {
          const taskPreview = round.task.slice(0, 60)
          conversationId = await createPkConversation(
            round.folderId,
            round.judgeAgent as PkContestant["agentType"],
            Number(roundId),
            `PK Judge · ${taskPreview}${round.task.length > 60 ? "…" : ""}`
          )
          judgeConvIdRef.current.set(roundId, conversationId)
        } catch {
          // 裁判没有 conversation 也能跑,只是 transcript 不持久化。
        }

        await sendPrompt(
          contextKey,
          buildJudgePrompt(
            round.task,
            contestantsWithDiffs,
            round.judgeDimensions,
            locale
          ),
          {
            folderId: round.folderId,
            conversationId: conversationId ?? undefined,
          }
        )
      } catch (error) {
        updateJudge(roundId, {
          judgeStatus: "error",
          judgeResult: {
            scores: [],
            summary: `Judge failed to start: ${String(error)}`,
            rawText: "",
          },
        })
      }
    },
    [connect, connectionStore, sendPrompt, updateJudge, fetchDiff, locale]
  )
  // Keep a ref so settleContestant can call it without circular deps.
  const runJudgeRef = useRef(runJudge)
  runJudgeRef.current = runJudge

  const settleContestant = useCallback(
    async (
      roundId: string,
      slot: number,
      outcome: "done" | "error",
      detail?: string
    ) => {
      const endedAt = Date.now()
      const round = roundsRef.current.find((r) => r.id === roundId)
      const contestant = round?.contestants.find((c) => c.slot === slot)
      if (!round || !contestant) return

      const startedAt = contestant.startedAt ?? endedAt
      updateContestant(roundId, slot, {
        status: outcome,
        statusDetail: detail ?? null,
        endedAt,
        durationMs: endedAt - startedAt,
      })
      if (contestant.conversationId != null) {
        const usage = await fetchUsage(contestant.conversationId)
        if (usage) {
          updateContestant(roundId, slot, { usage })
        }
      }

      const fresh = usePkArenaStore
        .getState()
        .rounds.find((r) => r.id === roundId)
      if (
        fresh &&
        fresh.contestants.every(
          (c) =>
            c.status === "done" ||
            c.status === "error" ||
            c.status === "canceled"
        )
      ) {
        markRound(roundId, "finished")
        // Capture the disposable worktrees as soon as the round settles. This
        // makes report export independent from the arena staying open and is
        // retried synchronously before an explicit worktree cleanup.
        void preparePkReportData(fresh).catch(() => undefined)
        // 结算即断开:侧边栏的选手会话立刻停止转圈,结果走向持久化
        // transcript。想继续追一条会话,把它当普通会话打开重连即可。
        void disconnectFinished(
          usePkArenaStore.getState().rounds.find((r) => r.id === roundId)
        )
        // 裁判自动触发:所有选手 settled 且配置了 judgeAgent 时启动。
        // 裁判在选手断开后才连(避免连接数叠加),用独立 contextKey。
        if (fresh.judgeAgent && fresh.judgeStatus === "idle") {
          void runJudgeRef.current(fresh)
        }
      }
    },
    [disconnectFinished, markRound, updateContestant]
  )

  useEffect(() => {
    const timer = window.setInterval(() => {
      for (const round of usePkArenaStore.getState().rounds) {
        if (round.status !== "ready" && round.status !== "running") continue
        for (const contestant of round.contestants) {
          if (contestant.contextKey) {
            try {
              touchActivity(contestant.contextKey)
            } catch {
              // 保活是尽力而为;失败不影响回合。
            }
          }
        }
      }
    }, 20000)
    return () => window.clearInterval(timer)
  }, [touchActivity])

  useAcpEvent((envelope) => {
    if (
      envelope.type !== "status_changed" &&
      envelope.type !== "error" &&
      envelope.type !== "turn_complete" &&
      envelope.type !== "permission_request"
    ) {
      return
    }
    const entry = contestantsByConnection.current.get(envelope.connection_id)
    if (!entry) return

    // Judge connections share the event pipeline but settle differently.
    if (entry.isJudge) {
      if (envelope.type === "turn_complete") {
        void settleJudge(entry.roundId)
      } else if (envelope.type === "error") {
        settleJudgeError(entry.roundId, envelope.message)
      } else if (
        envelope.type === "status_changed" &&
        envelope.status === "disconnected"
      ) {
        settleJudgeError(entry.roundId, "连接中断(空闲回收或进程退出)")
      }
      return
    }

    const round = roundsRef.current.find((r) => r.id === entry.roundId)
    const contestant = round?.contestants.find((c) => c.slot === entry.slot)
    if (!round || !contestant) return

    if (envelope.type === "permission_request") {
      if (
        round.permissionMode === "bypassPermissions" ||
        round.permissionMode === "acceptEdits"
      ) {
        const opts =
          (
            envelope as {
              options?: Array<{ option_id: string; name?: string }>
            }
          ).options ?? []
        const allowOpt =
          opts.find(
            (o) =>
              /allow|always|proceed|yes|approve|continue/i.test(o.option_id) ||
              /allow|always|proceed|yes|approve|continue/i.test(o.name ?? "")
          ) ?? opts[0]
        if (allowOpt) {
          const reqId = (envelope as { request_id: string }).request_id
          const targetKey = contestant.contextKey ?? envelope.connection_id
          void respondPermission(targetKey, reqId, allowOpt.option_id).catch(
            () => {
              void acpRespondPermission(
                envelope.connection_id,
                reqId,
                allowOpt.option_id
              ).catch(() => {})
            }
          )
        }
      }
      return
    }

    if (envelope.type === "error") {
      if (
        contestant.status === "running" ||
        contestant.status === "connecting"
      ) {
        void settleContestant(
          entry.roundId,
          entry.slot,
          "error",
          envelope.message
        )
      }
      return
    }

    // `turn_complete` is the REAL settle signal: the backend flips the turn
    // status at TurnComplete WITHOUT emitting a status_changed envelope
    // (session_state.rs: "bypassing StatusChanged entirely"), so waiting for
    // prompting→settled would leave finished contestants stuck on "running".
    if (envelope.type === "turn_complete") {
      if (contestant.status === "running") {
        void settleContestant(entry.roundId, entry.slot, "done")
      }
      return
    }

    // status_changed: only the prompting edge matters for the running flip;
    // the settle edge does not exist (see turn_complete above). A disconnect
    // mid-turn means the backend reaped the connection (idle sweep) or the
    // agent died — that is a failure, not a stuck running state.
    if (envelope.status === "disconnected") {
      if (
        contestant.status === "running" ||
        contestant.status === "connecting"
      ) {
        void settleContestant(
          entry.roundId,
          entry.slot,
          "error",
          "连接中断(空闲回收或进程退出)"
        )
      }
      return
    }
    if (envelope.status === "prompting") {
      if (contestant.status === "connecting" || contestant.status === "ready") {
        updateContestant(entry.roundId, entry.slot, {
          status: "running",
          startedAt: Date.now(),
        })
      }
    }
  })

  const startRound = useCallback(
    async (round: PkRound) => {
      for (const contestant of round.contestants) {
        const { slot, agentType } = contestant
        const contextKey = contestantContextKey(round.id, slot)
        const branchName = contestantBranchName(round.id, slot)
        const worktreePath = `${round.workingDir}/.codeg-pk/${round.id}/${slot}`
        try {
          await gitWorktreeAdd(
            round.workingDir,
            branchName,
            worktreePath,
            round.baseCommit
          )
        } catch (error) {
          updateContestant(round.id, slot, {
            status: "error",
            statusDetail: `worktree: ${String(error)}`,
          })
          continue
        }
        updateContestant(round.id, slot, {
          branchName,
          worktreePath,
        })

        let conversationId: number | null = null
        try {
          const taskPreview = round.task.slice(0, 60)
          conversationId = await createPkConversation(
            round.folderId,
            agentType,
            Number(round.id),
            `PK · ${taskPreview}${round.task.length > 60 ? "…" : ""}`
          )
        } catch (error) {
          updateContestant(round.id, slot, {
            status: "error",
            statusDetail: `conversation: ${String(error)}`,
          })
          continue
        }
        updateContestant(round.id, slot, {
          conversationId,
          contextKey,
          status: "connecting",
        })

        let initialModeId: string | null = null
        let initialConfigValues: Record<string, string> | null =
          Object.keys(contestant.configValues).length > 0
            ? { ...contestant.configValues }
            : null
        if (agentType === "claude_code") {
          initialModeId = round.permissionMode
        } else if (agentType === "codex") {
          if (round.permissionMode === "bypassPermissions") {
            initialModeId = "agent-full-access"
            initialConfigValues = {
              ...(initialConfigValues ?? {}),
              mode: "agent-full-access",
            }
          } else if (round.permissionMode === "acceptEdits") {
            initialModeId = "agent"
            initialConfigValues = {
              ...(initialConfigValues ?? {}),
              mode: "agent",
            }
          }
        }

        try {
          const connectResult = await connect(
            contextKey,
            agentType,
            worktreePath,
            undefined,
            undefined,
            initialModeId,
            initialConfigValues
          )
          const connectionId =
            connectResult ??
            connectionStore.getConnection(contextKey)?.connectionId ??
            null
          if (connectionId) {
            contestantsByConnection.current.set(connectionId, {
              roundId: round.id,
              slot,
            })
            updateContestant(round.id, slot, { connectionId })
            // LiveTranscriptView resolves its connection via
            // useConnectionStateById, which looks the store up BY
            // connectionId — the entry shape only delegation children have
            // (attach registers contextKey == connectionId). Attach the
            // contestant the same way so the battle panes mirror the live
            // stream; done BEFORE the first prompt so the whole turn flows
            // through the by-id entry (no mid-turn hydrate needed).
            attachDelegationChild({
              connectionId,
              parentConnectionId: connectionId,
              parentToolUseId: `pk-arena-${round.id}`,
              agentType,
            })
          }
          await applyPermissionMode(
            connectionStore,
            setMode,
            setConfigOption,
            contextKey,
            connectionId,
            round.permissionMode
          )
          const prepared = await applyPreparedOptions(
            connectionStore,
            setConfigOption,
            contextKey,
            connectionId,
            round.effort
          )
          updateContestant(round.id, slot, {
            status: "ready",
            modelOptions: prepared.modelOptions,
            modelConfigId: prepared.modelConfigId,
            effortOptions: prepared.effortOptions,
            effortConfigId: prepared.effortConfigId,
            selectedModel: prepared.selectedModel,
            selectedEffort: prepared.selectedEffort,
            // 诊断:无选择器时把原因写进面板可见的 statusDetail。
            statusDetail:
              prepared.modelOptions.length === 0 &&
              prepared.effortOptions.length === 0
                ? `no selectors (configOptions ${prepared.diagnostic})`
                : null,
          })
        } catch (error) {
          updateContestant(round.id, slot, {
            status: "error",
            statusDetail: `connect/prompt: ${String(error)}`,
          })
        }
      }

      const fresh = usePkArenaStore
        .getState()
        .rounds.find((r) => r.id === round.id)
      if (fresh && fresh.contestants.every((c) => c.status === "error")) {
        markRound(round.id, "canceled")
      }
    },
    [
      connect,
      connectionStore,
      markRound,
      setMode,
      setConfigOption,
      updateContestant,
      attachDelegationChild,
    ]
  )

  const cancelRound = useCallback(
    async (round: PkRound) => {
      markRound(round.id, "canceled")
      for (const contestant of round.contestants) {
        if (
          contestant.status === "done" ||
          contestant.status === "error" ||
          contestant.status === "canceled"
        ) {
          continue
        }
        if (contestant.connectionId) {
          detachDelegationChild(contestant.connectionId)
        }
        if (contestant.contextKey) {
          try {
            await cancel(contestant.contextKey)
          } catch {
            // A connection that never came up has nothing to cancel.
          }
          void disconnect(contestant.contextKey).catch(() => undefined)
        }
        if (contestant.conversationId != null) {
          await updateConversationStatus(
            contestant.conversationId,
            "cancelled"
          ).catch(() => undefined)
        }
        updateContestant(round.id, contestant.slot, {
          status: "canceled",
          endedAt: Date.now(),
        })
      }
      // 取消后也触发裁判:已完成的选手(done)仍可参与评分。
      // 复用 settleContestant 的逻辑——如果配了 judgeAgent 且
      // judgeStatus === "idle",调 runJudge(修复 issue #1)。
      const fresh = usePkArenaStore
        .getState()
        .rounds.find((r) => r.id === round.id)
      if (fresh && fresh.judgeAgent && fresh.judgeStatus === "idle") {
        const hasDone = fresh.contestants.some((c) => c.status === "done")
        if (hasDone) {
          void runJudgeRef.current(fresh)
        }
      }
      if (fresh) void preparePkReportData(fresh).catch(() => undefined)
    },
    [cancel, detachDelegationChild, disconnect, markRound, updateContestant]
  )

  const cleanupRound = useCallback(
    async (round: PkRound, keepBranches: boolean) => {
      const freshRound =
        usePkArenaStore
          .getState()
          .rounds.find((item) => item.id === round.id) ?? round
      const reportData = await preparePkReportData(freshRound)
      if (
        reportData.source === "empty" &&
        freshRound.contestants.some((contestant) => contestant.worktreePath)
      ) {
        throw new Error(
          "Could not preserve the PK report; worktrees were not removed"
        )
      }
      // Release the by-id viewer entries before the worktrees go.
      for (const contestant of round.contestants) {
        if (contestant.connectionId) {
          detachDelegationChild(contestant.connectionId)
        }
      }
      await Promise.allSettled(
        round.contestants
          .filter((c) => c.worktreePath != null && c.branchName != null)
          .map((c) =>
            gitRemoveWorktree(
              c.worktreePath as string,
              c.branchName as string,
              round.folderId,
              !keepBranches,
              true
            )
          )
      )
      for (const contestant of round.contestants) {
        updateContestant(round.id, contestant.slot, {
          worktreePath: null,
        })
      }
    },
    [detachDelegationChild, updateContestant]
  )

  const startPrompt = useCallback(
    async (round: PkRound) => {
      markRound(round.id, "running")
      await Promise.allSettled(
        round.contestants
          .filter((c) => c.status === "ready" && c.contextKey != null)
          .map(async (contestant) => {
            const contextKey = contestant.contextKey as string
            try {
              await sendPrompt(
                contextKey,
                taskPromptBlocks(
                  round.task,
                  contestant.worktreePath ?? round.workingDir,
                  round.bareMode
                ),
                {
                  folderId: round.folderId,
                  conversationId: contestant.conversationId,
                }
              )
              // sendPrompt 已下发,主动把选手推进到 running。
              // 不依赖 status_changed(prompting) 事件——server 模式下该事件
              // 可能因 attach stream 竞态丢失,导致选手永远停在 ready,
              // 后续 turn_complete 因 status !== "running" 被忽略,round
              // 卡死、裁判不触发(问题 #0)。
              updateContestant(round.id, contestant.slot, {
                status: "running",
                startedAt: Date.now(),
              })
            } catch (error) {
              updateContestant(round.id, contestant.slot, {
                status: "error",
                statusDetail: `prompt: ${String(error)}`,
              })
            }
          })
      )
      const fresh = usePkArenaStore
        .getState()
        .rounds.find((r) => r.id === round.id)
      if (fresh && fresh.contestants.every((c) => c.status === "error")) {
        markRound(round.id, "canceled")
      }
    },
    [markRound, sendPrompt, updateContestant]
  )

  /** Send a follow-up message to ONE contestant — the multi-turn / human-
   *  intervention path. Only works when the contestant is done (its previous
   *  turn settled) AND its connection is still alive (contextKey != null).
   *  Pushes the contestant back to running so the scoreboard reflects the
   *  new turn; the round stays "finished" if it was — the follow-up is a
   *  single-contestant side turn, not a new round. */
  const sendFollowUp = useCallback(
    async (round: PkRound, contestant: PkContestant, message: string) => {
      if (!contestant.contextKey || contestant.conversationId == null) return
      const trimmed = message.trim()
      if (!trimmed) return
      const blocks: PromptInputBlock[] = [
        {
          type: "text",
          text: [
            trimmed,
            "",
            `Continue working inside this directory: ${contestant.worktreePath ?? round.workingDir}`,
          ].join("\n"),
        },
      ]
      try {
        await sendPrompt(contestant.contextKey, blocks, {
          folderId: round.folderId,
          conversationId: contestant.conversationId,
        })
        updateContestant(round.id, contestant.slot, {
          status: "running",
          endedAt: null,
          durationMs: null,
        })
      } catch (error) {
        updateContestant(round.id, contestant.slot, {
          status: "error",
          statusDetail: `follow-up: ${String(error)}`,
        })
      }
    },
    [sendPrompt, updateContestant]
  )

  const applyContestantSelection = useCallback(
    async (
      round: PkRound,
      contestant: PkContestant,
      configId: string,
      value: string
    ) => {
      if (!contestant.contextKey) return
      try {
        await setConfigOption(contestant.contextKey, configId, value)
        if (configId === contestant.modelConfigId) {
          const label = contestant.modelOptions.find(
            (option) => option.value === value
          )?.name
          updateContestant(round.id, contestant.slot, {
            selectedModel: value,
            ...(label ? { label } : {}),
          })
        } else {
          updateContestant(round.id, contestant.slot, {
            selectedEffort: value,
          })
        }
      } catch {
        // 选择被拒(模型临时下架等)不致命。
      }
    },
    [setConfigOption, updateContestant]
  )

  return {
    startRound,
    startPrompt,
    sendFollowUp,
    applyContestantSelection,
    cancelRound,
    disconnectFinished,
    cleanupRound,
    fetchDiff,
    runJudge,
  }
}
