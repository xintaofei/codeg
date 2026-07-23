import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { FileTree, FileTreeFile, FileTreeFolder } from "./file-tree"

function renderTree(keyboardNavigation: boolean) {
  return render(
    <FileTree
      keyboardNavigation={keyboardNavigation}
      expanded={new Set(["dir"])}
      selectedPath="dir"
    >
      <FileTreeFolder path="dir" name="dir" depth={0}>
        <FileTreeFile path="dir/file.ts" name="file.ts" depth={1} />
      </FileTreeFolder>
    </FileTree>
  )
}

describe("FileTree keyboard focus topology", () => {
  it("collapses the opted-in tree to a single tab stop: only the container", () => {
    renderTree(true)

    const container = screen.getByRole("tree")
    const [folderItem, fileItem] = screen.getAllByRole("treeitem")
    const folderButton = screen.getByRole("button", { name: "dir" })

    // The container is the sole focus host...
    expect(container.tabIndex).toBe(0)
    // ...and every row — including the folder's native <button> header, which
    // would otherwise remain a default tab stop — is out of the tab order.
    expect(folderItem.tabIndex).toBe(-1)
    expect(folderButton.tabIndex).toBe(-1)
    expect(fileItem.tabIndex).toBe(-1)
  })

  it("points aria-activedescendant at the selected row's id", () => {
    renderTree(true)

    const container = screen.getByRole("tree")
    const [folderItem] = screen.getAllByRole("treeitem")

    expect(folderItem.id).toBeTruthy()
    expect(container).toHaveAttribute("aria-activedescendant", folderItem.id)
  })

  it("leaves per-row tab stops unchanged when keyboard navigation is off", () => {
    renderTree(false)

    const container = screen.getByRole("tree")
    const [folderItem, fileItem] = screen.getAllByRole("treeitem")
    const folderButton = screen.getByRole("button", { name: "dir" })

    // Unchanged legacy behavior: rows are individually focusable and the
    // container is not a virtual-focus host.
    expect(folderItem.tabIndex).toBe(0)
    expect(folderButton.tabIndex).toBe(0)
    expect(fileItem.tabIndex).toBe(0)
    expect(container).not.toHaveAttribute("aria-activedescendant")
  })
})
