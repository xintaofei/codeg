import { describe, expect, it } from "vitest"

import type { DelegationBinding } from "@/contexts/delegation-context"
import type { DelegationCardSource } from "@/hooks/use-delegation-card-model"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import type { LiveContentBlock } from "@/contexts/acp-connections-context"
import {
  buildSubAgentSectionItems,
  buildSubAgentSectionRows,
  extractDelegationSources,
  extractLiveDelegationSources,
  extractLiveNativeSubAgentSources,
  extractNativeSubAgentSources,
} from "@/lib/delegation-sources"

function toolCall(
  overrides: Partial<AdaptedContentPart> & { toolCallId: string }
): AdaptedContentPart {
  return {
    type: "tool-call",
    toolName: "mcp__codeg__delegate_to_agent",
    input: null,
    state: "output-available",
    ...overrides,
  } as AdaptedContentPart
}

function liveToolCall(overrides: {
  tool_call_id: string
  title: string
  raw_input: string | null
  status?: string
  raw_output_chunks?: string[]
}): LiveContentBlock {
  return {
    type: "tool_call",
    info: {
      tool_call_id: overrides.tool_call_id,
      title: overrides.title,
      kind: "other",
      status: overrides.status ?? "in_progress",
      content: null,
      raw_input: overrides.raw_input,
      raw_output_chunks: overrides.raw_output_chunks ?? [],
      raw_output_total_bytes: 0,
      locations: null,
      meta: null,
      images: [],
    },
  }
}

function binding(overrides: Partial<DelegationBinding>): DelegationBinding {
  return {
    parentConnectionId: "conn-parent",
    parentToolUseId: "tool-delegate-1",
    childConnectionId: "child-conn",
    childConversationId: 99,
    agentType: "codex",
    status: "running",
    task: null,
    taskId: "task-1",
    ...overrides,
  }
}

describe("extractDelegationSources", () => {
  it("picks delegate calls and skips unrelated tools", () => {
    const sources = extractDelegationSources([
      toolCall({ toolCallId: "a" }),
      toolCall({ toolCallId: "b", toolName: "bash" }),
    ])
    expect(sources.map((s) => s.parentToolUseId)).toEqual(["a"])
  })

  it("recurses through tool groups and goal runs", () => {
    const group = {
      type: "tool-group",
      items: [toolCall({ toolCallId: "in-group" })],
    } as unknown as AdaptedContentPart
    const goalRun = {
      type: "goal-run",
      items: [toolCall({ toolCallId: "in-goal" })],
    } as unknown as AdaptedContentPart
    expect(extractDelegationSources([group, goalRun])).toHaveLength(2)
  })

  it("drops a refused resume and keys an accepted one by task id", () => {
    const refused = toolCall({
      toolCallId: "r1",
      toolName: "mcp__codeg__resume_delegation",
      input: JSON.stringify({ task_id: "task-1", reason: "go" }),
      output: "Not resumed: Unknown task id.",
    })
    expect(extractDelegationSources([refused])).toEqual([])

    const accepted = toolCall({
      toolCallId: "r2",
      toolName: "mcp__codeg__resume_delegation",
      input: JSON.stringify({ task_id: "task-2", reason: "go" }),
      output: JSON.stringify({ status: "running" }),
    })
    const sources = extractDelegationSources([accepted])
    expect(sources).toHaveLength(1)
    expect(sources[0].taskIdHint).toBe("task-2")
  })
})

describe("extractLiveDelegationSources", () => {
  it("maps an in-flight delegate call onto a source with raw input", () => {
    const sources = extractLiveDelegationSources([
      liveToolCall({
        tool_call_id: "live-1",
        title: "delegate_to_agent",
        raw_input: JSON.stringify({ agent_type: "codex", task: "do it" }),
      }),
    ])
    expect(sources).toHaveLength(1)
    expect(sources[0]).toMatchObject({
      parentToolUseId: "live-1",
      input: JSON.stringify({ agent_type: "codex", task: "do it" }),
      state: "input-available",
    })
  })

  it("flips state on a completed call and skips non-tool blocks", () => {
    const sources = extractLiveDelegationSources([
      { type: "text", text: "hi" },
      liveToolCall({
        tool_call_id: "live-2",
        title: "delegate_to_agent",
        raw_input: "{}",
        status: "completed",
      }),
    ])
    expect(sources).toHaveLength(1)
    expect(sources[0].state).toBe("output-available")
  })
})

