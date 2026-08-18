import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { useEffect, useState } from "react"
import { describe, expect, it, vi } from "vitest"

import { SessionFailureBanner } from "./session-failure-banner"
import enMessages from "@/i18n/messages/en.json"
import {
  dismissSessionFailures,
  upsertSessionFailure,
} from "@/lib/session-failures"
import type { SessionFailureRecord } from "@/lib/types"

function record(
  overrides: Partial<SessionFailureRecord> = {}
): SessionFailureRecord {
  return {
    id: "t1:error",
    revision: 1,
    category: "access",
    severity: "error",
    title: "Authentication required.",
    actions: ["login"],
    resolved: false,
    ...overrides,
  }
}

function renderBanner(
  failures: SessionFailureRecord[],
  onAction?: (action: string, failure: SessionFailureRecord) => void,
  onDismiss?: (ids: string[]) => void
) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <SessionFailureBanner
        failures={failures}
        onAction={onAction}
        onDismiss={onDismiss}
      />
    </NextIntlClientProvider>
  )
}

describe("SessionFailureBanner", () => {
  it("renders an active error strip with its suggested action wired", () => {
    const onAction = vi.fn()
    renderBanner([record()], onAction)
    expect(screen.getByRole("alert")).toBeInTheDocument()
    expect(screen.getByText("Authentication required.")).toBeInTheDocument()
    // Only the record's suggested (and known) actions render.
    expect(screen.queryByText("Retry")).not.toBeInTheDocument()
    fireEvent.click(screen.getByText("Sign in"))
    expect(onAction).toHaveBeenCalledWith(
      "login",
      expect.objectContaining({ id: "t1:error" })
    )
  })

  it("hides action buttons without a handler (read-only surfaces)", () => {
    renderBanner([record()])
    expect(screen.getByText("Authentication required.")).toBeInTheDocument()
    expect(screen.queryByText("Sign in")).not.toBeInTheDocument()
  })

  it("never renders action buttons on warning strips (adapter is mid-recovery)", () => {
    const onAction = vi.fn()
    renderBanner(
      [
        record({
          severity: "warning",
          title: "retrying request",
          actions: ["retry", "login", "new_session"],
        }),
      ],
      onAction
    )
    expect(screen.getByText("retrying request")).toBeInTheDocument()
    expect(screen.queryByText("Retry")).not.toBeInTheDocument()
    expect(screen.queryByText("Sign in")).not.toBeInTheDocument()
    expect(screen.queryByText("New session")).not.toBeInTheDocument()
  })

  it("falls back to the category label for a blank title and expands details", () => {
    renderBanner([
      record({
        category: "limit",
        title: "  ",
        details: "usage resets at 3pm",
        actions: [],
      }),
    ])
    expect(screen.getByText("Limit reached")).toBeInTheDocument()
    expect(screen.queryByText("usage resets at 3pm")).not.toBeInTheDocument()
    fireEvent.click(screen.getByLabelText("Toggle details"))
    expect(screen.getByText("usage resets at 3pm")).toBeInTheDocument()
  })

  it("maps unknown categories and actions onto safe fallbacks", () => {
    const onAction = vi.fn()
    renderBanner(
      [record({ category: "quantum", title: "", actions: ["sing", "retry"] })],
      onAction
    )
    // Unknown category → the generic label; unknown action → not rendered.
    expect(screen.getByText("Session issue")).toBeInTheDocument()
    expect(screen.queryByText("sing")).not.toBeInTheDocument()
    fireEvent.click(screen.getByText("Retry"))
    expect(onAction).toHaveBeenCalledWith("retry", expect.anything())
  })

  it("shows only the most recent recovered warning, and only when idle", () => {
    const resolvedWarning = (id: string, title: string) =>
      record({ id, severity: "warning", title, resolved: true, actions: [] })
    renderBanner([
      resolvedWarning("w1", "first retry incident"),
      resolvedWarning("w2", "second retry incident"),
      // Resolved ERRORS are watermarks only — never rendered.
      record({ id: "e1", resolved: true, title: "old auth error" }),
    ])
    expect(
      screen.getByText(/Recovered · second retry incident/)
    ).toBeInTheDocument()
    expect(screen.queryByText(/first retry incident/)).not.toBeInTheDocument()
    expect(screen.queryByText(/old auth error/)).not.toBeInTheDocument()
  })

  it("suppresses the recovered line while an active strip is showing", () => {
    renderBanner([
      record({ id: "w1", severity: "warning", resolved: true, actions: [] }),
      record({ id: "e2", title: "still failing" }),
    ])
    expect(screen.getByText("still failing")).toBeInTheDocument()
    expect(screen.queryByText(/Recovered/)).not.toBeInTheDocument()
  })

  it("renders nothing for an empty or fully-settled-error table", () => {
    const { container } = renderBanner([record({ resolved: true })])
    expect(container).toBeEmptyDOMElement()
  })

  it("collapses stacked warnings to the latest one plus a count (#496)", () => {
    const incident = (id: string, title: string) =>
      record({ id, severity: "warning", category: "connection", title })
    renderBanner([
      incident("i1", "Reconnecting... 1/5"),
      incident("i2", "Reconnecting... 1/5"),
      incident("i3", "Reconnecting... 1/5"),
    ])
    // One strip, not three — the older incidents become a count.
    expect(screen.getAllByRole("alert")).toHaveLength(1)
    expect(screen.getByText("+2 more")).toBeInTheDocument()
  })

  it("renders every active error, with no count", () => {
    renderBanner([
      record({ id: "e1", title: "first failure" }),
      record({ id: "e2", title: "second failure" }),
    ])
    expect(screen.getAllByRole("alert")).toHaveLength(2)
    expect(screen.queryByText(/more$/)).not.toBeInTheDocument()
  })

  it("wires a close button on warnings and errors, opt-in via onDismiss", () => {
    const onDismiss = vi.fn()
    renderBanner([record({ id: "e1" })], undefined, onDismiss)
    fireEvent.click(screen.getByLabelText("Dismiss"))
    expect(onDismiss).toHaveBeenCalledWith(["e1"])

    onDismiss.mockClear()
    renderBanner(
      [record({ id: "w1", severity: "warning", title: "retrying" })],
      undefined,
      onDismiss
    )
    // Warnings get no recovery actions but must still be closable.
    fireEvent.click(screen.getAllByLabelText("Dismiss")[1])
    expect(onDismiss).toHaveBeenCalledWith(["w1"])
  })

  it("closes every warning the collapsed strip stands for, not just the visible one", () => {
    const onDismiss = vi.fn()
    const incident = (id: string) =>
      record({ id, severity: "warning", category: "connection", title: id })
    renderBanner(
      [incident("i1"), incident("i2"), incident("i3")],
      undefined,
      onDismiss
    )
    fireEvent.click(screen.getByLabelText("Dismiss"))
    expect(onDismiss).toHaveBeenCalledWith(["i1", "i2", "i3"])
  })

  it("omits the close button when no handler is wired", () => {
    renderBanner([record()])
    expect(screen.queryByLabelText("Dismiss")).not.toBeInTheDocument()
  })

  it("shows nothing at all after a dismissal — never a 'Recovered' line", () => {
    // Regression: dismissal used to be plain `resolved`, so closing the only
    // active warning swapped the strip for "Recovered · …" — a second bar, and
    // a false claim whenever the connection was still down.
    const { container } = renderBanner([
      record({
        id: "w1",
        severity: "warning",
        title: "Reconnecting... 1/5",
        resolved: true,
        dismissed: true,
      }),
    ])
    expect(container).toBeEmptyDOMElement()
  })

  it("still shows a genuine recovery alongside an unrelated dismissal", () => {
    renderBanner([
      record({
        id: "w1",
        severity: "warning",
        title: "recovered incident",
        resolved: true,
      }),
      record({
        id: "w2",
        severity: "warning",
        title: "silenced incident",
        resolved: true,
        dismissed: true,
      }),
    ])
    expect(
      screen.getByText(/Recovered · recovered incident/)
    ).toBeInTheDocument()
    expect(screen.queryByText(/silenced incident/)).not.toBeInTheDocument()
  })

  describe("the recovered line is transient", () => {
    const recoveredWarning = () =>
      record({
        id: "w1",
        severity: "warning",
        title: "Reconnecting... 4/5",
        resolved: true,
      })

    it("self-dismisses instead of hanging under the composer forever", () => {
      // Field report 2026-08-17: the network came back, the incident settled,
      // and "Recovered · Reconnecting... 4/5" then sat there for the rest of
      // the session — records are kept as revision watermarks, so nothing
      // else ever took it down.
      vi.useFakeTimers()
      try {
        const onDismiss = vi.fn()
        renderBanner([recoveredWarning()], undefined, onDismiss)
        expect(screen.getByText(/Recovered/)).toBeInTheDocument()
        expect(onDismiss).not.toHaveBeenCalled()
        act(() => {
          vi.advanceTimersByTime(10_000)
        })
        // It writes the dismissal back to the store, so a remount can't
        // resurrect it.
        expect(onDismiss).toHaveBeenCalledWith(["w1"])
      } finally {
        vi.useRealTimers()
      }
    })

    it("can also be closed by hand, before the timer", () => {
      const onDismiss = vi.fn()
      renderBanner([recoveredWarning()], undefined, onDismiss)
      fireEvent.click(screen.getByLabelText("Dismiss"))
      expect(onDismiss).toHaveBeenCalledWith(["w1"])
    })

    // The tests above stub `onDismiss`, which proves only that the strip ASKS
    // to be removed. These drive the REAL `dismissSessionFailures` transition
    // and re-render, which is where the first attempt silently failed: the
    // helper used to ignore already-resolved records, so the recovered line
    // requested its own removal every time and never got it.
    function renderWired(initial: SessionFailureRecord[]) {
      const state = { current: initial }
      const capture = (next: SessionFailureRecord[]) => {
        state.current = next
      }
      function Harness() {
        const [failures, setFailures] = useState(initial)
        useEffect(() => capture(failures), [failures])
        return (
          <NextIntlClientProvider locale="en" messages={enMessages}>
            <SessionFailureBanner
              failures={failures}
              onDismiss={(ids) =>
                setFailures((prev) => dismissSessionFailures(prev, ids))
              }
            />
          </NextIntlClientProvider>
        )
      }
      return { ...render(<Harness />), state }
    }

    it("actually disappears once the real store transition runs", () => {
      vi.useFakeTimers()
      try {
        const { container, state, unmount } = renderWired([recoveredWarning()])
        expect(screen.getByText(/Recovered/)).toBeInTheDocument()
        act(() => {
          vi.advanceTimersByTime(10_000)
        })
        expect(container).toBeEmptyDOMElement()
        expect(state.current[0]).toMatchObject({
          resolved: true,
          dismissed: true,
        })

        // Remount on the post-dismissal table: it must stay gone, and must not
        // schedule another expiry.
        const settled = state.current
        unmount()
        const remounted = renderWired(settled)
        expect(remounted.container).toBeEmptyDOMElement()

        // A genuine recurrence at a higher revision still re-arms the strip.
        const recurred = upsertSessionFailure(settled, {
          ...recoveredWarning(),
          revision: 2,
          resolved: false,
        })
        remounted.unmount()
        renderWired(recurred)
        expect(screen.getByRole("alert")).toBeInTheDocument()
        expect(screen.getByText("Reconnecting... 4/5")).toBeInTheDocument()
      } finally {
        vi.useRealTimers()
      }
    })

    it("disappears on a manual close through the real store too", () => {
      const { container } = renderWired([recoveredWarning()])
      fireEvent.click(screen.getByLabelText("Dismiss"))
      expect(container).toBeEmptyDOMElement()
    })

    it("renders without a handler and never schedules a stray dismissal", () => {
      vi.useFakeTimers()
      try {
        renderBanner([recoveredWarning()])
        expect(screen.getByText(/Recovered/)).toBeInTheDocument()
        expect(screen.queryByLabelText("Dismiss")).not.toBeInTheDocument()
        act(() => {
          vi.advanceTimersByTime(10_000)
        })
        expect(screen.getByText(/Recovered/)).toBeInTheDocument()
      } finally {
        vi.useRealTimers()
      }
    })
  })
})
