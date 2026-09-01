import { describe, expect, it } from "vitest"

import {
  advanceReplyFold,
  mergeConsecutiveAssistantTurns,
  singletonSourceTurns,
  type MergedAssistantRunCache,
  type ReplyFoldState,
  type ResolvedMessageGroup,
  type ThreadRenderItem,
} from "./message-list-view"
import type { MessageTurn } from "@/lib/types"

function turn(id: string): MessageTurn {
  return { id, role: "assistant", blocks: [], timestamp: "" }
}

type ThreadItem = Parameters<typeof mergeConsecutiveAssistantTurns>[0][number]
type TurnItem = Extract<ThreadItem, { kind: "turn" }>

function assistantItem(
  id: string,
  groupOverrides: Partial<TurnItem["group"]> = {}
): ThreadItem {
  return {
    key: `persisted-${id}`,
    kind: "turn",
    group: {
      id,
      role: "assistant",
      parts: [{ type: "text", text: `reply ${id}` }],
      resources: [],
      images: [],
      ...groupOverrides,
    },
    phase: "persisted",
    isResponseComplete: true,
    showStats: false,
    isRoleTransition: false,
    previousUserIndex: null,
    isLastAssistantRun: false,
    sourceTurns: [],
  }
}

describe("advanceReplyFold", () => {
  const initial: ReplyFoldState = {
    signal: 0,
    epoch: 0,
    armed: false,
    running: false,
    runId: null,
    roundOpen: true,
  }
  // Live-message ids — the logical run, one per reply.
  const A = "lm-a"
  const B = "lm-b"
  const idle = (runId: string | null = null, sendSignal = 0) => ({
    sendSignal,
    running: false,
    runId,
  })
  const live = (runId: string, sendSignal = 0) => ({
    sendSignal,
    running: true,
    runId,
  })

  it("opens a conversation with every reply folded", () => {
    // Nothing is streaming on load, so no run is the current round and every
    // reply falls back to its own (empty) fold override.
    expect(advanceReplyFold(initial, idle(A))).toBe(initial)
  })

  it("arms once the agent starts replying and stays armed after it settles", () => {
    const armed = advanceReplyFold(initial, live(A))
    expect(armed.armed).toBe(true)
    // The reply settling must NOT disarm — that is the auto-fold-on-finish
    // this replaced. Only the running edge moves.
    const settled = advanceReplyFold(armed, idle(A))
    expect(settled.armed).toBe(true)
    expect(settled.roundOpen).toBe(true)
    expect(settled.running).toBe(false)
    // Steady state after that: nothing left to move.
    expect(advanceReplyFold(settled, idle(A))).toBe(settled)
  })

  it("keeps a hand-folded round folded while it settles", () => {
    const folded: ReplyFoldState = {
      signal: 0,
      epoch: 0,
      armed: true,
      running: true,
      runId: A,
      roundOpen: false,
    }
    expect(advanceReplyFold(folded, idle(A)).roundOpen).toBe(false)
  })

  it("folds the thread and disarms on send", () => {
    const settled = advanceReplyFold(
      advanceReplyFold(initial, live(A)),
      idle(A)
    )
    const sent = advanceReplyFold(settled, idle(A, 1))
    expect(sent).toMatchObject({
      signal: 1,
      armed: false,
      running: false,
      roundOpen: true,
    })
    // The epoch only ever has to MOVE — its absolute value is an invalidation
    // token, and a round starting bumps it too.
    expect(sent.epoch).toBeGreaterThan(settled.epoch)
  })

  it("re-arms immediately when a send lands mid-reply (steering)", () => {
    const armed = advanceReplyFold(initial, live(A))
    const steered = advanceReplyFold(armed, live(A, 1))
    // The reply being written is at once the new round: bumping the epoch folds
    // the history above it without folding the reply itself.
    expect(steered).toMatchObject({
      signal: 1,
      armed: true,
      running: true,
      roundOpen: true,
    })
    expect(steered.epoch).toBeGreaterThan(armed.epoch)
  })

  it("starts a fresh round on the running edge, with no send signal at all", () => {
    // `live-transcript-view` mounts MessageListView WITHOUT `sendSignal` for a
    // work-task transcript the engine drives through many rounds. Keying the
    // round off `sendSignal` alone left every finished round expanded there.
    let s = advanceReplyFold(initial, live(A)) // round 1 starts
    s = advanceReplyFold(s, idle(A)) // round 1 settles
    const roundOneEpoch = s.epoch

    const roundTwo = advanceReplyFold(s, live(B)) // round 2, same signal
    expect(roundTwo.armed).toBe(true)
    expect(roundTwo.roundOpen).toBe(true)
    expect(roundTwo.runId).toBe(B)
    // A fresh epoch is what folds round 1 shut behind round 2.
    expect(roundTwo.epoch).toBeGreaterThan(roundOneEpoch)
  })

  it("does not hand a hand-folded round's collapse to the next round", () => {
    // Same no-send host: folding round 1 by hand set `roundOpen: false`, and a
    // latched `armed` meant round 2 inherited it and arrived already collapsed.
    let s = advanceReplyFold(initial, live(A))
    s = { ...s, roundOpen: false } // reader folds the live round
    s = advanceReplyFold(s, idle(A)) // it settles, still folded
    expect(s.roundOpen).toBe(false)

    expect(advanceReplyFold(s, live(B)).roundOpen).toBe(true)
  })

  it("starts a fresh round for a streaming reply that merges behind a settled one", () => {
    // Background / loop turns arrive with no user turn between, so a settled
    // reply and a brand-new streaming one end up CONSECUTIVE and
    // `mergeConsecutiveAssistantTurns` folds them into one render item whose id
    // is pinned to the first member. Identifying the round by that id would
    // read the new reply as the old one resuming — it would arrive folded (if
    // the reader had folded the old one) with its live content hidden. The live
    // message is the run, so the merge cannot alias the two.
    let s = advanceReplyFold(initial, live(A))
    s = { ...s, roundOpen: false } // reader folds reply A
    s = advanceReplyFold(s, idle(null)) // A settles, live message cleared
    const settledEpoch = s.epoch

    const merged = advanceReplyFold(s, live(B))
    expect(merged.roundOpen).toBe(true)
    expect(merged.armed).toBe(true)
    expect(merged.epoch).toBeGreaterThan(settledEpoch)
  })

  it("latches a live identity that arrives after the round started", () => {
    // Mid-turn attach: a viewer joining a reply already in flight sees it
    // through the backend's in-flight marker first (running, but no live
    // message yet) and bridges the live stream a beat later. Leaving the round
    // anonymous gives a later re-bridge nothing to recognise the reply by.
    let s = advanceReplyFold(initial, {
      sendSignal: 0,
      running: true,
      runId: null,
    })
    s = advanceReplyFold(s, live(A)) // live stream attaches mid-round
    expect(s.runId).toBe(A)
    const latchedEpoch = s.epoch
    s = { ...s, roundOpen: false } // reader folds the reply

    s = advanceReplyFold(s, idle(null)) // premature COMPLETE_TURN
    const rebridged = advanceReplyFold(s, live(A))
    // Still the same reply: the latch is what keeps this from reading as new.
    expect(rebridged.epoch).toBe(latchedEpoch)
    expect(rebridged.roundOpen).toBe(false)
  })

  it("re-identifies, not re-rounds, when a running reply's id is rebased", () => {
    // `STATUS_CHANGED(prompting)` mints a client-side `randomUUID` live
    // message; a reconnect that hydrates from a snapshot swaps it wholesale for
    // the backend's, mid-reply. Reading that as a new round would re-open a
    // reply the reader had folded and fold the history they had opened — on
    // every reconnect.
    let s = advanceReplyFold(initial, live(A))
    s = { ...s, roundOpen: false } // reader folds the live reply
    const rebased = advanceReplyFold(s, live(B)) // snapshot hydration
    expect(rebased.runId).toBe(B)
    expect(rebased.epoch).toBe(s.epoch)
    expect(rebased.roundOpen).toBe(false)
    // And the new id is what a later re-bridge is recognised by.
    const settled = advanceReplyFold(rebased, idle(null))
    expect(advanceReplyFold(settled, live(B)).roundOpen).toBe(false)
  })

  it("treats a re-bridged live reply as the same round, not a new one", () => {
    // The runtime completes a live reply prematurely and then re-bridges the
    // SAME liveMessage while it is still streaming (see "drops the promoted
    // snapshot when the same liveMessage is still streaming" in
    // conversation-runtime-context.test.tsx). That reaches the fold state as
    // running true → false → true for ONE reply, carrying one unchanged id.
    let s = advanceReplyFold(initial, live(A))
    s = { ...s, roundOpen: false } // reader folds the live reply
    const armedEpoch = s.epoch

    s = advanceReplyFold(s, idle(A)) // premature COMPLETE_TURN
    const rebridged = advanceReplyFold(s, live(A)) // same liveMessage returns

    expect(rebridged.running).toBe(true)
    // Neither may move: a bump would fold history the reader had opened, and a
    // reset would re-open the reply they had just folded.
    expect(rebridged.epoch).toBe(armedEpoch)
    expect(rebridged.roundOpen).toBe(false)
  })
})

