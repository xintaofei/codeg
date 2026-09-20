import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import type { PipelineDiff } from "@/lib/types"
import {
  extractFilePatch,
  parseFileHunks,
  PipelineDiffPanel,
} from "./pipeline-diff-panel"

const messages = {
  Pipeline: {
    title: "Agent pipeline",
    sendToCoder: "Send to coder",
    fixMyself: "I will fix it myself",
    apply: "Apply changes",
    applySquash: "Squash merge",
    applyNoFf: "Merge commit",
    diffEmpty: "No changes yet",
    diffTruncated: "Diff truncated, open the worktree to see everything",
    commentPlaceholder: "Note for the coder about this line",
    addComment: "Add note",
    notesForCoder: "Notes for the coder",
  },
  Folder: {
    diffPreview: {
      noDiffData: "No diff data",
      binaryFile: "Binary file",
      viewMode: {
        split: "Side by side",
        unified: "Unified",
      },
      showRemainingLines: "Show remaining {count} lines",
    },
  },
}

function renderWithIntl(ui: React.ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={messages}>
      {ui}
    </NextIntlClientProvider>
  )
}

const sampleDiff: PipelineDiff = {
  files: [
    {
      path: "src/main.rs",
      status: "M",
      additions: 2,
      deletions: 1,
    },
    {
      path: "src/lib.rs",
      status: "A",
      additions: 10,
      deletions: 0,
    },
  ],
  patch: `diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!("hello");
+    let greeting = "hello world";
+    println!("{}", greeting);
 }
diff --git a/src/lib.rs b/src/lib.rs
--- /dev/null
+++ b/src/lib.rs
@@ -0,0 +1,2 @@
+pub fn run() {}
+pub fn stop() {}
`,
  truncated: false,
}

describe("PipelineDiffPanel helpers", () => {
  it("extractFilePatch extracts patch for specific file", () => {
    const mainPatch = extractFilePatch(sampleDiff.patch, "src/main.rs")
    expect(mainPatch).toContain("fn main()")
    expect(mainPatch).toContain('println!("{}", greeting);')
    expect(mainPatch).not.toContain("pub fn run()")

    const libPatch = extractFilePatch(sampleDiff.patch, "src/lib.rs")
    expect(libPatch).toContain("pub fn run()")
    expect(libPatch).not.toContain("fn main()")
  })

  it("parseFileHunks parses lines into context, added, and deleted", () => {
    const mainPatch = extractFilePatch(sampleDiff.patch, "src/main.rs")
    const hunks = parseFileHunks(mainPatch)
    expect(hunks).toHaveLength(1)
    expect(hunks[0]?.rows).toHaveLength(5)

    const rows = hunks[0]?.rows ?? []
    expect(rows[0]?.type).toBe("context")
    expect(rows[1]?.type).toBe("deleted")
    expect(rows[2]?.type).toBe("added")
    expect(rows[3]?.type).toBe("added")
    expect(rows[4]?.type).toBe("context")
  })
})

describe("PipelineDiffPanel", () => {
  it("renders panel with file tree and diff lines", () => {
    renderWithIntl(<PipelineDiffPanel diff={sampleDiff} />)

    expect(screen.getByText("Agent pipeline")).toBeInTheDocument()
    expect(screen.getByText("main.rs")).toBeInTheDocument()
    expect(screen.getByText("lib.rs")).toBeInTheDocument()
    expect(
      screen.getByText('let greeting = "hello world";')
    ).toBeInTheDocument()
  })

  it("shows empty message when diff has no files or patch", () => {
    const emptyDiff: PipelineDiff = {
      files: [],
      patch: "",
      truncated: false,
    }

    renderWithIntl(<PipelineDiffPanel diff={emptyDiff} />)

    expect(screen.getByTestId("diff-empty-message")).toHaveTextContent(
      "No changes yet"
    )
  })

  it("shows truncated banner when diff.truncated is true", () => {
    const truncatedDiff: PipelineDiff = {
      ...sampleDiff,
      truncated: true,
    }

    renderWithIntl(<PipelineDiffPanel diff={truncatedDiff} />)

    const banner = screen.getByTestId("diff-truncated-banner")
    expect(banner).toBeInTheDocument()
    expect(banner).toHaveTextContent(
      "Diff truncated, open the worktree to see everything"
    )
  })

  it("allows switching files via file tree", () => {
    renderWithIntl(<PipelineDiffPanel diff={sampleDiff} />)

    expect(
      screen.getByText('let greeting = "hello world";')
    ).toBeInTheDocument()

    // Click on lib.rs in file tree
    const libTreeItem = screen.getByTestId("file-tree-item-src/lib.rs")
    fireEvent.click(libTreeItem)

    expect(screen.getByText("pub fn run() {}")).toBeInTheDocument()
  })

  it("allows adding line comment and submits formatted notes to onRequestChanges", () => {
    const handleRequestChanges = vi.fn()
    const handleNotesChange = vi.fn()

    renderWithIntl(
      <PipelineDiffPanel
        diff={sampleDiff}
        onRequestChanges={handleRequestChanges}
        onNotesChange={handleNotesChange}
      />
    )

    // Find the comment button for row-3 of src/main.rs (line 2)
    const commentBtn = screen.getByTestId("line-comment-btn-row-3")
    fireEvent.click(commentBtn)

    // Check that textarea appears
    const input = screen.getByTestId("line-comment-input")
    fireEvent.change(input, {
      target: { value: "use formatted string instead" },
    })

    // Save the comment
    const saveBtn = screen.getByTestId("save-comment-btn")
    fireEvent.click(saveBtn)

    // Check notes summary bar and line note display
    expect(screen.getByTestId("notes-summary-bar")).toBeInTheDocument()
    expect(screen.getByTestId("notes-summary-bar")).toHaveTextContent(
      "src/main.rs:2"
    )
    expect(
      screen.getByTestId("line-note-display-src/main.rs-2")
    ).toBeInTheDocument()

    // Click "Send to coder"
    const sendBtn = screen.getByTestId("request-changes-button")
    fireEvent.click(sendBtn)

    expect(handleRequestChanges).toHaveBeenCalledWith(
      "src/main.rs:2: use formatted string instead"
    )
  })

  it("calls onStopManual when 'I will fix it myself' is clicked", () => {
    const handleStopManual = vi.fn()

    renderWithIntl(
      <PipelineDiffPanel diff={sampleDiff} onStopManual={handleStopManual} />
    )

    const stopBtn = screen.getByTestId("stop-manual-button")
    fireEvent.click(stopBtn)

    expect(handleStopManual).toHaveBeenCalledTimes(1)
  })

  it("calls onApply when Apply changes button is clicked", () => {
    const handleApply = vi.fn()

    renderWithIntl(
      <PipelineDiffPanel diff={sampleDiff} onApply={handleApply} />
    )

    const applyBtn = screen.getByTestId("apply-button")
    fireEvent.click(applyBtn)

    expect(handleApply).toHaveBeenCalledWith("squash")
  })
})
