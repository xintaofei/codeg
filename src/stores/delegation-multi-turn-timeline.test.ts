import { afterEach, describe, expect, it } from "vitest"
import type { LiveMessage } from "@/contexts/acp-connections-context"
import type { DbConversationDetail, MessageTurn } from "@/lib/types"
import {
  getTimelineTurns,
  resetConversationRuntimeStore,
  useConversationRuntimeStore,
  type ConversationRuntimeSession,
} from "@/stores/conversation-runtime-store"

/**
 * Property 5 (design.md · Requirement 4.7): a delegation child that has been
 * continued holds MANY persisted turns. While a continued turn streams, every
 * previously persisted turn must stay visible — the viewer may only suppress
 * the persisted copy of the reply currently in flight, never earlier rounds.
 */

const CID = 77

function turn(
  id: string,
  role: "user" | "assistant",
  timestamp = "2026-07-26T00:00:00.000Z"
): MessageTurn {
  return { id, role, blocks: [{ type: "text", text: id }], timestamp }
}

function makeDetail(
  turns: MessageTurn[],
  inFlightUserTurnId: string | null = null
): DbConversationDetail {
  return {
    summary: {
      id: CID,
      folder_id: 1,
      title: "child",
      title_locked: false,
      agent_type: "codex",
      status: "in_progress",
      kind: "delegate",
      model: null,
      git_branch: null,
      external_id: "ext-1",
      message_count: turns.length,
      child_count: 0,
      created_at: "2026-07-26T00:00:00.000Z",
      updated_at: "2026-07-26T00:00:00.000Z",
      pinned_at: null,
    },
    turns,
    in_flight_user_turn_id: inFlightUserTurnId,
  }
}

function seedSession(overrides: Partial<ConversationRuntimeSession>) {
  const session: ConversationRuntimeSession = {
    conversationId: CID,
    externalId: null,
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
    syncState: "idle",
    activeTurnToken: null,
    lastTurnOwned: false,
    liveOwnsActiveTurn: false,
    delegationKickoffText: null,
    sessionStats: null,
    historyAssistantBaseline: null,
    pendingCleanup: false,
    ...overrides,
  }
  const next = new Map(
    useConversationRuntimeStore.getState().byConversationId
  ).set(CID, session)
  useConversationRuntimeStore.setState({ byConversationId: next })
}

const liveMsg = (id: string, text: string): LiveMessage => ({
  id,
  role: "assistant",
  content: [{ type: "text", text }],
  startedAt: 1_785_000_000_000,
})

afterEach(() => {
  resetConversationRuntimeStore()
})

describe("delegation child multi-turn timeline (Property 5)", () => {
  it("keeps every persisted turn of earlier rounds while a continued turn streams", () => {
    // Two completed rounds on disk; the child's third prompt has not been
    // persisted yet (the agent CLI writes its JSONL asynchronously), so the
    // backend reports no in-flight prompt id.
    const persisted = [
      turn("u1", "user", "2026-07-26T00:00:00.000Z"),
      turn("a1", "assistant", "2026-07-26T00:00:01.000Z"),
      turn("u2", "user", "2026-07-26T00:00:02.000Z"),
      turn("a2", "assistant", "2026-07-26T00:00:03.000Z"),
    ]
    seedSession({
      detail: makeDetail(persisted),
      liveOwnsActiveTurn: true,
      liveMessage: liveMsg("m3", "third round streaming"),
    })

    const timeline = getTimelineTurns(CID)
    // Property 5: at least the N persisted turns are still rendered.
    expect(timeline.length).toBeGreaterThanOrEqual(persisted.length)
    expect(timeline.map((e) => e.turn.id)).toEqual([
      "u1",
      "a1",
      "u2",
      "a2",
      `live-${CID}-m3`,
    ])
  })

  it("suppresses only the in-flight round's persisted partial reply, keeping all history", () => {
    // Third prompt HAS landed and the backend stamped it as the in-flight
    // prompt; the agent also persisted a partial reply after it.
    const persisted = [
      turn("u1", "user", "2026-07-26T00:00:00.000Z"),
      turn("a1", "assistant", "2026-07-26T00:00:01.000Z"),
      turn("u2", "user", "2026-07-26T00:00:02.000Z"),
      turn("a2", "assistant", "2026-07-26T00:00:03.000Z"),
      turn("u3", "user", "2026-07-26T00:00:04.000Z"),
      turn("a3-partial", "assistant", "2026-07-26T00:00:05.000Z"),
    ]
    seedSession({
      detail: makeDetail(persisted, "u3"),
      liveOwnsActiveTurn: true,
      liveMessage: liveMsg("m3", "third round streaming"),
    })

    const ids = getTimelineTurns(CID).map((e) => e.turn.id)
    // The partial copy of the streaming reply is hidden (the live stream shows
    // it); everything before the in-flight prompt survives untouched.
    expect(ids).toEqual(["u1", "a1", "u2", "a2", "u3", `live-${CID}-m3`])
  })

  it("keeps earlier rounds visible while a continued reply sits in localTurns (post-promotion grace)", () => {
    const persisted = [
      turn("u1", "user", "2026-07-26T00:00:00.000Z"),
      turn("a1", "assistant", "2026-07-26T00:00:01.000Z"),
      turn("u2", "user", "2026-07-26T00:00:02.000Z"),
    ]
    seedSession({
      detail: makeDetail(persisted),
      liveOwnsActiveTurn: true,
      localTurns: [turn("a2-local", "assistant", "2026-07-26T00:00:04.000Z")],
    })

    const ids = getTimelineTurns(CID).map((e) => e.turn.id)
    expect(ids).toEqual(["u1", "a1", "u2", "a2-local"])
  })

  it("still suppresses the persisted partial of a first, single-round reply (no history to lose)", () => {
    // Regression guard for the one-shot case the projection was built for: a
    // transcript with at most the kickoff user turn carries no earlier round,
    // so the trailing assistant turn IS the reply the live stream owns.
    seedSession({
      detail: makeDetail([
        turn("u1", "user"),
        turn("a1-partial", "assistant", "2026-07-26T00:00:01.000Z"),
      ]),
      liveOwnsActiveTurn: true,
      liveMessage: liveMsg("m1", "streaming"),
    })
    expect(getTimelineTurns(CID).map((e) => e.turn.id)).toEqual([
      "u1",
      `live-${CID}-m1`,
    ])

    // …and the same with no persisted user turn at all (DB lags the stream).
    resetConversationRuntimeStore()
    seedSession({
      detail: makeDetail([turn("a1-partial", "assistant")]),
      liveOwnsActiveTurn: true,
      delegationKickoffText: "do the thing",
      liveMessage: liveMsg("m1", "streaming"),
    })
    expect(getTimelineTurns(CID).map((e) => e.turn.id)).toEqual([
      `kickoff-${CID}`,
      `live-${CID}-m1`,
    ])
  })

  it("leaves a normal (non-delegation) session's history untouched", () => {
    seedSession({
      detail: makeDetail([
        turn("u1", "user"),
        turn("a1", "assistant", "2026-07-26T00:00:01.000Z"),
      ]),
      liveMessage: liveMsg("m2", "next reply"),
    })
    expect(getTimelineTurns(CID).map((e) => e.turn.id)).toEqual([
      "u1",
      "a1",
      `live-${CID}-m2`,
    ])
  })
})
