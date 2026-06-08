import { describe, expect, it } from "vitest"

import { inferLiveToolName, normalizeToolName } from "./tool-call-normalization"

describe("inferLiveToolName meta.claudeCode.toolName override", () => {
  it("returns memory_recall for synthesized recall events without rawInput", () => {
    // Mirrors what claude-agent-acp >=0.37 emits for memory recall:
    // title carries the human-readable count, kind borrows the file-read
    // category, rawInput is null. Only the meta field knows the real name.
    expect(
      inferLiveToolName({
        title: "Recalled 3 memories",
        kind: "read",
        rawInput: null,
        meta: { claudeCode: { toolName: "memory_recall" } },
      })
    ).toBe("memory_recall")

    expect(
      inferLiveToolName({
        title: "Recalled synthesized memory",
        kind: "read",
        rawInput: null,
        meta: { claudeCode: { toolName: "memory_recall" } },
      })
    ).toBe("memory_recall")
  })

  it("falls back to title-based inference when no meta is provided", () => {
    // Pre-0.37 traffic / non-Claude agents have no meta.claudeCode.toolName.
    // The legacy paths must keep working.
    expect(
      inferLiveToolName({
        title: "Recalled 3 memories",
        kind: "read",
        rawInput: null,
      })
    ).not.toBe("memory_recall")
  })

  it("returns canonical lower-case 'agent' for the SDK Agent tool before rawInput streams in", () => {
    // claude-agent-acp reports the Agent/Task tool as `Agent` (capitalised) and
    // often emits the initial ToolCall before `rawInput` (which carries
    // `subagent_type`) is available. The metaToolName fallback must return the
    // canonical lower-case `agent` so the live agent-card nesting check
    // (`getToolName(...) === "agent"`) recognises it and child tool calls nest
    // under the card. A capitalised `Agent` slipped past that check, leaving the
    // children un-nested and the card stuck on its placeholder title.
    expect(
      inferLiveToolName({
        title: "Explore the codebase",
        kind: "other",
        rawInput: null,
        meta: { claudeCode: { toolName: "Agent" } },
      })
    ).toBe("agent")
  })

  it("keeps memory_recall intact when lower-casing the metaToolName fallback", () => {
    // Guard: the lower-case fix must NOT route metaToolName through
    // `normalizeToolName`, whose live-title heuristic rewrites `memory_recall`
    // to `memory_re`.
    expect(
      inferLiveToolName({
        title: "Recalled 3 memories",
        kind: "read",
        rawInput: null,
        meta: { claudeCode: { toolName: "memory_recall" } },
      })
    ).toBe("memory_recall")
  })

  it("preserves sub-agent detection when rawInput carries subagent_type", () => {
    // Regression guard: meta.claudeCode.toolName="Task" must NOT override
    // input-shape detection. Otherwise Claude Code's Task tool stops
    // routing into the AgentToolCallPart card and child tool calls no
    // longer nest under their parent.
    expect(
      inferLiveToolName({
        title: "Implement feature X",
        kind: "other",
        rawInput: JSON.stringify({
          subagent_type: "general-purpose",
          prompt: "Do the thing",
        }),
        meta: { claudeCode: { toolName: "Task" } },
      })
    ).toBe("agent")
  })

  it("resolves delegation companion tools from meta over the input-shape heuristic", () => {
    // Regression guard for Task B: these companion tools must resolve from the
    // authoritative meta.claudeCode.toolName (the raw mcp__ name), not from their
    // input shape. get_delegation_status takes `{ task_ids }` and cancel_delegation
    // takes `{ task_id }` — the latter would otherwise be classified by
    // inferFromInput as the generic "task" tool (rendered as "任务" with no
    // detail). meta must win.
    expect(
      inferLiveToolName({
        title: "mcp__codeg-delegate__get_delegation_status",
        kind: "other",
        rawInput: JSON.stringify({ task_ids: ["t1"], wait_ms: 1000 }),
        meta: {
          claudeCode: {
            toolName: "mcp__codeg-delegate__get_delegation_status",
          },
        },
      })
    ).toBe("get_delegation_status")

    expect(
      inferLiveToolName({
        title: "mcp__codeg-delegate__cancel_delegation",
        kind: "other",
        rawInput: JSON.stringify({ task_id: "t1" }),
        meta: {
          claudeCode: { toolName: "mcp__codeg-delegate__cancel_delegation" },
        },
      })
    ).toBe("cancel_delegation")

    expect(
      inferLiveToolName({
        title: "mcp__codeg-delegate__delegate_to_agent",
        kind: "other",
        rawInput: JSON.stringify({ agent_type: "codex", task: "do it" }),
        meta: {
          claudeCode: { toolName: "mcp__codeg-delegate__delegate_to_agent" },
        },
      })
    ).toBe("delegate_to_agent")
  })

  it("still classifies a {task_id} tool as task when no Claude Code meta is present", () => {
    // Non-Claude agents (no meta.claudeCode.toolName) keep the legacy
    // input-shape behavior — the fix is meta-driven, not a removal of the
    // task_id heuristic.
    expect(
      inferLiveToolName({
        title: "Some task",
        kind: "other",
        rawInput: JSON.stringify({ task_id: "t1" }),
        meta: null,
      })
    ).toBe("task")
  })

  it("ignores meta when claudeCode is missing or malformed", () => {
    expect(
      inferLiveToolName({
        title: "Recalled 3 memories",
        kind: "read",
        rawInput: null,
        meta: null,
      })
    ).not.toBe("memory_recall")

    expect(
      inferLiveToolName({
        title: "Recalled 3 memories",
        kind: "read",
        rawInput: null,
        meta: { somethingElse: { toolName: "memory_recall" } },
      })
    ).not.toBe("memory_recall")

    expect(
      inferLiveToolName({
        title: "Recalled 3 memories",
        kind: "read",
        rawInput: null,
        meta: { claudeCode: { toolName: "   " } },
      })
    ).not.toBe("memory_recall")
  })
})

