import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"
import type { PipelineLineNote } from "@/lib/pipeline-notes"
import type { PipelineDiffFile } from "@/lib/types"
import { PipelineDiffFileTree } from "./pipeline-diff-file-tree"

describe("PipelineDiffFileTree", () => {
  const sampleFiles: PipelineDiffFile[] = [
    { path: "src/lib/utils.ts", status: "M", additions: 5, deletions: 2 },
    {
      path: "src/components/button.tsx",
      status: "A",
      additions: 40,
      deletions: 0,
    },
    { path: "src/old-file.ts", status: "D", additions: 0, deletions: 12 },
    { path: "docs/readme.md", status: "R", additions: 1, deletions: 1 },
  ]

  it("renders files with status badges and diff counts", () => {
    const handleSelect = vi.fn()
    render(
      <PipelineDiffFileTree
        files={sampleFiles}
        selectedPath="src/lib/utils.ts"
        onSelectFile={handleSelect}
      />
    )

    // Check filenames are rendered
    expect(screen.getByText("utils.ts")).toBeInTheDocument()
    expect(screen.getByText("button.tsx")).toBeInTheDocument()
    expect(screen.getByText("old-file.ts")).toBeInTheDocument()
    expect(screen.getByText("readme.md")).toBeInTheDocument()

    // Check stats are rendered in header
    expect(screen.getByText("4 files")).toBeInTheDocument()
    expect(screen.getByText("+46")).toBeInTheDocument()
    expect(screen.getByText("-15")).toBeInTheDocument()
  })

  it("calls onSelectFile when a file is clicked", () => {
    const handleSelect = vi.fn()
    render(
      <PipelineDiffFileTree
        files={sampleFiles}
        selectedPath={null}
        onSelectFile={handleSelect}
      />
    )

    const fileButton = screen.getByTestId(
      "file-tree-item-src/components/button.tsx"
    )
    fireEvent.click(fileButton)

    expect(handleSelect).toHaveBeenCalledWith("src/components/button.tsx")
  })

  it("renders note badges when notes are present", () => {
    const notes: PipelineLineNote[] = [
      { path: "src/lib/utils.ts", line: 10, note: "check this" },
      { path: "src/lib/utils.ts", line: 20, note: "and this" },
    ]

    render(
      <PipelineDiffFileTree
        files={sampleFiles}
        selectedPath={null}
        onSelectFile={vi.fn()}
        notes={notes}
      />
    )

    const badge = screen.getByTestId("file-note-badge-src/lib/utils.ts")
    expect(badge).toBeInTheDocument()
    expect(badge).toHaveTextContent("2")
  })

  it("filters files when search query is typed", () => {
    // 6 files to trigger search input display
    const manyFiles: PipelineDiffFile[] = [
      ...sampleFiles,
      { path: "src/api/client.ts", status: "M", additions: 10, deletions: 0 },
      { path: "src/api/auth.ts", status: "M", additions: 4, deletions: 1 },
    ]

    render(
      <PipelineDiffFileTree
        files={manyFiles}
        selectedPath={null}
        onSelectFile={vi.fn()}
      />
    )

    const searchInput = screen.getByPlaceholderText("Filter files...")
    fireEvent.change(searchInput, { target: { value: "button" } })

    expect(screen.getByText("button.tsx")).toBeInTheDocument()
    expect(screen.queryByText("utils.ts")).not.toBeInTheDocument()
  })

  it("renders 50+ files smoothly", () => {
    const fiftyFiles: PipelineDiffFile[] = Array.from(
      { length: 50 },
      (_, i) => ({
        path: `src/modules/module_${i}/file_${i}.ts`,
        status: (i % 4 === 0
          ? "A"
          : i % 4 === 1
            ? "M"
            : i % 4 === 2
              ? "D"
              : "R") as PipelineDiffFile["status"],
        additions: i + 1,
        deletions: i % 5,
      })
    )

    render(
      <PipelineDiffFileTree
        files={fiftyFiles}
        selectedPath="src/modules/module_0/file_0.ts"
        onSelectFile={vi.fn()}
      />
    )

    expect(screen.getByText("50 files")).toBeInTheDocument()
    expect(screen.getByText("file_0.ts")).toBeInTheDocument()
    expect(screen.getByText("file_49.ts")).toBeInTheDocument()
  })
})
