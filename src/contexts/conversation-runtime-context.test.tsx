/**
 * Regression coverage for the per-conversation fetch-generation guard
 * that protects `FETCH_DETAIL_SUCCESS` / `FETCH_DETAIL_ERROR` from
 * out-of-order resolution and from resurrecting a removed session.
 *
 * The bug fixed by the generation counter:
 *
 *   1. Open sheet for child 99 → `refetchDetail(99)` issues fetch A.
 *   2. User closes the sheet → `removeConversation(99)` deletes state.
 *   3. Fetch A resolves AFTER the unmount → `FETCH_DETAIL_SUCCESS`
 *      reducer recreates the session with stale detail.
 *   4. User reopens → `useConversationDetail`'s active-data guard
 *      skips the auto-fetch because `session.detail` is set.
 *   5. The user is shown a stale pre-completion transcript.
 *
 * The counter also prevents a stale-response-wins race:
 *
 *   1. Open A → fetch A (slow).
 *   2. Close A.
 *   3. Open B → fetch B (faster).
 *   4. Fetch B resolves first — fresh detail in state.
 *   5. Fetch A resolves second — would overwrite B's fresh detail
 *      with stale, but the generation guard ignores it.
 */

import { act, render, screen } from "@testing-library/react"
import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi,
  type MockInstance,
} from "vitest"
import { useEffect, type ReactNode } from "react"

import {
  ConversationRuntimeProvider,
  useConversationRuntime,
} from "@/contexts/conversation-runtime-context"
import type { DbConversationDetail, MessageTurn } from "@/lib/types"

vi.mock("@/lib/api", () => ({
  getFolderConversation: vi.fn(),
}))

const { getFolderConversation } = await import("@/lib/api")
const mockGetFolderConversation = vi.mocked(getFolderConversation)

function detailWithTitle(title: string): DbConversationDetail {
  return {
    summary: {
      id: 99,
      folder_id: 1,
      agent_type: "codex",
      title,
      status: "in_progress",
      model: null,
      git_branch: null,
      external_id: "ext-1",
      message_count: 0,
      created_at: "2026-05-28T00:00:00.000Z",
      updated_at: "2026-05-28T00:00:00.000Z",
    },
    turns: [],
    session_stats: null,
  }
}

/** Probe component that exposes runtime actions to the test and lets it
 *  read back the session state via DOM attributes. */
function Probe() {
  const { refetchDetail, removeConversation, getSession } =
    useConversationRuntime()
  const session = getSession(99)
  return (
    <div>
      <button
        data-testid="refetch"
        type="button"
        onClick={() => refetchDetail(99)}
      >
        refetch
      </button>
      <button
        data-testid="remove"
        type="button"
        onClick={() => removeConversation(99)}
      >
        remove
      </button>
      <div data-testid="title">
        {session?.detail?.summary.title ?? "no-detail"}
      </div>
      <div data-testid="has-session">{session ? "yes" : "no"}</div>
      <div data-testid="loading">{session?.detailLoading ? "yes" : "no"}</div>
    </div>
  )
}

function renderProvider(children: ReactNode = <Probe />) {
  return render(
    <ConversationRuntimeProvider>{children}</ConversationRuntimeProvider>
  )
}

