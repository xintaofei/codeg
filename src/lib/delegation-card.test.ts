import { describe, expect, it } from "vitest"

import { ALL_AGENT_TYPES } from "@/lib/types"
import {
  parseDelegateTaskId,
  parseDelegationMeta,
  parseInput,
  parseToolOutput,
} from "./delegation-card"

describe("parseInput wrapper peeling", () => {
  it("reads top-level delegation args", () => {
    const parsed = parseInput(
      JSON.stringify({
        agent_type: "codex",
        task: "run the build",
        working_dir: "/tmp/proj",
      })
    )
    expect(parsed.agentType).toBe("codex")
    expect(parsed.task).toBe("run the build")
    expect(parsed.workingDir).toBe("/tmp/proj")
  })

  it("peels Cursor's MCP args wrapper", () => {
    // Cursor surfaces MCP calls as {providerIdentifier, toolName, args} — the
    // delegation fields live one level down under `args`. Mirrors the Rust
    // walker in acp/lifecycle.rs (ARGS_WRAPPER_KEYS).
    const parsed = parseInput(
      JSON.stringify({
        providerIdentifier: "codeg-mcp",
        toolName: "delegate_to_agent",
        args: { agent_type: "claude_code", task: "执行 pnpm build" },
      })
    )
    expect(parsed.agentType).toBe("claude_code")
    expect(parsed.task).toBe("执行 pnpm build")
    expect(parsed.workingDir).toBeNull()
  })

  it("returns empty for undelegation-like payloads", () => {
    const parsed = parseInput(JSON.stringify({ command: "ls -la" }))
    expect(parsed.agentType).toBeNull()
    expect(parsed.task).toBeNull()
  })

  // Guards the allowlist against drifting behind the canonical agent list — the
  // regression that left `grok` and `cursor` delegation cards iconless. Every
  // known agent must resolve so its sub-agent card shows the right icon/label.
  it.each(ALL_AGENT_TYPES)("recognizes the %s agent_type", (agentType) => {
    const parsed = parseInput(
      JSON.stringify({ agent_type: agentType, task: "do the thing" })
    )
    expect(parsed.agentType).toBe(agentType)
  })
})

describe("codex live-wire result envelope", () => {
  /**
   * codex-acp forwards every MCP call's outcome as
   * `rawOutput = { result: <CallToolResult>, error: <string|null> }`
   * (`createMcpRawOutput`) — one layer above the shapes the parsers read.
   */
  function codexLive(callToolResult: Record<string, unknown>): string {
    return JSON.stringify({
      error: null,
      result: { meta: null, ...callToolResult },
    })
  }

  const runningAck = {
    agent_type: "codex",
    child_conversation_id: 2781,
    status: "running",
    task_id: "8cb72a7c-1a96-44aa-9c26-d4356862c9c2",
    message:
      "Delegation successful. task_id=8cb72a7c-1a96-44aa-9c26-d4356862c9c2.",
  }

  it("reads a running ack through the wrapper (ack, not a terminal outcome)", () => {
    const parsed = parseToolOutput(
      codexLive({
        content: [{ type: "text", text: runningAck.message }],
        structuredContent: runningAck,
      })
    )
    expect(parsed).toEqual({ kind: "ack", childConversationId: 2781 })
  })

  // Note: this one already passed pre-fix via the `task_id=<id>` text scan —
  // it guards that the structured path doesn't regress that resolution.
  it("resolves the task id through the wrapper", () => {
    const output = codexLive({
      content: [{ type: "text", text: "Delegation successful." }],
      structuredContent: runningAck,
    })
    expect(parseDelegateTaskId(output, null)).toBe(runningAck.task_id)
  })

  it("surfaces a failed envelope's error string instead of the raw JSON", () => {
    const parsed = parseToolOutput(
      JSON.stringify({ error: "mcp server disconnected", result: null })
    )
    expect(parsed).toEqual({
      kind: "outcome",
      text: "mcp server disconnected",
      isError: true,
      childConversationId: null,
    })
  })

  it("leaves a child's nested {status, task_id} payload alone", () => {
    // Peeling on the `result` KEY alone would turn opaque child output into a
    // failed delegation outcome and hand out `child-job` as the task id.
    const childOutput = JSON.stringify({
      result: { status: "failed", task_id: "child-job", message: "domain" },
    })
    expect(parseToolOutput(childOutput)).toEqual({
      kind: "outcome",
      text:
        "```json\n" +
        JSON.stringify(JSON.parse(childOutput), null, 2) +
        "\n```",
      isError: false,
      childConversationId: null,
    })
    expect(parseDelegateTaskId(childOutput, null)).toBeNull()
  })

  it("does NOT treat a child's own `error` field as a host failure", () => {
    // No `result` key ⇒ not codex-acp's failure envelope. Must stay a
    // non-error outcome rendered as-is.
    const childOutput = JSON.stringify({ error: "domain validation", rows: [] })
    const parsed = parseToolOutput(childOutput)
    expect(parsed).toMatchObject({ kind: "outcome", isError: false })
    expect(parsed).not.toMatchObject({ text: "domain validation" })
  })
})

describe("parseDelegationMeta task fields", () => {
  it("surfaces the broker-stamped task_preview and task_id", () => {
    // The persisted Cursor shape: raw_input is "{}" forever, so the meta the
    // broker stamped is the card's ONLY label source after a refresh.
    const parsed = parseDelegationMeta({
      "codeg.delegation": {
        status: "running",
        child_conversation_id: 42,
        task_preview: "执行 pnpm build",
        task_id: "task-uuid-1",
      },
    })
    expect(parsed).not.toBeNull()
    expect(parsed?.task).toBe("执行 pnpm build")
    expect(parsed?.taskId).toBe("task-uuid-1")
    expect(parsed?.childConversationId).toBe(42)
  })

  it("keeps task fields null when the meta lacks them (older backend)", () => {
    const parsed = parseDelegationMeta({
      "codeg.delegation": { status: "completed" },
    })
    expect(parsed?.task).toBeNull()
    expect(parsed?.taskId).toBeNull()
  })

  it("ignores empty and non-string task fields", () => {
    const parsed = parseDelegationMeta({
      "codeg.delegation": {
        status: "running",
        task_preview: "",
        task_id: 7,
      },
    })
    expect(parsed?.task).toBeNull()
    expect(parsed?.taskId).toBeNull()
  })

  it("parses permission_mode and trims it", () => {
    const parsed = parseInput(
      JSON.stringify({
        agent_type: "codex",
        task: "t",
        permission_mode: "  plan  ",
      })
    )
    expect(parsed.permissionMode).toBe("plan")
  })

  it("treats a blank permission_mode as absent", () => {
    const parsed = parseInput(
      JSON.stringify({ agent_type: "codex", task: "t", permission_mode: "   " })
    )
    expect(parsed.permissionMode).toBeNull()
  })

  it("leaves permissionMode null when the argument is omitted", () => {
    const parsed = parseInput(
      JSON.stringify({ agent_type: "codex", task: "t" })
    )
    expect(parsed.permissionMode).toBeNull()
  })
})
