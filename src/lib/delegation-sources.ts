import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import {
  inferLiveToolName,
  normalizeToolName,
} from "@/lib/tool-call-normalization"
import {
  classifyCollabOp,
  COLLAB_AGENT_TOOL_NAME,
  parseCollabOp,
} from "@/lib/collab-tool"
import { parseResumeTaskId } from "@/lib/codeg-mcp-tool"
import {
  isDelegateToAgentToolName,
  isRefusedResume,
  parseDelegateTaskId,
  parseDelegationMeta,
} from "@/lib/delegation-card"
import type { DelegationCardSource } from "@/hooks/use-delegation-card-model"
import type { DelegationBinding } from "@/contexts/delegation-context"
import type {
  LiveContentBlock,
  ToolCallMeta,
} from "@/contexts/acp-connections-context"
import type { AgentExecutionStats } from "@/lib/types"

// Shared by `MessageListView`'s sub-agent overlay and the aux panel's session
// details "sub-agents" section: both list the session's delegations, and the
// two surfaces must agree on WHICH tool calls count as one.
//
// Two kinds qualify:
//   - `delegate_to_agent`, which STARTED a sub-agent, keyed by its own
//     tool_use_id;
//   - `resume_delegation`, which brought an interrupted one BACK. Its own
//     tool_call_id is not a binding key (the broker re-binds the child to the
//     original delegate call, usually in an earlier turn), so it is keyed by
//     the task id in its arguments — `taskIdHint`, exactly as
//     `ResumedDelegationCard` does. Without this arm a resumed sub-agent would
//     be missing from the overlay while it runs, because the reply that
//     resumed it contains no `delegate_to_agent` call at all.
//
// `seenTaskIds` de-dupes repeated resumes of one task inside a single scan
// (the second is refused, but the list renders a row per source regardless).
function collectDelegationSources(
  parts: AdaptedContentPart[],
  out: DelegationCardSource[],
  seenTaskIds: Set<string>
): void {
  for (const part of parts) {
    if (part.type === "tool-call") {
      if (!part.toolCallId) continue
      const name = normalizeToolName(part.toolName)
      if (isDelegateToAgentToolName(name)) {
        out.push({
          parentToolUseId: part.toolCallId,
          input: part.input ?? null,
          output: part.output ?? null,
          errorText: part.errorText ?? null,
          state: part.state,
          meta: part.meta ?? null,
        })
      } else if (name === "resume_delegation") {
        // A refusal names the task's agent and child but revived nothing —
        // listing it would put a sub-agent in the list that is not running on
        // this turn's behalf. Same judgement as `ResumedDelegationCard`, which
        // falls back to the plain tool card here.
        if (isRefusedResume(part.output ?? null, part.errorText ?? null)) {
          continue
        }
        const taskId = parseResumeTaskId(part.input ?? null)
        // No task id ⇒ nothing to resolve the sub-agent by; a duplicate ⇒
        // already listed.
        if (!taskId || seenTaskIds.has(taskId)) continue
        seenTaskIds.add(taskId)
        out.push({
          parentToolUseId: part.toolCallId,
          taskIdHint: taskId,
          // Deliberately not the resume's `{task_id, reason}` arguments —
          // `parseInput` looks for `task`/`agent_type`/`working_dir` and would
          // only warn about an unrecognized shape. See `ResumedDelegationCard`.
          input: null,
          output: part.output ?? null,
          errorText: part.errorText ?? null,
          state: part.state,
          meta: part.meta ?? null,
        })
      }
    } else if (part.type === "tool-group") {
      collectDelegationSources(part.items, out, seenTaskIds)
    } else if (part.type === "goal-run") {
      collectDelegationSources(part.items, out, seenTaskIds)
    }
  }
}

export function extractDelegationSources(
  parts: AdaptedContentPart[]
): DelegationCardSource[] {
  const out: DelegationCardSource[] = []
  collectDelegationSources(parts, out, new Set())
  return out
}

// ── Live (in-flight reply) extraction ───────────────────────────────────────
//
// The aux panel's sub-agents section watches the runtime store's timeline,
// which ends in a streaming-phase turn built by
// `buildStreamingTurnsFromLiveMessage` — so scanning `liveMessage.content`
// directly mirrors that builder's `tool_use` mapping and catches a delegation
// the moment its `delegate_to_agent` call arrives, before any promoted turn
// carries it. Same qualifying rules as the adapted path above.

const LIVE_TOOL_CALL_STATE: Record<string, DelegationCardSource["state"]> = {
  completed: "output-available",
  failed: "output-error",
}

