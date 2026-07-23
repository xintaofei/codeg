import { describe, expect, it } from "vitest"

import {
  adaptMessageTurn,
  createMessageTurnAdapter,
  dropEmptyInFlightToolCalls,
  dropHiddenFeedbackChecks,
  extractUserResourcesFromText,
  groupConsecutiveDelegationStatus,
  groupGoalRuns,
  groupConsecutiveToolCalls,
  mergeAdjacentDelegationStatusGroups,
  type AdaptedContentPart,
  type AdaptedToolCallPart,
} from "./ai-elements-adapter"

function poll(toolName: string, taskId?: string): AdaptedToolCallPart {
  return {
    type: "tool-call",
    toolCallId: `${toolName}:${taskId ?? ""}`,
    toolName,
    input: taskId ? JSON.stringify({ task_id: taskId }) : null,
    state: "output-available",
  }
}

const text: AdaptedContentPart = { type: "text", text: "checking again" }

function pollsOf(part: AdaptedContentPart): AdaptedToolCallPart[] {
  if (part.type !== "delegation-status-group") {
    throw new Error(`expected a delegation-status-group, got ${part.type}`)
  }
  return part.polls
}

function goalRunOf(part: AdaptedContentPart) {
  if (part.type !== "goal-run") {
    throw new Error(`expected a goal-run, got ${part.type}`)
  }
  return part
}

describe("groupConsecutiveDelegationStatus", () => {
  it("wraps a run of consecutive status polls into one group", () => {
    const out = groupConsecutiveDelegationStatus([
      poll("get_delegation_status", "t1"),
      poll("get_delegation_status", "t1"),
      poll("get_delegation_status", "t1"),
    ])
    expect(out).toHaveLength(1)
    expect(pollsOf(out[0])).toHaveLength(3)
  })

  it("wraps even a single poll (so the settled-status rule applies uniformly)", () => {
    const out = groupConsecutiveDelegationStatus([
      poll("get_delegation_status", "t1"),
    ])
    expect(out).toHaveLength(1)
    expect(pollsOf(out[0])).toHaveLength(1)
  })

  it("groups interleaved parallel polls together (consecutive run)", () => {
    const out = groupConsecutiveDelegationStatus([
      poll("get_delegation_status", "t1"),
      poll("get_delegation_status", "t2"),
      poll("get_delegation_status", "t1"),
    ])
    expect(out).toHaveLength(1)
    expect(pollsOf(out[0])).toHaveLength(3)
  })

  it("does NOT merge polls separated by text", () => {
    const out = groupConsecutiveDelegationStatus([
      poll("get_delegation_status", "t1"),
      text,
      poll("get_delegation_status", "t1"),
    ])
    expect(out.map((p) => p.type)).toEqual([
      "delegation-status-group",
      "text",
      "delegation-status-group",
    ])
  })

  it("breaks the run on delegate_to_agent and cancel_delegation", () => {
    const out = groupConsecutiveDelegationStatus([
      poll("get_delegation_status", "t1"),
      poll("delegate_to_agent", "t2"),
      poll("get_delegation_status", "t1"),
      poll("cancel_delegation", "t1"),
      poll("get_delegation_status", "t1"),
    ])
    expect(out.map((p) => p.type)).toEqual([
      "delegation-status-group",
      "tool-call",
      "delegation-status-group",
      "tool-call",
      "delegation-status-group",
    ])
  })

  it("matches host-prefixed historical names", () => {
    const out = groupConsecutiveDelegationStatus([
      poll("mcp__codeg-mcp__get_delegation_status", "t1"),
      poll("mcp__codeg-delegate__get_delegation_status", "t1"),
      poll("codeg-delegate/get_delegation_status", "t1"),
    ])
    expect(out).toHaveLength(1)
    expect(pollsOf(out[0])).toHaveLength(3)
  })

  it("leaves a non-status part untouched", () => {
    const toolGroup: AdaptedContentPart = {
      type: "tool-group",
      items: [],
      isStreaming: false,
    }
    expect(groupConsecutiveDelegationStatus([toolGroup])).toEqual([toolGroup])
  })
})

describe("groupConsecutiveToolCalls", () => {
  it("leaves Codex goal calls standalone so they can render as cards", () => {
    const out = groupConsecutiveToolCalls([
      poll("create_goal"),
      poll("exec_command"),
      poll("update_goal"),
    ])

    expect(out.map((p) => p.type)).toEqual([
      "tool-call",
      "tool-group",
      "tool-call",
    ])
  })

  it("leaves plan-mode tools standalone (no '思考 N 次' tool-group)", () => {
    const out = groupConsecutiveToolCalls([
      poll("read"),
      poll("EnterPlanMode"),
      poll("read"),
    ])

    expect(out.map((p) => p.type)).toEqual([
      "tool-group",
      "tool-call",
      "tool-group",
    ])
  })

  it("does not wrap a lone plan-mode tool into a group", () => {
    expect(
      groupConsecutiveToolCalls([poll("EnterPlanMode")]).map((p) => p.type)
    ).toEqual(["tool-call"])
    expect(
      groupConsecutiveToolCalls([poll("switch_mode")]).map((p) => p.type)
    ).toEqual(["tool-call"])
  })
})

