import { fireEvent, render } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import {
  WorkbenchRouteProvider,
  useWorkbenchRoute,
} from "./workbench-route-context"

function Probe() {
  const { routeId, isConversations, setRoute, openConversations } =
    useWorkbenchRoute()
  return (
    <div>
      <span data-testid="route">{routeId}</span>
      <span data-testid="isConv">{String(isConversations)}</span>
      <button onClick={() => setRoute("automations")}>go</button>
      <button onClick={openConversations}>back</button>
    </div>
  )
}

describe("WorkbenchRouteProvider", () => {
  it("defaults to the conversation workspace and switches routes", () => {
    const { getByTestId, getByText } = render(
      <WorkbenchRouteProvider>
        <Probe />
      </WorkbenchRouteProvider>
    )
    expect(getByTestId("route").textContent).toBe("conversations")
    expect(getByTestId("isConv").textContent).toBe("true")

    fireEvent.click(getByText("go"))
    expect(getByTestId("route").textContent).toBe("automations")
    expect(getByTestId("isConv").textContent).toBe("false")

    fireEvent.click(getByText("back"))
    expect(getByTestId("route").textContent).toBe("conversations")
    expect(getByTestId("isConv").textContent).toBe("true")
  })

  it("throws when used outside the provider", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {})
    expect(() => render(<Probe />)).toThrow(/WorkbenchRouteProvider/)
    spy.mockRestore()
  })
})