export function extractLiveDelegationSources(
  content: LiveContentBlock[]
): DelegationCardSource[] {
  const out: DelegationCardSource[] = []
  const seenTaskIds = new Set<string>()
  for (const block of content) {
    if (block.type !== "tool_call") continue
    const info = block.info
    if (!info.tool_call_id) continue
    const name = normalizeToolName(info.title || info.kind)
    if (isDelegateToAgentToolName(name)) {
      out.push({
        parentToolUseId: info.tool_call_id,
        input: info.raw_input,
        output:
          info.raw_output_chunks.length > 0
            ? info.raw_output_chunks.join("")
            : null,
        state: LIVE_TOOL_CALL_STATE[info.status] ?? "input-available",
        meta: info.meta,
      })
    } else if (name === "resume_delegation") {
      const joinedOutput =
        info.raw_output_chunks.length > 0
          ? info.raw_output_chunks.join("")
          : null
      if (isRefusedResume(joinedOutput, null)) continue
      const taskId = parseResumeTaskId(info.raw_input)
      if (!taskId || seenTaskIds.has(taskId)) continue
      seenTaskIds.add(taskId)
      out.push({
        parentToolUseId: info.tool_call_id,
        taskIdHint: taskId,
        input: null,
        output: joinedOutput,
        state: LIVE_TOOL_CALL_STATE[info.status] ?? "input-available",
        meta: info.meta,
      })
    }
  }
  return out
}

// ── The aux panel's per-session delegation list ─────────────────────────────
//
// The session-details "sub-agents" section merges three feeds into one list:
//   - every turn of the loaded timeline (all replies, not just the last one —
//     the floating overlay only shows the LAST reply, this list shows the
//     session);
//   - the in-flight reply's raw `liveMessage` blocks (a delegation created
//     mid-stream isn't in any promoted turn yet);
//   - the live `DelegationContext` bindings, which for a running child carry
//     fresher status/task text than the tool call's output does.
//
// Identity is by the broker task id when known (it survives resume — a
// resume's card re-binds to the ORIGINAL delegate call), otherwise by
// tool-call id. The binding for the same delegation merges into the earliest
// turn-sourced row (turn order = chronology) and is dropped from the
// binding-only pass; bindings with no turn-side row appear last.
//
// `parentConnectionId` scopes the bindings to THIS conversation: the provider
// is mounted once above every conversation, so without the filter one
// session's list would show another session's delegations.

export interface SubAgentSectionItem {
  source: DelegationCardSource
  /** The live binding merged into this row, if one matched. */
  binding?: DelegationBinding
}

export function buildSubAgentSectionItems(
  turnSources: DelegationCardSource[],
  liveSources: DelegationCardSource[],
  bindings: readonly DelegationBinding[],
  parentConnectionId: string | null
): SubAgentSectionItem[] {
  const items: SubAgentSectionItem[] = []
  // key → index into `items`, for cross-source de-dup.
  const indexByKey = new Map<string, number>()

  const keyOfSource = (s: DelegationCardSource): string | null => {
    const meta = parseDelegationMeta(s.meta ?? null)
    const taskId =
      s.taskIdHint ??
      parseDelegateTaskId(s.output ?? null, s.errorText ?? null) ??
      meta?.taskId ??
      null
    // taskIdHint alone is not identity: two DIFFERENT resumes could name the
    // same task only if it were resumed twice, and the second is refused
    // (already filtered) — so it is safe here. Fall back to tool-call id for
    // never-completed delegate calls (no task id parsed yet).
    return taskId ? `task:${taskId}` : `tool:${s.parentToolUseId}`
  }

  const seenTask = new Set<string>()
  const upsertSource = (s: DelegationCardSource): void => {
    const key = keyOfSource(s)
    if (key) {
      const existing = indexByKey.get(key)
      if (existing != null) {
        // Enrich the earlier row only, never overwrite: the earliest is the
        // originating `delegate_to_agent` (a resume's source is a
        // representation of the same task, not new evidence). But a resume
        // DOES carry output/meta the origin lacks — fold in what's missing.
        const prev = items[existing].source
        items[existing] = {
          ...items[existing],
          source: {
            ...prev,
            output: prev.output ?? s.output,
            meta: prev.meta ?? s.meta,
            errorText: prev.errorText ?? s.errorText,
            state: s.state ?? prev.state,
            taskIdHint: prev.taskIdHint ?? s.taskIdHint,
          },
        }
        return
      }
      indexByKey.set(key, items.length)
      seenTask.add(key)
    }
    items.push({ source: s })
  }

  for (const s of turnSources) upsertSource(s)
  for (const s of liveSources) upsertSource(s)

  const scoped = parentConnectionId
    ? bindings.filter((b) => b.parentConnectionId === parentConnectionId)
    : []

  const matchedBindings = new Set<string>()
  for (const item of items) {
    const binding = scoped.find(
      (b) =>
        b.parentToolUseId === item.source.parentToolUseId ||
        (item.source.taskIdHint != null && b.taskId === item.source.taskIdHint)
    )
    if (binding) {
      item.binding = binding
      matchedBindings.add(
        binding.taskId
          ? `task:${binding.taskId}`
          : `tool:${binding.parentToolUseId}`
      )
    }
  }
  for (const b of scoped) {
    const key = b.taskId ? `task:${b.taskId}` : `tool:${b.parentToolUseId}`
    if (matchedBindings.has(key) || seenTask.has(key)) continue
    matchedBindings.add(key)
    items.push({
      source: {
        parentToolUseId: b.parentToolUseId,
        input: null,
        output: null,
        errorText: null,
        state: "input-available",
        meta: null,
      },
      binding: b,
    })
  }
  return items
}

