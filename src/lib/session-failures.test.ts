import { describe, expect, it } from "vitest"

import {
  activeSessionFailureView,
  activeSessionFailures,
  dismissSessionFailures,
  hasSettleableRetryIncident,
  knownSessionFailureActions,
  lastUserPromptText,
  mergeSessionFailures,
  mostRecentRecoveredWarning,
  resolvedSessionFailures,
  settleSessionFailures,
  upsertSessionFailure,
} from "./session-failures"
import type { MessageTurn, SessionFailureRecord } from "@/lib/types"

function record(
  id: string,
  revision: number,
  overrides: Partial<SessionFailureRecord> = {}
): SessionFailureRecord {
  return {
    id,
    revision,
    category: "limit",
    severity: "warning",
    title: `${id}@${revision}`,
    actions: ["retry"],
    resolved: false,
    ...overrides,
  }
}

describe("upsertSessionFailure / mergeSessionFailures", () => {
  it("accepts fresh ids and strictly higher revisions, in place", () => {
    let table = upsertSessionFailure([], record("a", 1))
    table = upsertSessionFailure(table, record("b", 1))
    table = upsertSessionFailure(table, record("a", 2, { title: "revised" }))
    expect(table).toHaveLength(2)
    expect(table.find((f) => f.id === "a")?.title).toBe("revised")
  })

  it("rejects equal and lower revisions by reference (replay-safe)", () => {
    const table = upsertSessionFailure([], record("a", 2))
    // Equal revision = verbatim replay (claude re-publishes still-active
    // failures on session/load) — must be a no-op, same reference.
    expect(upsertSessionFailure(table, record("a", 2))).toBe(table)
    expect(upsertSessionFailure(table, record("a", 1))).toBe(table)
  })

  it("adopts an equal-revision resolution a snapshot carries", () => {
    // `resolved` is client-INFERRED, so a client that missed the progress /
    // turn-end events it came from can only learn about it from a hydrating
    // snapshot — at the SAME revision, since the adapter never bumped one.
    const table = upsertSessionFailure([], record("a", 2))
    expect(table[0].resolved).toBe(false)
    const hydrated = mergeSessionFailures(table, [
      record("a", 2, { resolved: true }),
    ])
    expect(hydrated).not.toBe(table)
    expect(hydrated[0].resolved).toBe(true)
    expect(hydrated[0].revision).toBe(2)
    // Only false → true: a later replay carrying `false` cannot un-settle it.
    expect(mergeSessionFailures(hydrated, [record("a", 2)])).toBe(hydrated)
    // And a genuine recurrence still re-arms through a higher revision.
    expect(mergeSessionFailures(hydrated, [record("a", 3)])[0].resolved).toBe(
      false
    )
  })

  it("keeps the watermark on resolved entries — stale upserts cannot resurrect", () => {
    // upsert rev2 → settle (tombstone-equivalent) → delayed rev2 replay must
    // be rejected: the resolved entry retains the revision watermark.
    let table = upsertSessionFailure([], record("a", 2))
    table = settleSessionFailures(table, "all")
    expect(table[0].resolved).toBe(true)
    expect(upsertSessionFailure(table, record("a", 2))).toBe(table)
    expect(upsertSessionFailure(table, record("a", 1))).toBe(table)
    // A genuinely newer revision re-arms it.
    const rearmed = upsertSessionFailure(table, record("a", 3))
    expect(rearmed[0].resolved).toBe(false)
    expect(rearmed[0].revision).toBe(3)
  })

  it("re-arms resolved on escalation via id reuse (warning → error)", () => {
    let table = upsertSessionFailure([], record("t1:error", 1))
    table = settleSessionFailures(table, "warnings")
    table = upsertSessionFailure(
      table,
      record("t1:error", 2, { severity: "error", title: "gave up" })
    )
    expect(table).toHaveLength(1)
    expect(table[0].severity).toBe("error")
    expect(table[0].resolved).toBe(false)
  })

  it("merges snapshot batches monotonically and reports no-ops by reference", () => {
    const live = [record("a", 3), record("b", 1)]
    // Snapshot older for a, newer for b, plus an unseen c.
    const merged = mergeSessionFailures(live, [
      record("a", 2),
      record("b", 2),
      record("c", 1),
    ])
    expect(merged.find((f) => f.id === "a")?.revision).toBe(3)
    expect(merged.find((f) => f.id === "b")?.revision).toBe(2)
    expect(merged.find((f) => f.id === "c")).toBeDefined()
    // Entirely stale batch → same reference.
    expect(mergeSessionFailures(merged, [record("a", 1)])).toBe(merged)
    expect(mergeSessionFailures(merged, [])).toBe(merged)
    expect(mergeSessionFailures(merged, null)).toBe(merged)
    // Records without usable identity are skipped, not crashed on.
    expect(mergeSessionFailures(merged, [record("", 1), record("d", 0)])).toBe(
      merged
    )
  })
})

