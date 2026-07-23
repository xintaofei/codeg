import { render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({
  getTailscaleFunnelStatus: vi.fn(),
  setTailscaleFunnelEnabled: vi.fn(),
  openTailscaleLogin: vi.fn(),
}))

vi.mock("@/lib/platform", () => ({
  openUrl: vi.fn(),
}))

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}))

vi.mock("@/lib/utils", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/utils")>()
  return {
    ...actual,
    copyTextToClipboard: vi.fn(),
  }
})

vi.mock("@/hooks/use-copied-flag", () => ({
  useCopiedFlag: () => [false, vi.fn()],
}))

import { WebServiceFunnelSection } from "./web-service-funnel-section"
import enMessages from "@/i18n/messages/en.json"
import { getTailscaleFunnelStatus, type TailscaleFunnelStatus } from "@/lib/api"

const mockGet = vi.mocked(getTailscaleFunnelStatus)

function status(
  overrides: Partial<TailscaleFunnelStatus> = {}
): TailscaleFunnelStatus {
  return {
    supported: true,
    enabled: false,
    state: "stopped",
    ...overrides,
  }
}

function renderSection(webRunning = true) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <WebServiceFunnelSection webRunning={webRunning} />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  mockGet.mockReset()
})

describe("WebServiceFunnelSection", () => {
  it("shows login action when needs_login", async () => {
    mockGet.mockResolvedValue(
      status({
        enabled: true,
        state: "needs_login",
        loginUrl: "https://login.tailscale.com/a/x",
      })
    )
    renderSection(true)
    expect(
      await screen.findByRole("button", { name: /Open Tailscale login/i })
    ).toBeInTheDocument()
  })

  it("renders funnel url when ready", async () => {
    mockGet.mockResolvedValue(
      status({
        enabled: true,
        state: "funnel_ready",
        funnelUrl: "https://codeg-abc.ts.net",
      })
    )
    renderSection(true)
    await waitFor(() => {
      expect(screen.getByText("https://codeg-abc.ts.net")).toBeInTheDocument()
    })
  })
})
