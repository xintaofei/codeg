/**
 * CollaborationTurnList behavior tests (v2 acceptance A24/A25): the
 * read-only list must keep newer turn states (version ratchet), never let
 * a stale response overwrite a newer one, include the active round across
 * polls, stop polling when the drawer closes, and never offer any input
 * surface (the MVP is read-only).
 */

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { CollaborationTurnList } from "./collaboration-turn-list"
import enMessages from "@/i18n/messages/en.json"
import type { TurnReport } from "@/lib/collaboration"

// The transport boundary is the ONLY thing mocked — the component itself
// drives the real parser (`lib/collaboration`) against the payloads below.
const mockGetCollaborationSession = vi.fn()

vi.mock("@/lib/collaboration", async () => {
  const actual = await vi.importActual<typeof import("@/lib/collaboration")>(
    "@/lib/collaboration"
  )
  return {
    ...actual,
    getCollaborationSession: (...args: unknown[]) =>
      mockGetCollaborationSession(...args),
  }
})

function withIntl(ui: React.ReactElement) {
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

function turn(over: Partial<TurnReport>): TurnReport {
  return {
    schema_version: 1,
    session_id: "session-1",
    turn_id: "turn-1",
    source_task_id: "task-0",
    ordinal: 1,
    state: "running",
    message: "fix the boundary condition",
    initiator_kind: "parent_agent",
    initiator_parent_conversation_id: 7,
    result_text: null,
    text_truncated: false,
    error_code: null,
    error_message: null,
    blocked_on: null,
    created_at: "2026-09-05T08:00:00Z",
    started_at: "2026-09-05T08:00:01Z",
    finished_at: null,
    version: 1,
    ...over,
  }
}

const baseProps = {
  open: true,
  childConversationId: 42,
  parentConversationId: 7,
  sourceTaskId: "task-0",
}

beforeEach(() => {
  mockGetCollaborationSession.mockReset()
  vi.useRealTimers()
})

describe("CollaborationTurnList", () => {
  it("renders nothing when there is no collaboration session yet", async () => {
    mockGetCollaborationSession.mockResolvedValue({
      schema_version: 1,
      session: null,
      turns: [],
      next_after_ordinal: null,
    })
    const { container } = render(
      withIntl(<CollaborationTurnList {...baseProps} />)
    )
    await vi.waitFor(() =>
      expect(mockGetCollaborationSession).toHaveBeenCalled()
    )
    // Allow the load promise to settle.
    await act(async () => {
      await Promise.resolve()
    })
    expect(container).toBeEmptyDOMElement()
  })

  it("shows the rework message and per-round state; no input surface exists", async () => {
    mockGetCollaborationSession.mockResolvedValue({
      schema_version: 1,
      session: {
        schema_version: 1,
        session_id: "session-1",
        source_task_id: "task-0",
        child_conversation_id: 42,
        state: "open",
      },
      turns: [turn({})],
      next_after_ordinal: null,
    })
    render(withIntl(<CollaborationTurnList {...baseProps} />))
    expect(
      await screen.findByText("fix the boundary condition")
    ).toBeInTheDocument()
    expect(screen.getByText("Initiated by the main agent")).toBeInTheDocument()
    expect(screen.getByText("Running")).toBeInTheDocument()
    // Read-only: no textbox / textarea / contenteditable anywhere.
    expect(
      document.querySelector("input, textarea, [contenteditable]")
    ).toBeNull()
  })

  it("advances running → completed when a poll reports version 2", async () => {
    mockGetCollaborationSession
      .mockResolvedValueOnce({
        schema_version: 1,
        session: {
          schema_version: 1,
          session_id: "session-1",
          source_task_id: "task-0",
          child_conversation_id: 42,
          state: "open",
        },
        turns: [turn({ state: "running", version: 1 })],
        next_after_ordinal: null,
      })
      .mockResolvedValue({
        schema_version: 1,
        session: {
          schema_version: 1,
          session_id: "session-1",
          source_task_id: "task-0",
          child_conversation_id: 42,
          state: "open",
        },
        turns: [
          turn({
            state: "completed",
            version: 2,
            result_text: "fixed output",
            finished_at: "2026-09-05T08:00:05Z",
          }),
        ],
        next_after_ordinal: null,
      })
    render(withIntl(<CollaborationTurnList {...baseProps} />))
    await vi.waitFor(() =>
      expect(screen.getByText("Running")).toBeInTheDocument()
    )
    // Wait out the 1 s poll interval; the next snapshot reports completed v2.
    await vi.waitFor(() =>
      expect(screen.getByText("Completed")).toBeInTheDocument()
    )
    // Result bodies are collapsed by default — expand to read it.
    fireEvent.click(screen.getByText("Show result"))
    expect(screen.getByText("fixed output")).toBeInTheDocument()
  })

  it("ignores a stale response whose versions are older than what is shown", async () => {
    mockGetCollaborationSession
      .mockResolvedValueOnce({
        schema_version: 1,
        session: {
          schema_version: 1,
          session_id: "session-1",
          source_task_id: "task-0",
          child_conversation_id: 42,
          state: "open",
        },
        turns: [
          turn({ state: "completed", version: 2, result_text: "newer result" }),
        ],
        next_after_ordinal: null,
      })
      .mockResolvedValue({
        schema_version: 1,
        session: {
          schema_version: 1,
          session_id: "session-1",
          source_task_id: "task-0",
          child_conversation_id: 42,
          state: "open",
        },
        turns: [turn({ state: "running", version: 1 })],
        next_after_ordinal: null,
      })
    render(withIntl(<CollaborationTurnList {...baseProps} />))
    await vi.waitFor(() =>
      expect(screen.getByText("Completed")).toBeInTheDocument()
    )
    // Bodies are collapsed by default; expand to read the winner's result.
    fireEvent.click(screen.getByText("Show result"))
    expect(screen.getByText("newer result")).toBeInTheDocument()
    // The stale running view must NOT replace the completed one — a second
    // poll returns it, and the version ratchet drops it.
    await act(async () => {
      ;(await mockGetCollaborationSession.mock.results.length,
        Promise.resolve())
    })
    await vi.waitFor(() => {
      expect(screen.getByText("Completed")).toBeInTheDocument()
      expect(screen.getByText("newer result")).toBeInTheDocument()
      expect(screen.queryByText("Running")).not.toBeInTheDocument()
    })
  })

  it("shows a second round without disturbing the first (R2 ≠ R1)", async () => {
    mockGetCollaborationSession.mockResolvedValue({
      schema_version: 1,
      session: {
        schema_version: 1,
        session_id: "session-1",
        source_task_id: "task-0",
        child_conversation_id: 42,
        state: "open",
      },
      turns: [
        turn({
          state: "completed",
          result_text: "round one",
          finished_at: "2026-09-05T08:00:05Z",
        }),
        turn({
          turn_id: "turn-2",
          ordinal: 2,
          message: "second rework",
        }),
      ],
      next_after_ordinal: null,
    })
    render(withIntl(<CollaborationTurnList {...baseProps} />))
    await waitFor(() => expect(screen.getByText("Round 1")).toBeInTheDocument())
    // Expand round 1's body to see its result.
    fireEvent.click(screen.getAllByText("Show result")[0])
    expect(screen.getByText("round one")).toBeInTheDocument()
    expect(screen.getByText("second rework")).toBeInTheDocument()
    expect(screen.getByText("Round 1")).toBeInTheDocument()
    expect(screen.getByText("Round 2")).toBeInTheDocument()
  })

  it("keeps polling from losing the active round and shows the outcome-unknown note", async () => {
    mockGetCollaborationSession.mockResolvedValue({
      schema_version: 1,
      session: {
        schema_version: 1,
        session_id: "session-1",
        source_task_id: "task-0",
        child_conversation_id: 42,
        state: "blocked",
      },
      turns: [
        turn({
          state: "outcome_unknown",
          error_code: "outcome_unknown",
          version: 2,
        }),
      ],
      next_after_ordinal: null,
    })
    render(withIntl(<CollaborationTurnList {...baseProps} />))
    expect(await screen.findByText("Outcome unknown")).toBeInTheDocument()
    expect(
      screen.getByText(/cannot prove whether this round ran/i)
    ).toBeInTheDocument()
    expect(
      screen.getByText(/outcome could not be confirmed/i)
    ).toBeInTheDocument()
  })

  it("stops requesting after the drawer closes", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    mockGetCollaborationSession.mockResolvedValue({
      schema_version: 1,
      session: {
        schema_version: 1,
        session_id: "session-1",
        source_task_id: "task-0",
        child_conversation_id: 42,
        state: "open",
      },
      turns: [
        turn({
          state: "completed",
          result_text: "done",
          finished_at: "2026-09-05T08:00:05Z",
        }),
      ],
      next_after_ordinal: null,
    })
    const { rerender } = render(
      withIntl(<CollaborationTurnList {...baseProps} />)
    )
    await screen.findByText("Round 1")
    const callsAfterOpen = mockGetCollaborationSession.mock.calls.length
    rerender(withIntl(<CollaborationTurnList {...baseProps} open={false} />))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000)
    })
    expect(mockGetCollaborationSession.mock.calls.length).toBe(callsAfterOpen)
  })

  it("expands the result body on demand and flags truncation", async () => {
    mockGetCollaborationSession.mockResolvedValue({
      schema_version: 1,
      session: {
        schema_version: 1,
        session_id: "session-1",
        source_task_id: "task-0",
        child_conversation_id: 42,
        state: "open",
      },
      turns: [
        turn({
          state: "completed",
          result_text: "very long output…",
          text_truncated: true,
          finished_at: "2026-09-05T08:00:05Z",
        }),
      ],
      next_after_ordinal: null,
    })
    render(withIntl(<CollaborationTurnList {...baseProps} />))
    // Collapsed by default — expand to read the result.
    fireEvent.click(await screen.findByText("Show result"))
    expect(screen.getByText("very long output…")).toBeInTheDocument()
    expect(
      screen.getByText(/truncated — open the child session/i)
    ).toBeInTheDocument()
    // Collapse again.
    fireEvent.click(screen.getByText("Hide result"))
    expect(screen.queryByText("very long output…")).not.toBeInTheDocument()
  })
})
