import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { MarketWallpaper } from "@/lib/workspace-background-market"

const searchMock = vi.fn()

vi.mock("@/lib/workspace-background-market", () => ({
  MARKET_CATEGORIES: ["all", "general", "anime", "people"] as const,
  searchWorkspaceBgMarket: (input: unknown) => searchMock(input),
}))

// The real hook pulls `fetchWorkspaceBgMarketAsset` out of the module mocked
// above (which deliberately only exports the search surface) and mints blob
// URLs — neither of which exists under jsdom. Pin it to a resolved thumb so
// the card renders its <img> branch deterministically.
vi.mock("@/hooks/use-proxied-background-thumb", () => ({
  useProxiedBackgroundThumb: () => ({
    src: "blob:x",
    loading: false,
    failed: false,
  }),
}))

vi.mock("next-intl", () => ({
  useTranslations: (ns: string) => (key: string) => `${ns}.${key}`,
}))

import { WorkspaceBackgroundMarketDialog } from "./workspace-background-market-dialog"

const ITEM: MarketWallpaper = {
  id: "abc123",
  thumbUrl: "https://th.wallhaven.cc/small/ab/abc123.jpg",
  fullUrl: "https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg",
  sourceUrl: "https://wallhaven.cc/w/abc123",
  resolution: "1920×1080",
  category: "general",
}

beforeEach(() => {
  searchMock.mockReset()
})

describe("WorkspaceBackgroundMarketDialog", () => {
  it("renders listing items once loaded", async () => {
    searchMock.mockResolvedValue({ items: [ITEM], page: 1, lastPage: 3 })
    render(
      <WorkspaceBackgroundMarketDialog
        open
        onOpenChange={() => {}}
        appliedSourceUrl={null}
        onApply={vi.fn()}
      />
    )
    await waitFor(() =>
      expect(screen.getByText("1920×1080")).toBeInTheDocument()
    )
    expect(searchMock).toHaveBeenCalledWith({
      query: "",
      category: "all",
      page: 1,
    })
  })

  it("marks the applied wallpaper and applies on click", async () => {
    searchMock.mockResolvedValue({ items: [ITEM], page: 1, lastPage: 1 })
    const onApply = vi.fn().mockResolvedValue(undefined)
    render(
      <WorkspaceBackgroundMarketDialog
        open
        onOpenChange={() => {}}
        appliedSourceUrl="https://wallhaven.cc/w/abc123"
        onApply={onApply}
      />
    )
    await waitFor(() =>
      expect(screen.getByText("1920×1080")).toBeInTheDocument()
    )
    expect(
      screen.getByText("AppearanceSettings.workspaceBackground.market.applied")
    ).toBeInTheDocument()
    await userEvent.click(screen.getByRole("button", { name: /abc123/ }))
    await waitFor(() =>
      expect(onApply).toHaveBeenCalledWith(ITEM.fullUrl, ITEM.sourceUrl)
    )
  })

  it("shows a retryable error state on failure", async () => {
    searchMock.mockRejectedValue(new Error("network"))
    render(
      <WorkspaceBackgroundMarketDialog
        open
        onOpenChange={() => {}}
        appliedSourceUrl={null}
        onApply={vi.fn()}
      />
    )
    await waitFor(() =>
      expect(
        screen.getByText("AppearanceSettings.workspaceBackground.market.error")
      ).toBeInTheDocument()
    )
    searchMock.mockResolvedValue({ items: [ITEM], page: 1, lastPage: 1 })
    // The toolbar's icon-only refresh button and the error-state retry button
    // share one accessible name; the error-state one is the last rendered.
    const retryButtons = screen.getAllByRole("button", {
      name: "AppearanceSettings.workspaceBackground.market.retry",
    })
    await userEvent.click(retryButtons[retryButtons.length - 1])
    await waitFor(() =>
      expect(screen.getByText("1920×1080")).toBeInTheDocument()
    )
  })
})