describe("dropHiddenFeedbackChecks", () => {
  const FEEDBACK_OUT =
    'Wall time: 0.003 seconds\nOutput:\n{"count":1,"feedback":[{"created_at":"2026-06-09T07:47:12Z","text":"还有package"}]}'
  const NO_FEEDBACK_OUT =
    'Wall time: 0.002 seconds\nOutput:\n{"count":0,"feedback":[]}'

  function feedbackCheck(
    output: string | null,
    extra: Partial<AdaptedToolCallPart> = {}
  ): AdaptedToolCallPart {
    return {
      type: "tool-call",
      toolCallId: `cuf:${output ?? "pending"}`,
      toolName: "check_user_feedback",
      input: "{}",
      state: output ? "output-available" : "input-available",
      output,
      ...extra,
    }
  }

  it("drops no-feedback, in-flight, and unparseable checks", () => {
    const out = dropHiddenFeedbackChecks([
      feedbackCheck(NO_FEEDBACK_OUT),
      feedbackCheck(null),
      feedbackCheck("some unrelated output"),
    ])
    expect(out).toHaveLength(0)
  })

  it("keeps checks that received feedback", () => {
    const part = feedbackCheck(FEEDBACK_OUT)
    expect(dropHiddenFeedbackChecks([part])).toEqual([part])
  })

  it("keeps errored checks so failures aren't swallowed", () => {
    const errored = feedbackCheck(null, {
      state: "output-error",
      errorText: "boom",
    })
    expect(dropHiddenFeedbackChecks([errored])).toEqual([errored])
  })

  it("never touches non-feedback parts", () => {
    const parts: AdaptedContentPart[] = [
      poll("exec_command"),
      text,
      poll("read"),
    ]
    expect(dropHiddenFeedbackChecks(parts)).toEqual(parts)
  })

  it("collapses neighbours into one group once a no-op check is dropped", () => {
    const grouped = groupConsecutiveToolCalls(
      dropHiddenFeedbackChecks([
        poll("exec_command"),
        feedbackCheck(NO_FEEDBACK_OUT),
        poll("read"),
      ])
    )
    // Without the drop, the standalone check would split this into two groups.
    expect(grouped.map((p) => p.type)).toEqual(["tool-group"])
  })

  it("breaks the run when a check carries feedback", () => {
    const grouped = groupConsecutiveToolCalls(
      dropHiddenFeedbackChecks([
        poll("exec_command"),
        feedbackCheck(FEEDBACK_OUT),
        poll("read"),
      ])
    )
    expect(grouped.map((p) => p.type)).toEqual([
      "tool-group",
      "tool-call",
      "tool-group",
    ])
  })
})

describe("dropEmptyInFlightToolCalls", () => {
  // A generic, still-running tool call (arg-less by default) — the shape
  // claude-agent-acp emits at content_block_start before the args arrive.
  function running(
    toolName: string,
    extra: Partial<AdaptedToolCallPart> = {}
  ): AdaptedToolCallPart {
    return {
      type: "tool-call",
      toolCallId: `${toolName}:live`,
      toolName,
      input: "{}",
      state: "input-available",
      ...extra,
    }
  }

  it("drops empty, still-running generic tool calls in every empty shape", () => {
    expect(
      dropEmptyInFlightToolCalls([
        running("bash", { input: "{}" }),
        running("bash", { input: "" }),
        running("bash", { input: null }),
        running("bash", { input: "  " }),
      ])
    ).toHaveLength(0)
  })

  it("keeps an in-flight call that already carries a real command", () => {
    const live = running("bash", {
      input: JSON.stringify({ command: "pnpm build" }),
    })
    expect(dropEmptyInFlightToolCalls([live])).toEqual([live])
  })

  it("keeps an empty in-flight call that is already streaming output", () => {
    const live = running("bash", { input: "{}", output: "...building..." })
    expect(dropEmptyInFlightToolCalls([live])).toEqual([live])
  })

  it("keeps an in-flight call that surfaced an error", () => {
    const live = running("bash", { input: "{}", errorText: "boom" })
    expect(dropEmptyInFlightToolCalls([live])).toEqual([live])
  })

  it("keeps DB-history parts that carry no forwarded status", () => {
    // Persisted rows have no `toolStatus` (undefined) → treated as settled, so
    // an arg-less-but-completed historical tool is never mistaken for an orphan.
    const hist = running("bash", { input: "{}", state: "output-available" })
    expect(dropEmptyInFlightToolCalls([hist])).toEqual([hist])
  })

  it("drops a promoted orphan: state settled to output-available but status unsettled", () => {
    // The COMPLETE_TURN promotion path: the same unpruned orphan is re-adapted
    // with isStreaming=false, so its state flips to output-available while the
    // forwarded ACP status stays pending/in_progress.
    expect(
      dropEmptyInFlightToolCalls([
        running("bash", {
          input: "{}",
          state: "output-available",
          toolStatus: "pending",
        }),
        running("bash", {
          input: "{}",
          state: "output-available",
          toolStatus: "in_progress",
        }),
      ])
    ).toHaveLength(0)
  })

  it("keeps a promoted part once its status settles (completed/failed)", () => {
    const done = running("bash", {
      input: "{}",
      state: "output-available",
      toolStatus: "completed",
    })
    expect(dropEmptyInFlightToolCalls([done])).toEqual([done])
  })

  it("keeps a promoted orphan that already streamed output", () => {
    const withOutput = running("bash", {
      input: "{}",
      state: "output-available",
      toolStatus: "in_progress",
      output: "...partial...",
    })
    expect(dropEmptyInFlightToolCalls([withOutput])).toEqual([withOutput])
  })

  it("never touches specialized lanes (agent/delegation/ask/background/plan)", () => {
    // Empty + in-flight, but each renders through its own card and handles its
    // own empty polls (see commit 1ddf751b) — this filter must leave them be.
    const lanes: AdaptedContentPart[] = [
      running("get_delegation_status"),
      running("question"),
      running("TaskOutput"),
      running("switch_mode"),
    ]
    expect(dropEmptyInFlightToolCalls(lanes)).toEqual(lanes)
  })

  it("collapses the phantom scenario to a single one-command group", () => {
    // Live turn: one real completed build + two orphaned arg-less bash blocks
    // left by an interrupted/retried attempt. Settled transcript has only the
    // real one, so the live count must converge to it.
    const real: AdaptedToolCallPart = {
      type: "tool-call",
      toolCallId: "bash:real",
      toolName: "bash",
      input: JSON.stringify({ command: "pnpm build 2>&1 | tail -20" }),
      state: "output-available",
      output: "Build succeeded",
    }
    const grouped = groupConsecutiveToolCalls(
      dropEmptyInFlightToolCalls([
        real,
        running("bash", { input: "{}" }),
        running("bash", { input: "{}" }),
      ])
    )
    expect(grouped.map((p) => p.type)).toEqual(["tool-group"])
    const group = grouped[0]
    if (group.type !== "tool-group") throw new Error("expected a tool-group")
    expect(group.items).toHaveLength(1)
    expect(group.items[0].toolCallId).toBe("bash:real")
  })

  it("prunes a promoted orphan end-to-end via adaptMessageTurn (isStreaming=false)", () => {
    // Shape of a localTurn after COMPLETE_TURN: one real completed bash (with a
    // matching result) plus one interrupted arg-less orphan — status still
    // "pending", no result block. Adapted with isStreaming=false (promoted),
    // the orphan's state becomes output-available; only its forwarded status
    // reveals it. The group must still converge to the single real command.
    const adapted = adaptMessageTurn(
      {
        id: "promoted-turn",
        role: "assistant",
        timestamp: "2026-07-23T00:00:00.000Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "tc-real",
            tool_name: "bash",
            input_preview: JSON.stringify({ command: "pnpm build" }),
            status: "completed",
          },
          {
            type: "tool_result",
            tool_use_id: "tc-real",
            output_preview: "Build succeeded",
            is_error: false,
          },
          {
            type: "tool_use",
            tool_use_id: "tc-orphan",
            tool_name: "bash",
            input_preview: "{}",
            status: "pending",
          },
        ],
      },
      {
        attachedResources: "Attached resources",
        toolCallFailed: "Tool failed",
      },
      false
    )
    expect(adapted.content.map((p) => p.type)).toEqual(["tool-group"])
    const group = adapted.content[0]
    if (group.type !== "tool-group") throw new Error("expected a tool-group")
    expect(group.items).toHaveLength(1)
    expect(group.items[0].toolCallId).toBe("tc-real")
  })
})

