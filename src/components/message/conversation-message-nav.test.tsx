import { describe, it, expect, vi, beforeEach } from "vitest"
import { render, screen, fireEvent } from "@testing-library/react"
import type { RefObject } from "react"
import {
  ConversationMessageNav,
  type MessageNavEntry,
} from "./conversation-message-nav"
import type { MessageScrollContextValue } from "./message-scroll-context"

// Stable `t` (per next-intl mock guidance) — interpolates {label} so marker
// aria-labels stay distinguishable.
const { stableT, mockOpenDiff } = vi.hoisted(() => {
  const t = (key: string, params?: Record<string, unknown>) =>
    params && "label" in params ? `${key}:${String(params.label)}` : key
  return { stableT: t, mockOpenDiff: vi.fn() }
})

vi.mock("next-intl", () => ({ useTranslations: () => stableT }))
vi.mock("@/contexts/workspace-context", () => ({
  useWorkspaceContext: () => ({ openSessionFileDiff: mockOpenDiff }),
}))
vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({ activeFolder: { path: "/repo" } }),
}))

const STORAGE_KEY = "workspace:message-nav"

const DELETION_DIFF = "*** Delete File: old.ts\n-a\n-b\n-c\n-d"

const entries: MessageNavEntry[] = [
  {
    threadIndex: 0,
    turnId: "u1",
    ordinal: 1,
    label: "first message",
    additions: 0,
    deletions: 0,
    files: [],
    hasChanges: false,
  },
  {
    // One-sided change: additions only — the -0 must still render.
    threadIndex: 3,
    turnId: "u2",
    ordinal: 2,
    label: "edit something",
    additions: 5,
    deletions: 0,
    files: [
      {
        id: "f1",
        path: "/repo/src/a.ts",
        additions: 5,
        deletions: 0,
        diff: "diff --git a/src/a.ts b/src/a.ts\n@@ -0,0 +1 @@\n+new",
      },
    ],
    hasChanges: true,
  },
  {
    // Deleted file — must remain clickable to open its deletion diff.
    threadIndex: 6,
    turnId: "u3",
    ordinal: 3,
    label: "delete a file",
    additions: 0,
    deletions: 4,
    files: [
      {
        id: "f2",
        path: "/repo/old.ts",
        additions: 0,
        deletions: 4,
        diff: DELETION_DIFF,
      },
    ],
    hasChanges: true,
  },
]

function renderNav() {
  const scrollToIndex = vi.fn()
  const scrollApiRef = {
    current: { scrollToIndex },
  } as RefObject<MessageScrollContextValue | null>
  render(
    <ConversationMessageNav
      entries={entries}
      scrollApiRef={scrollApiRef}
      activeThreadIndex={null}
    />
  )
  return { scrollToIndex }
}

describe("ConversationMessageNav", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    window.localStorage.clear()
  })

  it("renders one marker per user message, placeholders included", () => {
    renderNav()
    const markers = screen.getAllByRole("button", { name: /^jumpToMessage:/ })
    expect(markers).toHaveLength(3)
  })

  it("scrolls to the message when a marker is clicked", () => {
    const { scrollToIndex } = renderNav()
    fireEvent.click(
      screen.getByRole("button", { name: "jumpToMessage:edit something" })
    )
    expect(scrollToIndex).toHaveBeenCalledWith(3, {
      align: "start",
      smooth: true,
    })
  })

  it("optimistically activates the clicked entry before scrolling", () => {
    const onActivate = vi.fn()
    const scrollToIndex = vi.fn()
    const scrollApiRef = {
      current: { scrollToIndex },
    } as RefObject<MessageScrollContextValue | null>
    render(
      <ConversationMessageNav
        entries={entries}
        scrollApiRef={scrollApiRef}
        activeThreadIndex={null}
        onActivate={onActivate}
      />
    )
    fireEvent.click(
      screen.getByRole("button", { name: "jumpToMessage:edit something" })
    )
    // The parent gets the clicked threadIndex synchronously so it can highlight
    // the tick before the (possibly clamped) smooth scroll settles.
    expect(onActivate).toHaveBeenCalledWith(3)
    expect(scrollToIndex).toHaveBeenCalledWith(3, {
      align: "start",
      smooth: true,
    })
  })

  it("always shows both +N and -N, including a zero side", () => {
    renderNav()
    fireEvent.click(screen.getByRole("button", { name: "expand" }))
    // u2 added 5, deleted 0 — both rendered.
    expect(screen.getByText("+5")).toBeInTheDocument()
    expect(screen.getByText("-0")).toBeInTheDocument()
    // u3 added 0, deleted 4 — both rendered.
    expect(screen.getByText("+0")).toBeInTheDocument()
    expect(screen.getByText("-4")).toBeInTheDocument()
    // Placeholder message still renders (its label) but no "no changes" text.
    expect(screen.getByText("first message")).toBeInTheDocument()
    expect(screen.queryByText("noChanges")).not.toBeInTheDocument()
  })

  it("opens a file diff when a changed file is clicked", () => {
    renderNav()
    fireEvent.click(screen.getByRole("button", { name: "expand" }))
    // Expand u2's file list (first changed group).
    fireEvent.click(screen.getAllByRole("button", { name: "fileCount" })[0])
    fireEvent.click(screen.getByTitle("src/a.ts"))
    expect(mockOpenDiff).toHaveBeenCalledWith(
      "/repo/src/a.ts",
      "diff --git a/src/a.ts b/src/a.ts\n@@ -0,0 +1 @@\n+new",
      "msg-2-chg-1"
    )
  })

  it("opens the deletion diff when a deleted file is clicked", () => {
    renderNav()
    fireEvent.click(screen.getByRole("button", { name: "expand" }))
    // Expand u3's file list (second changed group).
    fireEvent.click(screen.getAllByRole("button", { name: "fileCount" })[1])
    fireEvent.click(screen.getByTitle("old.ts"))
    expect(mockOpenDiff).toHaveBeenCalledWith(
      "/repo/old.ts",
      DELETION_DIFF,
      "msg-3-chg-1"
    )
  })

  it("hydrates the expanded state from localStorage after mount", async () => {
    window.localStorage.setItem(STORAGE_KEY, "1")
    renderNav()
    // The popout (its header title) appears once the hydrate effect runs.
    expect(await screen.findByText("title")).toBeInTheDocument()
  })
})
