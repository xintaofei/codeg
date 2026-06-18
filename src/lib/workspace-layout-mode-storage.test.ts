import { beforeEach, describe, expect, it } from "vitest"
import { loadLayoutMode, saveLayoutMode } from "./workspace-layout-mode-storage"

describe("workspace-layout-mode-storage", () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it("falls back to fusion when storage is missing or invalid", () => {
    expect(loadLayoutMode()).toBe("fusion")

    localStorage.setItem("workspace:layout-mode", "unexpected")

    expect(loadLayoutMode()).toBe("fusion")
  })

  it("round-trips saved layout mode values", () => {
    saveLayoutMode("files")
    expect(loadLayoutMode()).toBe("files")

    saveLayoutMode("fusion")
    expect(loadLayoutMode()).toBe("fusion")
  })
})