describe("groupGoalRuns", () => {
  it("wraps create_goal through update_goal with intervening process parts", () => {
    const grouped = groupConsecutiveToolCalls([
      poll("create_goal"),
      text,
      poll("exec_command"),
      poll("update_goal"),
      { type: "text", text: "final answer" },
    ])

    const out = groupGoalRuns(grouped)

    expect(out.map((p) => p.type)).toEqual(["goal-run", "text"])
    const goalRun = goalRunOf(out[0])
    expect(goalRun.start.toolName).toBe("create_goal")
    expect(goalRun.end?.toolName).toBe("update_goal")
    expect(goalRun.items.map((p) => p.type)).toEqual(["text", "tool-group"])
    expect(goalRun.isRunning).toBe(false)
  })

  it("wraps an unfinished goal run as running while streaming", () => {
    const out = groupGoalRuns([poll("create_goal"), text], true)

    expect(out).toHaveLength(1)
    const goalRun = goalRunOf(out[0])
    expect(goalRun.end).toBeNull()
    expect(goalRun.items).toEqual([text])
    expect(goalRun.isRunning).toBe(true)
  })

  it("settles an unfinished goal run when not streaming", () => {
    // codex leaves a `/goal` active without a closing update_goal, so a stopped
    // turn or a reloaded conversation must NOT shimmer the capsule forever.
    const out = groupGoalRuns([poll("create_goal"), text], false)

    expect(out).toHaveLength(1)
    const goalRun = goalRunOf(out[0])
    expect(goalRun.end).toBeNull()
    expect(goalRun.items).toEqual([text])
    expect(goalRun.isRunning).toBe(false)
  })

  it("does not mutate a reopened unfinished goal run when closing across turns", () => {
    const firstText: AdaptedContentPart = {
      type: "text",
      text: "started goal",
    }
    const nextText: AdaptedContentPart = {
      type: "text",
      text: "continued goal",
    }
    const unfinished: AdaptedContentPart = {
      type: "goal-run",
      start: poll("create_goal"),
      end: null,
      items: [firstText],
      isRunning: true,
    }

    const firstMerge = groupGoalRuns([
      unfinished,
      nextText,
      poll("update_goal"),
    ])
    expect(goalRunOf(firstMerge[0]).items).toEqual([firstText, nextText])
    expect(goalRunOf(unfinished).items).toEqual([firstText])

    const secondMerge = groupGoalRuns([
      unfinished,
      nextText,
      poll("update_goal"),
    ])
    expect(goalRunOf(secondMerge[0]).items).toEqual([firstText, nextText])
  })

  it("merges repeated unfinished goal runs into one cross-turn card", () => {
    const firstText: AdaptedContentPart = {
      type: "text",
      text: "started goal",
    }
    const nextText: AdaptedContentPart = {
      type: "text",
      text: "continued goal",
    }
    const firstRun: AdaptedContentPart = {
      type: "goal-run",
      start: poll("create_goal"),
      end: null,
      items: [firstText],
      isRunning: true,
    }
    const repeatedRun: AdaptedContentPart = {
      type: "goal-run",
      start: poll("create_goal"),
      end: null,
      items: [],
      isRunning: true,
    }

    const out = groupGoalRuns([firstRun, repeatedRun, nextText])

    expect(out).toHaveLength(1)
    expect(goalRunOf(out[0]).items).toEqual([firstText, nextText])
  })

  it("closes an active cross-turn goal when the next turn already has a completed goal run", () => {
    const firstText: AdaptedContentPart = {
      type: "text",
      text: "started goal",
    }
    const toolGroup: AdaptedContentPart = {
      type: "tool-group",
      items: [poll("exec_command")],
      isStreaming: false,
    }
    const finalText: AdaptedContentPart = {
      type: "text",
      text: "final answer",
    }
    const unfinished: AdaptedContentPart = {
      type: "goal-run",
      start: poll("create_goal"),
      end: null,
      items: [firstText],
      isRunning: true,
    }
    const completed: AdaptedContentPart = {
      type: "goal-run",
      start: poll("create_goal"),
      end: poll("update_goal"),
      items: [toolGroup],
      isRunning: false,
    }

    const out = groupGoalRuns([unfinished, completed, finalText])

    expect(out.map((p) => p.type)).toEqual(["goal-run", "text"])
    expect(goalRunOf(out[0]).items).toEqual([firstText, toolGroup])
    expect(out[1]).toEqual(finalText)
  })
})

