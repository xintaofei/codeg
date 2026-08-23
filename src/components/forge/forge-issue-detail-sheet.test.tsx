/**
 * The right-side detail panel the row's title now opens.
 *
 * What matters beyond plain rendering: the body goes through the Markdown
 * renderer rather than being printed as source, the panel shows EVERY label
 * (the row has to drop all but four), and the footer offers the same
 * three-state action the row does — with the way out to the forge kept as a
 * real link.
 */
import { cleanup, render, screen, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import type {
  ForgeIssueRow,
  ForgeLabel,
  ForgeTaskLink,
  WorkTaskStatus,
} from "@/lib/types"

import { ForgeIssueDetailSheet } from "./forge-issue-detail-sheet"

const setRoute = vi.fn()
vi.mock("@/contexts/workbench-route-context", () => ({
  useWorkbenchRoute: () => ({
    routeId: "forge",
    isConversations: false,
    setRoute,
    openConversations: vi.fn(),
  }),
}))
// The real one reaches the workspace context (link safety routes file links
// into the file panel), which this panel is mounted outside of. The stub keeps
// the assertion honest where it counts: it reports WHAT it was handed, so a
// panel that stopped sending the body through the renderer would fail.
vi.mock("@/components/ai-elements/message", () => ({
  MessageResponse: ({ children }: { children?: string }) => (
    <div data-testid="markdown">{children}</div>
  ),
}))

function label(name: string, color: string | null = null): ForgeLabel {
  return { name, color }
}

function row(overrides: Partial<ForgeIssueRow> = {}): ForgeIssueRow {
  return {
    number: 42,
    title: "Login times out",
    body: "## Steps\n\n1. Sign in",
    state: "open",
    draft: false,
    labels: [label("bug")],
    author: "octocat",
    updated_at: null,
    html_url: "https://github.com/o/r/issues/42",
    is_pr: false,
    comments: 0,
    ...overrides,
  }
}

function taskLink(status: WorkTaskStatus): ForgeTaskLink {
  return {
    source_key: "github:github.com/o/r/issue/42",
    task_id: 3,
    status,
    verdict: null,
    updated_at: "2026-08-19T00:00:00Z",
  }
}

function mount(
  item: ForgeIssueRow | null,
  link: ForgeTaskLink | null = null,
  handlers: { onOpenChange?: () => void; onStart?: () => void } = {}
) {
  const onOpenChange = handlers.onOpenChange ?? vi.fn()
  const onStart = handlers.onStart ?? vi.fn()
  render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <ForgeIssueDetailSheet
        row={item}
        link={link}
        onOpenChange={onOpenChange}
        onStart={onStart}
      />
    </NextIntlClientProvider>
  )
  return { onOpenChange, onStart }
}

beforeEach(() => {
  vi.clearAllMocks()
})

