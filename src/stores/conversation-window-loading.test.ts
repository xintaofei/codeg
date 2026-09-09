/**
 * Coverage for the windowed conversation-loading protocol:
 * - cold loads request a tail window; refreshes re-request the loaded window
 *   (`fromIndex = turns_offset`) and fall back to a fresh tail when the
 *   prefix fingerprint no longer matches (compaction rewrote history);
 * - `loadOlderTurns` prepends an older page only when its seam proof matches
 *   the current window fingerprint, participates in the fetch-generation
 *   total order, and is single-flight;
 * - the watermark overlay retirement respects window coverage (an overlay
 *   whose persisted twin fell before the window start must never be retired
 *   into nothingness);
 * - `syncTurnMetadata` anchors at the captured batch boundary and refuses to
 *   patch when the boundary fingerprint mismatches (metadata absence is
 *   recoverable, a mis-patch is not).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type {
  ConversationTurnsPage,
  DbConversationDetail,
  MessageTurn,
} from "@/lib/types"
import { extendPrefixFingerprint, FNV_SEED_HEX } from "@/lib/turn-window"
import {
  resetConversationRuntimeStore,
  useConversationRuntimeStore,
  TAIL_TURNS_DEFAULT,
  OLDER_TURNS_PAGE_SIZE,
  type ConversationRuntimeSession,
} from "@/stores/conversation-runtime-store"

vi.mock("@/lib/api", () => ({
  getFolderConversation: vi.fn(),
  getFolderConversationTurns: vi.fn(),
}))

const { getFolderConversation, getFolderConversationTurns } =
  await import("@/lib/api")
const mockGet = vi.mocked(getFolderConversation)
const mockGetTurns = vi.mocked(getFolderConversationTurns)

const CID = 77

function ts(secs: number): string {
  return new Date(1_700_000_000_000 + secs * 1000).toISOString()
}

function turn(
  id: string,
  role: MessageTurn["role"],
  secs: number
): MessageTurn {
  return {
    id,
    role,
    blocks: [{ type: "text", text: id }],
    timestamp: ts(secs),
  }
}

/** Full synthetic transcript: u0 a1 u2 a3 u4 a5 (global indices 0..5). */
const FULL: MessageTurn[] = [
  turn("turn-0", "user", 0),
  turn("turn-1", "assistant", 1),
  turn("turn-2", "user", 2),
  turn("turn-3", "assistant", 3),
  turn("turn-4", "user", 4),
  turn("turn-5", "assistant", 5),
]

function hashPrefix(count: number): string {
  const h = extendPrefixFingerprint(FNV_SEED_HEX, FULL.slice(0, count))
  if (h === null) throw new Error("fingerprint failed")
  return h
}

function windowedDetail(
  offset: number,
  overrides: Partial<DbConversationDetail> = {}
): DbConversationDetail {
  const turns = FULL.slice(offset)
  return {
    summary: {
      id: CID,
      folder_id: 1,
      agent_type: "claude_code",
      title: null,
      title_locked: false,
      status: "completed",
      kind: "regular",
      model: null,
      git_branch: null,
      external_id: "ext-1",
      message_count: FULL.length,
      child_count: 0,
      created_at: ts(0),
      updated_at: ts(10),
      pinned_at: null,
    },
    turns,
    session_stats: null,
    transcript_watermark: null,
    in_flight_user_turn_id: null,
    turns_offset: offset,
    turns_total: FULL.length,
    assistant_turns_before_offset: FULL.slice(0, offset).filter(
      (t) => t.role === "assistant"
    ).length,
    prefix_hash: hashPrefix(offset),
    uncovered_prefix_max_ts: offset > 0 ? FULL[offset - 1].timestamp : null,
    ...overrides,
  }
}

function page(
  start: number,
  end: number,
  overrides: Partial<ConversationTurnsPage> = {}
): ConversationTurnsPage {
  return {
    turns: FULL.slice(start, end),
    turns_offset: start,
    turns_total: FULL.length,
    assistant_turns_before_offset: FULL.slice(0, start).filter(
      (t) => t.role === "assistant"
    ).length,
    prefix_hash: hashPrefix(start),
    prefix_hash_before_index: hashPrefix(end),
    uncovered_prefix_max_ts: start > 0 ? FULL[start - 1].timestamp : null,
    ...overrides,
  }
}

