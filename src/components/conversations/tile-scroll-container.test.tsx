import { render } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => {
  const destroy = vi.fn()
  const initialize = vi.fn()
  const instance = vi.fn<() => { destroy: () => void } | null>(() => null)
  return { destroy, initialize, instance }
})

vi.mock("overlayscrollbars-react", () => ({
  useOverlayScrollbars: () => [mocks.initialize, mocks.instance],
}))

import { TileScrollContainer } from "./tile-scroll-container"

beforeEach(() => {
  mocks.destroy.mockClear()
  mocks.initialize.mockReset()
  mocks.instance.mockReset()
  mocks.instance.mockReturnValue(null)
  // Mirror the real hook: once initialized, instance() returns a live
  // (destroyable) object.
  mocks.initialize.mockImplementation(() => {
    mocks.instance.mockReturnValue({ destroy: mocks.destroy })
  })
})

describe("TileScrollContainer", () => {
  it("does not initialize OverlayScrollbars while untiled", () => {
    render(
      <TileScrollContainer canTile={false}>
        <div data-testid="child" />
      </TileScrollContainer>
    )

    expect(mocks.initialize).not.toHaveBeenCalled()
  })

  it("initializes on tile-on targeting the host with the contents viewport", () => {
    const { container, rerender } = render(
      <TileScrollContainer canTile={false}>
        <div data-testid="child" />
      </TileScrollContainer>
    )
    rerender(
      <TileScrollContainer canTile={true}>
        <div data-testid="child" />
      </TileScrollContainer>
    )

    expect(mocks.initialize).toHaveBeenCalledTimes(1)
    const target = container.firstElementChild
    const contents = target?.firstElementChild
    expect(mocks.initialize).toHaveBeenCalledWith({
      target,
      elements: { viewport: contents, content: contents },
    })
  })

  it("destroys the instance on tile-off", () => {
    const { rerender } = render(
      <TileScrollContainer canTile={true}>
        <div data-testid="child" />
      </TileScrollContainer>
    )
    expect(mocks.initialize).toHaveBeenCalledTimes(1)

    rerender(
      <TileScrollContainer canTile={false}>
        <div data-testid="child" />
      </TileScrollContainer>
    )

    expect(mocks.destroy).toHaveBeenCalledTimes(1)
  })

  it("keeps the same child DOM nodes across tile flips", () => {
    const { container, getByTestId, rerender } = render(
      <TileScrollContainer canTile={false}>
        <div data-testid="child" />
      </TileScrollContainer>
    )
    const host = container.firstElementChild
    const child = getByTestId("child")
    expect(host?.hasAttribute("data-overlayscrollbars-initialize")).toBe(false)

    rerender(
      <TileScrollContainer canTile={true}>
        <div data-testid="child" />
      </TileScrollContainer>
    )
    expect(container.firstElementChild).toBe(host)
    expect(getByTestId("child")).toBe(child)
    expect(host?.hasAttribute("data-overlayscrollbars-initialize")).toBe(true)

    rerender(
      <TileScrollContainer canTile={false}>
        <div data-testid="child" />
      </TileScrollContainer>
    )
    expect(container.firstElementChild).toBe(host)
    expect(getByTestId("child")).toBe(child)
    expect(host?.hasAttribute("data-overlayscrollbars-initialize")).toBe(false)
  })
})
