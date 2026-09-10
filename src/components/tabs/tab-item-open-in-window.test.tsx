import { Reorder } from "motion/react"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, describe, expect, it, vi } from "vitest"

import { TabItem } from "./tab-item"
import type { TabItem as TabItemData } from "@/contexts/tab-context"
import enMessages from "@/i18n/messages/en.json"

const tab: TabItemData = {
  id: "conv-4-claude_code-7",
  kind: "conversation",
  folderId: 4,
  conversationId: 7,
  agentType: "claude_code",
  title: "Fix the parser",
  isPinned: true,
}

const noop = () => {}

function renderTab(onOpenInNewWindow?: (tab: TabItemData) => void) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <Reorder.Group as="div" axis="x" values={[tab]} onReorder={noop}>
        <TabItem
          tab={tab}
          isActive
          isTileMode={false}
          folderName="repo"
          folderBranch={null}
          isSplit={false}
          canSplitMove={false}
          canMoveToGroup={false}
          moveTargets={[]}
          onOpenInNewWindow={onOpenInNewWindow}
          onSwitch={noop}
          onClose={noop}
          onCloseOthers={noop}
          onCloseAll={noop}
          onPin={noop}
          onToggleTile={noop}
          onSplit={noop}
          onMoveToGroup={noop}
          onToggleSplitOrientation={noop}
          onUnsplit={noop}
          onUnsplitAll={noop}
          isCoarsePointer={false}
          isTouchSorting={false}
          onTouchSortingStart={noop}
          onTouchSortingEnd={noop}
        />
      </Reorder.Group>
    </NextIntlClientProvider>
  )
}

afterEach(cleanup)

describe("TabItem open-in-new-window", () => {
  it("hands the whole tab to the caller, which is what identifies the window", () => {
    const onOpenInNewWindow = vi.fn()
    renderTab(onOpenInNewWindow)

    fireEvent.contextMenu(screen.getByText("Fix the parser"))
    fireEvent.click(screen.getByText(enMessages.Folder.tabs.openInNewWindow))

    expect(onOpenInNewWindow).toHaveBeenCalledTimes(1)
    expect(onOpenInNewWindow).toHaveBeenCalledWith(tab)
  })

  // Drafts pass no handler (see tab-bar): an unsent draft has no conversation
  // for a window to show, and an inert menu item reads as broken.
  it("is absent when the strip withholds the handler", () => {
    renderTab(undefined)

    fireEvent.contextMenu(screen.getByText("Fix the parser"))

    expect(
      screen.queryByText(enMessages.Folder.tabs.openInNewWindow)
    ).toBeNull()
    // The rest of the menu is untouched.
    expect(screen.getByText(enMessages.Folder.tabs.splitRight)).toBeTruthy()
    expect(screen.getByText(enMessages.Folder.tabs.closeAll)).toBeTruthy()
  })
})