function emptySession(conversationId: number): ConversationRuntimeSession {
  return {
    conversationId,
    externalId: "ext-1",
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
    batchBoundaryIndex: null,
    batchBoundaryPrefixHash: null,
    loadingOlderTurns: false,
    olderTurnsPrependEpoch: 0,
    pendingCleanup: false,
  }
}

function seed(overrides: Partial<ConversationRuntimeSession>): void {
  useConversationRuntimeStore.setState({
    byConversationId: new Map([[CID, { ...emptySession(CID), ...overrides }]]),
  })
}

function session(): ConversationRuntimeSession | undefined {
  return useConversationRuntimeStore.getState().byConversationId.get(CID)
}

function actions() {
  return useConversationRuntimeStore.getState().actions
}

async function flush(): Promise<void> {
  await Promise.resolve()
  await Promise.resolve()
  await Promise.resolve()
}

beforeEach(() => {
  resetConversationRuntimeStore()
  mockGet.mockReset()
  mockGetTurns.mockReset()
})

afterEach(() => {
  vi.useRealTimers()
})

describe("windowed fetch/refetch", () => {
  it("cold fetch requests the default tail window", async () => {
    seed({})
    mockGet.mockResolvedValue(windowedDetail(4))
    actions().fetchDetail(CID)
    await flush()
    expect(mockGet).toHaveBeenCalledWith(CID, { tailTurns: TAIL_TURNS_DEFAULT })
    expect(session()?.detail?.turns_offset).toBe(4)
  })

  it("refetch on a windowed detail refreshes the loaded window in place", async () => {
    seed({ detail: windowedDetail(2) })
    mockGet.mockResolvedValue(windowedDetail(2))
    actions().refetchDetail(CID)
    await flush()
    expect(mockGet).toHaveBeenCalledTimes(1)
    expect(mockGet).toHaveBeenCalledWith(CID, { fromIndex: 2 })
    expect(session()?.detail?.turns.map((t) => t.id)).toEqual([
      "turn-2",
      "turn-3",
      "turn-4",
      "turn-5",
    ])
  })

  it("refetch falls back to a fresh tail when the prefix fingerprint changed", async () => {
    seed({ detail: windowedDetail(2) })
    // The window-refresh read reports a REWRITTEN prefix (different hash);
    // the loaded window's coordinates are garbage → a second request must
    // reload a fresh tail and commit that instead.
    mockGet
      .mockResolvedValueOnce(
        windowedDetail(2, { prefix_hash: "00000000000000ff" })
      )
      .mockResolvedValueOnce(windowedDetail(4))
    actions().refetchDetail(CID)
    await flush()
    expect(mockGet).toHaveBeenNthCalledWith(1, CID, { fromIndex: 2 })
    expect(mockGet).toHaveBeenNthCalledWith(2, CID, {
      tailTurns: TAIL_TURNS_DEFAULT,
    })
    expect(session()?.detail?.turns_offset).toBe(4)
  })

  it("refetch falls back to a fresh tail when the total collapsed below the window start", async () => {
    seed({ detail: windowedDetail(4) })
    mockGet
      .mockResolvedValueOnce(
        windowedDetail(4, {
          turns: [],
          turns_total: 2,
          prefix_hash: "00000000000000aa",
        })
      )
      .mockResolvedValueOnce(windowedDetail(0))
    actions().refetchDetail(CID)
    await flush()
    expect(mockGet).toHaveBeenNthCalledWith(2, CID, {
      tailTurns: TAIL_TURNS_DEFAULT,
    })
    expect(session()?.detail?.turns_offset).toBe(0)
  })

  it("passes a legacy full response through untouched (old server)", async () => {
    seed({ detail: windowedDetail(2) })
    const legacy = windowedDetail(0, {
      turns_offset: null,
      turns_total: null,
      assistant_turns_before_offset: null,
      prefix_hash: null,
      uncovered_prefix_max_ts: null,
    })
    mockGet.mockResolvedValue(legacy)
    actions().refetchDetail(CID)
    await flush()
    expect(mockGet).toHaveBeenCalledTimes(1)
    expect(session()?.detail?.turns_offset).toBeNull()
    expect(session()?.detail?.turns).toHaveLength(FULL.length)
  })
})