describe("adaptMessageTurn goal update text", () => {
  it("converts streaming Codex goal update text into a running goal card", () => {
    const adapted = adaptMessageTurn(
      {
        id: "live-turn",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "text",
            text: "我会先建立这个目标。\nGoal updated (active): 分析 README 文件\n",
          },
          {
            type: "tool_use",
            tool_use_id: "exec-1",
            tool_name: "exec_command",
            input_preview: JSON.stringify({ cmd: "sed -n '1,120p' README.md" }),
          },
          {
            type: "tool_result",
            tool_use_id: "exec-1",
            output_preview: "README content",
            is_error: false,
          },
          {
            type: "text",
            text: "Goal updated (active): 分析 README 文件\n",
          },
        ],
      },
      {
        attachedResources: "Attached resources",
        toolCallFailed: "Tool failed",
      },
      true
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["text", "goal-run"])
    expect(adapted.content[0]).toEqual({
      type: "text",
      text: "我会先建立这个目标。",
    })
    const goalRun = goalRunOf(adapted.content[1])
    expect(goalRun.start.toolName).toBe("create_goal")
    expect(goalRun.end).toBeNull()
    expect(goalRun.isRunning).toBe(true)
    expect(goalRun.items.map((p) => p.type)).toEqual(["tool-group"])
    expect(JSON.parse(goalRun.start.input ?? "{}")).toEqual({
      objective: "分析 README 文件",
    })
  })

  it("keeps final text outside a completed goal when a stale active update arrives after completion", () => {
    const adapted = adaptMessageTurn(
      {
        id: "live-turn-complete",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "text",
            text: "Goal updated (active): 分析 README 文件\n",
          },
          {
            type: "tool_use",
            tool_use_id: "exec-1",
            tool_name: "exec_command",
            input_preview: JSON.stringify({ cmd: "sed -n '1,120p' README.md" }),
          },
          {
            type: "tool_result",
            tool_use_id: "exec-1",
            output_preview: "README content",
            is_error: false,
          },
          {
            type: "text",
            text:
              "Goal updated (complete): 分析 README 文件\n" +
              "Goal updated (active): 分析 README 文件\n" +
              "已完成 README 分析。",
          },
        ],
      },
      {
        attachedResources: "Attached resources",
        toolCallFailed: "Tool failed",
      },
      true
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["goal-run", "text"])
    const goalRun = goalRunOf(adapted.content[0])
    expect(goalRun.end?.toolName).toBe("update_goal")
    expect(goalRun.isRunning).toBe(false)
    expect(adapted.content[1]).toEqual({
      type: "text",
      text: "已完成 README 分析。",
    })
  })

  it("does not absorb unseparated prose and later goal markers into the objective", () => {
    const adapted = adaptMessageTurn(
      {
        id: "live-turn-concatenated",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "text",
            text:
              "Goal updated (active): 分析 README 文件" +
              "我也顺手对照了 `package.json` 和 `app` 目录。" +
              "Goal updated (active): 分析 README 文件" +
              "Goal updated (complete): 分析 README 文件" +
              "已分析 [README.md](/Users/xggz/my/my-app/README.md:1)。",
          },
        ],
      },
      {
        attachedResources: "Attached resources",
        toolCallFailed: "Tool failed",
      },
      true
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["goal-run", "text"])
    const goalRun = goalRunOf(adapted.content[0])
    expect(JSON.parse(goalRun.start.input ?? "{}")).toEqual({
      objective: "分析 README 文件",
    })
    expect(JSON.parse(goalRun.end?.output ?? "{}")).toMatchObject({
      goal: {
        objective: "分析 README 文件",
        status: "complete",
      },
    })
    expect(goalRun.items).toEqual([
      {
        type: "text",
        text: "我也顺手对照了 `package.json` 和 `app` 目录。",
      },
    ])
    expect(adapted.content[1]).toEqual({
      type: "text",
      text: "已分析 [README.md](/Users/xggz/my/my-app/README.md:1)。",
    })
  })

  it("keeps the known streaming objective when later text is appended without a separator", () => {
    const adapter = createMessageTurnAdapter()
    const textLabels = {
      attachedResources: "Attached resources",
      toolCallFailed: "Tool failed",
    }
    const firstTurn = {
      id: "live-turn-single-marker",
      role: "assistant" as const,
      timestamp: "2026-06-02T00:00:00.000Z",
      blocks: [
        {
          type: "text" as const,
          text: "Goal updated (active): 分析 README 文件",
        },
      ],
    }
    const secondTurn = {
      ...firstTurn,
      blocks: [
        {
          type: "text" as const,
          text:
            "Goal updated (active): 分析 README 文件" +
            "我也顺手对照了 `package.json` 和 `app` 目录。",
        },
      ],
    }

    adapter.adapt([firstTurn], textLabels, new Set([0]))
    const [adapted] = adapter.adapt([secondTurn], textLabels, new Set([0]))

    expect(adapted.content.map((p) => p.type)).toEqual(["goal-run"])
    const goalRun = goalRunOf(adapted.content[0])
    expect(JSON.parse(goalRun.start.input ?? "{}")).toEqual({
      objective: "分析 README 文件",
    })
    expect(goalRun.items).toEqual([
      {
        type: "text",
        text: "我也顺手对照了 `package.json` 和 `app` 目录。",
      },
    ])
  })

  it("does not absorb adjacent Chinese prose into a single active marker objective", () => {
    const adapted = adaptMessageTurn(
      {
        id: "live-turn-single-marker-prose",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "text",
            text:
              "Goal updated (active): 分析 README 文件" +
              "我也顺手对照了 `package.json` 和 `app` 目录。",
          },
        ],
      },
      {
        attachedResources: "Attached resources",
        toolCallFailed: "Tool failed",
      },
      true
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["goal-run"])
    const goalRun = goalRunOf(adapted.content[0])
    expect(JSON.parse(goalRun.start.input ?? "{}")).toEqual({
      objective: "分析 README 文件",
    })
    expect(goalRun.items).toEqual([
      {
        type: "text",
        text: "我也顺手对照了 `package.json` 和 `app` 目录。",
      },
    ])
  })
})

