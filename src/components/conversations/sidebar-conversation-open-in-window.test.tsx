import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, describe, expect, it, vi } from "vitest"

import { SidebarConversationCard } from "./sidebar-conversation-card"
import { openConversationWindow } from "@/lib/api"
import type { DbConversationSummary } from "@/lib/types"
import enMessages from "@/i18n/messages/en.json"

vi.mock("@/lib/api", () => ({
  openConversationWindow: vi.fn(async () => {}),
}))

const conversation: DbConversationSummary = {
  id: 7,
  folder_id: 4,
  title: "Fix the parser",
  title_locked: false,
  agent_type: "claude_code",
  status: "pending",
  kind: "regular",
  model: null,
  git_branch: null,
  external_id: null,
  message_count: 0,
  child_count: 0,
  created_at: new Date(0).toISOString(),
  updated_at: new Date(0).toISOString(),
  pinned_at: null,
}

const noop = () => {}

afterEach(cleanup)

describe("SidebarConversationCard open-in-new-window", () => {
  it("opens the conversation it belongs to, titled for the taskbar", () => {
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <SidebarConversationCard
          conversation={conversation}
          isSelected={false}
          timeLabel="5m"
          onSelect={noop}
          onRename={async () => {}}
          onDelete={async () => {}}
          onStatusChange={async () => {}}
        />
      </NextIntlClientProvider>
    )

    fireEvent.contextMenu(screen.getByText("Fix the parser"))
    fireEvent.click(
      screen.getByText(enMessages.Folder.conversationCard.openInNewWindow)
    )

    expect(openConversationWindow).toHaveBeenCalledWith(
      { folderId: 4, conversationId: 7, agentType: "claude_code" },
      "Fix the parser"
    )
  })
})
