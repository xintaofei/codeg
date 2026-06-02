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