// ── Native sub-agents (every agent's OWN spawned children) ──────────────────
//
// A `delegate_to_agent` delegation is only ONE way a session grows children.
// Each host agent also spawns sub-agents natively — Claude Code's `Task`,
// Codex's `spawn_agent`, Grok's `spawn_subagent`, Cursor's `task`, OpenCode's
// `call_omo_agent` — and the message area already renders each as an Agent
// capsule. The aux panel's sub-agents section must show the SAME set: whosever
// agent the parent is, its native children belong in the list next to the
// codeg delegations.
//
// Identity predicate — the EXACT dispatch the message area uses
// (`content-parts-renderer`), which renders a native sub-agent under TWO
// normalized names:
//   - `"agent"` — Claude Code's `Task`, Grok's `spawn_subagent`, Cursor's
//     `task`, OpenCode's `call_omo_agent`, and every settled codex `spawn_agent`
//     (the alias table folds them all in; the rollout parser rewrites history
//     to "agent");
//   - `COLLAB_AGENT_TOOL_NAME` ("collab_agent") — codex's LIVE `spawn_agent`,
//     which `inferLiveToolName` routes to the collab card by input shape
//     (`isCodexCollabInput`) BEFORE the alias would map it to "agent".
// Missing the second name would make a running codex child invisible until
// reload — the opposite of what the live list is for. The live side reuses the
// runtime store's own `inferLiveToolName`, so a streaming call lands here under
// the same predicate its streaming card uses. codeg's delegation tools
// normalize elsewhere ("delegate_to_agent") and can never collide.

export interface NativeSubAgentSource {
  /** The launching tool call's id — the row's identity. */
  toolCallId: string
  /** Raw JSON input of the launching call (subagent_type/description/…). */
  input: string | null
  /** Raw output (result text, background marker, async-launch ack, …). */
  output: string | null
  errorText: string | null
  state: DelegationCardSource["state"]
  meta: ToolCallMeta
  /** Only present in history: the parser folded `agent_stats` off the
   *  matching tool_result (duration, child_session_id). */
  agentStats?: AgentExecutionStats | null
}

const NATIVE_AGENT_TOOL_NAMES: ReadonlySet<string> = new Set([
  "agent",
  COLLAB_AGENT_TOOL_NAME,
])

// Hermes's native sub-agent launcher batches a whole team into ONE tool call:
// `delegate_task` with input {tasks: [{goal, context}, …]} — one child per
// entry. Normalizing its name would collide (the canonical `"task"` is shared
// by every generic task tool), so the launcher is identified by its RAW name
// here and expanded at extraction: one row per task, the tool-call id gaining a
// per-task suffix. The wire carries no child-session handle, so rows are
// status-only UNLESS the history parser (`parsers/hermes.rs`) resolved a task
// to its own child session — it then injects `__codegChildSessionId` into that
// task entry, and the row below lifts it into a ROW-scoped
// `agent_stats.child_session_id`, making just that row clickable.
const HERMES_BATCH_LAUNCH_RAW_NAME = "delegate_task"

function isHermesBatchLaunch(rawName: string | null | undefined): boolean {
  if (!rawName) return false
  const canonical = rawName
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
  return canonical === HERMES_BATCH_LAUNCH_RAW_NAME
}