describe("settleSessionFailures", () => {
  it("settles warnings only at turn boundaries; errors survive", () => {
    const table = [record("w", 1), record("e", 1, { severity: "error" })]
    const settled = settleSessionFailures(table, "warnings")
    expect(settled.find((f) => f.id === "w")?.resolved).toBe(true)
    expect(settled.find((f) => f.id === "e")?.resolved).toBe(false)
    // New prompt settles everything.
    const all = settleSessionFailures(settled, "all")
    expect(all.every((f) => f.resolved)).toBe(true)
  })

  it("is a reference-preserving no-op when nothing needs settling", () => {
    const table = settleSessionFailures([record("w", 1)], "all")
    expect(settleSessionFailures(table, "all")).toBe(table)
    expect(settleSessionFailures([], "warnings")).toEqual([])
  })

  it("settles retry incidents on turn progress, sparing notices and errors", () => {
    const table = [
      record("conn", 1, { category: "connection" }),
      record("svc", 1, { category: "service" }),
      // Non-incident informational records (codex config/skill-budget
      // notices, claude advisories) — they must survive to the turn boundary.
      record("notice", 1, { category: "unknown" }),
      record("err", 1, { category: "connection", severity: "error" }),
    ]
    const settled = settleSessionFailures(table, "retry_incidents")
    expect(settled.find((f) => f.id === "conn")?.resolved).toBe(true)
    expect(settled.find((f) => f.id === "svc")?.resolved).toBe(true)
    expect(settled.find((f) => f.id === "notice")?.resolved).toBe(false)
    expect(settled.find((f) => f.id === "err")?.resolved).toBe(false)
    // The clean turn end still sweeps the notice that progress spared.
    const atTurnEnd = settleSessionFailures(settled, "warnings")
    expect(atTurnEnd.find((f) => f.id === "notice")?.resolved).toBe(true)
    expect(atTurnEnd.find((f) => f.id === "err")?.resolved).toBe(false)
  })
})

describe("hasSettleableRetryIncident", () => {
  it("is true only for an unresolved non-'unknown' warning", () => {
    expect(hasSettleableRetryIncident([])).toBe(false)
    expect(
      hasSettleableRetryIncident([record("n", 1, { category: "unknown" })])
    ).toBe(false)
    expect(
      hasSettleableRetryIncident([record("e", 1, { severity: "error" })])
    ).toBe(false)
    expect(
      hasSettleableRetryIncident([
        record("c", 1, { category: "connection", resolved: true }),
      ])
    ).toBe(false)
    expect(
      hasSettleableRetryIncident([record("c", 1, { category: "connection" })])
    ).toBe(true)
  })
})

describe("dismissSessionFailures", () => {
  it("resolves just those ids and keeps them as watermarks", () => {
    const table = [record("a", 2), record("b", 1)]
    const next = dismissSessionFailures(table, ["a"])
    expect(next.find((f) => f.id === "a")?.resolved).toBe(true)
    expect(next.find((f) => f.id === "b")?.resolved).toBe(false)
    // Watermark retained: a stale re-publish at the same revision is rejected,
    // a genuine recurrence at a higher revision re-arms the strip.
    expect(upsertSessionFailure(next, record("a", 2))).toBe(next)
    const rearmed = upsertSessionFailure(next, record("a", 3)).find(
      (f) => f.id === "a"
    )
    expect(rearmed?.resolved).toBe(false)
    expect(rearmed?.dismissed).toBeUndefined()
  })

  it("closes every id one collapsed strip stood for", () => {
    const table = [record("w1", 1), record("w2", 1), record("w3", 1)]
    const next = dismissSessionFailures(table, ["w1", "w2", "w3"])
    expect(next.every((f) => f.resolved && f.dismissed)).toBe(true)
    expect(activeSessionFailureView(next).warning).toBeNull()
  })

  it("marks dismissal distinctly so it never renders as recovery", () => {
    const table = dismissSessionFailures([record("w", 1)], ["w"])
    expect(table[0].dismissed).toBe(true)
    // Resolved, but NOT a recovery — closing a strip must leave nothing behind.
    expect(resolvedSessionFailures(table)).toHaveLength(1)
    expect(mostRecentRecoveredWarning(table)).toBeNull()
  })

  it("silences an ALREADY-RESOLVED record — that is the recovered line's exit", () => {
    // Regression: gating on `!resolved` made the recovered strip's close
    // button and auto-expiry silent no-ops, so it hung under the composer
    // forever (field report 2026-08-17).
    const table = [record("a", 1, { resolved: true })]
    const next = dismissSessionFailures(table, ["a"])
    expect(next).not.toBe(table)
    expect(next[0]).toMatchObject({ resolved: true, dismissed: true })
    expect(mostRecentRecoveredWarning(next)).toBeNull()
  })

  it("is a reference-preserving no-op for unknown / already-dismissed ids", () => {
    const table = [
      record("a", 1, { resolved: true, dismissed: true }),
      record("b", 1),
    ]
    expect(dismissSessionFailures(table, ["a"])).toBe(table)
    expect(dismissSessionFailures(table, ["nope"])).toBe(table)
    expect(dismissSessionFailures(table, [])).toBe(table)
  })

  it("survives a backend snapshot that never saw the dismissal", () => {
    const dismissed = dismissSessionFailures([record("w", 1)], ["w"])
    // The backend still has it unresolved at the same revision: a replay must
    // not un-silence it.
    const hydrated = mergeSessionFailures(dismissed, [
      record("w", 1, { resolved: false }),
    ])
    expect(hydrated).toBe(dismissed)
    expect(activeSessionFailures(hydrated)).toHaveLength(0)
  })
})

