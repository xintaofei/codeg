import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  UnifiedDiffPreview,
  toSplitRows,
  type ParsedDiffRow,
} from "./unified-diff-preview"
import enMessages from "@/i18n/messages/en.json"

// The component reads the active folder only to strip a path prefix from the
// file header; a null folder is enough for these tests.
vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({ activeFolder: null }),
}))

function Wrapper({ children }: { children: React.ReactNode }) {
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {children}
    </NextIntlClientProvider>
  )
}

// `wrapper` so `rerender` re-applies the intl provider automatically.
function renderWithIntl(ui: React.ReactElement) {
  return render(ui, { wrapper: Wrapper })
}

/** A single-file "add" diff with `lineCount` added rows. */
function newFileDiff(lineCount: number): string {
  const header = `diff --git a/big.txt b/big.txt\n--- /dev/null\n+++ b/big.txt\n@@ -0,0 +1,${lineCount} @@\n`
  const body = Array.from(
    { length: lineCount },
    (_, i) => `+line ${i + 1}`
  ).join("\n")
  return `${header}${body}\n`
}

describe("UnifiedDiffPreview", () => {
  it("renders every row for a small diff and shows no reveal control", () => {
    renderWithIntl(<UnifiedDiffPreview diffText={newFileDiff(3)} />)

    expect(screen.getByText("line 1")).toBeInTheDocument()
    expect(screen.getByText("line 3")).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /more lines/ })
    ).not.toBeInTheDocument()
  })

  it("caps a large diff at 500 rows and offers to reveal the rest", () => {
    renderWithIntl(<UnifiedDiffPreview diffText={newFileDiff(600)} />)

    // The first 500 rows render; rows past the cap do not.
    expect(screen.getByText("line 500")).toBeInTheDocument()
    expect(screen.queryByText("line 501")).not.toBeInTheDocument()
    expect(screen.queryByText("line 600")).not.toBeInTheDocument()

    // The reveal control reports exactly the hidden count (600 - 500).
    expect(
      screen.getByRole("button", { name: "Show 100 more lines" })
    ).toBeInTheDocument()
  })

  it("reveals all rows and drops the control once expanded", () => {
    renderWithIntl(<UnifiedDiffPreview diffText={newFileDiff(600)} />)

    fireEvent.click(screen.getByRole("button", { name: "Show 100 more lines" }))

    expect(screen.getByText("line 600")).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /more lines/ })
    ).not.toBeInTheDocument()
  })

  it("re-caps when the same preview receives a different large diff", () => {
    const { rerender } = renderWithIntl(
      <UnifiedDiffPreview diffText={newFileDiff(600)} />
    )
    // Fully expand the first diff.
    fireEvent.click(screen.getByRole("button", { name: "Show 100 more lines" }))
    expect(screen.getByText("line 600")).toBeInTheDocument()

    // A different, larger diff arrives at the same file position. Sections are
    // keyed positionally, so the instance is reused — the reveal must reset so
    // the new diff is capped again rather than rendered in full.
    rerender(<UnifiedDiffPreview diffText={newFileDiff(700)} />)

    expect(
      screen.getByRole("button", { name: "Show 200 more lines" })
    ).toBeInTheDocument()
    expect(screen.getByText("line 500")).toBeInTheDocument()
    expect(screen.queryByText("line 700")).not.toBeInTheDocument()
  })
})

/** A single-file modified diff: one context row, two deletions, one addition,
 *  one more context row — exercises the split pairing with unequal runs. */
function modifiedDiff(): string {
  return [
    "diff --git a/app.ts b/app.ts",
    "--- a/app.ts",
    "+++ b/app.ts",
    "@@ -1,5 1,4 @@",
    " keep one",
    "-old line A",
    "-old line B",
    "+new line A",
    " keep two",
  ].join("\n")
}

/** A second modified file, for asserting that two previews on one screen share
 *  the view-mode preference. */
function otherModifiedDiff(): string {
  return [
    "diff --git a/other.ts b/other.ts",
    "--- a/other.ts",
    "+++ b/other.ts",
    "@@ -1,2 +1,2 @@",
    " shared context",
    "-was here",
    "+is here",
  ].join("\n")
}