describe("loadOlderTurns", () => {
  it("prepends a verified page and adopts its fingerprint", async () => {
    seed({ detail: windowedDetail(4) })
    mockGetTurns.mockResolvedValue(page(2, 4))
    actions().loadOlderTurns(CID)
    expect(session()?.loadingOlderTurns).toBe(true)
    await flush()
    const d = session()?.detail
    expect(mockGetTurns).toHaveBeenCalledWith(CID, 4, OLDER_TURNS_PAGE_SIZE)
    expect(d?.turns.map((t) => t.id)).toEqual([
      "turn-2",
      "turn-3",
      "turn-4",
      "turn-5",
    ])
    expect(d?.turns_offset).toBe(2)
    expect(d?.prefix_hash).toBe(hashPrefix(2))
    expect(d?.assistant_turns_before_offset).toBe(1)
    expect(session()?.loadingOlderTurns).toBe(false)
  })

  it("produces a NEW detail object so identity-keyed caches invalidate", async () => {
    const before = windowedDetail(4)
    seed({ detail: before })
    mockGetTurns.mockResolvedValue(page(2, 4))
    actions().loadOlderTurns(CID)
    await flush()
    expect(session()?.detail).not.toBe(before)
  })

  it("no-ops at offset 0, on legacy details, and while already loading", () => {
    seed({ detail: windowedDetail(0) })
    actions().loadOlderTurns(CID)
    seed({
      detail: windowedDetail(2, { turns_offset: null, prefix_hash: null }),
    })
    actions().loadOlderTurns(CID)
    seed({ detail: windowedDetail(4), loadingOlderTurns: true })
    actions().loadOlderTurns(CID)
    expect(mockGetTurns).not.toHaveBeenCalled()
  })

  it("rejects a page whose seam proof mismatches and resets via refetch", async () => {
    seed({ detail: windowedDetail(4) })
    mockGetTurns.mockResolvedValue(
      page(2, 4, { prefix_hash_before_index: "00000000000000ff" })
    )
    mockGet.mockResolvedValue(windowedDetail(4))
    actions().loadOlderTurns(CID)
    await flush()
    // No merge happened; the standard refetch path was invoked instead.
    expect(session()?.detail?.turns_offset).toBe(4)
    expect(session()?.loadingOlderTurns).toBe(false)
    expect(mockGet).toHaveBeenCalledWith(CID, { fromIndex: 4 })
  })

  it("drops a page that lost the fetch-generation race to a newer fetch", async () => {
    seed({ detail: windowedDetail(4) })
    let resolvePage: (p: ConversationTurnsPage) => void = () => {}
    mockGetTurns.mockImplementation(
      () =>
        new Promise<ConversationTurnsPage>((r) => {
          resolvePage = r
        })
    )
    actions().loadOlderTurns(CID)
    // A newer refetch supersedes the in-flight page…
    mockGet.mockResolvedValue(windowedDetail(4))
    actions().refetchDetail(CID)
    await flush()
    // …so the page must be discarded when it finally lands.
    resolvePage(page(2, 4))
    await flush()
    expect(session()?.detail?.turns_offset).toBe(4)
    expect(session()?.loadingOlderTurns).toBe(false)
  })

  it("a fork removes only the turns that existed when it was requested", async () => {
    // "Fork from here" re-points the row at a session whose history stops at
    // the chosen turn, so the turns this session streamed on the old session
    // are stale — but ONLY the ones present when the fork was asked for. The
    // fork RPC is a window in which a send can still start (a queued auto-
    // flush, a fast typist, another client), and such a turn runs on the
    // forked session, so a blanket clear would take it off screen. The stale
    // ids ride on the refetch so both land in one dispatch.
    seed({
      detail: windowedDetail(4),
      localTurns: [turn("parent-reply", "assistant", 9)],
    })
    let resolveDetail: (d: DbConversationDetail) => void = () => {}
    mockGet.mockImplementation(
      () =>
        new Promise<DbConversationDetail>((r) => {
          resolveDetail = r
        })
    )

    actions().refetchDetail(CID, {
      preserveLive: true,
      dropLiveTurnIds: ["parent-reply"],
    })
    await flush()

    // A reply the user started right after forking completes while the
    // response is still out.
    useConversationRuntimeStore.setState({
      byConversationId: new Map([
        [
          CID,
          {
            ...session()!,
            localTurns: [
              ...session()!.localTurns,
              turn("forked-reply", "assistant", 11),
            ],
          },
        ],
      ]),
    })

    resolveDetail(windowedDetail(4))
    await flush()
    expect(session()?.detailLoading).toBe(false)
    expect(session()?.detail?.turns_offset).toBe(4)
    expect(session()?.localTurns.map((t) => t.id)).toEqual(["forked-reply"])
  })

  it("a fork keeps an optimistic turn whose prompt will run on the fork", async () => {
    // A send that has not reached the agent yet leaves an optimistic user
    // turn standing. The backend refuses a fork while a turn is in flight, so
    // a fork that SUCCEEDS proves that prompt never ran on the old session —
    // it will run on the forked one. The panel therefore snapshots completed
    // turns only, and the reducer removes strictly what it was given.
    seed({
      detail: windowedDetail(4),
      localTurns: [turn("parent-reply", "assistant", 9)],
      optimisticTurns: [turn("pending-prompt", "user", 10)],
    })
    mockGet.mockResolvedValue(windowedDetail(4))
    actions().refetchDetail(CID, {
      preserveLive: true,
      dropLiveTurnIds: ["parent-reply"],
    })
    await flush()
    expect(session()?.localTurns).toEqual([])
    expect(session()?.optimisticTurns.map((t) => t.id)).toEqual([
      "pending-prompt",
    ])
  })

  it("a failed fork refetch removes nothing", async () => {
    // The removal is a property of the SUCCESS dispatch, so an error leaves
    // the timeline exactly as it was rather than stranding the row with
    // neither the pre-fork turns nor a new history.
    seed({
      detail: windowedDetail(4),
      localTurns: [turn("parent-reply", "assistant", 9)],
    })
    mockGet.mockRejectedValue(new Error("nope"))
    actions().refetchDetail(CID, {
      preserveLive: true,
      dropLiveTurnIds: ["parent-reply"],
    })
    await flush()
    expect(session()?.detailError).toBe("nope")
    expect(session()?.localTurns.map((t) => t.id)).toEqual(["parent-reply"])
  })

  it("dedupes turns already present at the seam", async () => {
    seed({ detail: windowedDetail(4) })
    // Malformed page that also carries the first loaded turn (turn-4).
    mockGetTurns.mockResolvedValue(page(2, 4, { turns: FULL.slice(2, 5) }))
    actions().loadOlderTurns(CID)
    await flush()
    const ids = session()?.detail?.turns.map((t) => t.id)
    expect(ids).toEqual(["turn-2", "turn-3", "turn-4", "turn-5"])
  })
})