describe("mostRecentRecoveredWarning", () => {
  it("picks the latest self-settled warning, ignoring errors and dismissals", () => {
    const table = [
      record("w1", 1, { resolved: true }),
      record("w2", 1, { resolved: true }),
      record("e1", 1, { severity: "error", resolved: true }),
      record("w3", 1),
    ]
    expect(mostRecentRecoveredWarning(table)?.id).toBe("w2")
    // Closing the line on screen falls back to the older genuine recovery,
    // never to the record just silenced.
    const afterDismiss = dismissSessionFailures(table, ["w2"])
    expect(mostRecentRecoveredWarning(afterDismiss)?.id).toBe("w1")
    // …and with that one silenced too, nothing is left to announce.
    expect(
      mostRecentRecoveredWarning(dismissSessionFailures(afterDismiss, ["w1"]))
    ).toBeNull()
  })

  it("returns null when there is nothing recovered", () => {
    expect(mostRecentRecoveredWarning([])).toBeNull()
    expect(mostRecentRecoveredWarning([record("w", 1)])).toBeNull()
  })
})

describe("activeSessionFailureView", () => {
  it("collapses active warnings to the latest plus a count", () => {
    const view = activeSessionFailureView([
      record("w1", 1),
      record("w2", 1),
      record("w3", 1),
      record("gone", 1, { resolved: true }),
    ])
    expect(view.warning?.id).toBe("w3")
    expect(view.hiddenWarnings).toBe(2)
    expect(view.errors).toEqual([])
  })

  it("never collapses errors — each carries its own actions", () => {
    const view = activeSessionFailureView([
      record("e1", 1, { severity: "error" }),
      record("e2", 1, { severity: "error" }),
      record("w", 1),
    ])
    expect(view.errors.map((f) => f.id)).toEqual(["e1", "e2"])
    expect(view.warning?.id).toBe("w")
    expect(view.hiddenWarnings).toBe(0)
  })

  it("reports an empty view when everything is resolved", () => {
    const view = activeSessionFailureView([record("w", 1, { resolved: true })])
    expect(view).toEqual({
      errors: [],
      warning: null,
      hiddenWarnings: 0,
      warningIds: [],
    })
  })
})

describe("lastUserPromptText", () => {
  function turn(
    role: MessageTurn["role"],
    blocks: MessageTurn["blocks"]
  ): MessageTurn {
    return { id: `${role}-${Math.random()}`, role, blocks, timestamp: "" }
  }
  const text = (t: string) => ({ type: "text", text: t }) as const
  const image = {
    type: "image",
    data: "aGk=",
    mime_type: "image/png",
    uri: null,
  } as const

  it("returns the MOST RECENT user turn's joined text", () => {
    const turns = [
      turn("user", [text("first prompt")]),
      turn("assistant", [text("reply")]),
      turn("user", [text("line one"), image, text("line two")]),
      turn("assistant", [text("failed mid-way")]),
    ]
    expect(lastUserPromptText(turns)).toBe("line one\nline two")
  })

  it("skips image-only and blank user turns, and handles no-user/undefined", () => {
    // The retry action must resend something MEANINGFUL: an image-only or
    // whitespace user turn yields nothing, so the scan continues backwards.
    const turns = [
      turn("user", [text("real prompt")]),
      turn("user", [image]),
      turn("user", [text("   ")]),
    ]
    expect(lastUserPromptText(turns)).toBe("real prompt")
    expect(lastUserPromptText([turn("assistant", [text("only agent")])])).toBe(
      null
    )
    expect(lastUserPromptText([])).toBe(null)
    expect(lastUserPromptText(undefined)).toBe(null)
  })
})

describe("selectors", () => {
  it("splits active from resolved and filters renderable actions", () => {
    const table = [
      record("w", 1, { resolved: true }),
      record("e", 1, {
        severity: "error",
        actions: ["login", "sing", "retry"],
      }),
    ]
    expect(activeSessionFailures(table).map((f) => f.id)).toEqual(["e"])
    expect(resolvedSessionFailures(table).map((f) => f.id)).toEqual(["w"])
    // Order follows the known vocabulary, unknown entries dropped.
    expect(knownSessionFailureActions(table[1])).toEqual(["retry", "login"])
    expect(
      knownSessionFailureActions(record("x", 1, { actions: undefined }))
    ).toEqual([])
  })
})
