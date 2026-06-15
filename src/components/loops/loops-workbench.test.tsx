import { describe, it, expect, vi, beforeEach } from "vitest"
import { render, screen, fireEvent, waitFor } from "@testing-library/react"

import { LoopsWorkbench } from "./loops-workbench"
import { defaultIssueConfig } from "@/lib/loop-config"
import type { LoopSpaceSummary } from "@/lib/types"

// Stable `t` — the workbench's `refresh` callback depends on `t`, so an
// unstable identity would loop the load effect forever (per next-intl mock
// guidance). Returns the key verbatim; enough to address every label here.
const { stableT, listLoopSpaces, deleteLoopSpace, getLoopEngineHealth } =
  vi.hoisted(() => ({
    stableT: (key: string) => key,
    listLoopSpaces: vi.fn(),
    deleteLoopSpace: vi.fn(),
    getLoopEngineHealth: vi.fn(),
  }))

vi.mock("next-intl", () => ({ useTranslations: () => stableT }))
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }))
// LoopsWorkbench now mounts the real LoopRealtimeProvider, whose effect calls
// the platform transport. Stub it so no real WS/IPC is touched in the test.
vi.mock("@/lib/platform", () => ({
  subscribe: vi.fn().mockResolvedValue(() => {}),
  onTransportReconnect: vi.fn(() => null),
}))
// The reconnect banner reads the web-connection store on mount, which lazily
// requires the web transport (unavailable in jsdom). Stub it to a steady
// "connected" so the banner renders null and never touches the transport.
vi.mock("@/lib/transport/web-connection-store", () => ({
  subscribeWebConnection: () => () => {},
  getWebConnectionSnapshot: () => "connected",
  getWebConnectionServerSnapshot: () => "connected",
}))
vi.mock("@/lib/loops-api", () => ({
  listLoopSpaces,
  deleteLoopSpace,
  getLoopEngineHealth,
}))
vi.mock("@/components/loops/space-detail", () => ({
  SpaceDetail: ({ space }: { space: LoopSpaceSummary }) => (
    <div data-testid="space-detail">{space.name}</div>
  ),
}))
vi.mock("@/components/loops/space-form-dialog", () => ({
  SpaceFormDialog: ({ open }: { open: boolean }) =>
    open ? <div data-testid="space-form" /> : null,
}))

function makeSpace(over: Partial<LoopSpaceSummary> = {}): LoopSpaceSummary {
  return {
    id: 1,
    name: "Space A",
    folder_id: 10,
    folder_path: "/repo",
    detached: false,
    issue_count: 2,
    running_count: 1,
    last_activity_at: null,
    created_at: "2026-06-13T00:00:00Z",
    default_config: defaultIssueConfig(),
    ...over,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  // Space selection is now URL-driven (useLoopNav), so reset the query string
  // between tests — otherwise a `?space=1` written by one test's card click
  // would leak into the next and pre-open a space.
  window.history.replaceState({}, "", "/")
  // The header mounts EngineHealthBadge, which polls getLoopEngineHealth on
  // mount. Resolve a quiet fixture so the effect doesn't throw or hit transport
  // (the badge renders null until this resolves and never affects assertions).
  getLoopEngineHealth.mockResolvedValue({
    runningIssues: 0,
    inFlightIterations: 0,
    pendingTokenIterations: 0,
    activeDrivers: 0,
    metrics: { settleEventsTotal: 0, lagSweepTotal: 0 },
  })
})

describe("LoopsWorkbench", () => {
  it("shows the empty state when there are no spaces", async () => {
    listLoopSpaces.mockResolvedValue([])
    render(<LoopsWorkbench />)
    expect(await screen.findByText("empty")).toBeInTheDocument()
  })

  it("renders a card per space", async () => {
    listLoopSpaces.mockResolvedValue([
      makeSpace(),
      makeSpace({ id: 2, name: "Space B" }),
    ])
    render(<LoopsWorkbench />)
    expect(await screen.findByText("Space A")).toBeInTheDocument()
    expect(screen.getByText("Space B")).toBeInTheDocument()
  })

  it("opens the create dialog from the New space button", async () => {
    listLoopSpaces.mockResolvedValue([])
    render(<LoopsWorkbench />)
    await screen.findByText("empty")
    fireEvent.click(screen.getByRole("button", { name: "newSpace" }))
    expect(screen.getByTestId("space-form")).toBeInTheDocument()
  })

  it("opens a space when its card is activated", async () => {
    listLoopSpaces.mockResolvedValue([makeSpace()])
    render(<LoopsWorkbench />)
    const card = await screen.findByText("Space A")
    fireEvent.click(card)
    expect(screen.getByTestId("space-detail")).toHaveTextContent("Space A")
  })

  it("deletes a space after confirmation", async () => {
    listLoopSpaces.mockResolvedValue([makeSpace()])
    deleteLoopSpace.mockResolvedValue(undefined)
    render(<LoopsWorkbench />)
    await screen.findByText("Space A")

    // Open the per-card menu (Radix opens on Enter), then pick Delete.
    fireEvent.keyDown(screen.getByLabelText("rename"), { key: "Enter" })
    fireEvent.click(
      await screen.findByRole("menuitem", { name: "deleteSpace" })
    )

    // Confirm in the alert dialog.
    const confirm = await screen.findByRole("button", { name: "deleteSpace" })
    fireEvent.click(confirm)

    await waitFor(() => expect(deleteLoopSpace).toHaveBeenCalledWith(1))
  })
})