describe("buildSubAgentSectionItems", () => {
  const delegate: DelegationCardSource = {
    parentToolUseId: "tool-delegate-1",
    input: JSON.stringify({ agent_type: "codex", task: "work" }),
    output: JSON.stringify({ kind: "ack", task_id: "task-1" }),
  }

  it("de-dupes a turn source and the live source by task id", () => {
    const items = buildSubAgentSectionItems(
      [delegate],
      [
        {
          parentToolUseId: "tool-resume-1",
          taskIdHint: "task-1",
        },
      ],
      [],
      "conn-parent"
    )
    expect(items).toHaveLength(1)
    // The resume's output folds into the original row rather than a new row.
    expect(items[0].source.taskIdHint).toBe("task-1")
  })

  it("merges the live binding into the matching row and drops the orphan", () => {
    const b = binding({})
    const items = buildSubAgentSectionItems([delegate], [], [b], "conn-parent")
    expect(items).toHaveLength(1)
    expect(items[0].binding).toBe(b)
  })

  it("keeps a binding that has no turn-side row", () => {
    const b = binding({ parentToolUseId: "tool-only", taskId: "task-9" })
    const items = buildSubAgentSectionItems([delegate], [], [b], "conn-parent")
    expect(items).toHaveLength(2)
    expect(items[1].binding).toBe(b)
    expect(items[1].source.parentToolUseId).toBe("tool-only")
  })

  it("scopes bindings to the parent connection", () => {
    const other = binding({ parentConnectionId: "conn-other" })
    const items = buildSubAgentSectionItems(
      [delegate],
      [],
      [other],
      "conn-parent"
    )
    // The foreign binding neither merges nor appends.
    expect(items).toHaveLength(1)
    expect(items[0].binding).toBeUndefined()
  })

  it("drops every binding when the parent connection is unknown", () => {
    const items = buildSubAgentSectionItems([delegate], [], [binding({})], null)
    expect(items).toHaveLength(1)
    expect(items[0].binding).toBeUndefined()
  })

  it("keeps two same-task delegations distinct when task id is absent", () => {
    // No output ⇒ no task id ⇒ tool-call-id identity: two calls stay separate.
    const items = buildSubAgentSectionItems(
      [
        { parentToolUseId: "x", input: "{}" },
        { parentToolUseId: "y", input: "{}" },
      ],
      [],
      [],
      "conn-parent"
    )
    expect(items.map((i) => i.source.parentToolUseId)).toEqual(["x", "y"])
  })
})

// ── Native sub-agents: every agent's OWN spawned children ───────────────────
//
// The message-area dispatch (content-parts-renderer) is the contract: a launch
// renders under the normalized names "agent" / "collab_agent", and the aux
// list must show exactly the same calls. One case per producer spelling —
// Claude/Grok/Cursor/OpenCode (`agent`), codex live collab (`collab_agent`,
// spawn-only), codex native team-of-agents (`multi_agent_v1__*`), and Hermes's
// batched `delegate_task` (one call, one row per task).

describe("extractNativeSubAgentSources", () => {
  it("collects native launches under the agent names and skips the rest", () => {
    const sources = extractNativeSubAgentSources([
      toolCall({ toolCallId: "t1", toolName: "Agent" }),
      toolCall({ toolCallId: "t2", toolName: "call_omo_agent" }),
      toolCall({ toolCallId: "t3", toolName: "bash" }),
      toolCall({ toolCallId: "t4", toolName: "mcp__codeg__delegate_to_agent" }),
    ])
    expect(sources.map((s) => s.toolCallId)).toEqual(["t1", "t2"])
  })

  it("reads a codex team-of-agents spawn despite the namespace prefix", () => {
    // `multi_agent_v1__spawn_agent` must collapse to "agent" (normalizeToolName
    // op-suffix rule) or the row never appears — neither the message area nor
    // the aux list knew these calls existed before.
    const sources = extractNativeSubAgentSources([
      toolCall({
        toolCallId: "s1",
        toolName: "multi_agent_v1__spawn_agent",
        input: '{"message":"用法语说你好"}',
      }),
      toolCall({ toolCallId: "w1", toolName: "multi_agent_v1__wait_agent" }),
      toolCall({ toolCallId: "c1", toolName: "multi_agent_v1__close_agent" }),
    ])
    expect(sources.map((s) => s.toolCallId)).toEqual(["s1"])
    expect(sources[0].input).toContain("message")
  })

  it("keeps only the collab spawn among the collab ops", () => {
    const collab = (op: string) =>
      JSON.stringify({
        senderThreadId: "t",
        receiverThreadIds: [],
        agentsStates: {},
        __codegCollabOp: op,
      })
    const sources = extractNativeSubAgentSources([
      toolCall({
        toolCallId: "sp",
        toolName: "collab_agent",
        input: collab("spawnAgent"),
      }),
      toolCall({
        toolCallId: "wa",
        toolName: "collab_agent",
        input: collab("wait"),
      }),
      toolCall({
        toolCallId: "cl",
        toolName: "collab_agent",
        input: collab("closeAgent"),
      }),
      toolCall({
        toolCallId: "ls",
        toolName: "collab_agent",
        input: collab("listAgents"),
      }),
    ])
    expect(sources.map((s) => s.toolCallId)).toEqual(["sp"])
  })

  it("expands a Hermes batched delegate_task into one row per task", () => {
    const sources = extractNativeSubAgentSources([
      toolCall({
        toolCallId: "batch1",
        toolName: "delegate_task",
        input: JSON.stringify({
          tasks: [
            { goal: "用日语问候", context: "演示" },
            { goal: "算 97 是否质数" },
            { context: "没有 goal 的条目" },
          ],
        }),
      }),
    ])
    expect(sources.map((s) => s.toolCallId)).toEqual([
      "batch1#task-0",
      "batch1#task-1",
    ])
    // The synthesized launch payload carries the goal as description/prompt so
    // the shared field parser titles the row; the real call output is kept.
    expect(JSON.parse(sources[0].input ?? "{}").description).toBe("用日语问候")
    expect(sources[0].output).toBeNull()
  })

  it("lifts a parser-injected per-task child handle into row-scoped stats", () => {
    // The hermes parser resolves each task to its own child session and
    // injects `__codegChildSessionId` into the task entry. The row MUST carry
    // that handle on ITS OWN agent_stats — sharing the call-level stats would
    // hand every row the same child session.
    const sources = extractNativeSubAgentSources([
      toolCall({
        toolCallId: "batch2",
        toolName: "delegate_task",
        input: JSON.stringify({
          tasks: [
            { goal: "map the repo", __codegChildSessionId: "child-a" },
            { goal: "write tests" },
          ],
        }),
      }),
    ])
    expect(sources.map((s) => s.toolCallId)).toEqual([
      "batch2#task-0",
      "batch2#task-1",
    ])
    expect(sources[0].agentStats?.child_session_id).toBe("child-a")
    expect(sources[1].agentStats?.child_session_id ?? null).toBeNull()
  })

  it("recurses through tool groups", () => {
    const sources = extractNativeSubAgentSources([
      {
        type: "tool-group",
        items: [toolCall({ toolCallId: "g1", toolName: "Agent" })],
      } as AdaptedContentPart,
    ])
    expect(sources.map((s) => s.toolCallId)).toEqual(["g1"])
  })
})

