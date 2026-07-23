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

  it("resolves delegate_to_agent from broker delegation meta on identity-less wires", () => {
    // Cursor announces MCP calls as the literal "MCP: tool" with an empty
    // rawInput and never resends either — the broker's
    // meta["codeg.delegation"] write is the only live identity signal.
    expect(
      inferLiveToolName({
        title: "MCP: tool",
        kind: "other",
        rawInput: "{}",
        meta: {
          "codeg.delegation": { status: "running", child_conversation_id: 42 },
        },
      })
    ).toBe("delegate_to_agent")
  })

  it("keeps input-shape priority for calls without delegation meta", () => {
    // A generic "MCP: tool" call WITHOUT the broker meta must stay generic —
    // the delegation resolution is scoped to the codeg-minted marker.
    expect(
      inferLiveToolName({
        title: "MCP: tool",
        kind: "other",
        rawInput: "{}",
        meta: null,
      })
    ).not.toBe("delegate_to_agent")
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

  it("resolves Grok companion tools from the unwrapped title over the input shape", () => {
    // Grok sets no claudeCode meta; its backend unwraps the `use_tool` envelope
    // so the title is the raw `<server>__<tool>` name. cancel_delegation's
    // {task_id} input would otherwise be misread as the generic "task" tool —
    // the title-companion priority must win.
    expect(
      inferLiveToolName({
        title: "codeg-mcp__cancel_delegation",
        kind: "other",
        rawInput: JSON.stringify({ task_id: "t1" }),
        meta: { "x.ai/tool": { name: "use_tool" } },
      })
    ).toBe("cancel_delegation")
    // Siblings stay correct too.
    expect(
      inferLiveToolName({
        title: "codeg-mcp__get_delegation_status",
        kind: "other",
        rawInput: JSON.stringify({ task_ids: ["t1"] }),
        meta: { "x.ai/tool": { name: "use_tool" } },
      })
    ).toBe("get_delegation_status")
    expect(
      inferLiveToolName({
        title: "codeg-mcp__delegate_to_agent",
        kind: "other",
        rawInput: JSON.stringify({ agent_type: "codex", task: "go" }),
        meta: { "x.ai/tool": { name: "use_tool" } },
      })
    ).toBe("delegate_to_agent")
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

describe("normalizeToolName collapses ask_user_question across hosts", () => {
  it.each([
    "question",
    "ask_user_question",
    "askuserquestion",
    "mcp__codeg-mcp__ask_user_question",
    "codeg-mcp/ask_user_question",
    "codeg-mcp.ask_user_question",
    "codeg-mcp:ask_user_question",
  ])("%s -> question", (input) => {
    expect(normalizeToolName(input)).toBe("question")
  })

  it("does not match a suffix without a separator", () => {
    expect(normalizeToolName("xask_user_question")).not.toBe("question")
  })
})

describe("normalizeToolName collapses check_user_feedback across hosts", () => {
  it.each([
    "check_user_feedback",
    "mcp__codeg-mcp__check_user_feedback",
    "mcp__codeg__check_user_feedback",
    "codeg-mcp/check_user_feedback",
    "codeg-mcp.check_user_feedback",
    "codeg-mcp:check_user_feedback",
  ])("%s -> check_user_feedback", (input) => {
    expect(normalizeToolName(input)).toBe("check_user_feedback")
  })

  it("does not match a suffix without a separator", () => {
    expect(normalizeToolName("xcheck_user_feedback")).not.toBe(
      "check_user_feedback"
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

describe("inferLiveToolName codex collab detection", () => {
  const collabRaw = JSON.stringify({
    prompt: "run pnpm build",
    senderThreadId: "t1",
    receiverThreadIds: ["t2"],
    agentsStates: [],
    status: "in_progress",
  })

  it("routes collab tool calls by rawInput shape regardless of title", () => {
    // codex-acp 1.0.1 #223: the live title is the bare collab op; detection is
    // by the inter-agent rawInput shape, so any of spawn/wait/close collapses to
    // the dedicated collab card — overriding the spawn_agent→"agent" /
    // wait_agent→"task" title aliases.
    for (const title of ["spawn_agent", "wait_agent", "close_agent"]) {
      expect(
        inferLiveToolName({ title, kind: "other", rawInput: collabRaw })
      ).toBe("collab_agent")
    }
  })

  it("does NOT treat the spawn_agent function_call args as collab", () => {
    // Same title, but the non-collab input (no sender/receiver/agentsStates)
    // must fall through to the title alias instead of the collab card.
    expect(
      inferLiveToolName({
        title: "spawn_agent",
        kind: "other",
        rawInput: JSON.stringify({ agent_type: "worker", message: "go" }),
      })
    ).toBe("agent")
  })
})

describe("normalizeToolName Grok terminal tool", () => {
  it("aliases Grok's run_terminal_command to bash", () => {
    // Grok Build (xAI) reports its terminal tool as `run_terminal_command`
    // (`_meta["x.ai/tool"].name`), which the history parser stores verbatim.
    // Without the alias the reload path would miss the "bash" classification
    // the live path infers from `rawInput.command`, rendering the command card
    // via the generic tool shell (raw ANSI, no terminal title) instead of the
    // Terminal card.
    expect(normalizeToolName("run_terminal_command")).toBe("bash")
  })
})

describe("inferLiveToolName cursor task and MCP shapes", () => {
  it("routes cursor's task tool to the Agent card from the bare _toolName snapshot", () => {
    // Cursor announces the tool_call before its args stream in, so the live
    // rawInput is often just the identity stamp — and with no args the wire
    // title is the placeholder "Task: Subagent task", which would otherwise
    // resolve to the generic task card via the freeform `^task` matcher.
    expect(
      inferLiveToolName({
        title: "Task: Subagent task",
        kind: "other",
        rawInput: JSON.stringify({ _toolName: "task" }),
      })
    ).toBe("agent")
  })

  it("routes a fully-populated cursor task payload to the Agent card", () => {
    expect(
      inferLiveToolName({
        title: "Task: run the build",
        kind: "other",
        rawInput: JSON.stringify({
          _toolName: "task",
          prompt: "Run pnpm build and report back.",
          description: "run the build",
          subagentType: { case: "generalPurpose", value: {} },
        }),
      })
    ).toBe("agent")
  })

  it("resolves cursor MCP calls to <provider>__<tool> instead of bash", () => {
    // Cursor's mcpToolCall rawInput carries an `args` object — without the
    // provider/tool resolution the `args` key heuristic would misclassify
    // the call as a terminal command.
    expect(
      inferLiveToolName({
        title: "codeg-mcp: delegate_to_agent",
        kind: "other",
        rawInput: JSON.stringify({
          providerIdentifier: "codeg-mcp",
          toolName: "delegate_to_agent",
          args: { agent_type: "codex", task: "run build" },
        }),
      })
    ).toBe("delegate_to_agent")
    expect(
      inferLiveToolName({
        title: "srv: custom_tool",
        kind: "other",
        rawInput: JSON.stringify({
          providerIdentifier: "srv",
          toolName: "custom_tool",
          args: { command: "echo hi" },
        }),
      })
    ).not.toBe("bash")
  })

  it("collapses other cursor _toolName hints to their canonical snake_case names", () => {
    expect(
      inferLiveToolName({
        title: "Create Plan: refactor",
        kind: "other",
        rawInput: JSON.stringify({ _toolName: "createPlan", name: "refactor" }),
      })
    ).toBe("create_plan")
  })
})
