import { act, render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { SearchCommandDialog } from "./search-command-dialog"
import enMessages from "@/i18n/messages/en.json"
import type { MessageSearchHit } from "@/lib/api"

const h = vi.hoisted(() => ({
  searchMessages: vi.fn(),
  listAllConversations: vi.fn(),
  openTab: vi.fn(),
  openConversations: vi.fn(),
  resetFileTree: vi.fn(),
}))

vi.mock("@/components/agent-icon", () => ({ AgentIcon: () => null }))

vi.mock("@/lib/api", () => ({
  listAllConversations: h.listAllConversations,
  searchMessages: h.searchMessages,
}))

vi.mock("@/contexts/tab-context", () => ({
  useTabActions: () => ({ openTab: h.openTab }),
}))

vi.mock("@/contexts/workbench-route-context", () => ({
  useWorkbenchRoute: () => ({ openConversations: h.openConversations }),
}))

vi.mock("@/contexts/workspace-context", () => ({
  useWorkspaceActions: () => ({ openFilePreview: vi.fn() }),
}))

vi.mock("@/contexts/aux-panel-context", () => ({
  useAuxPanelContext: () => ({ revealInFileTree: vi.fn() }),
}))

vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({ activeFolder: null, activeFolderId: null }),
}))

vi.mock("@/stores/app-workspace-store", () => ({
  useAppWorkspaceStore: (selector: (s: unknown) => unknown) =>
    selector({ conversations: [] }),
}))

vi.mock("@/hooks/use-file-tree", () => ({
  useFileTree: () => ({ allFiles: [], loading: false, reset: h.resetFileTree }),
}))

function hit(over: Partial<MessageSearchHit> = {}): MessageSearchHit {
  return {
    conversation_id: 7,
    folder_id: 3,
    agent_type: "codex",
    title: "Upload fixes",
    turn_idx: 4,
    role: "assistant",
    snippet: "…the [[mark]]retry[[/mark]] [[mark]]loop[[/mark]] re-enters",
    rank: -1.5,
    ...over,
  }
}

/** A request whose resolution the test controls. */
function defer<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((r) => {
    resolve = r
  })
  return { promise, resolve }
}

async function openMessagesTab(onOpenChange = vi.fn()) {
  render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <SearchCommandDialog open onOpenChange={onOpenChange} />
    </NextIntlClientProvider>
  )
  const user = userEvent.setup()
  await user.click(screen.getByRole("button", { name: "Messages" }))
  const input = screen.getByPlaceholderText("Search message content...")
  return { user, input }
}

describe("SearchCommandDialog messages tab", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    h.listAllConversations.mockResolvedValue([])
  })

  it("searches message content and highlights the matched words", async () => {
    h.searchMessages.mockResolvedValue([hit()])
    const { user, input } = await openMessagesTab()

    await user.type(input, "retry loop")

    expect(await screen.findByText("Upload fixes")).toBeTruthy()
    expect(h.searchMessages).toHaveBeenLastCalledWith("retry loop", 40)
    expect(screen.getByText("Assistant")).toBeTruthy()
    const marks = Array.from(document.querySelectorAll("mark"))
    expect(marks.map((mark) => mark.textContent)).toEqual(["retry", "loop"])
  })

  it("opens the conversation of the picked hit", async () => {
    h.searchMessages.mockResolvedValue([hit()])
    const onOpenChange = vi.fn()
    const { user, input } = await openMessagesTab(onOpenChange)

    await user.type(input, "retry")
    await user.click(await screen.findByText("Upload fixes"))

    expect(h.openConversations).toHaveBeenCalled()
    expect(h.openTab).toHaveBeenCalledWith(3, 7, "codex", true)
    expect(onOpenChange).toHaveBeenCalledWith(false)
  })

  it("keeps the newest query's hits when an earlier answer arrives late", async () => {
    const earlier = defer<MessageSearchHit[]>()
    h.searchMessages
      .mockReturnValueOnce(earlier.promise)
      .mockResolvedValueOnce([hit({ title: "Newer answer" })])
    const { user, input } = await openMessagesTab()

    await user.type(input, "retry")
    await waitFor(() => expect(h.searchMessages).toHaveBeenCalledTimes(1))
    await user.type(input, " loop")
    expect(await screen.findByText("Newer answer")).toBeTruthy()

    await act(async () => {
      earlier.resolve([hit({ title: "Older answer" })])
      await earlier.promise
    })

    expect(screen.queryByText("Older answer")).toBeNull()
    expect(screen.getByText("Newer answer")).toBeTruthy()
  })
})