describe("singletonSourceTurns", () => {
  it("returns the same array reference for the same turn", () => {
    const t = turn("t1")
    const first = singletonSourceTurns(t)
    const second = singletonSourceTurns(t)
    // Reference stability is the whole point: it lets HistoricalMessageGroup's
    // memo bail out when an unchanged historical turn re-renders per token.
    expect(first).toBe(second)
    expect(first).toEqual([t])
  })

  it("returns distinct arrays for distinct turns", () => {
    const a = singletonSourceTurns(turn("a"))
    const b = singletonSourceTurns(turn("b"))
    expect(a).not.toBe(b)
  })
})

describe("mergeConsecutiveAssistantTurns", () => {
  it("surfaces completion time patched onto a non-last sub-turn", () => {
    // Real-device bug (Cursor session 118b6805): the post-turn metadata
    // patch head-aligns onto the FIRST local sub-turn when the parser emits
    // fewer turns than the live stream split into. The merged footer must
    // still show that completion time (and its duration), not the last
    // sub-turn's empty fields.
    const merged = mergeConsecutiveAssistantTurns([
      assistantItem("a", {
        duration_ms: 15_975,
        completed_at: "2026-07-19T05:25:22.851Z",
      }),
      assistantItem("b"),
    ])
    expect(merged).toHaveLength(1)
    const item = merged[0] as TurnItem
    expect(item.group.completed_at).toBe("2026-07-19T05:25:22.851Z")
    expect(item.group.duration_ms).toBe(15_975)
  })

  it("keeps the latest completion when several sub-turns carry one", () => {
    const merged = mergeConsecutiveAssistantTurns([
      assistantItem("a", { completed_at: "2026-07-19T05:25:10.000Z" }),
      assistantItem("b", { completed_at: "2026-07-19T05:25:22.851Z" }),
    ])
    expect(merged).toHaveLength(1)
    const item = merged[0] as TurnItem
    expect(item.group.completed_at).toBe("2026-07-19T05:25:22.851Z")
  })

  it("does not fold a compaction divider into the preceding assistant reply", () => {
    // The compaction event sits BETWEEN two assistant replies (the reply before
    // `/compact` and the next). Two bare assistant turns would merge into one;
    // the dedicated "compaction" item must break that run so the divider renders
    // standalone in the correct between-turns position (and the first reply keeps
    // its own footer).
    const compaction: ThreadItem = {
      key: "persisted-compact",
      kind: "compaction",
      meta: { contextCompaction: true, tokensBefore: 51777, tokensAfter: 4616 },
    }
    // Sanity: without the divider, the two assistant turns DO merge to one.
    expect(
      mergeConsecutiveAssistantTurns([assistantItem("a"), assistantItem("b")])
    ).toHaveLength(1)
    // With the divider between them, the run is broken → 3 standalone items.
    const merged = mergeConsecutiveAssistantTurns([
      assistantItem("a"),
      compaction,
      assistantItem("b"),
    ])
    expect(merged.map((it) => it.kind)).toEqual(["turn", "compaction", "turn"])
  })
})

