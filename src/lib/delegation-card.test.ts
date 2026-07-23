import { describe, expect, it } from "vitest"

import { ALL_AGENT_TYPES } from "@/lib/types"
import { parseDelegationMeta, parseInput } from "./delegation-card"

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
})
