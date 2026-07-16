import { fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  push: vi.fn(),
  toggleSidebar: vi.fn(),
  tasks: [
    {
      id: "running-1",
      label: "Run tests",
      description: "修复移动端布局",
      status: "running" as const,
    },
    {
      id: "pending-1",
      label: "Await approval",
      status: "pending" as const,
    },
  ],
  alerts: [{ id: "alert-1", message: "需要批准", detail: undefined }],
}))

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: mocks.push }),
}))

vi.mock("@/contexts/alert-context", () => ({
  useAlertContext: () => ({ alerts: mocks.alerts }),
}))

vi.mock("@/contexts/sidebar-context", () => ({
  useSidebarContext: () => ({ toggle: mocks.toggleSidebar }),
}))

vi.mock("@/contexts/task-context", () => ({
  useTaskContext: () => ({ tasks: mocks.tasks }),
}))

vi.mock("@/lib/transport/detect", () => ({
  isMobileEnvironment: () => true,
}))

import { MobileBottomNavigation } from "./mobile-bottom-navigation"

describe("MobileBottomNavigation", () => {
  beforeEach(() => {
    mocks.push.mockClear()
    mocks.toggleSidebar.mockClear()
  })

  it("uses the Android navigation height and opens a bounded task center", () => {
    render(<MobileBottomNavigation />)

    const navigation = screen.getByRole("navigation", {
      name: "移动端主导航",
    })
    expect(navigation).toHaveClass("h-14")
    expect(screen.getByText("3 项需要关注")).toHaveClass("sr-only")
    const attentionIndicator = document.querySelector(
      '[data-slot="nav-attention-indicator"]'
    )
    expect(attentionIndicator).toHaveClass("h-2", "w-2", "bg-primary/70")
    expect(attentionIndicator).not.toHaveClass("bg-destructive")

    fireEvent.click(screen.getByRole("button", { name: /任务/ }))

    expect(screen.getByText("运行中与等待处理")).toBeInTheDocument()
    expect(screen.getByText("修复移动端布局")).toBeInTheDocument()
    expect(screen.getByText("需要批准")).toBeInTheDocument()

    const sheet = document.querySelector('[data-slot="sheet-content"]')
    expect(sheet).toHaveClass("h-[min(72dvh,720px)]")
    expect(sheet).toHaveClass("flex", "flex-col")
    expect(
      Array.from(sheet?.querySelectorAll("div") ?? []).some((element) =>
        element.classList.contains("min-h-[68px]")
      )
    ).toBe(true)
  })

  it("keeps the primary navigation actions wired", () => {
    render(<MobileBottomNavigation />)

    fireEvent.click(screen.getByRole("button", { name: "会话" }))
    expect(mocks.toggleSidebar).toHaveBeenCalledTimes(1)

    fireEvent.click(screen.getByRole("button", { name: "设置" }))
    expect(mocks.push).toHaveBeenCalledWith("/mobile-settings")
  })
})