function makeGroup(
  role: "user" | "assistant",
  id: string
): ResolvedMessageGroup {
  return { id, role, parts: [], resources: [], images: [] }
}

// Fresh render-item objects per call, like the rawItems map in threadItems —
// only `group`, `key`, and the sourceTurns wrapper carry identity.
function makeItem(
  group: ResolvedMessageGroup,
  index: number,
  phase: "persisted" | "optimistic" | "streaming" = "persisted"
): ThreadRenderItem {
  return {
    key: `${phase}-${group.id}-${index}`,
    kind: "turn",
    group,
    phase,
    isResponseComplete: phase === "persisted",
    showStats: false,
    isRoleTransition: false,
    previousUserIndex: null,
    isLastAssistantRun: false,
    sourceTurns: singletonSourceTurns(turn(group.id)),
  }
}

function makeUserItem(id: string, index: number): ThreadRenderItem {
  const item = makeItem(makeGroup("user", id), index)
  if (item.kind === "turn") {
    item.group.parts = [{ type: "text", text: "hi" }]
  }
  return item
}

describe("mergeConsecutiveAssistantTurns merged-run cache", () => {
  it("reuses the merged item (group/parts/sourceTurns) when membership is unchanged", () => {
    const cache: MergedAssistantRunCache = new WeakMap()
    const g1 = makeGroup("assistant", "a1")
    const g2 = makeGroup("assistant", "a2")

    const out1 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), makeItem(g2, 1)],
      cache
    )
    const out2 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), makeItem(g2, 1)],
      cache
    )

    expect(out1).toHaveLength(1)
    const first = out1[0]
    const second = out2[0]
    if (first.kind !== "turn" || second.kind !== "turn") {
      throw new Error("expected turn items")
    }
    expect(second).toBe(first)
    expect(second.group).toBe(first.group)
    expect(second.group.parts).toBe(first.group.parts)
    expect(second.sourceTurns).toBe(first.sourceTurns)
    expect(first.key).toBe("merged-persisted-a1-0")
    expect(first.group.id).toBe("a1")
  })

  it("rebuilds a run whose member changed without touching a neighboring run", () => {
    const cache: MergedAssistantRunCache = new WeakMap()
    const g1 = makeGroup("assistant", "a1")
    const g2 = makeGroup("assistant", "a2")
    const g3 = makeGroup("assistant", "a3")
    const g4 = makeGroup("assistant", "a4")

    const out1 = mergeConsecutiveAssistantTurns(
      [
        makeItem(g1, 0),
        makeItem(g2, 1),
        makeUserItem("u1", 2),
        makeItem(g3, 3),
        makeItem(g4, 4),
      ],
      cache
    )
    // Second member of run A re-adapted (new group object, e.g. its turn was
    // reloaded); run B untouched.
    const g2b = makeGroup("assistant", "a2")
    const out2 = mergeConsecutiveAssistantTurns(
      [
        makeItem(g1, 0),
        makeItem(g2b, 1),
        makeUserItem("u1", 2),
        makeItem(g3, 3),
        makeItem(g4, 4),
      ],
      cache
    )

    expect(out2[0]).not.toBe(out1[0])
    expect(out2[2]).toBe(out1[2])
  })

  it("rebuilds when a persisted run changes from in-flight to completed", () => {
    const cache: MergedAssistantRunCache = new WeakMap()
    const g1 = makeGroup("assistant", "a1")
    const g2 = makeGroup("assistant", "a2")
    const firstItems = [makeItem(g1, 0), makeItem(g2, 1)]
    if (firstItems[1].kind !== "turn") throw new Error("expected turn")
    firstItems[1].isResponseComplete = false

    const out1 = mergeConsecutiveAssistantTurns(firstItems, cache)
    const out2 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), makeItem(g2, 1)],
      cache
    )

    expect(out2[0]).not.toBe(out1[0])
    if (out2[0].kind !== "turn") throw new Error("expected turn")
    expect(out2[0].isResponseComplete).toBe(true)
  })

  it("misses when the run gains a member, then caches the new membership", () => {
    const cache: MergedAssistantRunCache = new WeakMap()
    const g1 = makeGroup("assistant", "a1")
    const g2 = makeGroup("assistant", "a2")
    const g3 = makeGroup("assistant", "a3")

    const out1 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), makeItem(g2, 1)],
      cache
    )
    const out2 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), makeItem(g2, 1), makeItem(g3, 2)],
      cache
    )
    const out3 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), makeItem(g2, 1), makeItem(g3, 2)],
      cache
    )

    expect(out2[0]).not.toBe(out1[0])
    expect(out3[0]).toBe(out2[0])
  })

  it("keeps cache hits across interleaved empty (skipped) turn items", () => {
    const cache: MergedAssistantRunCache = new WeakMap()
    const g1 = makeGroup("assistant", "a1")
    const g2 = makeGroup("assistant", "a2")
    const emptyUser = () => makeItem(makeGroup("user", "empty"), 1)

    const out1 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), emptyUser(), makeItem(g2, 2)],
      cache
    )
    const out2 = mergeConsecutiveAssistantTurns(
      [makeItem(g1, 0), emptyUser(), makeItem(g2, 2)],
      cache
    )

    // The empty user turn is transparent: one merged item, no user item.
    expect(out1).toHaveLength(1)
    expect(out2[0]).toBe(out1[0])
  })

  it("passes single-turn runs through untouched without caching", () => {
    const cache: MergedAssistantRunCache = new WeakMap()
    const item = makeItem(makeGroup("assistant", "solo"), 0)

    const out = mergeConsecutiveAssistantTurns([item], cache)

    expect(out).toHaveLength(1)
    expect(out[0]).toBe(item)
  })

  it("still merges correctly without a cache", () => {
    const g1 = makeGroup("assistant", "a1")
    const g2 = makeGroup("assistant", "a2")

    const out1 = mergeConsecutiveAssistantTurns([
      makeItem(g1, 0),
      makeItem(g2, 1),
    ])
    const out2 = mergeConsecutiveAssistantTurns([
      makeItem(g1, 0),
      makeItem(g2, 1),
    ])

    expect(out1).toHaveLength(1)
    expect(out2[0]).not.toBe(out1[0])
    expect(out2[0]).toEqual(out1[0])
  })
})