describe("watermark overlay retirement with window coverage", () => {
  const overlayIn = { turn: turn("bg-in", "assistant", 5), watermark: 100 }
  const overlayOut = { turn: turn("bg-out", "assistant", 1), watermark: 90 }
  const overlayAhead = {
    turn: turn("bg-ahead", "assistant", 9),
    watermark: 900,
  }

  it("full coverage retires all caught-up overlays (legacy rule)", async () => {
    seed({ backgroundTurns: [overlayIn, overlayOut, overlayAhead] })
    mockGet.mockResolvedValue(windowedDetail(0, { transcript_watermark: 500 }))
    actions().refetchDetail(CID)
    await flush()
    expect(session()?.backgroundTurns.map((e) => e.turn.id)).toEqual([
      "bg-ahead",
    ])
  })

  it("windowed coverage keeps a caught-up overlay whose twin is before the window", async () => {
    seed({
      detail: windowedDetail(4),
      backgroundTurns: [overlayIn, overlayOut, overlayAhead],
    })
    mockGet.mockResolvedValue(windowedDetail(4, { transcript_watermark: 500 }))
    actions().refetchDetail(CID)
    await flush()
    // bg-in (ts 5 > uncovered max ts 3) retired; bg-out (ts 1 ≤ 3) kept —
    // its content is NOT in the loaded window, retiring it would vanish the
    // message; bg-ahead beyond the watermark kept.
    expect(session()?.backgroundTurns.map((e) => e.turn.id)).toEqual([
      "bg-out",
      "bg-ahead",
    ])
  })

  it("an overlay with a timestamp EQUAL to the boundary is kept (strictly greater)", async () => {
    const boundaryTwin = {
      turn: turn("bg-boundary", "assistant", 3),
      watermark: 80,
    }
    seed({ detail: windowedDetail(4), backgroundTurns: [boundaryTwin] })
    mockGet.mockResolvedValue(windowedDetail(4, { transcript_watermark: 500 }))
    actions().refetchDetail(CID)
    await flush()
    expect(session()?.backgroundTurns.map((e) => e.turn.id)).toEqual([
      "bg-boundary",
    ])
  })

  it("a prepended page widens coverage and retires newly-covered overlays", async () => {
    seed({
      detail: windowedDetail(4, { transcript_watermark: 500 }),
      backgroundTurns: [overlayOut],
    })
    mockGetTurns.mockResolvedValue(page(0, 4))
    actions().loadOlderTurns(CID)
    await flush()
    // Page start is 0 → full coverage → bg-out's twin is now loaded.
    expect(session()?.backgroundTurns).toEqual([])
  })
})

