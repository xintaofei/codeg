import { describe, expect, it } from "vitest"

import {
  parsePermissionOptionChanges,
  parsePermissionToolCall,
} from "./permission-request"

function metaWith(changes: unknown[]): Record<string, unknown> {
  return { permission: { version: 1, changes } }
}

describe("parsePermissionOptionChanges", () => {
  it("keeps the agent's description and normalizes every lifetime shape", () => {
    // The six shapes the two adapters emit. claude-agent-acp ≥0.64.1 (#930)
    // spans all of them; codex-acp only ever sends `{scope: "session"}`.
    const changes = parsePermissionOptionChanges(
      metaWith([
        { description: "session", lifetime: { scope: "session" } },
        {
          description: "process",
          lifetime: { scope: "process", storage: "cli_argument" },
        },
        {
          description: "user",
          lifetime: { scope: "persistent", storage: "user" },
        },
        {
          description: "project",
          lifetime: { scope: "persistent", storage: "project" },
        },
        {
          description: "local",
          lifetime: { scope: "persistent", storage: "project_local" },
        },
        // Persistent with a storage this build has never seen: the destination
        // is unknown but "outlives the session" must still reach the user.
        {
          description: "future",
          lifetime: { scope: "persistent", storage: "enterprise_policy" },
        },
      ])
    )
    expect(changes).toEqual([
      { description: "session", scope: "session" },
      { description: "process", scope: "process" },
      { description: "user", scope: "user" },
      { description: "project", scope: "project" },
      { description: "local", scope: "project_local" },
      { description: "future", scope: "persistent" },
    ])
  })

  it("reports no scope when the lifetime is absent, unknown, or malformed", () => {
    const changes = parsePermissionOptionChanges(
      metaWith([
        { description: "no lifetime at all" },
        { description: "explicitly unknown", lifetime: { scope: "unknown" } },
        { description: "scope from the future", lifetime: { scope: "orbit" } },
        { description: "lifetime is not an object", lifetime: 42 },
        { description: "empty lifetime", lifetime: {} },
      ])
    )
    expect(changes.map((c) => c.scope)).toEqual([null, null, null, null, null])
    // The sentence itself still renders — only the duration is withheld.
    expect(changes.map((c) => c.description)).toHaveLength(5)
  })

  it("drops changes without a description but keeps the rest", () => {
    const changes = parsePermissionOptionChanges(
      metaWith([
        { lifetime: { scope: "session" } },
        { description: "   ", lifetime: { scope: "session" } },
        { description: "kept", lifetime: { scope: "session" } },
      ])
    )
    expect(changes).toEqual([{ description: "kept", scope: "session" }])
  })

  it("caps the list and the per-description length", () => {
    const changes = parsePermissionOptionChanges(
      metaWith(
        Array.from({ length: 9 }, (_, i) => ({
          description: `x`.repeat(250) + i,
          lifetime: { scope: "session" },
        }))
      )
    )
    expect(changes).toHaveLength(6)
    expect(changes[0].description).toHaveLength(200)
  })

  it("returns nothing for metadata it does not understand", () => {
    // A future revision may reshape `changes`; showing it half-understood is
    // worse than showing nothing.
    expect(
      parsePermissionOptionChanges({
        permission: { version: 2, changes: [{ description: "nope" }] },
      })
    ).toEqual([])
    expect(
      parsePermissionOptionChanges({ permission: { version: 1, changes: {} } })
    ).toEqual([])
    expect(parsePermissionOptionChanges({ somethingElse: true })).toEqual([])
    expect(parsePermissionOptionChanges(null)).toEqual([])
    expect(parsePermissionOptionChanges(undefined)).toEqual([])
  })

  it("reads codex-acp ≥1.7.0's flat description as one scope-less change", () => {
    // 1.7.0 dropped `changes[]`; MCP elicitation approvals now carry the
    // explanation directly. No lifetime is sent, so no duration is claimed.
    expect(
      parsePermissionOptionChanges({
        permission: {
          version: 1,
          description:
            "Run the tool and remember this choice for this session.",
        },
      })
    ).toEqual([
      {
        description: "Run the tool and remember this choice for this session.",
        scope: null,
      },
    ])
    // `changes[]` still wins where an adapter sends it (claude ≥0.64.1).
    expect(
      parsePermissionOptionChanges({
        permission: {
          version: 1,
          description: "ignored",
          changes: [{ description: "kept", lifetime: { scope: "session" } }],
        },
      })
    ).toEqual([{ description: "kept", scope: "session" }])
    // Same version gate as the structured form.
    expect(
      parsePermissionOptionChanges({
        permission: { version: 2, description: "nope" },
      })
    ).toEqual([])
  })
})

describe("parsePermissionToolCall — request-level _meta.permission", () => {
  it("surfaces the codex reason and prefers its title over the generic one", () => {
    // codex-acp ≥1.7.0, after the backend hoists the request meta onto the
    // card. Before 1.7.0 this sentence WAS `toolCall.title`, so dropping it
    // would be a straight regression on upgrade.
    const parsed = parsePermissionToolCall({
      toolCallId: "command-7",
      kind: "execute",
      status: "pending",
      title: "Run command",
      rawInput: { command: "npm test", cwd: "/workspace" },
      _meta: {
        permission: {
          version: 1,
          title: "Run command?",
          description:
            "The test suite needs to run outside the current sandbox.",
        },
      },
    })
    expect(parsed.reason).toBe(
      "The test suite needs to run outside the current sandbox."
    )
    expect(parsed.description).toBe("Run command?")
    // The standard fields stay authoritative for the action itself.
    expect(parsed.command).toBe("npm test")
    expect(parsed.title).toBe("Run command")
  })

  it("keeps claude's action title ahead of the permission title", () => {
    // `_meta.claudeCode.title` names the ACTION and has always been the
    // heading; the permission block only fills a gap claude does not leave.
    const parsed = parsePermissionToolCall({
      toolCallId: "t1",
      title: "npm test",
      _meta: {
        claudeCode: { title: "Run the test suite" },
        permission: { version: 1, title: "Run command?", description: "why" },
      },
    })
    expect(parsed.description).toBe("Run the test suite")
    expect(parsed.reason).toBe("why")
  })

  it("reports no reason for agents that send none, or a version it cannot read", () => {
    expect(parsePermissionToolCall({ toolCallId: "t1" }).reason).toBeNull()
    expect(
      parsePermissionToolCall({
        toolCallId: "t1",
        _meta: { permission: { version: 1, title: "Make edits?" } },
      }).reason
    ).toBeNull()
    expect(
      parsePermissionToolCall({
        toolCallId: "t1",
        _meta: { permission: { version: 2, description: "unreadable" } },
      }).reason
    ).toBeNull()
  })
})
