import { fireEvent, render, screen } from "@testing-library/react"
import { useState } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"

import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "./instant-collapsible"

afterEach(() => {
  vi.restoreAllMocks()
})

// Pretend the collapsible content has an animate-out style. Scoped to the
// content node only — testing-library's role queries call getComputedStyle on
// other elements and need the real implementation.
function mockExitAnimation() {
  const real = window.getComputedStyle.bind(window)
  vi.spyOn(window, "getComputedStyle").mockImplementation((el, pseudo) => {
    const element = el as HTMLElement
    if (element.dataset?.slot === "collapsible-content") {
      return {
        animationName: "exit",
        animationDuration: "0.15s",
        animationDelay: "0s",
      } as CSSStyleDeclaration
    }
    return real(element, pseudo as never)
  })
}

describe("InstantCollapsible", () => {
  it("starts closed (uncontrolled) with no content in the DOM and toggles on click", () => {
    render(
      <Collapsible>
        <CollapsibleTrigger>toggle</CollapsibleTrigger>
        <CollapsibleContent>
          <div data-testid="body" />
        </CollapsibleContent>
      </Collapsible>
    )

    expect(screen.queryByTestId("body")).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "toggle" }))
    expect(screen.getByTestId("body")).toBeInTheDocument()

    // jsdom has no animations, so closing unmounts synchronously.
    fireEvent.click(screen.getByRole("button", { name: "toggle" }))
    expect(screen.queryByTestId("body")).not.toBeInTheDocument()
  })

  it("respects defaultOpen", () => {
    render(
      <Collapsible defaultOpen>
        <CollapsibleTrigger>toggle</CollapsibleTrigger>
        <CollapsibleContent>
          <div data-testid="body" />
        </CollapsibleContent>
      </Collapsible>
    )

    expect(screen.getByTestId("body")).toBeInTheDocument()
  })

  it("works controlled: reports toggles through onOpenChange and follows the prop", () => {
    const onOpenChange = vi.fn()
    function Harness() {
      const [open, setOpen] = useState(false)
      return (
        <Collapsible
          open={open}
          onOpenChange={(next) => {
            onOpenChange(next)
            setOpen(next)
          }}
        >
          <CollapsibleTrigger>toggle</CollapsibleTrigger>
          <CollapsibleContent>
            <div data-testid="body" />
          </CollapsibleContent>
        </Collapsible>
      )
    }
    render(<Harness />)

    fireEvent.click(screen.getByRole("button", { name: "toggle" }))
    expect(onOpenChange).toHaveBeenLastCalledWith(true)
    expect(screen.getByTestId("body")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "toggle" }))
    expect(onOpenChange).toHaveBeenLastCalledWith(false)
    expect(screen.queryByTestId("body")).not.toBeInTheDocument()
  })

  it("exposes the Radix data/aria contract", () => {
    const { container } = render(
      <Collapsible defaultOpen className="root-class">
        <CollapsibleTrigger>toggle</CollapsibleTrigger>
        <CollapsibleContent>
          <div data-testid="body" />
        </CollapsibleContent>
      </Collapsible>
    )

    const root = container.querySelector('[data-slot="collapsible"]')
    const trigger = screen.getByRole("button", { name: "toggle" })
    const content = container.querySelector('[data-slot="collapsible-content"]')

    expect(root).toHaveClass("root-class")
    expect(root).toHaveAttribute("data-state", "open")
    expect(trigger).toHaveAttribute("data-slot", "collapsible-trigger")
    expect(trigger).toHaveAttribute("data-state", "open")
    expect(trigger).toHaveAttribute("aria-expanded", "true")
    expect(content).toHaveAttribute("data-state", "open")
    expect(content).toHaveAttribute("id", trigger.getAttribute("aria-controls"))

    fireEvent.click(trigger)
    expect(root).toHaveAttribute("data-state", "closed")
    expect(trigger).toHaveAttribute("data-state", "closed")
    expect(trigger).toHaveAttribute("aria-expanded", "false")
  })

  it("does not toggle when the root is disabled", () => {
    render(
      <Collapsible disabled>
        <CollapsibleTrigger>toggle</CollapsibleTrigger>
        <CollapsibleContent>
          <div data-testid="body" />
        </CollapsibleContent>
      </Collapsible>
    )

    const trigger = screen.getByRole("button", { name: "toggle" })
    expect(trigger).toBeDisabled()
    fireEvent.click(trigger)
    expect(screen.queryByTestId("body")).not.toBeInTheDocument()
  })

  it("keeps the content mounted through an exit animation, then unmounts", () => {
    mockExitAnimation()

    render(
      <Collapsible defaultOpen>
        <CollapsibleTrigger>toggle</CollapsibleTrigger>
        <CollapsibleContent>
          <div data-testid="body" />
        </CollapsibleContent>
      </Collapsible>
    )

    fireEvent.click(screen.getByRole("button", { name: "toggle" }))
    // Exit animation in flight: still mounted, in the closed state.
    const body = screen.getByTestId("body")
    const content = body.parentElement as HTMLElement
    expect(content).toHaveAttribute("data-state", "closed")

    fireEvent.animationEnd(content)
    expect(screen.queryByTestId("body")).not.toBeInTheDocument()
  })

  it("cancels a pending exit when reopened mid-animation", () => {
    mockExitAnimation()

    render(
      <Collapsible defaultOpen>
        <CollapsibleTrigger>toggle</CollapsibleTrigger>
        <CollapsibleContent>
          <div data-testid="body" />
        </CollapsibleContent>
      </Collapsible>
    )

    const trigger = screen.getByRole("button", { name: "toggle" })
    fireEvent.click(trigger)
    const content = screen.getByTestId("body").parentElement as HTMLElement

    fireEvent.click(trigger)
    expect(content).toHaveAttribute("data-state", "open")

    // The stale exit animation finishing must not unmount reopened content.
    fireEvent.animationEnd(content)
    expect(screen.getByTestId("body")).toBeInTheDocument()
  })

  it("ignores bubbling child animation ends while exiting", () => {
    mockExitAnimation()

    render(
      <Collapsible defaultOpen>
        <CollapsibleTrigger>toggle</CollapsibleTrigger>
        <CollapsibleContent>
          <div data-testid="body" />
        </CollapsibleContent>
      </Collapsible>
    )

    fireEvent.click(screen.getByRole("button", { name: "toggle" }))
    const body = screen.getByTestId("body")

    fireEvent.animationEnd(body)
    expect(screen.getByTestId("body")).toBeInTheDocument()

    fireEvent.animationEnd(body.parentElement as HTMLElement)
    expect(screen.queryByTestId("body")).not.toBeInTheDocument()
  })
})