describe("syncTurnMetadata windowed gate", () => {
  const local = [turn("local-a", "assistant", 6)]

  it("patches from a boundary-anchored window when the fingerprint verifies", async () => {
    vi.useFakeTimers()
    seed({
      localTurns: local,
      historyAssistantBaseline: 3,
      batchBoundaryIndex: 6,
      batchBoundaryPrefixHash: hashPrefix(6),
    })
    const parsedNew: MessageTurn = {
      ...turn("turn-6", "assistant", 6),
      usage: {
        input_tokens: 10,
        output_tokens: 5,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
      },
    }
    mockGet.mockResolvedValue(
      windowedDetail(6, {
        turns: [parsedNew],
        turns_total: 7,
        assistant_turns_before_offset: 3,
        prefix_hash: hashPrefix(6),
        uncovered_prefix_max_ts: FULL[5].timestamp,
      })
    )
    actions().syncTurnMetadata(CID)
    await vi.advanceTimersByTimeAsync(1600)
    expect(mockGet).toHaveBeenCalledWith(CID, { fromIndex: 6 })
    expect(session()?.localTurns[0]?.usage?.input_tokens).toBe(10)
  })

  it("skips turn patches entirely when the boundary fingerprint mismatches", async () => {
    vi.useFakeTimers()
    seed({
      localTurns: local,
      historyAssistantBaseline: 3,
      batchBoundaryIndex: 6,
      batchBoundaryPrefixHash: hashPrefix(6),
    })
    const parsedNew: MessageTurn = {
      ...turn("turn-6", "assistant", 6),
      usage: {
        input_tokens: 10,
        output_tokens: 5,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
      },
    }
    // Compaction between capture and sync: same offset, different prefix.
    mockGet.mockResolvedValue(
      windowedDetail(6, {
        turns: [parsedNew],
        turns_total: 7,
        prefix_hash: "00000000000000ff",
      })
    )
    actions().syncTurnMetadata(CID)
    await vi.advanceTimersByTimeAsync(1600)
    // Retry fires once (metadata still missing), then gives up — usage must
    // never be pinned from an unverified window.
    await vi.advanceTimersByTimeAsync(3100)
    expect(session()?.localTurns[0]?.usage).toBeUndefined()
  })

  it("legacy full response uses the global baseline math", async () => {
    vi.useFakeTimers()
    seed({
      localTurns: local,
      historyAssistantBaseline: 3,
      batchBoundaryIndex: null,
      batchBoundaryPrefixHash: null,
    })
    const parsedNew: MessageTurn = {
      ...turn("turn-6", "assistant", 6),
      usage: {
        input_tokens: 42,
        output_tokens: 5,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
      },
    }
    mockGet.mockResolvedValue(
      windowedDetail(0, {
        turns: [...FULL, parsedNew],
        turns_offset: null,
        turns_total: null,
        assistant_turns_before_offset: null,
        prefix_hash: null,
        uncovered_prefix_max_ts: null,
      })
    )
    actions().syncTurnMetadata(CID)
    await vi.advanceTimersByTimeAsync(1600)
    expect(mockGet).toHaveBeenCalledWith(CID, undefined)
    expect(session()?.localTurns[0]?.usage?.input_tokens).toBe(42)
  })
})