describe("ForgeIssueDetailSheet", () => {
  /** `null` is the closed state — the page clears the row to close. */
  it("renders nothing without an item", () => {
    mount(null)
    expect(screen.queryByText("Login times out")).not.toBeInTheDocument()
  })

  it("renders the item's body as Markdown, not as source", () => {
    mount(row())
    expect(screen.getByTestId("markdown")).toHaveTextContent("## Steps")
    expect(screen.queryByText("No description")).not.toBeInTheDocument()
  })

  /** An empty body must not leave the panel looking like it failed to load.
   *  Whitespace counts as empty — GitLab hands back "" for a description that
   *  was never written, GitHub `null`. */
  it.each([
    ["null", null],
    ["empty", ""],
    ["blank", "   \n  "],
  ])("says so when the body is %s", (_case, body) => {
    mount(row({ body }))
    expect(screen.getByText("No description")).toBeInTheDocument()
    expect(screen.queryByTestId("markdown")).not.toBeInTheDocument()
  })

  /** The row caps labels at four so the action stays on screen; the panel is
   *  where the dropped ones are finally readable. */
  it("shows every label, not the row's first four", () => {
    mount(row({ labels: ["a", "b", "c", "d", "e"].map((n) => label(n)) }))
    for (const name of ["a", "b", "c", "d", "e"]) {
      expect(screen.getByText(name)).toBeInTheDocument()
    }
  })

  /** The state is a glyph on the row, where a column of them reads at a
   *  glance. A single item has no column to compare against, so the panel
   *  spells the state out — and the glyph beside it becomes decoration, or a
   *  screen reader would say the word twice. */
  it("spells the state out beside the title", () => {
    mount(row({ is_pr: true, state: "merged" }))
    expect(screen.getByText("Merged")).toBeInTheDocument()
    expect(
      screen.queryByRole("img", { name: "Merged" })
    ).not.toBeInTheDocument()
  })

  it("keeps the forge one click away as a real link", () => {
    mount(row())
    const link = screen.getByRole("link", { name: "Open in browser" })
    expect(link).toHaveAttribute("href", "https://github.com/o/r/issues/42")
    expect(link).toHaveAttribute("target", "_blank")
  })

  it("offers Start when no task has ever handled the item", async () => {
    const user = userEvent.setup()
    const { onStart } = mount(row())
    await user.click(screen.getByRole("button", { name: "Start" }))
    expect(onStart).toHaveBeenCalledTimes(1)
    expect(setRoute).not.toHaveBeenCalled()
  })

  it("shows a live task's status chip, which goes to the board", async () => {
    const user = userEvent.setup()
    const { onStart } = mount(row(), taskLink("running"))
    expect(
      screen.queryByRole("button", { name: "Start" })
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "re-trigger" })
    ).not.toBeInTheDocument()

    await user.click(screen.getByRole("button", { name: "Running" }))
    expect(setRoute).toHaveBeenCalledWith("tasks")
    expect(onStart).not.toHaveBeenCalled()
  })

  /** Same rule as the row: siblings, never nested — a control inside a button
   *  folds its text into that button's accessible name. */
  it("keeps the chip and the re-trigger as separate controls once settled", async () => {
    const user = userEvent.setup()
    const { onStart } = mount(row(), taskLink("done"))
    const chip = screen.getByRole("button", { name: "Done" })
    const retrigger = screen.getByRole("button", { name: "re-trigger" })
    expect(chip).not.toContainElement(retrigger)

    await user.click(retrigger)
    expect(onStart).toHaveBeenCalledTimes(1)
    expect(setRoute).not.toHaveBeenCalled()
  })

  /** The count is the panel's only word about a discussion it does not carry
   *  (comments are not in the list payload), so it has to be there when there
   *  is one — and absent, not zero, when there is not. */
  it("reports the comment count only when there is a discussion", () => {
    mount(row({ comments: 7 }))
    expect(screen.getByText("7 comments")).toBeInTheDocument()

    cleanup()
    mount(row({ comments: 0 }))
    expect(screen.queryByText(/comments/)).not.toBeInTheDocument()
  })

  /** The page owns the open state (it holds the row), so every exit has to
   *  travel back out through `onOpenChange` — a panel that only closed itself
   *  internally would leave the page thinking it was still open. `close-press`
   *  rather than `anything()`: the drawer wrapper cancels ambient dismissals,
   *  so only that reason proves the button is really wired. */
  it("asks the page to close from the close button and from Escape", async () => {
    const user = userEvent.setup()
    const { onOpenChange } = mount(row())
    await user.click(screen.getByRole("button", { name: "Close" }))
    expect(onOpenChange).toHaveBeenCalledWith(
      false,
      expect.objectContaining({ reason: "close-press" })
    )

    cleanup()
    const second = mount(row())
    await user.keyboard("{Escape}")
    expect(second.onOpenChange).toHaveBeenCalledWith(
      false,
      expect.objectContaining({ reason: "escape-key" })
    )
  })

  /** The identity line under the title: the number, who opened it, nothing the
   *  reader has to go looking for elsewhere. */
  it("identifies the item under its title", () => {
    mount(row({ updated_at: "2026-08-20T00:00:00Z" }))
    const title = screen.getByText("Login times out")
    const header = title.closest("[data-slot='drawer-header']")
    expect(header).not.toBeNull()
    expect(within(header as HTMLElement).getByText("· #42")).toBeInTheDocument()
    expect(
      within(header as HTMLElement).getByText("· octocat")
    ).toBeInTheDocument()
    expect(
      within(header as HTMLElement).getByText(/updated/)
    ).toBeInTheDocument()
  })
})