describe("mergeAdjacentDelegationStatusGroups", () => {
  const group = (taskId: string): AdaptedContentPart => ({
    type: "delegation-status-group",
    polls: [poll("get_delegation_status", taskId)],
  })

  it("merges adjacent groups (cross-turn concatenation)", () => {
    const out = mergeAdjacentDelegationStatusGroups([group("t1"), group("t1")])
    expect(out).toHaveLength(1)
    expect(pollsOf(out[0])).toHaveLength(2)
  })

  it("does not merge groups separated by another part", () => {
    const out = mergeAdjacentDelegationStatusGroups([
      group("t1"),
      text,
      group("t1"),
    ])
    expect(out.map((p) => p.type)).toEqual([
      "delegation-status-group",
      "text",
      "delegation-status-group",
    ])
  })
})

describe("adaptMessageTurn plan handling", () => {
  const msgText = {
    attachedResources: "Attached resources",
    toolCallFailed: "Tool failed",
  }

  it("renders a live synthetic plan block as a plan part (not reasoning) and marks the last block streaming", () => {
    const adapted = adaptMessageTurn(
      {
        id: "live-plan",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "plan",
            entries: [
              { content: "Step A", status: "in_progress", priority: "high" },
              { content: "Step B", status: "completed", priority: "low" },
            ],
          },
        ],
      },
      msgText,
      true
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["plan"])
    const plan = adapted.content[0]
    if (plan.type !== "plan") throw new Error("expected a plan part")
    expect(plan.isStreaming).toBe(true)
    expect(plan.entries).toEqual([
      { content: "Step A", status: "in_progress", priority: "high" },
      { content: "Step B", status: "completed", priority: "low" },
    ])
  })

  it("drops an empty redacted-thinking block and renders EnterPlanMode standalone (history)", () => {
    const adapted = adaptMessageTurn(
      {
        id: "plan-mode",
        role: "assistant",
        timestamp: "2026-06-29T00:00:00.000Z",
        blocks: [
          { type: "thinking", text: "" },
          { type: "text", text: "I'll plan it first" },
          {
            type: "tool_use",
            tool_use_id: "epm-1",
            tool_name: "EnterPlanMode",
            input_preview: "{}",
          },
        ],
      },
      msgText,
      false
    )

    // Empty thinking is dropped; EnterPlanMode is a standalone tool-call (not a
    // "思考 N 次" tool-group).
    expect(adapted.content.map((p) => p.type)).toEqual(["text", "tool-call"])
    const tc = adapted.content[1]
    if (tc.type !== "tool-call") throw new Error("expected a tool-call")
    expect(tc.toolName).toBe("EnterPlanMode")
  })

  it("keeps an empty thinking block while streaming (live Thinking… indicator)", () => {
    const adapted = adaptMessageTurn(
      {
        id: "plan-mode-live",
        role: "assistant",
        timestamp: "2026-06-29T00:00:00.000Z",
        blocks: [{ type: "thinking", text: "" }],
      },
      msgText,
      true
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["reasoning"])
    const reasoning = adapted.content[0]
    if (reasoning.type !== "reasoning") throw new Error("expected a reasoning")
    expect(reasoning.isStreaming).toBe(true)
  })

  it("converts a persisted TodoWrite tool_use (+ its result) into a single plan part with no orphan tool-result", () => {
    const adapted = adaptMessageTurn(
      {
        id: "hist-plan",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "todo-1",
            tool_name: "TodoWrite",
            input_preview: JSON.stringify({
              todos: [
                { content: "X", status: "pending", priority: "medium" },
                { content: "Y", status: "completed", priority: "high" },
              ],
            }),
          },
          {
            type: "tool_result",
            tool_use_id: "todo-1",
            output_preview: "Todos have been modified successfully",
            is_error: false,
          },
        ],
      },
      msgText,
      false
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["plan"])
    expect(adapted.content.some((p) => p.type === "tool-result")).toBe(false)
    const plan = adapted.content[0]
    if (plan.type !== "plan") throw new Error("expected a plan part")
    expect(plan.isStreaming).toBe(false)
    expect(plan.entries).toEqual([
      { content: "X", status: "pending", priority: "medium" },
      { content: "Y", status: "completed", priority: "high" },
    ])
  })

  it("does NOT convert a TodoWrite tool_use while streaming (live plan source is the synthetic block)", () => {
    const adapted = adaptMessageTurn(
      {
        id: "live-todo",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "todo-1",
            tool_name: "TodoWrite",
            input_preview: JSON.stringify({
              todos: [{ content: "X", status: "pending", priority: "medium" }],
            }),
          },
        ],
      },
      msgText,
      true
    )

    expect(adapted.content.every((p) => p.type !== "plan")).toBe(true)
  })

  it("falls back to a normal tool card when a plan-like tool has unparsable input", () => {
    const adapted = adaptMessageTurn(
      {
        id: "hist-bad",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "todo-1",
            tool_name: "TodoWrite",
            input_preview: "not json",
          },
        ],
      },
      msgText,
      false
    )

    expect(adapted.content.every((p) => p.type !== "plan")).toBe(true)
  })

  it("converts a persisted Kimi Code TodoList write (title/status shape) into a single plan part", () => {
    const adapted = adaptMessageTurn(
      {
        id: "hist-kimi-plan",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "kc-todo-1",
            tool_name: "TodoList",
            input_preview: JSON.stringify({
              todos: [
                { status: "in_progress", title: "Confirm 401 behavior" },
                { status: "pending", title: "Unify request.js" },
                { status: "done", title: "Verify changes" },
              ],
            }),
          },
          {
            type: "tool_result",
            tool_use_id: "kc-todo-1",
            output_preview: "Todo list updated.",
            is_error: false,
          },
        ],
      },
      msgText,
      false
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["plan"])
    expect(adapted.content.some((p) => p.type === "tool-result")).toBe(false)
    const plan = adapted.content[0]
    if (plan.type !== "plan") throw new Error("expected a plan part")
    expect(plan.entries).toEqual([
      {
        content: "Confirm 401 behavior",
        status: "in_progress",
        priority: "medium",
      },
      { content: "Unify request.js", status: "pending", priority: "medium" },
      { content: "Verify changes", status: "completed", priority: "medium" },
    ])
  })

  it.each([
    ["read", "{}"],
    ["clear", JSON.stringify({ todos: [] })],
  ])(
    "keeps a persisted Kimi TodoList %s (no entries) as a tool card, not a plan part",
    (_label, inputPreview) => {
      const adapted = adaptMessageTurn(
        {
          id: "hist-kimi-noop",
          role: "assistant",
          timestamp: "2026-06-02T00:00:00.000Z",
          blocks: [
            {
              type: "tool_use",
              tool_use_id: "kc-todo-1",
              tool_name: "TodoList",
              input_preview: inputPreview,
            },
            {
              type: "tool_result",
              tool_use_id: "kc-todo-1",
              output_preview: "Todo list (empty).",
              is_error: false,
            },
          ],
        },
        msgText,
        false
      )

      expect(adapted.content.every((p) => p.type !== "plan")).toBe(true)
      // The non-write TodoList renders through the normal tool-card path
      // (wrapped in a tool-group by groupConsecutiveToolCalls).
      expect(adapted.content.some((p) => p.type === "tool-group")).toBe(true)
    }
  )
})

