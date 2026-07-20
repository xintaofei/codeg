import { afterEach, describe, expect, it } from "vitest"
import type { LiveMessage } from "@/contexts/acp-connections-context"
import type { SessionStats } from "@/lib/types"
import {
  resetConversationRuntimeStore,
  useConversationRuntimeStore,
} from "@/stores/conversation-runtime-store"

const CID = 42

// A complete runtime session (mirrors the store's internal `createEmptySession`)
// with distinct non-null references for the three fields the conversation panel
// reads from it, seeded straight into the store.
function seedSession(sessionStats: SessionStats) {
  useConversationRuntimeStore.setState({
    byConversationId: new Map([
      [
        CID,
        {
          conversationId: CID,
          externalId: "sid-1",
          dbConversationId: null,
          detail: null,
          detailLoading: false,
          detailError: null,
          acpLoadError: null,
          localTurns: [],
          backgroundTurns: [],
          pendingBackgroundSettlements: [],
          optimisticTurns: [],
          liveMessage: null,
          syncState: "awaiting_persist",
          activeTurnToken: null,
          lastTurnOwned: false,
          liveOwnsActiveTurn: false,
          delegationKickoffText: null,
          sessionStats,
          historyAssistantBaseline: null,
          pendingCleanup: false,
        },
      ],
    ]),
  })
}

const liveMsg = (id: string): LiveMessage => ({
  id,
  role: "assistant",
  content: [],
  startedAt: 0,
})

// The three fields the keep-alive panel actually reads from its session.
function panelSlice() {
  const s = useConversationRuntimeStore.getState().byConversationId.get(CID)
  return {
    sessionStats: s?.sessionStats ?? null,
    externalId: s?.externalId ?? null,
    syncState: s?.syncState ?? "idle",
  }
}

// The keep-alive conversation panel (`ConversationTabView`) subscribes to a
// `useShallow` slice of {sessionStats, externalId, syncState} from its runtime
// session — NOT the whole session object. The live-message sink rewrites the
// session object on every streaming batch (~60/s, via SET_LIVE_MESSAGE); a
// whole-object selector would re-render the panel per token. These tests encode
// the store invariant that narrowing depends on: SET_LIVE_MESSAGE replaces the
// session object (so the OLD whole-object selector churned) while preserving the
// references of those three fields (so the slice is Object.is-stable and
// `useShallow` bails → no re-render) — and that a real change to one of the
// three still propagates (no over-suppression).
describe("runtime session panel slice is decoupled from live-message streaming", () => {
  afterEach(() => resetConversationRuntimeStore())

  it("replaces the session object but keeps the panel's three fields stable across a streaming batch", () => {
    const stats: SessionStats = { total_usage: null, total_duration_ms: 0 }
    seedSession(stats)

    const before = useConversationRuntimeStore
      .getState()
      .byConversationId.get(CID)
    const beforeSlice = panelSlice()

    // A streaming batch lands: the connection sink writes liveMessage (isLive).
    useConversationRuntimeStore
      .getState()
      .actions.setLiveMessage(CID, liveMsg("m1"), true)

    const after = useConversationRuntimeStore
      .getState()
      .byConversationId.get(CID)
    // The session OBJECT was replaced — exactly why a whole-object selector
    // re-rendered the keep-alive panel on every token.
    expect(after).not.toBe(before)
    expect(after?.liveMessage).not.toBeNull()

    // ...but every field the panel's narrow slice reads kept its identity, so
    // `useShallow` shallow-compares equal and the panel does NOT re-render.
    const afterSlice = panelSlice()
    expect(afterSlice.sessionStats).toBe(beforeSlice.sessionStats)
    expect(afterSlice.sessionStats).toBe(stats)
    expect(afterSlice.externalId).toBe(beforeSlice.externalId)
    expect(afterSlice.syncState).toBe(beforeSlice.syncState)
  })

  it("still propagates a real change to a slice field (no over-suppression)", () => {
    const stats: SessionStats = { total_usage: null, total_duration_ms: 0 }
    seedSession(stats)
    const beforeSlice = panelSlice()
    expect(beforeSlice.syncState).toBe("awaiting_persist")

    // A genuine syncState transition must change the slice so the panel updates.
    useConversationRuntimeStore.getState().actions.setSyncState(CID, "idle")

    const afterSlice = panelSlice()
    expect(afterSlice.syncState).toBe("idle")
    expect(afterSlice.syncState).not.toBe(beforeSlice.syncState)
    // Unrelated slice fields keep their identity across the syncState change.
    expect(afterSlice.sessionStats).toBe(beforeSlice.sessionStats)
    expect(afterSlice.externalId).toBe(beforeSlice.externalId)
  })
})
