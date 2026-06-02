import { describe, expect, it } from "vitest"

import {
  groupConsecutiveDelegationStatus,
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
      poll("mcp__codeg-delegate__get_delegation_status", "t1"),
      poll("codeg-delegate/get_delegation_status", "t1"),
    ])
    expect(out).toHaveLength(1)
    expect(pollsOf(out[0])).toHaveLength(2)
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