describe("adaptMessageTurn — image tool results", () => {
  const msgText = {
    attachedResources: "Attached resources",
    toolCallFailed: "Tool failed",
  }

  it("renders a Read whose result carries an image as a generated-image part (matching the live path), not a Read tool card", () => {
    const adapted = adaptMessageTurn(
      {
        id: "read-img",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "toolu_1",
            tool_name: "Read",
            input_preview: JSON.stringify({ file_path: "clean-v1.png" }),
          },
          {
            type: "tool_result",
            tool_use_id: "toolu_1",
            output_preview: null,
            is_error: false,
            images: [{ data: "QUJD", mime_type: "image/png" }],
          },
        ],
      },
      msgText,
      false
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["generated-image"])
    expect(adapted.content.some((p) => p.type === "tool-result")).toBe(false)
    expect(adapted.content.some((p) => p.type === "tool-group")).toBe(false)
    const part = adapted.content[0]
    if (part.type !== "generated-image") {
      throw new Error("expected a generated-image part")
    }
    expect(part.image).not.toBeNull()
    expect(part.image?.data).toBe("QUJD")
    expect(part.image?.mime_type).toBe("image/png")
    expect(part.revisedPrompt).toBeNull()
  })

  it("emits one generated-image part per image (multi-page PDF read)", () => {
    const adapted = adaptMessageTurn(
      {
        id: "read-pdf",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "toolu_2",
            tool_name: "Read",
            input_preview: JSON.stringify({ file_path: "doc.pdf" }),
          },
          {
            type: "tool_result",
            tool_use_id: "toolu_2",
            output_preview: null,
            is_error: false,
            images: [
              { data: "UAGE1", mime_type: "image/png" },
              { data: "UAGE2", mime_type: "image/png" },
            ],
          },
        ],
      },
      msgText,
      false
    )

    expect(adapted.content.map((p) => p.type)).toEqual([
      "generated-image",
      "generated-image",
    ])
  })

  it("leaves a normal text Read result as a tool card (no regression)", () => {
    const adapted = adaptMessageTurn(
      {
        id: "read-text",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "toolu_3",
            tool_name: "Read",
            input_preview: JSON.stringify({ file_path: "notes.txt" }),
          },
          {
            type: "tool_result",
            tool_use_id: "toolu_3",
            output_preview: "hello world",
            is_error: false,
          },
        ],
      },
      msgText,
      false
    )

    expect(adapted.content.some((p) => p.type === "generated-image")).toBe(
      false
    )
    // A lone tool call folds into a tool-group.
    const group = adapted.content.find((p) => p.type === "tool-group")
    expect(group).toBeDefined()
    if (group?.type !== "tool-group") throw new Error("expected tool-group")
    expect(group.items[0]?.toolName).toBe("Read")
  })

  it("keeps the running tool card (spinner) when the image result's tool is still in-flight", () => {
    const adapted = adaptMessageTurn(
      {
        id: "read-img-live",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "toolu_4",
            tool_name: "Read",
            input_preview: JSON.stringify({ file_path: "clean.png" }),
          },
          {
            type: "tool_result",
            tool_use_id: "toolu_4",
            output_preview: null,
            is_error: false,
            images: [{ data: "QUJD", mime_type: "image/png" }],
          },
        ],
      },
      msgText,
      true,
      new Set(["toolu_4"])
    )

    expect(adapted.content.some((p) => p.type === "generated-image")).toBe(
      false
    )
    const group = adapted.content.find((p) => p.type === "tool-group")
    if (group?.type !== "tool-group") throw new Error("expected tool-group")
    expect(group.items[0]?.state).toBe("input-available")
  })
})

