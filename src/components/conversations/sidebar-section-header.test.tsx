import { type ReactElement } from "react"
import { fireEvent, render } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi, beforeEach } from "vitest"

import { SidebarSectionHeader } from "./sidebar-section-header"
import enMessages from "@/i18n/messages/en.json"

// The hover-reveal action buttons carry only an aria-label (icon, no text), so
// getByLabelText addresses them unambiguously. CSS hides them until hover, but
// fireEvent dispatches directly on the node regardless of pointer-events, so the
// wiring is testable without a real pointer.
const onToggle = vi.fn()
const onNewChat = vi.fn()
const onOpenFolder = vi.fn()
const onCloneRepository = vi.fn()
const onNewFolderGroup = vi.fn()

function renderWithIntl(ui: ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  onToggle.mockClear()
  onNewChat.mockClear()
  onOpenFolder.mockClear()
  onCloneRepository.mockClear()
  onNewFolderGroup.mockClear()
})

describe("SidebarSectionHeader folders-section actions", () => {
  it("renders Open Folder and Clone Repository buttons on the folders section", () => {
    const { getByLabelText } = renderWithIntl(
      <SidebarSectionHeader
        section="folders"
        expanded
        onToggle={onToggle}
        onOpenFolder={onOpenFolder}
        onCloneRepository={onCloneRepository}
      />
    )
    expect(getByLabelText("Open Folder")).not.toBeNull()
    expect(getByLabelText("Clone Repository")).not.toBeNull()
  })

  it("invokes the matching handler without toggling the section", () => {
    const { getByLabelText } = renderWithIntl(
      <SidebarSectionHeader
        section="folders"
        expanded
        onToggle={onToggle}
        onOpenFolder={onOpenFolder}
        onCloneRepository={onCloneRepository}
      />
    )

    fireEvent.click(getByLabelText("Open Folder"))
    expect(onOpenFolder).toHaveBeenCalledTimes(1)

    fireEvent.click(getByLabelText("Clone Repository"))
    expect(onCloneRepository).toHaveBeenCalledTimes(1)

    // The actions are siblings of the toggle button (never nested), so clicking
    // them never collapses/expands the section.
    expect(onToggle).not.toHaveBeenCalled()
  })

  it("renders only the actions whose callbacks are provided", () => {
    const { getByLabelText, queryByLabelText } = renderWithIntl(
      <SidebarSectionHeader
        section="folders"
        expanded
        onToggle={onToggle}
        onOpenFolder={onOpenFolder}
      />
    )
    expect(getByLabelText("Open Folder")).not.toBeNull()
    expect(queryByLabelText("Clone Repository")).toBeNull()
  })

  it("renders New group alongside the three add-a-folder actions", () => {
    const { getByLabelText } = renderWithIntl(
      <SidebarSectionHeader
        section="folders"
        expanded
        onToggle={onToggle}
        onOpenFolder={onOpenFolder}
        onCloneRepository={onCloneRepository}
        onNewFolderGroup={onNewFolderGroup}
      />
    )
    // Four hover actions now share this cluster; the sidebar's 300px minimum
    // leaves room, but all four must actually be present and addressable.
    expect(getByLabelText("New group")).not.toBeNull()
    expect(getByLabelText("Open Folder")).not.toBeNull()
    expect(getByLabelText("Clone Repository")).not.toBeNull()

    fireEvent.click(getByLabelText("New group"))
    expect(onNewFolderGroup).toHaveBeenCalledTimes(1)
    expect(onToggle).not.toHaveBeenCalled()
  })

  it("gates New group on the folders section, like the other folder actions", () => {
    const { queryByLabelText } = renderWithIntl(
      <SidebarSectionHeader
        section="chats"
        expanded
        onToggle={onToggle}
        onNewFolderGroup={onNewFolderGroup}
      />
    )
    expect(queryByLabelText("New group")).toBeNull()
  })

  it("still toggles the section when the header label is clicked", () => {
    const { getByText } = renderWithIntl(
      <SidebarSectionHeader
        section="folders"
        expanded
        onToggle={onToggle}
        onOpenFolder={onOpenFolder}
        onCloneRepository={onCloneRepository}
      />
    )
    fireEvent.click(getByText("Folders"))
    expect(onToggle).toHaveBeenCalledWith("folders")
  })
})

describe("SidebarSectionHeader action gating by section", () => {
  it("offers only New chat on the chats section, never the folder actions", () => {
    // Pass the folder callbacks too: they must be gated by `section`, not merely
    // by callback presence.
    const { getByLabelText, queryByLabelText } = renderWithIntl(
      <SidebarSectionHeader
        section="chats"
        expanded
        onToggle={onToggle}
        onNewChat={onNewChat}
        onOpenFolder={onOpenFolder}
        onCloneRepository={onCloneRepository}
      />
    )
    expect(getByLabelText("New chat")).not.toBeNull()
    expect(queryByLabelText("Open Folder")).toBeNull()
    expect(queryByLabelText("Clone Repository")).toBeNull()
  })

  it("offers no action buttons on the pinned section", () => {
    const { queryByLabelText } = renderWithIntl(
      <SidebarSectionHeader
        section="pinned"
        expanded
        onToggle={onToggle}
        onNewChat={onNewChat}
        onOpenFolder={onOpenFolder}
        onCloneRepository={onCloneRepository}
      />
    )
    expect(queryByLabelText("New chat")).toBeNull()
    expect(queryByLabelText("Open Folder")).toBeNull()
    expect(queryByLabelText("Clone Repository")).toBeNull()
  })
})
