import { readFileSync } from "node:fs"
import { resolve } from "node:path"
import { describe, it, expect } from "vitest"
import { render } from "@testing-library/react"

const source = readFileSync(
  resolve(
    process.cwd(),
    "src/components/conversations/conversation-detail-panel.tsx"
  ),
  "utf8"
)

/**
 * The zero-remount invariant, proven behaviourally.
 *
 * A conversation view owns a live ACP connection and streaming state, so a
 * remount is destructive: the split feature is only correct if flipping into
 * and out of split leaves every surviving view's DOM node identity untouched.
 *
 * The conditional shapes `renderGroupShell` relies on are:
 *   1. TWO leading `{isSplit && …}` siblings (the group strip, then the
 *      group's conversation title bar) ahead of the unkeyed content wrapper
 *      (inside each shell), and
 *   2. a trailing `{isSplit && handles.map(...)}` sibling after the keyed shell
 *      array (inside the container).
 *
 * Both are safe: React's array reconciler tracks each child's slot index, and
 * a `false` slot is a hole rather than a shift, so the following child is still
 * matched at its own index (`oldFiber.index > newIdx` skips the holes instead
 * of pairing the content wrapper with a newly-appearing sibling). These tests
 * pin that down so a future refactor of the shell's child shape — e.g. wrapping
 * the trio in a conditional fragment, which WOULD shift slots — fails loudly
 * here.
 */
function Shell({ isSplit }: { isSplit: boolean }) {
  return (
    <div data-testid="shell">
      {isSplit && (
        <div data-testid="strip" className="flex h-10 shrink-0 items-stretch">
          strip
        </div>
      )}
      {isSplit && (
        <div data-testid="header" className="shrink-0">
          title bar
        </div>
      )}
      <div
        data-testid="content"
        className="relative min-h-0 flex-1 overflow-hidden"
      >
        <span data-testid="view">conversation view</span>
      </div>
    </div>
  )
}

function Container({
  groupIds,
  isSplit,
}: {
  groupIds: string[]
  isSplit: boolean
}) {
  return (
    <div className="relative min-h-0 flex-1 overflow-hidden">
      {groupIds.map((groupId) => (
        <div key={groupId} data-testid={`shell-${groupId}`}>
          <Shell isSplit={isSplit} />
        </div>
      ))}
      {isSplit &&
        ["s-1:0"].map((handle) => (
          <div key={handle} data-testid={`handle-${handle}`} />
        ))}
    </div>
  )
}

describe("split group shell reconciliation", () => {
  it("keeps the content subtree mounted when the strip + title bar appear and disappear", () => {
    const { rerender, getByTestId, queryByTestId } = render(
      <Shell isSplit={false} />
    )
    const content = getByTestId("content")
    const view = getByTestId("view")
    expect(queryByTestId("strip")).toBeNull()
    expect(queryByTestId("header")).toBeNull()

    // Split: the strip AND the group title bar are prepended.
    rerender(<Shell isSplit={true} />)
    expect(queryByTestId("strip")).not.toBeNull()
    expect(queryByTestId("header")).not.toBeNull()
    expect(getByTestId("content")).toBe(content)
    expect(getByTestId("view")).toBe(view)

    // Unsplit: both go away again.
    rerender(<Shell isSplit={false} />)
    expect(queryByTestId("strip")).toBeNull()
    expect(queryByTestId("header")).toBeNull()
    expect(getByTestId("content")).toBe(content)
    expect(getByTestId("view")).toBe(view)
  })

  it("keeps existing shells mounted when a group is added, removed, and dividers toggle", () => {
    const { rerender, getByTestId, queryByTestId } = render(
      <Container groupIds={["g-main"]} isSplit={false} />
    )
    const mainShell = getByTestId("shell-g-main")
    const mainView = getByTestId("view")

    // Split Right: a second group is appended AFTER the source group and the
    // divider overlay appears.
    rerender(<Container groupIds={["g-main", "g-2"]} isSplit={true} />)
    expect(getByTestId("shell-g-main")).toBe(mainShell)
    expect(getByTestId("shell-g-2")).toBeTruthy()
    expect(getByTestId("handle-s-1:0")).toBeTruthy()

    // A third group joins the same row (same-orientation flatten).
    rerender(<Container groupIds={["g-main", "g-2", "g-3"]} isSplit={true} />)
    expect(getByTestId("shell-g-main")).toBe(mainShell)

    // Unsplit All: back to one group, dividers gone.
    rerender(<Container groupIds={["g-main"]} isSplit={false} />)
    expect(getByTestId("shell-g-main")).toBe(mainShell)
    expect(getByTestId("view")).toBe(mainView)
    expect(queryByTestId("handle-s-1:0")).toBeNull()
  })
})

describe("split group shell source shape", () => {
  // Ties the mirrored components above to the real render path: if the shell's
  // children stop being [conditional strip, conditional title bar, content
  // wrapper] siblings, the behavioural proof above no longer describes
  // production.
  it("keeps strip, title bar, and content wrapper as plain sibling slots", () => {
    const shellStart = source.indexOf("const renderGroupShell = (groupId")
    expect(shellStart).toBeGreaterThan(-1)
    const shellBody = source.slice(shellStart, shellStart + 6000)
    const stripIdx = shellBody.indexOf("{isSplit && (")
    const headerIdx = shellBody.indexOf("{isSplit && selConversationTab && (")
    const contentIdx = shellBody.indexOf(
      '<div className="relative min-h-0 flex-1 overflow-hidden">'
    )
    expect(stripIdx).toBeGreaterThan(-1)
    expect(headerIdx).toBeGreaterThan(stripIdx)
    expect(contentIdx).toBeGreaterThan(headerIdx)
    // The per-group title bar lives between them.
    expect(shellBody.slice(headerIdx, contentIdx)).toContain(
      "<ConversationDetailHeader"
    )
    // No fragment/wrapper around the trio — that would make the flip shift
    // slots and remount the content subtree.
    expect(shellBody.slice(stripIdx, contentIdx)).not.toContain("<>")
  })
})