describe("extractLiveNativeSubAgentSources", () => {
  it("routes a live codex collab spawn and drops the sibling wait", () => {
    const collab = (op: string) =>
      JSON.stringify({
        senderThreadId: "t",
        receiverThreadIds: [],
        agentsStates: {},
        __codegCollabOp: op,
      })
    const sources = extractLiveNativeSubAgentSources([
      liveToolCall({
        tool_call_id: "sp",
        title: "spawnAgent",
        raw_input: collab("spawnAgent"),
        status: "completed",
      }),
      liveToolCall({
        tool_call_id: "wa",
        title: "wait",
        raw_input: collab("wait"),
      }),
    ])
    expect(sources.map((s) => s.toolCallId)).toEqual(["sp"])
    expect(sources[0].state).toBe("output-available")
  })

  it("falls back to the live ACP title when the collab op is unmerged", () => {
    // Live wire: the input shaper has not folded COLLAB_OP_KEY in yet — the
    // title IS the op.
    const raw = JSON.stringify({
      senderThreadId: "t",
      receiverThreadIds: [],
      agentsStates: {},
    })
    const sources = extractLiveNativeSubAgentSources([
      liveToolCall({ tool_call_id: "sp", title: "spawnAgent", raw_input: raw }),
      liveToolCall({ tool_call_id: "wa", title: "wait", raw_input: raw }),
    ])
    expect(sources.map((s) => s.toolCallId)).toEqual(["sp"])
  })

  it("expands a live Hermes batched launch by raw title before inference", () => {
    const sources = extractLiveNativeSubAgentSources([
      liveToolCall({
        tool_call_id: "b",
        title: "delegate_task",
        raw_input: '{"tasks":[{"goal":"任务甲"},{"goal":"任务乙"}]}',
      }),
    ])
    expect(sources.map((s) => s.toolCallId)).toEqual(["b#task-0", "b#task-1"])
    expect(sources[1].input).toContain("任务乙")
  })
})

describe("buildSubAgentSectionRows natives", () => {
  it("de-dupes by tool-call id and lets the live state win", () => {
    const rows = buildSubAgentSectionRows(
      [],
      [],
      [],
      null,
      [
        {
          toolCallId: "n1",
          input: "{}",
          output: null,
          errorText: null,
          state: "input-available",
          meta: null,
        },
      ],
      [
        {
          toolCallId: "n1",
          input: "{}",
          output: "done",
          errorText: null,
          state: "output-available",
          meta: null,
        },
      ]
    )
    const natives = rows.filter((r) => r.kind === "native")
    expect(natives).toHaveLength(1)
    if (natives[0].kind === "native") {
      expect(natives[0].source.state).toBe("output-available")
      expect(natives[0].source.output).toBe("done")
    }
  })

  it("lists delegations before natives", () => {
    const row = (toolCallId: string) => ({
      toolCallId,
      input: '{"description":"问候"}',
      output: null,
      errorText: null,
      state: "output-available" as const,
      meta: null,
    })
    const rows = buildSubAgentSectionRows(
      [{ parentToolUseId: "d1", input: "{}" }],
      [],
      [],
      null,
      [row("n1"), row("n2")],
      []
    )
    expect(rows.map((r) => r.kind)).toEqual(["delegation", "native", "native"])
  })
})
