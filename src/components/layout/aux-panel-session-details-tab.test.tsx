import { render } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi, type Mock } from "vitest"

import type { DbConversationSummary } from "@/lib/types"
import enMessages from "@/i18n/messages/en.json"

// Heavy, context-hungry children relocated from the title bar — stub them so
// this test exercises only the tab's own layout/gating, not their internals.
vi.mock("./branch-dropdown", () => ({
  BranchDropdown: () => <div data-testid="branch-dropdown" />,
}))
vi.mock("./command-dropdown", () => ({
  CommandDropdown: () => <div data-testid="command-dropdown" />,
}))
// The agent icon renders inline SVG with a <title> that duplicates the label.
vi.mock("@/components/agent-icon", () => ({ AgentIcon: () => null }))
// Stats are always supplied here, so the cold-fetch path must never fire; stub
// the API so an accidental call is inert rather than a real transport hit.
vi.mock("@/lib/api", () => ({ getFolderConversation: vi.fn() }))

vi.mock("@/contexts/aux-panel-context", () => ({ useAuxPanelContext: vi.fn() }))
vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: vi.fn(),
}))
vi.mock("@/hooks/use-is-active-chat-mode", () => ({
  useIsActiveChatMode: vi.fn(),
}))
// The tab now reads viewport size to size its header (desktop h-10 vs the
// mobile Sheet's original py-2); the real hook calls `window.matchMedia`, which
// jsdom lacks, so mock it.
vi.mock("@/hooks/use-mobile", () => ({ useIsMobile: vi.fn() }))
vi.mock("@/contexts/tab-context", () => ({ useTabStore: vi.fn() }))
vi.mock("@/stores/conversation-runtime-store", () => ({
  useConversationRuntimeStore: vi.fn(),
}))
vi.mock("@/stores/app-workspace-store", () => ({
  useAppWorkspaceStore: vi.fn(),
}))

import { SessionDetailsTab } from "./aux-panel-session-details-tab"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useIsActiveChatMode } from "@/hooks/use-is-active-chat-mode"
import { useIsMobile } from "@/hooks/use-mobile"
import { useTabStore } from "@/contexts/tab-context"
import { useConversationRuntimeStore } from "@/stores/conversation-runtime-store"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"

const mockAux = useAuxPanelContext as unknown as Mock
const mockFolder = useActiveFolder as unknown as Mock
const mockChat = useIsActiveChatMode as unknown as Mock
const mockMobile = useIsMobile as unknown as Mock
const mockTabs = useTabStore as unknown as Mock
const mockRuntime = useConversationRuntimeStore as unknown as Mock
const mockWorkspace = useAppWorkspaceStore as unknown as Mock

type TabSlice = {
  tabs: Array<{
    id: number
    conversationId: number | null
    runtimeConversationId?: number
  }>
  activeTabId: number | null
}
type RuntimeSlice = { byConversationId: Map<number, unknown> }
type WorkspaceSlice = { conversations: DbConversationSummary[] }

function summary(
  over: Partial<DbConversationSummary> = {}
): DbConversationSummary {
  return {
    id: 7,
    folder_id: 1,
    title: "My session",
    title_locked: false,
    agent_type: "claude_code",
    status: "in_progress",
    kind: "regular",
    model: "claude-opus-4-8",
    git_branch: "main",
    external_id: "ext-abc",
    message_count: 12,
    child_count: 0,
    created_at: "2026-06-10T10:00:00.000Z",
    updated_at: "2026-06-12T12:00:00.000Z",
    pinned_at: null,
    ...over,
  }
}

function setupScene(opts: {
  activeFolderId: number | null
  isChatMode: boolean
  hasActiveConversation: boolean
  isMobile?: boolean
}) {
  mockAux.mockReturnValue({ isOpen: true, activeTab: "session_details" })
  mockFolder.mockReturnValue({ activeFolderId: opts.activeFolderId })
  mockChat.mockReturnValue(opts.isChatMode)
  mockMobile.mockReturnValue(opts.isMobile ?? false)

  const tabState: TabSlice = {
    tabs: opts.hasActiveConversation ? [{ id: 1, conversationId: 7 }] : [],
    activeTabId: opts.hasActiveConversation ? 1 : null,
  }
  mockTabs.mockImplementation((sel: (s: TabSlice) => unknown) => sel(tabState))
  mockRuntime.mockImplementation((sel: (s: RuntimeSlice) => unknown) =>
    sel({ byConversationId: new Map() })
  )
  mockWorkspace.mockImplementation((sel: (s: WorkspaceSlice) => unknown) =>
    sel({ conversations: opts.hasActiveConversation ? [summary()] : [] })
  )
}

function renderTab() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <SessionDetailsTab />
    </NextIntlClientProvider>
  )
}

describe("SessionDetailsTab", () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it("renders the active session's details and the folder actions bar", () => {
    setupScene({
      activeFolderId: 1,
      isChatMode: false,
      hasActiveConversation: true,
    })
    const { getByText, getByTestId } = renderTab()
    expect(getByText("My session")).toBeTruthy()
    expect(getByText("Claude Code")).toBeTruthy()
    expect(getByTestId("branch-dropdown")).toBeTruthy()
    expect(getByTestId("command-dropdown")).toBeTruthy()
  })

  it("hides the folder actions bar in chat mode but still shows details", () => {
    setupScene({
      activeFolderId: 1,
      isChatMode: true,
      hasActiveConversation: true,
    })
    const { getByText, queryByTestId } = renderTab()
    expect(getByText("My session")).toBeTruthy()
    expect(queryByTestId("branch-dropdown")).toBeNull()
    expect(queryByTestId("command-dropdown")).toBeNull()
  })

  it("shows the empty state when there is no active session", () => {
    setupScene({
      activeFolderId: null,
      isChatMode: false,
      hasActiveConversation: false,
    })
    const { getByText, queryByTestId } = renderTab()
    expect(getByText("No active session")).toBeTruthy()
    expect(queryByTestId("branch-dropdown")).toBeNull()
  })

  it("sizes the actions bar h-10 on desktop but keeps the mobile Sheet's py-2", () => {
    // Desktop: the bar matches the conversation/file detail headers (h-10).
    setupScene({
      activeFolderId: 1,
      isChatMode: false,
      hasActiveConversation: true,
      isMobile: false,
    })
    const desktop = renderTab()
    const desktopBar = desktop.getByTestId("branch-dropdown").parentElement
    expect(desktopBar?.className).toContain("h-10")
    expect(desktopBar?.className).not.toContain("py-2")
    desktop.unmount()

    // Mobile (Sheet): unchanged from before — the original py-2, no fixed height.
    setupScene({
      activeFolderId: 1,
      isChatMode: false,
      hasActiveConversation: true,
      isMobile: true,
    })
    const mobileBar = renderTab().getByTestId("branch-dropdown").parentElement
    expect(mobileBar?.className).toContain("py-2")
    expect(mobileBar?.className).not.toContain("h-10")
  })
})
