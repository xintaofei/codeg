/**
 * Contract + behavior tests for the collaboration client (`lib/collaboration`).
 *
 * The "frozen" fixtures live in `src-tauri/tests/fixtures/collaboration/` —
 * they are ALSO loaded by the Rust contract test, so the two languages are
 * pinned to byte-identical wire shapes (v2 design §7 / acceptance A26).
 */

import { describe, expect, it } from "vitest"
import { readFileSync } from "node:fs"
import { join } from "node:path"

import {
  parseCollaborationSnapshot,
  parseTurnReport,
  parseSessionSummary,
  ContinuationParseError,
} from "@/lib/collaboration"

const fixtureDir = join(
  __dirname,
  "../../src-tauri/tests/fixtures/collaboration"
)

function loadFixture(name: string): unknown {
  return JSON.parse(readFileSync(join(fixtureDir, name), "utf8"))
}

describe("turn report contract (Rust fixtures)", () => {
  for (const state of [
    "accepted",
    "preparing",
    "dispatching",
    "running",
    "cancel_requested",
    "completed",
    "failed",
    "canceled",
    "interrupted",
    "outcome_unknown",
  ]) {
    it(`parses the frozen ${state} fixture`, () => {
      const raw = loadFixture(`turn_${state}.json`)
      const report = parseTurnReport(raw)
      expect(report.state).toBe(state)
      expect(report.schema_version).toBe(1)
      expect(report.turn_id).toBe("turn-1")
      expect(report.session_id).toBe("session-1")
      expect(report.source_task_id).toBe("task-0")
      expect(report.ordinal).toBe(1)
      expect(report.version).toBeGreaterThanOrEqual(1)
      expect(report.text_truncated).toBe(false)
      expect(report.message).toBe("rework instruction")
      expect(report.initiator_kind).toBe("parent_agent")
    })
  }

  it("rejects an unknown schema_version", () => {
    const raw = loadFixture("turn_completed.json") as Record<string, unknown>
    expect(() => parseTurnReport({ ...raw, schema_version: 2 })).toThrow(
      ContinuationParseError
    )
  })

  it("rejects a missing turn_id", () => {
    const raw = loadFixture("turn_completed.json") as Record<string, unknown>
    const withoutId = { ...raw }
    delete withoutId.turn_id
    expect(() => parseTurnReport(withoutId)).toThrow(ContinuationParseError)
  })

  it("rejects an unknown state string", () => {
    const raw = loadFixture("turn_completed.json") as Record<string, unknown>
    expect(() => parseTurnReport({ ...raw, state: "zombie" })).toThrow(
      ContinuationParseError
    )
  })
})

describe("session summary contract", () => {
  it("parses the frozen open-session fixture", () => {
    const summary = parseSessionSummary(loadFixture("session_open.json"))
    expect(summary.state).toBe("open")
    expect(summary.session_id).toBe("session-1")
    expect(summary.child_conversation_id).toBe(42)
  })

  it("rejects an unknown session state", () => {
    const raw = loadFixture("session_open.json") as Record<string, unknown>
    expect(() => parseSessionSummary({ ...raw, state: "paused" })).toThrow(
      ContinuationParseError
    )
  })
})

describe("snapshot contract", () => {
  it("parses an empty snapshot (no session) as-is", () => {
    const snap = parseCollaborationSnapshot(loadFixture("snapshot_empty.json"))
    expect(snap.session).toBeNull()
    expect(snap.turns).toEqual([])
    expect(snap.next_after_ordinal).toBeNull()
  })

  it("parses a populated snapshot and preserves null result fields", () => {
    const snap = parseCollaborationSnapshot(loadFixture("snapshot_page.json"))
    expect(snap.session?.state).toBe("open")
    expect(snap.turns).toHaveLength(2)
    const [completed, running] = snap.turns
    expect(completed.state).toBe("completed")
    expect(completed.result_text).toBe("reworked output")
    expect(completed.finished_at).not.toBeNull()
    expect(running.state).toBe("running")
    expect(running.result_text).toBeNull()
    expect(running.started_at).not.toBeNull()
    expect(running.finished_at).toBeNull()
    expect(snap.next_after_ordinal).toBeNull()
  })
})