/** Goal lines of a batched `delegate_task` launch; [] unless the input
 *  parses to a non-empty `tasks` array. (Truncated live inputs simply yield
 *  no rows until the parse succeeds.) Each entry also carries the parser-
 *  injected per-task child handle (`__codegChildSessionId`,
 *  `parsers/hermes.rs::attach_delegate_task_child_handles`) when the task's
 *  goal matched its own child session. */
function parseBatchDelegationGoals(
  input: string | null
): { goal: string; childSessionId: string | null }[] {
  if (!input) return []
  try {
    const parsed = JSON.parse(input) as { tasks?: unknown } | null
    if (!parsed || !Array.isArray(parsed.tasks)) return []
    const goals: { goal: string; childSessionId: string | null }[] = []
    for (const task of parsed.tasks) {
      if (!task || typeof task !== "object") continue
      const entry = task as Record<string, unknown>
      const goal =
        typeof entry.goal === "string" && entry.goal.trim()
          ? entry.goal.trim()
          : typeof entry.description === "string" && entry.description.trim()
            ? entry.description.trim()
            : null
      if (!goal) continue
      const childSessionId =
        typeof entry.__codegChildSessionId === "string" &&
        entry.__codegChildSessionId.length > 0
          ? entry.__codegChildSessionId
          : null
      goals.push({ goal, childSessionId })
    }
    return goals
  } catch {
    return []
  }
}

/**
 * Whether a call under one of the native-agent names actually LAUNCHES a
 * child. Under plain `"agent"` every match does (the name only ever comes
 * from a Task/spawn-class tool). Under `collab_agent` the name is shared by
 * codex's whole team-of-agents op family — `spawn_agent` (launch),
 * `wait_agent` (poll), `close_agent`, `list_agents` (roster) — and only
 * spawn starts a child. Listing a `wait` would duplicate the row its spawn
 * already created under a different tool-call id, so the op decides.
 */
function isNativeAgentLaunch(
  name: string,
  input: string | null,
  /** The live wire carries the collab op as the tool_call's ACP title; the
   *  input shaper only folds it under `COLLAB_OP_KEY` once a turn promotes. */
  liveTitle?: string | null
): boolean {
  if (name !== COLLAB_AGENT_TOOL_NAME) return true
  return classifyCollabOp(parseCollabOp(input) ?? liveTitle ?? null) === "spawn"
}

function collectNativeSubAgentSources(
  parts: AdaptedContentPart[],
  out: NativeSubAgentSource[]
): void {
  for (const part of parts) {
    if (part.type === "tool-call") {
      if (!part.toolCallId) continue
      if (isHermesBatchLaunch(part.toolName)) {
        pushBatchLaunchRows(
          {
            toolCallId: part.toolCallId,
            input: part.input ?? null,
            output: part.output ?? null,
            errorText: part.errorText ?? null,
            state: part.state,
            meta: part.meta ?? null,
            agentStats: part.agentStats ?? null,
          },
          out
        )
        continue
      }
      const name = normalizeToolName(part.toolName).toLowerCase()
      if (!NATIVE_AGENT_TOOL_NAMES.has(name)) continue
      if (!isNativeAgentLaunch(name, part.input ?? null)) continue
      out.push({
        toolCallId: part.toolCallId,
        input: part.input ?? null,
        output: part.output ?? null,
        errorText: part.errorText ?? null,
        state: part.state,
        meta: part.meta ?? null,
        agentStats: part.agentStats ?? null,
      })
    } else if (part.type === "tool-group") {
      collectNativeSubAgentSources(part.items, out)
    } else if (part.type === "goal-run") {
      collectNativeSubAgentSources(part.items, out)
    }
  }
}

/** One batched launch → one row per task. The row's `input` is a synthesized
 *  launch payload (`{description, prompt}` = the task goal) so the shared
 *  `parseSubAgentLaunchFields` gives the row a title without a bespoke parser;
 *  `output` stays the REAL call output (the batch result text) for the status
 *  reading. Tool-call id gains a per-task suffix, keeping rows distinct and
 *  stable across the live → promoted handoff. A resolved task carries its
 *  OWN child handle as row-scoped `agent_stats` — the shared call stats would
 *  otherwise hand EVERY row the same session. */
function pushBatchLaunchRows(
  base: NativeSubAgentSource,
  out: NativeSubAgentSource[]
): void {
  const tasks = parseBatchDelegationGoals(base.input)
  tasks.forEach((task, i) => {
    out.push({
      ...base,
      toolCallId: `${base.toolCallId}#task-${i}`,
      input: JSON.stringify({ description: task.goal, prompt: task.goal }),
      agentStats: task.childSessionId
        ? { ...(base.agentStats ?? {}), child_session_id: task.childSessionId }
        : base.agentStats,
    })
  })
}