/** The unified rows `parseUnifiedDiff` derives from `modifiedDiff`, so the
 *  pairing unit tests feed the exact shape production renders. */
function modifiedRows(): ParsedDiffRow[] {
  return [
    { type: "context", text: "keep one", sign: " ", oldLine: 1, newLine: 1 },
    {
      type: "deleted",
      text: "old line A",
      sign: "-",
      oldLine: 2,
      newLine: null,
    },
    {
      type: "deleted",
      text: "old line B",
      sign: "-",
      oldLine: 3,
      newLine: null,
    },
    { type: "added", text: "new line A", sign: "+", oldLine: null, newLine: 2 },
    { type: "context", text: "keep two", sign: " ", oldLine: 4, newLine: 3 },
  ]
}

describe("UnifiedDiffPreview — side-by-side view", () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it("renders unified signs by default and keeps the split mode opt-in", () => {
    renderWithIntl(<UnifiedDiffPreview diffText={modifiedDiff()} />)

    expect(screen.getAllByText("+")).toHaveLength(1)
    expect(screen.getAllByText("-")).toHaveLength(2)
  })

  it("reads a persisted split preference before the first render", () => {
    localStorage.setItem("workspace:diff-view-mode", "split")
    renderWithIntl(<UnifiedDiffPreview diffText={modifiedDiff()} />)

    expect(screen.queryByText("+")).not.toBeInTheDocument()
    // Context rows now render once per side.
    expect(screen.getAllByText("keep one")).toHaveLength(2)
  })

  it("toggles to split, renders both sides, and persists the choice", () => {
    renderWithIntl(<UnifiedDiffPreview diffText={modifiedDiff()} />)

    fireEvent.click(
      screen.getByRole("button", { name: "Switch to side-by-side view" })
    )

    // No unified sign column anymore; both sides' texts are on screen.
    expect(screen.queryByText("+")).not.toBeInTheDocument()
    expect(screen.getAllByText("keep one")).toHaveLength(2)
    expect(screen.getByText("old line A")).toBeInTheDocument()
    expect(screen.getByText("new line A")).toBeInTheDocument()
    expect(localStorage.getItem("workspace:diff-view-mode")).toBe("split")
  })

  it("toggles back to unified and persists it", () => {
    localStorage.setItem("workspace:diff-view-mode", "split")
    renderWithIntl(<UnifiedDiffPreview diffText={modifiedDiff()} />)

    fireEvent.click(
      screen.getByRole("button", { name: "Switch to inline view" })
    )

    expect(screen.getByText("+")).toBeInTheDocument()
    expect(localStorage.getItem("workspace:diff-view-mode")).toBe("unified")
  })

  it("flips every preview mounted alongside it, not just the clicked one", () => {
    // A transcript renders one preview per Edit/Write tool call, and a
    // permission dialog stacks another on top. The view mode is a single
    // preference, so toggling one must not leave its siblings inline until
    // they happen to remount.
    renderWithIntl(
      <>
        <UnifiedDiffPreview diffText={modifiedDiff()} />
        <UnifiedDiffPreview diffText={otherModifiedDiff()} />
      </>
    )

    fireEvent.click(
      screen.getAllByRole("button", { name: "Switch to side-by-side view" })[0]!
    )

    // Context rows render once per side in split mode — both previews switched.
    expect(screen.getAllByText("keep one")).toHaveLength(2)
    expect(screen.getAllByText("shared context")).toHaveLength(2)
  })

  it("lays the split rows out on one grid sized to its content", () => {
    // jsdom does no layout, so this pins the two declarations that ARE the
    // behaviour. Both were verified in Chromium and WebKit: with per-row flex
    // boxes (or a grid without `w-max`) the container resolves to the tracks'
    // minimum size, `1fr 1fr` halves it, and the longer side of a row paints
    // its line over the opposite column — two lines of code on top of each
    // other. Shared tracks give every row the same break point; `w-max` keeps
    // each track wide enough for its widest line.
    localStorage.setItem("workspace:diff-view-mode", "split")
    const { container } = renderWithIntl(
      <UnifiedDiffPreview diffText={modifiedDiff()} />
    )

    const grid = container.querySelector('[class*="grid-cols-"]')
    expect(grid).not.toBeNull()
    expect(grid).toHaveClass("w-max")
    expect(grid).toHaveClass("grid-cols-[repeat(2,minmax(260px,1fr))]")
    // Cells must not size themselves — the track owns the width.
    expect(grid?.firstElementChild?.className).not.toMatch(/flex-1|basis-0/)
  })

  it("still switches when persisting the choice throws", () => {
    // Storage disabled / over quota: the mode won't survive a reload, but the
    // button must not look dead. The broadcast carries the new mode instead of
    // making every listener re-read what was never written.
    const setItem = vi
      .spyOn(Storage.prototype, "setItem")
      .mockImplementation(() => {
        throw new Error("QuotaExceededError")
      })
    try {
      renderWithIntl(<UnifiedDiffPreview diffText={modifiedDiff()} />)

      fireEvent.click(
        screen.getByRole("button", { name: "Switch to side-by-side view" })
      )

      expect(screen.getAllByText("keep one")).toHaveLength(2)
    } finally {
      setItem.mockRestore()
    }
  })

  it("hides the toggle for a diff that has no side to split", () => {
    // A pure new-file diff renders as plain content in both modes, so the
    // control would be a visible no-op.
    renderWithIntl(<UnifiedDiffPreview diffText={newFileDiff(3)} />)

    expect(
      screen.queryByRole("button", { name: "Switch to side-by-side view" })
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Switch to inline view" })
    ).not.toBeInTheDocument()
  })

  it("keeps the toggle when a new file is mixed with a modified one", () => {
    renderWithIntl(
      <UnifiedDiffPreview diffText={`${newFileDiff(2)}\n${modifiedDiff()}`} />
    )

    expect(
      screen.getByRole("button", { name: "Switch to side-by-side view" })
    ).toBeInTheDocument()
  })
})