describe("ConversationRuntimeProvider fetch-generation guard", () => {
  let originalConsoleError: typeof console.error
  let consoleErrorSpy: MockInstance

  beforeEach(() => {
    mockGetFolderConversation.mockReset()
    originalConsoleError = console.error
    // Filter React's act() warnings produced when promise resolutions
    // commit asynchronously; the tests use act() correctly but the
    // microtask boundary is finer-grained than RTL's wrapper.
    consoleErrorSpy = vi.spyOn(console, "error").mockImplementation(() => {})
  })

  afterEach(() => {
    console.error = originalConsoleError
    consoleErrorSpy.mockRestore()
  })

  it("ignores a fetch response that resolves after removeConversation — no zombie session is created", async () => {
    let resolveA!: (detail: DbConversationDetail) => void
    mockGetFolderConversation.mockImplementationOnce(
      () =>
        new Promise<DbConversationDetail>((resolve) => {
          resolveA = resolve
        })
    )

    renderProvider()
    await act(async () => {
      screen.getByTestId("refetch").click()
    })
    expect(screen.getByTestId("loading").textContent).toBe("yes")

    // Tear down the session BEFORE fetch A resolves — simulates the user
    // closing the sheet while the detail is still loading.
    await act(async () => {
      screen.getByTestId("remove").click()
    })
    expect(screen.getByTestId("has-session").textContent).toBe("no")

    // Fetch A resolves with stale detail AFTER removal. The
    // generation-counter guard must drop this resolution silently — no
    // FETCH_DETAIL_SUCCESS dispatched, so the session stays gone.
    await act(async () => {
      resolveA(detailWithTitle("stale-A"))
      await Promise.resolve()
    })
    expect(screen.getByTestId("has-session").textContent).toBe("no")
    expect(screen.getByTestId("title").textContent).toBe("no-detail")
  })

  it("drops a stale fetch resolution that arrives after a fresh refetchDetail (fresh-wins regardless of order)", async () => {
    let resolveA!: (detail: DbConversationDetail) => void
    let resolveB!: (detail: DbConversationDetail) => void
    mockGetFolderConversation
      .mockImplementationOnce(
        () =>
          new Promise<DbConversationDetail>((resolve) => {
            resolveA = resolve
          })
      )
      .mockImplementationOnce(
        () =>
          new Promise<DbConversationDetail>((resolve) => {
            resolveB = resolve
          })
      )

    renderProvider()
    // First open — fetch A in flight.
    await act(async () => {
      screen.getByTestId("refetch").click()
    })
    // Close, then second open — fetch B in flight. Each refetchDetail
    // bumps the generation counter, so A's eventual resolution should
    // be ignored.
    await act(async () => {
      screen.getByTestId("remove").click()
    })
    await act(async () => {
      screen.getByTestId("refetch").click()
    })

    // Resolve B FIRST — fresh detail lands.
    await act(async () => {
      resolveB(detailWithTitle("fresh-B"))
      await Promise.resolve()
    })
    expect(screen.getByTestId("title").textContent).toBe("fresh-B")

    // Then resolve A — stale. Without the generation guard this would
    // overwrite fresh-B; with it, fresh-B stays put.
    await act(async () => {
      resolveA(detailWithTitle("stale-A"))
      await Promise.resolve()
    })
    expect(screen.getByTestId("title").textContent).toBe("fresh-B")
  })

  it("a fresh fetch resolution after a stale one still wins (forward direction unchanged)", async () => {
    let resolveA!: (detail: DbConversationDetail) => void
    let resolveB!: (detail: DbConversationDetail) => void
    mockGetFolderConversation
      .mockImplementationOnce(
        () =>
          new Promise<DbConversationDetail>((resolve) => {
            resolveA = resolve
          })
      )
      .mockImplementationOnce(
        () =>
          new Promise<DbConversationDetail>((resolve) => {
            resolveB = resolve
          })
      )

    renderProvider()
    await act(async () => {
      screen.getByTestId("refetch").click()
    })
    await act(async () => {
      screen.getByTestId("remove").click()
    })
    await act(async () => {
      screen.getByTestId("refetch").click()
    })

    // Resolve A first (stale, already invalidated by remove + new refetch).
    await act(async () => {
      resolveA(detailWithTitle("stale-A"))
      await Promise.resolve()
    })
    // A's resolution was ignored — title stays empty until B lands.
    expect(screen.getByTestId("title").textContent).toBe("no-detail")

    // Resolve B — fresh detail wins as the latest generation.
    await act(async () => {
      resolveB(detailWithTitle("fresh-B"))
      await Promise.resolve()
    })
    expect(screen.getByTestId("title").textContent).toBe("fresh-B")
  })
})

/**
 * `getTimelineTurns` memoizes per conversation by session reference, so a
 * dispatch that updates conversation A leaves conversation B's timeline array
 * referentially identical. This is what lets MessageListView's `threadItems`
 * useMemo short-circuit for every tab except the one whose session actually
 * changed — neutralizing the cross-tab broadcast fan-out without unmounting
 * any session (tile mode keeps every active conversation mounted).
 */
describe("ConversationRuntimeProvider getTimelineTurns memoization", () => {
  const runtimeHolder: {
    current: ReturnType<typeof useConversationRuntime> | undefined
  } = { current: undefined }

  function RuntimeCapture() {
    const runtime = useConversationRuntime()
    useEffect(() => {
      runtimeHolder.current = runtime
    })
    return null
  }

  function userTurn(id: string): MessageTurn {
    return {
      id,
      role: "user",
      blocks: [{ type: "text", text: id }],
      timestamp: "2026-05-28T00:00:00.000Z",
    }
  }

  beforeEach(() => {
    runtimeHolder.current = undefined
  })

  it("returns a stable reference for a conversation untouched by an unrelated update, and a fresh reference for the one that changed", () => {
    renderProvider(<RuntimeCapture />)
    const api = () => runtimeHolder.current!

    // Seed two independent conversations.
    act(() => {
      api().appendOptimisticTurn(1, userTurn("a1"), "a1")
    })
    act(() => {
      api().appendOptimisticTurn(2, userTurn("b1"), "b1")
    })

    // Prime the cache for both.
    const timeline1Before = api().getTimelineTurns(1)
    const timeline2Before = api().getTimelineTurns(2)
    expect(timeline1Before).toHaveLength(1)
    expect(timeline2Before).toHaveLength(1)

    // Update only conversation 1.
    act(() => {
      api().appendOptimisticTurn(1, userTurn("a2"), "a2")
    })

    const timeline1After = api().getTimelineTurns(1)
    const timeline2After = api().getTimelineTurns(2)

    // Conversation 2 was untouched → identical array reference (cache hit).
    expect(timeline2After).toBe(timeline2Before)
    // Conversation 1 changed → new reference and new content.
    expect(timeline1After).not.toBe(timeline1Before)
    expect(timeline1After).toHaveLength(2)
  })

  it("returns a stable empty-array reference for an unknown conversation", () => {
    renderProvider(<RuntimeCapture />)
    const first = runtimeHolder.current!.getTimelineTurns(12345)
    const second = runtimeHolder.current!.getTimelineTurns(67890)
    expect(first).toHaveLength(0)
    expect(second).toBe(first)
  })
})