export function extractNativeSubAgentSources(
  parts: AdaptedContentPart[]
): NativeSubAgentSource[] {
  const out: NativeSubAgentSource[] = []
  collectNativeSubAgentSources(parts, out)
  return out
}

export function extractLiveNativeSubAgentSources(
  content: LiveContentBlock[]
): NativeSubAgentSource[] {
  const out: NativeSubAgentSource[] = []
  for (const block of content) {
    if (block.type !== "tool_call") continue
    const info = block.info
    if (!info.tool_call_id) continue
    if (isHermesBatchLaunch(info.title || info.kind)) {
      pushBatchLaunchRows(
        {
          toolCallId: info.tool_call_id,
          input: info.raw_input,
          output:
            info.raw_output_chunks.length > 0
              ? info.raw_output_chunks.join("")
              : null,
          errorText: null,
          state: LIVE_TOOL_CALL_STATE[info.status] ?? "input-available",
          meta: info.meta,
        },
        out
      )
      continue
    }
    const name = inferLiveToolName({
      title: info.title,
      kind: info.kind,
      rawInput: info.raw_input,
      meta: info.meta,
    }).toLowerCase()
    if (!NATIVE_AGENT_TOOL_NAMES.has(name)) continue
    // Live wire: the codex collab op is the tool_call's ACP title (the
    // input shaper folds it under COLLAB_OP_KEY only for promoted turns).
    if (!isNativeAgentLaunch(name, info.raw_input, info.title)) continue
    out.push({
      toolCallId: info.tool_call_id,
      input: info.raw_input,
      output:
        info.raw_output_chunks.length > 0
          ? info.raw_output_chunks.join("")
          : null,
      errorText: null,
      state: LIVE_TOOL_CALL_STATE[info.status] ?? "input-available",
      meta: info.meta,
    })
  }
  return out
}

// The section list merges BOTH families into one list: codeg delegations,
// exactly as `buildSubAgentSectionItems` above resolves them, then every
// agent's native children (each family keeps its own turn order — the two
// families grouping, not interleaving, is acceptable in a collapsed-count
// section and keeps each family's de-dup logic untouched). Identity differs
// by family — delegations key on broker task id / tool id, natives on the
// launching tool-call id — and the two key spaces never collide. The message
// area already renders both, so a row per tool call here agrees with what the
// transcript shows.

export type SubAgentSectionRow =
  | { kind: "delegation"; item: SubAgentSectionItem }
  | { kind: "native"; source: NativeSubAgentSource }

export function buildSubAgentSectionRows(
  turnSources: DelegationCardSource[],
  liveSources: DelegationCardSource[],
  bindings: readonly DelegationBinding[],
  parentConnectionId: string | null,
  turnNatives: readonly NativeSubAgentSource[],
  liveNatives: readonly NativeSubAgentSource[]
): SubAgentSectionRow[] {
  const delegations = buildSubAgentSectionItems(
    turnSources,
    liveSources,
    bindings,
    parentConnectionId
  )
  const rows: SubAgentSectionRow[] = delegations.map((item) => ({
    kind: "delegation" as const,
    item,
  }))
  // Live natives enrich the turn-side row with the freshest state while the
  // same call streams; a never-seen id appends. (Turn-side natives from the
  // SAME adapter cache are already settled; the live block is the only
  // richer source mid-stream.)
  const indexByToolCallId = new Map<string, number>()
  for (let i = 0; i < turnNatives.length; i += 1) {
    const s = turnNatives[i]
    if (indexByToolCallId.has(s.toolCallId)) continue
    indexByToolCallId.set(s.toolCallId, rows.length)
    rows.push({ kind: "native", source: s })
  }
  for (const s of liveNatives) {
    const existing = indexByToolCallId.get(s.toolCallId)
    if (existing != null) {
      const prev = (
        rows[existing] as Extract<SubAgentSectionRow, { kind: "native" }>
      ).source
      rows[existing] = {
        kind: "native",
        source: {
          ...prev,
          output: prev.output ?? s.output,
          meta: prev.meta ?? s.meta,
          // The live state tracks the call as it streams; take the newer.
          state: s.state,
          agentStats: prev.agentStats ?? s.agentStats,
        },
      }
      continue
    }
    indexByToolCallId.set(s.toolCallId, rows.length)
    rows.push({ kind: "native", source: s })
  }
  return rows
}