describe("toSplitRows", () => {
  it("spans context rows across both sides", () => {
    const [row] = toSplitRows(modifiedRows().slice(0, 1))

    expect(row?.left).toEqual({
      line: 1,
      text: "keep one",
      marker: "none",
    })
    expect(row?.right).toEqual({
      line: 1,
      text: "keep one",
      marker: "none",
    })
  })

  it("pairs a delete-run with its add-run positionally", () => {
    const rows = toSplitRows(modifiedRows().slice(1, 4))

    expect(rows).toHaveLength(2)
    // First pair: old A faces new A.
    expect(rows[0]?.left).toMatchObject({
      text: "old line A",
      marker: "deleted",
    })
    expect(rows[0]?.right).toMatchObject({
      text: "new line A",
      marker: "added",
    })
    // The longer delete run leaves a filler cell on the right.
    expect(rows[1]?.left).toMatchObject({ text: "old line B" })
    expect(rows[1]?.right).toEqual({
      line: null,
      text: null,
      marker: "none",
    })
  })

  it("spans a 'modified' row instead of spinning on it", () => {
    // `ParsedDiffRow` has a fourth type the current classifier never emits.
    // Pairing must still consume it: a row that matches neither the delete run
    // nor the add run would leave the cursor parked and hang the tab in a
    // synchronous loop no test timeout can interrupt.
    const rows = toSplitRows([
      { type: "modified", text: "touched", sign: " ", oldLine: 7, newLine: 7 },
      { type: "added", text: "after", sign: "+", oldLine: null, newLine: 8 },
    ])

    expect(rows).toHaveLength(2)
    expect(rows[0]?.left).toMatchObject({ text: "touched", line: 7 })
    expect(rows[0]?.right).toMatchObject({ text: "touched", line: 7 })
    expect(rows[1]?.right).toMatchObject({ text: "after", marker: "added" })
  })

  it("gives a pure insertion an empty left cell", () => {
    const rows = toSplitRows([
      { type: "added", text: "fresh", sign: "+", oldLine: null, newLine: 5 },
    ])

    expect(rows).toHaveLength(1)
    expect(rows[0]?.left).toEqual({ line: null, text: null, marker: "none" })
    expect(rows[0]?.right).toMatchObject({ text: "fresh", line: 5 })
  })
})
