import { render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, test, vi } from "vitest"
import { McpSettings } from "./mcp-settings"

const t = (key: string) => key

const apiMocks = vi.hoisted(() => ({
  mcpGetMarketplaceServerDetail: vi.fn(),
  mcpInstallFromMarketplace: vi.fn(),
  mcpListMarketplaces: vi.fn(),
  mcpRemoveServer: vi.fn(),
  mcpScanLocal: vi.fn(),
  mcpSearchMarketplace: vi.fn(),
  mcpUpsertLocalServer: vi.fn(),
}))

vi.mock("next-intl", () => ({
  useTranslations: () => t,
}))

vi.mock("sonner", () => ({
  toast: {
    error: vi.fn(),
    success: vi.fn(),
  },
}))

vi.mock("@/lib/api", () => apiMocks)

describe("McpSettings", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    apiMocks.mcpListMarketplaces.mockResolvedValue([])
    apiMocks.mcpSearchMarketplace.mockResolvedValue([])
  })

  test("omits local MCP entries with blank ids", async () => {
    apiMocks.mcpScanLocal.mockResolvedValue([
      {
        id: "visible-server",
        spec: { type: "stdio", command: "npx", args: ["visible"] },
        apps: ["codex"],
      },
      {
        id: "   ",
        spec: { type: "stdio", command: "npx", args: ["invisible"] },
        apps: ["codex"],
      },
    ])

    render(<McpSettings />)

    expect(await screen.findByText("npx visible")).toBeInTheDocument()
    expect(screen.queryByText("npx invisible")).not.toBeInTheDocument()
  })

  test("keeps local MCP rows inside an isolated scroll region", async () => {
    apiMocks.mcpScanLocal.mockResolvedValue([
      {
        id: "open-design",
        spec: {
          type: "stdio",
          command: "/Users/kogeki/.nvm/versions/node/v24.16.0/bin/node",
          args: [
            "/Users/kogeki/Documents/Codex/2026-06-08/open-design/bin/od.mjs",
            "mcp",
          ],
        },
        apps: ["codex"],
      },
      {
        id: "playwright",
        spec: {
          type: "stdio",
          command: "npx",
          args: ["-y", "@playwright/mcp@latest"],
        },
        apps: ["codex"],
      },
    ])

    render(<McpSettings />)

    expect(
      await screen.findByText("npx -y @playwright/mcp@latest")
    ).toBeInTheDocument()
    const scrollRegion = screen.getByTestId("mcp-local-list-scroll")
    expect(scrollRegion).toHaveClass("min-h-0", "flex-1", "overflow-auto")
    expect(scrollRegion).not.toHaveClass("space-y-1")
    expect(scrollRegion.firstElementChild).toHaveClass(
      "flex",
      "flex-col",
      "gap-1"
    )
  })
})
