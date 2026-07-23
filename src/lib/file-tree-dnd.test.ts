import { describe, expect, it } from "vitest"

import {
  FILE_TREE_DND_MIME,
  FILE_TREE_DROP_COMPOSER_ATTR,
  FILE_TREE_DROP_DIR_ATTR,
  hasFileTreeDragType,
  readFileTreeDragPayload,
  resolveFileTreeDropZone,
  writeFileTreeDragData,
  type FileTreeDragPayload,
} from "./file-tree-dnd"

/** Minimal in-memory stand-in for `DataTransfer` (jsdom's is not faithful). */
class MockDataTransfer {
  private store = new Map<string, string>()
  setData(format: string, data: string) {
    this.store.set(format.toLowerCase(), data)
  }
  getData(format: string) {
    return this.store.get(format.toLowerCase()) ?? ""
  }
  get types(): ReadonlyArray<string> {
    return Array.from(this.store.keys())
  }
}

const payload: FileTreeDragPayload = {
  rootPath: "/repo",
  relPath: "src/app.ts",
  absPath: "/repo/src/app.ts",
  name: "app.ts",
  kind: "file",
}

describe("file-tree drag payload codec", () => {
  it("round-trips a payload through write/read", () => {
    const dt = new MockDataTransfer()
    writeFileTreeDragData(dt, payload)
    expect(readFileTreeDragPayload(dt)).toEqual(payload)
  })

  it("also writes a text/plain absolute-path fallback", () => {
    const dt = new MockDataTransfer()
    writeFileTreeDragData(dt, payload)
    expect(dt.getData("text/plain")).toBe("/repo/src/app.ts")
  })

  it("detects the drag type from `types` (dragover-safe)", () => {
    const dt = new MockDataTransfer()
    writeFileTreeDragData(dt, payload)
    expect(hasFileTreeDragType(dt)).toBe(true)
  })

  it("reports no drag type for an OS file drop", () => {
    const osDrop = { types: ["Files"] as ReadonlyArray<string> }
    expect(hasFileTreeDragType(osDrop)).toBe(false)
  })

  it("returns null for a nullish transfer or missing payload", () => {
    expect(readFileTreeDragPayload(null)).toBeNull()
    expect(hasFileTreeDragType(null)).toBe(false)
    expect(readFileTreeDragPayload(new MockDataTransfer())).toBeNull()
  })

  it("returns null for malformed JSON", () => {
    const dt = new MockDataTransfer()
    dt.setData(FILE_TREE_DND_MIME, "{not json")
    expect(readFileTreeDragPayload(dt)).toBeNull()
  })

  it("returns null when the shape is wrong (bad kind)", () => {
    const dt = new MockDataTransfer()
    dt.setData(
      FILE_TREE_DND_MIME,
      JSON.stringify({ ...payload, kind: "symlink" })
    )
    expect(readFileTreeDragPayload(dt)).toBeNull()
  })

  it("carries directory payloads", () => {
    const dir: FileTreeDragPayload = {
      rootPath: "/repo",
      relPath: "src",
      absPath: "/repo/src",
      name: "src",
      kind: "dir",
    }
    const dt = new MockDataTransfer()
    writeFileTreeDragData(dt, dir)
    expect(readFileTreeDragPayload(dt)).toEqual(dir)
  })
})

describe("resolveFileTreeDropZone", () => {
  it("returns null for a nullish element or an unmarked subtree", () => {
    expect(resolveFileTreeDropZone(null)).toBeNull()
    const div = document.createElement("div")
    div.innerHTML = `<span><em>leaf</em></span>`
    expect(resolveFileTreeDropZone(div.querySelector("em"))).toBeNull()
  })

  it("resolves a directory zone from a descendant of the marked row", () => {
    const row = document.createElement("button")
    row.setAttribute(FILE_TREE_DROP_DIR_ATTR, "src/components")
    row.innerHTML = `<span class="icon"></span><span class="name">components</span>`
    expect(resolveFileTreeDropZone(row.querySelector(".name"))).toEqual({
      kind: "dir",
      destDir: "src/components",
    })
  })

  it("treats an empty dir attribute as the workspace root", () => {
    const row = document.createElement("button")
    row.setAttribute(FILE_TREE_DROP_DIR_ATTR, "")
    expect(resolveFileTreeDropZone(row)).toEqual({ kind: "dir", destDir: "" })
  })

  it("resolves a composer zone and carries its tab id", () => {
    const composer = document.createElement("div")
    composer.setAttribute(FILE_TREE_DROP_COMPOSER_ATTR, "tab-42")
    composer.innerHTML = `<div class="editor"><p>x</p></div>`
    expect(resolveFileTreeDropZone(composer.querySelector("p"))).toEqual({
      kind: "composer",
      tabId: "tab-42",
    })
  })

  it("ignores a composer zone with an empty tab id", () => {
    const composer = document.createElement("div")
    composer.setAttribute(FILE_TREE_DROP_COMPOSER_ATTR, "")
    expect(resolveFileTreeDropZone(composer)).toBeNull()
  })

  it("prefers the deeper zone when a dir row is nested in a composer", () => {
    const composer = document.createElement("div")
    composer.setAttribute(FILE_TREE_DROP_COMPOSER_ATTR, "tab-1")
    const nestedDir = document.createElement("button")
    nestedDir.setAttribute(FILE_TREE_DROP_DIR_ATTR, "pkg")
    composer.append(nestedDir)
    expect(resolveFileTreeDropZone(nestedDir)).toEqual({
      kind: "dir",
      destDir: "pkg",
    })
  })
})
