import { render, waitFor } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

// Exercises the REAL Streamdown pipeline (no streamdown mock) so the assertion
// covers actual rendered output, not just which remark plugins are wired up.
// Only the link-safety hook is stubbed (it has no bearing on soft breaks).
vi.mock("@/components/ai-elements/link-safety", () => ({
  useStreamdownLinkSafety: () => ({ enabled: false }),
}))

import { MessageResponse } from "./message"

describe("MessageResponse soft breaks (real Streamdown)", () => {
  it("renders a single newline as <br> when softBreaks is set (user messages)", async () => {
    const { container } = render(
      <MessageResponse softBreaks>{"first\nsecond"}</MessageResponse>
    )

    await waitFor(() => {
      expect(container.querySelector("br")).not.toBeNull()
    })
    expect(container.textContent).toContain("first")
    expect(container.textContent).toContain("second")
  })

  it("collapses a single newline without softBreaks (assistant messages)", async () => {
    const { container } = render(
      <MessageResponse>{"first\nsecond"}</MessageResponse>
    )

    await waitFor(() => {
      expect(container.textContent).toContain("second")
    })
    expect(container.querySelector("br")).toBeNull()
  })
})