describe("extractUserResourcesFromText — codeg references stay inline", () => {
  it("keeps a codeg://agent link inline (the @-prefixed label no longer lifts it to a chip)", () => {
    const input = "ask [@Codex](codeg://agent/codex) to review"
    const { text, resources } = extractUserResourcesFromText(input)
    expect(resources).toEqual([])
    expect(text).toBe(input)
  })

  it("keeps codeg://session and codeg://commit links inline", () => {
    const session = extractUserResourcesFromText(
      "see [#42](codeg://session/claude_code_abc)"
    )
    expect(session.resources).toEqual([])
    expect(session.text).toBe("see [#42](codeg://session/claude_code_abc)")

    const commit = extractUserResourcesFromText(
      "from [a1b2c3d](codeg://commit/%2Frepo@a1b2c3ddeadbeef)"
    )
    expect(commit.resources).toEqual([])
    expect(commit.text).toBe(
      "from [a1b2c3d](codeg://commit/%2Frepo@a1b2c3ddeadbeef)"
    )
  })

  it("keeps a codeg://session link inline even when its label starts with @ (a session titled '@…')", () => {
    const input = "ping [@周报](codeg://session/codex_99)"
    const { text, resources } = extractUserResourcesFromText(input)
    expect(resources).toEqual([])
    expect(text).toBe(input)
  })

  it("keeps a file:// link inline AND copies it to the resource row", () => {
    const { text, resources } = extractUserResourcesFromText(
      "look at [foo.ts](file:///x/foo.ts) here"
    )
    // Copied to the row (original grey-chip attachment list)…
    expect(resources).toEqual([
      { name: "foo.ts", uri: "file:///x/foo.ts", mime_type: null },
    ])
    // …and left in place in the prose so it still renders as an inline badge.
    expect(text).toBe("look at [foo.ts](file:///x/foo.ts) here")
  })

  it("chips a file:// link with a space (CommonMark angle-bracket destination)", () => {
    // `referenceToMarkdown` wraps uris with spaces/parens in <…>; the row must
    // still pick the file up (the bare-destination regex would have missed it).
    const { text, resources } = extractUserResourcesFromText(
      "see [a b.ts](<file:///x/a b.ts>) please"
    )
    expect(resources).toEqual([
      { name: "a b.ts", uri: "file:///x/a b.ts", mime_type: null },
    ])
    // The original bracketed form is preserved inline (Streamdown parses it).
    expect(text).toBe("see [a b.ts](<file:///x/a b.ts>) please")
  })

  it("unescapes a filename with parentheses for the row chip (e.g. `Screenshot (1).png`)", () => {
    // `referenceToMarkdown` backslash-escapes label punctuation and wraps the
    // space/paren uri in <…>, so the text carries `[Screenshot \(1\).png](<…>)`.
    // The chip name must read cleanly, not leak the escaping backslashes.
    const { text, resources } = extractUserResourcesFromText(
      "look at [Screenshot \\(1\\).png](<file:///x/Screenshot (1).png>) here"
    )
    expect(resources).toEqual([
      {
        name: "Screenshot (1).png",
        uri: "file:///x/Screenshot (1).png",
        mime_type: null,
      },
    ])
    // Inline form (with its escaping) is preserved for Streamdown to render.
    expect(text).toBe(
      "look at [Screenshot \\(1\\).png](<file:///x/Screenshot (1).png>) here"
    )
  })

  it("chips a filename containing `]` (escaped as `\\]` in the label)", () => {
    // The escaped `]` would defeat a `[^\]]+` label regex, dropping the chip; the
    // escape-aware regex matches it and the unescaped name reads `a]b.ts`.
    const { text, resources } = extractUserResourcesFromText(
      "open [a\\]b.ts](file:///x/a]b.ts) now"
    )
    expect(resources).toEqual([
      { name: "a]b.ts", uri: "file:///x/a]b.ts", mime_type: null },
    ])
    expect(text).toBe("open [a\\]b.ts](file:///x/a]b.ts) now")
  })

  it("preserves consecutive spaces in a file path verbatim (no whitespace collapse)", () => {
    // A filename with two spaces must round-trip byte-for-byte: collapsing the
    // run would rewrite the inline link's path and break the badge target.
    const { text, resources } = extractUserResourcesFromText(
      "open [a  b.ts](<file:///x/a  b.ts>) now"
    )
    expect(resources).toEqual([
      { name: "a  b.ts", uri: "file:///x/a  b.ts", mime_type: null },
    ])
    expect(text).toBe("open [a  b.ts](<file:///x/a  b.ts>) now")
  })

  it("keeps a leading `@` in a file name (scoped-package path), not a mention", () => {
    // A file whose name starts with `@` (e.g. a scoped-package dir) must keep the
    // `@` — the file uri takes precedence over the `@`-mention heuristic.
    const { text, resources } = extractUserResourcesFromText(
      "see [@scope](file:///repo/node_modules/@scope) here"
    )
    expect(resources).toEqual([
      {
        name: "@scope",
        uri: "file:///repo/node_modules/@scope",
        mime_type: null,
      },
    ])
    expect(text).toBe("see [@scope](file:///repo/node_modules/@scope) here")
  })

  it("does not let the blocked-mention pass corrupt a file link containing `[blocked]`", () => {
    // Pathological filename `@foo [blocked].txt`: the blocked-`@mention` pre-pass
    // must NOT run inside the kept file link, so the inline link survives verbatim
    // and the chip name is the real (unescaped) filename.
    const { text, resources } = extractUserResourcesFromText(
      "see [@foo \\[blocked\\].txt](<file:///x/@foo [blocked].txt>) ok"
    )
    expect(resources).toEqual([
      {
        name: "@foo [blocked].txt",
        uri: "file:///x/@foo [blocked].txt",
        mime_type: null,
      },
    ])
    expect(text).toBe(
      "see [@foo \\[blocked\\].txt](<file:///x/@foo [blocked].txt>) ok"
    )
  })

  it("strips a real blocked @-mention in prose while keeping an adjacent file link", () => {
    const { text, resources } = extractUserResourcesFromText(
      "@secret.txt [blocked: outside] see [foo.ts](file:///x/foo.ts)"
    )
    expect(resources).toEqual([
      { name: "secret.txt", uri: "secret.txt", mime_type: null },
      { name: "foo.ts", uri: "file:///x/foo.ts", mime_type: null },
    ])
    expect(text).toBe("see [foo.ts](file:///x/foo.ts)")
  })

  it("does not corrupt a typed <file://…> angle-string containing [blocked]", () => {
    // A bare angle-wrapped uri is not a Markdown link; the blocked-mention pass
    // must skip `<…>` spans so it can't strip an `@…[blocked…]` substring out of
    // a typed uri and rewrite the path.
    const { text, resources } = extractUserResourcesFromText(
      "raw <file:///x/@foo [blocked].txt> ok"
    )
    expect(resources).toEqual([])
    expect(text).toBe("raw <file:///x/@foo [blocked].txt> ok")
  })

  it("chips a codeg://embedded attachment while keeping its inert badge inline", () => {
    const { text, resources } = extractUserResourcesFromText(
      "here [report.pdf](codeg://embedded/abc-123) ok"
    )
    expect(resources).toEqual([
      { name: "report.pdf", uri: "codeg://embedded/abc-123", mime_type: null },
    ])
    expect(text).toBe("here [report.pdf](codeg://embedded/abc-123) ok")
  })

  it("still lifts blocked @-mentions to the resource list", () => {
    const { resources } = extractUserResourcesFromText(
      "@secret.txt [blocked: outside workspace]"
    )
    expect(resources).toEqual([
      { name: "secret.txt", uri: "secret.txt", mime_type: null },
    ])
  })

  it("keeps both file:// and session links inline; only the file is also chipped", () => {
    const { text, resources } = extractUserResourcesFromText(
      "compare [foo.ts](file:///x/foo.ts) with [#42](codeg://session/codex_abc)"
    )
    expect(resources).toEqual([
      { name: "foo.ts", uri: "file:///x/foo.ts", mime_type: null },
    ])
    expect(text).toContain("[#42](codeg://session/codex_abc)")
    expect(text).toContain("[foo.ts](file:///x/foo.ts)")
  })

  it("recovers a file chip after stray/unbalanced brackets in prose", () => {
    // The unmatched `[oops` must not swallow the later real file reference.
    const { text, resources } = extractUserResourcesFromText(
      "text [oops [still open] [foo.ts](file:///x/foo.ts)"
    )
    expect(resources).toEqual([
      { name: "foo.ts", uri: "file:///x/foo.ts", mime_type: null },
    ])
    expect(text).toContain("[foo.ts](file:///x/foo.ts)")
  })

  it("ignores an empty-label [](file://…) link, adding no chip", () => {
    const { text, resources } = extractUserResourcesFromText(
      "see [](file:///x/foo.ts) ok"
    )
    expect(resources).toEqual([])
    expect(text).toBe("see [](file:///x/foo.ts) ok")
  })
})