describe("normalizeToolName collapses delegate_to_agent across hosts", () => {
  // The codeg multi-agent delegation MCP tool is named the same across hosts
  // (`delegate_to_agent`) but each host serializes the server prefix
  // differently: Claude Code uses `mcp__<server>__`, Codex live ACP uses
  // `<server>/`, others use `.` or `:`. All forms must collapse to the
  // canonical name so the renderer routes them into DelegatedSubThread.
  it.each([
    "delegate_to_agent",
    "mcp__codeg-mcp__delegate_to_agent",
    "mcp__codeg-delegate__delegate_to_agent",
    "mcp__codeg__delegate_to_agent",
    "codeg-mcp/delegate_to_agent",
    "codeg-delegate/delegate_to_agent",
    "codeg-delegate.delegate_to_agent",
    "codeg-delegate:delegate_to_agent",
    "codeg_delegate__delegate_to_agent",
  ])("%s -> delegate_to_agent", (input) => {
    expect(normalizeToolName(input)).toBe("delegate_to_agent")
  })

  it("does not match suffixes without a separator", () => {
    expect(normalizeToolName("xdelegate_to_agent")).not.toBe(
      "delegate_to_agent"
    )
  })
})

describe("normalizeToolName collapses delegation companion tools across hosts", () => {
  it.each([
    "get_delegation_status",
    "mcp__codeg-mcp__get_delegation_status",
    "mcp__codeg-delegate__get_delegation_status",
    "mcp__codeg__get_delegation_status",
    "codeg-mcp/get_delegation_status",
    "codeg-delegate/get_delegation_status",
    "codeg-delegate.get_delegation_status",
    "codeg-delegate:get_delegation_status",
  ])("%s -> get_delegation_status", (input) => {
    expect(normalizeToolName(input)).toBe("get_delegation_status")
  })

  it.each([
    "cancel_delegation",
    "mcp__codeg-mcp__cancel_delegation",
    "mcp__codeg-delegate__cancel_delegation",
    "mcp__codeg__cancel_delegation",
    "codeg-mcp/cancel_delegation",
    "codeg-delegate/cancel_delegation",
    "codeg-delegate.cancel_delegation",
    "codeg-delegate:cancel_delegation",
  ])("%s -> cancel_delegation", (input) => {
    expect(normalizeToolName(input)).toBe("cancel_delegation")
  })

  it("does not match suffixes without a separator", () => {
    expect(normalizeToolName("xget_delegation_status")).not.toBe(
      "get_delegation_status"
    )
    expect(normalizeToolName("xcancel_delegation")).not.toBe(
      "cancel_delegation"
    )
  })
})

describe("normalizeToolName collapses Codex goal tools across wrappers", () => {
  it.each([
    ["create_goal", "create_goal"],
    ["functions.create_goal", "create_goal"],
    ["mcp__codeg__create_goal", "create_goal"],
    ["Goal updated (active): 分析 README 文件", "create_goal"],
    ["update_goal", "update_goal"],
    ["functions.update_goal", "update_goal"],
    ["mcp__codeg__update_goal", "update_goal"],
    ["Goal updated (complete): 分析 README 文件", "update_goal"],
  ])("%s -> %s", (input, expected) => {
    expect(normalizeToolName(input)).toBe(expected)
  })

  it("infers live Codex goal updates from ACP titles", () => {
    expect(
      inferLiveToolName({
        title: "Goal updated (active): 分析 README 文件",
        kind: "other",
        rawInput: JSON.stringify({ objective: "分析 README 文件" }),
      })
    ).toBe("create_goal")

    expect(
      inferLiveToolName({
        title: "Goal updated (complete): 分析 README 文件",
        kind: "other",
        rawInput: JSON.stringify({ status: "complete" }),
      })
    ).toBe("update_goal")
  })
})
