import { describe, expect, it } from "vitest"

import {
  childSessionOfLaunch,
  parseChildSessionId,
  parseSubAgentLaunchFields,
} from "@/lib/native-subagent-fields"

describe("parseSubAgentLaunchFields", () => {
  it("reads every Claude Task spelling", () => {
    const f = parseSubAgentLaunchFields(
      JSON.stringify({
        subagent_type: "general-purpose",
        description: "法语问候",
        prompt: "用法语问好",
        model: "sonnet",
      })
    )
    expect(f.subagentType).toBe("general-purpose")
    expect(f.description).toBe("法语问候")
    expect(f.prompt).toBe("用法语问好")
    expect(f.model).toBe("sonnet")
    expect(f.isCursorTask).toBe(false)
    expect(f.isCodexSubagentLaunch).toBe(false)
  })

  it("falls back to message for the codex team spawn payload", () => {
    // codex's native team-of-agents spawn carries only {message}: the title
    // has to come from the task text, not the generic fallback.
    const f = parseSubAgentLaunchFields('{"message":"先用法语说你好"}')
    expect(f.subagentType).toBeNull()
    expect(f.description).toBe("先用法语说你好")
  })

  it("treats non-string fields as absent, not as renderable objects", () => {
    // CodeBuddy hands over {subagent_type: {}} — an object leaking into a
    // React child position crashes the render.
    const f = parseSubAgentLaunchFields('{"subagent_type":{},"description":{}}')
    expect(f.subagentType).toBeNull()
    expect(f.description).toBeNull()
  })

  it("recognizes the Cursor task identity stamp", () => {
    const f = parseSubAgentLaunchFields(
      '{"prompt":"go","_toolName":"task","subagentType":{"case":"custom"}}'
    )
    expect(f.isCursorTask).toBe(true)
    expect(f.subagentType).toBe("custom")
  })

  it("reads codex launch marker and child state", () => {
    const f = parseSubAgentLaunchFields(
      '{"agent_id":"019f07aa-f57b-4000-8000-000000000000","__codegCodexSubagentLaunch":true,"__codegCodexSubagentState":"completed"}'
    )
    expect(f.isCodexSubagentLaunch).toBe(true)
    expect(f.agentId).toBe("019f07aa-f57b-4000-8000-000000000000")
    expect(f.codexSubagentState).toBe("completed")
  })

  it("rescues fields from a truncated live input", () => {
    // The full JSON.parse fails mid-stream; extractJsonField still reads the
    // field as long as its value itself is closed.
    const f = parseSubAgentLaunchFields(
      '{"subagent_type":"Explore","description":"扫代码","prompt'
    )
    expect(f.subagentType).toBe("Explore")
    expect(f.description).toBe("扫代码")
  })

  it("survives null and garbage", () => {
    expect(parseSubAgentLaunchFields(null).subagentType).toBeNull()
    expect(parseSubAgentLaunchFields("not json").subagentType).toBeNull()
  })
})

describe("parseChildSessionId", () => {
  it("prefers the grok live meta marker", () => {
    expect(
      parseChildSessionId(
        { grokSubagentSession: { childSessionId: "grok-child" } },
        null,
        null
      )
    ).toEqual({ sessionId: "grok-child", agentType: "grok" })
  })

  it("reads the history stats field", () => {
    expect(parseChildSessionId(null, "hist-child", null)).toEqual({
      sessionId: "hist-child",
      agentType: "grok",
    })
  })

  it("uses the codex agent id as the session key", () => {
    expect(parseChildSessionId(null, null, "019f-uuid")).toEqual({
      sessionId: "019f-uuid",
      agentType: "codex",
    })
  })

  it("returns null with nothing to key on", () => {
    expect(parseChildSessionId(null, null, null)).toBeNull()
  })

  it("follows the parent's agent type when the caller knows it", () => {
    // The aux-panel row passes the conversation's own agent: a parent's child
    // is a session of the parent's kind, so the parent decides the type for
    // every handle branch (and the per-branch pins below only serve callers
    // without one, like the message-area capsule).
    expect(
      parseChildSessionId(
        { grokSubagentSession: { childSessionId: "c" } },
        null,
        null,
        "codex"
      )
    ).toEqual({ sessionId: "c", agentType: "codex" })
    expect(parseChildSessionId(null, "hist-child", null, "cursor")).toEqual({
      sessionId: "hist-child",
      agentType: "cursor",
    })
    expect(parseChildSessionId(null, null, "019f-uuid", "codex")).toEqual({
      sessionId: "019f-uuid",
      agentType: "codex",
    })
  })
})

describe("childSessionOfLaunch", () => {
  it("only folds the codex agent id in for a marked launch", () => {
    const fields = {
      agentId: "019f-uuid",
      isCodexSubagentLaunch: false,
    }
    expect(childSessionOfLaunch(fields, null, null)).toBeNull()
    expect(
      childSessionOfLaunch(
        { ...fields, isCodexSubagentLaunch: true },
        null,
        null
      )
    ).toEqual({ sessionId: "019f-uuid", agentType: "codex" })
  })

  it("takes the stats child session id from agentStats", () => {
    expect(
      childSessionOfLaunch(
        { agentId: null, isCodexSubagentLaunch: false },
        null,
        { child_session_id: "grok-hist" } as never
      )
    ).toEqual({ sessionId: "grok-hist", agentType: "grok" })
  })
})
