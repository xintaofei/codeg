import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi, type Mock } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import type { AgentQuotaInfo } from "@/lib/types"

vi.mock("@/contexts/tab-context", () => ({
  useTabStore: vi.fn(),
}))

const mockGetConnection = vi.fn()
const mockGetConnectPending = vi.fn()
const mockGetConnectError = vi.fn()
const mockSubscribeKey = vi.fn(() => () => {})

vi.mock("@/contexts/acp-connections-context", () => ({
  useConnectionStore: () => ({
    getConnection: mockGetConnection,
    getConnectPending: mockGetConnectPending,
    getConnectError: mockGetConnectError,
    subscribeKey: mockSubscribeKey,
  }),
}))

vi.mock("@/lib/api", () => ({
  getAgentQuota: vi.fn(),
  refreshAgentQuota: vi.fn(),
}))

import { ComposerQuotaBadge, resetQuotaCache } from "./composer-quota-badge"
import { useTabStore } from "@/contexts/tab-context"
import { getAgentQuota, refreshAgentQuota } from "@/lib/api"

const mockTabs = useTabStore as unknown as Mock
const mockGetAgentQuota = getAgentQuota as unknown as Mock
const mockRefreshAgentQuota = refreshAgentQuota as unknown as Mock

const sampleCodexQuota: AgentQuotaInfo = {
  agentType: "codex",
  planName: "Pro Plan",
  shortWindow: {
    label: "5-Hour Window",
    usedPercent: 20,
    remainingPercent: 80,
    resetsAt: "2026-09-26T12:00:00Z",
    resetInSeconds: 5100, // 1h 25m
  },
  weeklyWindow: {
    label: "Weekly Limit",
    usedPercent: 5,
    remainingPercent: 95,
    resetsAt: "2026-10-01T00:00:00Z",
    resetInSeconds: 400000,
  },
  spendLimit: {
    usedUsd: 12.5,
    limitUsd: 50.0,
  },
  lastUpdated: "2026-09-26T10:35:00Z",
}

const sampleLowQuota: AgentQuotaInfo = {
  agentType: "codex",
  planName: "Free Plan",
  shortWindow: {
    label: "5-Hour Window",
    usedPercent: 85,
    remainingPercent: 15,
    resetsAt: "2026-09-26T12:00:00Z",
    resetInSeconds: 300, // 5m
  },
  weeklyWindow: {
    label: "Weekly Limit",
    usedPercent: 98,
    remainingPercent: 2,
    resetsAt: "2026-10-01T00:00:00Z",
    resetInSeconds: 400000,
  },
  lastUpdated: "2026-09-26T10:35:00Z",
}

function renderBadge(agentType: string | null = "codex", tabId = "tab-1") {
  mockTabs.mockImplementation(
    (sel: (s: { tabs: Array<{ id: string; agentType: string }> }) => unknown) =>
      sel({
        tabs: agentType ? [{ id: tabId, agentType }] : [],
      })
  )
  mockGetConnection.mockReturnValue(
    agentType ? { agentType, status: "connected" } : undefined
  )
  mockGetConnectPending.mockReturnValue(undefined)
  mockGetConnectError.mockReturnValue(undefined)

  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <ComposerQuotaBadge tabId={tabId} />
    </NextIntlClientProvider>
  )
}

describe("ComposerQuotaBadge", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    resetQuotaCache()
  })

  it("renders null for unsupported agent types", () => {
    renderBadge("claude_code")
    expect(screen.queryByLabelText("View quota status")).toBeNull()
    expect(mockGetAgentQuota).not.toHaveBeenCalled()
  })

  it("renders null when tabId is null or not found", () => {
    renderBadge(null, "non-existent")
    expect(screen.queryByLabelText("View quota status")).toBeNull()
  })

  it("fetches quota and renders short text for codex", async () => {
    mockGetAgentQuota.mockResolvedValueOnce(sampleCodexQuota)
    renderBadge("codex")

    expect(mockGetAgentQuota).toHaveBeenCalledWith("codex")

    await waitFor(() => {
      expect(screen.getByLabelText("View quota status")).toHaveTextContent(
        "5h: 80%"
      )
    })
  })

  it("fetches quota and renders short text for antigravity", async () => {
    const antigravityQuota: AgentQuotaInfo = {
      agentType: "antigravity",
      planName: "Pro Tier",
      shortWindow: {
        label: "5h Window",
        usedPercent: 40,
        remainingPercent: 60,
        resetInSeconds: 3600,
      },
      lastUpdated: "2026-09-26T10:35:00Z",
    }
    mockGetAgentQuota.mockResolvedValueOnce(antigravityQuota)
    renderBadge("antigravity")

    expect(mockGetAgentQuota).toHaveBeenCalledWith("antigravity")

    await waitFor(() => {
      expect(screen.getByLabelText("View quota status")).toHaveTextContent(
        "5h: 60%"
      )
    })
  })

  it("renders low quota warning styling when remaining is low", async () => {
    mockGetAgentQuota.mockResolvedValueOnce(sampleLowQuota)
    renderBadge("codex")

    await waitFor(() => {
      const button = screen.getByLabelText("View quota status")
      expect(button).toHaveTextContent("5h: 15%")
      expect(button.className).toContain("text-red-500")
    })
  })

  it("opens popover with details and countdown on click", async () => {
    mockGetAgentQuota.mockResolvedValueOnce(sampleCodexQuota)
    renderBadge("codex")

    await waitFor(() => {
      expect(screen.getByLabelText("View quota status")).toHaveTextContent(
        "5h: 80%"
      )
    })

    await userEvent.click(screen.getByLabelText("View quota status"))

    // Popover content checks
    expect(screen.getByText("Pro Plan")).toBeInTheDocument()
    expect(screen.getByText("5-Hour Window")).toBeInTheDocument()
    expect(screen.getByText("Remaining 80%")).toBeInTheDocument()
    expect(screen.getByText("Weekly Limit")).toBeInTheDocument()
    expect(screen.getByText("Remaining 95%")).toBeInTheDocument()
    expect(screen.getByText(/Resets in 1h 25m/)).toBeInTheDocument()
    expect(screen.getByText("$12.50 / $50.00")).toBeInTheDocument()
  })

  it("triggers manual refresh when refresh button inside popover is clicked", async () => {
    mockGetAgentQuota.mockResolvedValueOnce(sampleCodexQuota)
    const refreshedQuota: AgentQuotaInfo = {
      ...sampleCodexQuota,
      shortWindow: {
        ...sampleCodexQuota.shortWindow!,
        remainingPercent: 75,
      },
    }
    mockRefreshAgentQuota.mockResolvedValueOnce(refreshedQuota)

    renderBadge("codex")

    await waitFor(() => {
      expect(screen.getByLabelText("View quota status")).toHaveTextContent(
        "5h: 80%"
      )
    })

    // Open popover
    await userEvent.click(screen.getByLabelText("View quota status"))

    // Find refresh button
    const refreshBtn = screen.getByLabelText("Refresh Quota")
    await userEvent.click(refreshBtn)

    expect(mockRefreshAgentQuota).toHaveBeenCalledWith("codex")

    await waitFor(() => {
      expect(screen.getByLabelText("View quota status")).toHaveTextContent(
        "5h: 75%"
      )
    })
  })
})