describe("batch boundary capture", () => {
  it("captures global baseline + boundary + extended fingerprint on an owner send", () => {
    seed({ detail: windowedDetail(2) })
    actions().appendOptimisticTurn(CID, turn("opt-1", "user", 10), "tok-1")
    const s = session()
    // Window [2..6): 2 assistants in window + 1 before = 3 global.
    expect(s?.historyAssistantBaseline).toBe(3)
    expect(s?.batchBoundaryIndex).toBe(6)
    expect(s?.batchBoundaryPrefixHash).toBe(hashPrefix(6))
  })

  it("captures the seed fingerprint when no detail exists yet (fresh chat)", () => {
    seed({})
    actions().appendOptimisticTurn(CID, turn("opt-1", "user", 10), "tok-1")
    const s = session()
    expect(s?.historyAssistantBaseline).toBe(0)
    expect(s?.batchBoundaryIndex).toBe(0)
    expect(s?.batchBoundaryPrefixHash).toBe(FNV_SEED_HEX)
  })

  it("captures a null fingerprint under a legacy detail (gate disabled)", () => {
    seed({
      detail: windowedDetail(0, {
        turns_offset: null,
        turns_total: null,
        assistant_turns_before_offset: null,
        prefix_hash: null,
      }),
    })
    actions().appendOptimisticTurn(CID, turn("opt-1", "user", 10), "tok-1")
    const s = session()
    expect(s?.historyAssistantBaseline).toBe(3)
    expect(s?.batchBoundaryIndex).toBe(FULL.length)
    expect(s?.batchBoundaryPrefixHash).toBeNull()
  })
})

describe("olderTurnsPrependEpoch (explicit shift signal)", () => {
  it("bumps exactly on a successful prepend — including the final page", async () => {
    seed({ detail: windowedDetail(4) })
    mockGetTurns.mockResolvedValueOnce(page(2, 4))
    actions().loadOlderTurns(CID)
    await flush()
    expect(session()?.olderTurnsPrependEpoch).toBe(1)

    // Final page reaches offset 0: epoch still bumps on the same commit that
    // turns the loader row off (hasOlder false) — the viewport must not jump
    // even though the loader row unmounts.
    mockGetTurns.mockResolvedValueOnce(page(0, 2))
    actions().loadOlderTurns(CID)
    await flush()
    expect(session()?.detail?.turns_offset).toBe(0)
    expect(session()?.olderTurnsPrependEpoch).toBe(2)
  })

  it("does not bump on refetch, rejected seam, or generation-raced pages", async () => {
    seed({ detail: windowedDetail(4) })
    // Plain window refresh: no prepend.
    mockGet.mockResolvedValue(windowedDetail(4))
    actions().refetchDetail(CID)
    await flush()
    expect(session()?.olderTurnsPrependEpoch).toBe(0)

    // Seam-rejected page: no prepend.
    mockGetTurns.mockResolvedValueOnce(
      page(2, 4, { prefix_hash_before_index: "00000000000000ff" })
    )
    actions().loadOlderTurns(CID)
    await flush()
    expect(session()?.olderTurnsPrependEpoch).toBe(0)

    // Generation-raced page: superseded by a newer refetch → dropped.
    let resolvePage: (p: ConversationTurnsPage) => void = () => {}
    mockGetTurns.mockImplementationOnce(
      () =>
        new Promise<ConversationTurnsPage>((r) => {
          resolvePage = r
        })
    )
    actions().loadOlderTurns(CID)
    actions().refetchDetail(CID)
    await flush()
    resolvePage(page(2, 4))
    await flush()
    expect(session()?.olderTurnsPrependEpoch).toBe(0)
  })

  it("does not resurrect a ghost session when the tab closed mid-page", async () => {
    seed({ detail: windowedDetail(4) })
    let resolvePage: (p: ConversationTurnsPage) => void = () => {}
    mockGetTurns.mockImplementationOnce(
      () =>
        new Promise<ConversationTurnsPage>((r) => {
          resolvePage = r
        })
    )
    actions().loadOlderTurns(CID)
    // Tab closes: the session is removed while the page is in flight.
    useConversationRuntimeStore.setState({ byConversationId: new Map() })
    resolvePage(page(2, 4))
    await flush()
    expect(session()).toBeUndefined()
  })
})
