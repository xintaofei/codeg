import { fireEvent, render, screen } from "@testing-library/react"
import { useState } from "react"
import { beforeEach, describe, expect, it } from "vitest"

import { LoopsViewProvider, useLoopsView } from "@/contexts/loops-view-context"

function Probe() {
  const { view, setView } = useLoopsView()
  return (
    <div>
      <span data-testid="view">{view}</span>
      <button onClick={() => setView("loops")}>to-loops</button>
    </div>
  )
}

function Harness({ initialTab }: { initialTab: string | null }) {
  const [tab, setTab] = useState<string | null>(initialTab)
  return (
    <LoopsViewProvider activeTabId={tab}>
      <Probe />
      <button onClick={() => setTab("tab-2")}>change-tab</button>
    </LoopsViewProvider>
  )
}

beforeEach(() => {
  window.localStorage.clear()
})

describe("LoopsViewProvider", () => {
  it("defaults to chat", () => {
    render(<Harness initialTab="tab-1" />)
    expect(screen.getByTestId("view")).toHaveTextContent("chat")
  })

  it("switches to loops and persists the choice", () => {
    render(<Harness initialTab="tab-1" />)
    fireEvent.click(screen.getByText("to-loops"))
    expect(screen.getByTestId("view")).toHaveTextContent("loops")
    expect(window.localStorage.getItem("codeg:loops-view:v1")).toBe("loops")
  })

  it("hydrates the loops view from storage on mount", () => {
    window.localStorage.setItem("codeg:loops-view:v1", "loops")
    render(<Harness initialTab="tab-1" />)
    expect(screen.getByTestId("view")).toHaveTextContent("loops")
  })

  it("flips back to chat when the active tab changes", () => {
    window.localStorage.setItem("codeg:loops-view:v1", "loops")
    render(<Harness initialTab="tab-1" />)
    expect(screen.getByTestId("view")).toHaveTextContent("loops")
    fireEvent.click(screen.getByText("change-tab"))
    expect(screen.getByTestId("view")).toHaveTextContent("chat")
  })
})