describe("adaptMessageTurn — user reference resources", () => {
  const msgText = {
    attachedResources: "Attached resources",
    toolCallFailed: "Tool failed",
  }

  it("keeps an agent reference inline in the user turn (no chip row)", () => {
    const adapted = adaptMessageTurn(
      {
        id: "u1",
        role: "user",
        timestamp: "2026-06-11T00:00:00.000Z",
        blocks: [
          { type: "text", text: "ask [@Codex](codeg://agent/codex) to review" },
        ],
      },
      msgText
    )

    expect(adapted.userResources).toBeUndefined()
    expect(adapted.content).toHaveLength(1)
    const part = adapted.content[0]
    if (part.type !== "text") throw new Error("expected a text part")
    expect(part.text).toContain("[@Codex](codeg://agent/codex)")
  })

  it("chips a folded file link AND keeps it inline as a badge; session stays inline", () => {
    // Mirrors the backend fold: prose+session in one text block, the file
    // resource_link folded to a trailing `[name](uri)` text block. The file is
    // copied to the row AND kept inline (rendered as an inline file badge).
    const adapted = adaptMessageTurn(
      {
        id: "u2",
        role: "user",
        timestamp: "2026-06-11T00:00:00.000Z",
        blocks: [
          {
            type: "text",
            text: "compare these [#42](codeg://session/codex_abc)",
          },
          { type: "text", text: "[foo.ts](file:///x/foo.ts)" },
        ],
      },
      msgText
    )

    expect(adapted.userResources).toEqual([
      { name: "foo.ts", uri: "file:///x/foo.ts", mime_type: null },
    ])
    const joined = adapted.content
      .map((p) => (p.type === "text" ? p.text : ""))
      .join("\n")
    expect(joined).toContain("[#42](codeg://session/codex_abc)")
    expect(joined).toContain("[foo.ts](file:///x/foo.ts)")
  })
})
