import { describe, expect, it } from "vitest"

import { denormalizeSnapshot } from "@/lib/snapshot-denormalize"
import type { LiveSessionSnapshot } from "@/lib/types"

function baseSnapshot(
  overrides: Partial<LiveSessionSnapshot> = {}
): LiveSessionSnapshot {
  return {
    connection_id: "conn-1",
    conversation_id: null,
    folder_id: null,
    status: "connected",
    external_id: null,
    live_message: null,
    active_tool_calls: [],
    pending_permission: null,
    modes: null,
    current_mode: null,
    config_options: null,
    prompt_capabilities: null,
    usage: null,
    fork_supported: false,
    available_commands: [],
    selectors_ready: false,
    event_seq: 0,
    ...overrides,
  }
}

describe("denormalizeSnapshot — active_delegations", () => {
  it("carries active_delegations through to the patch", () => {
    const patch = denormalizeSnapshot(
      baseSnapshot({
        active_delegations: [
          {
            parent_tool_use_id: "pt-1",
            child_connection_id: "c1",
            child_conversation_id: 9,
            agent_type: "codex",
          },
        ],
      })
    )
    expect(patch.activeDelegations).toHaveLength(1)
    expect(patch.activeDelegations[0].parent_tool_use_id).toBe("pt-1")
    expect(patch.activeDelegations[0].child_conversation_id).toBe(9)
  })

  it("defaults activeDelegations to [] when the field is absent (older server payload)", () => {
    const snap = baseSnapshot()
    // Older server payloads omit the field entirely.
    delete (snap as { active_delegations?: unknown }).active_delegations
    const patch = denormalizeSnapshot(snap)
    expect(patch.activeDelegations).toEqual([])
  })
})

describe("denormalizeSnapshot — config staleness", () => {
  it("carries config_stale / config_stale_kind into the patch", () => {
    const patch = denormalizeSnapshot(
      baseSnapshot({ config_stale: true, config_stale_kind: "model_provider" })
    )
    expect(patch.configStale).toBe(true)
    expect(patch.configStaleKind).toBe("model_provider")
  })

  it("defaults to not-stale when the fields are absent (older server payload)", () => {
    const snap = baseSnapshot()
    delete (snap as { config_stale?: unknown }).config_stale
    delete (snap as { config_stale_kind?: unknown }).config_stale_kind
    const patch = denormalizeSnapshot(snap)
    expect(patch.configStale).toBe(false)
    expect(patch.configStaleKind).toBeNull()
  })
})

describe("denormalizeSnapshot — last_error", () => {
  it("carries last_error.message into the patch", () => {
    const patch = denormalizeSnapshot(
      baseSnapshot({
        last_error: {
          message: " ACP protocol error: Forbidden ",
          code: "forbidden",
        },
      })
    )
    expect(patch.lastError).toBe("ACP protocol error: Forbidden")
    expect(patch.status).toBe("connected")
  })

  it("defaults lastError to null when the field is absent", () => {
    const snap = baseSnapshot()
    delete (snap as { last_error?: unknown }).last_error
    const patch = denormalizeSnapshot(snap)
    expect(patch.lastError).toBeNull()
    expect(patch.status).toBe("connected")
  })
})
