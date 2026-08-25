import { render } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { ChunkLoadRecovery, isChunkLoadError } from "./chunk-load-recovery"

afterEach(() => {
  window.sessionStorage.clear()
})

describe("ChunkLoadRecovery", () => {
  it("recognizes async chunk failures without matching unrelated errors", () => {
    expect(
      isChunkLoadError(
        new Error(
          "Failed to load chunk http://localhost:3000/_next/static/chunks/opener.js"
        )
      )
    ).toBe(true)
    expect(isChunkLoadError(new Error("Permission denied"))).toBe(false)
  })

  it("reloads once when a lazy chunk is stale", () => {
    const reloadPage = vi.fn()
    render(<ChunkLoadRecovery reloadPage={reloadPage} />)

    const rejection = new Event("unhandledrejection")
    Object.defineProperty(rejection, "reason", {
      value: Object.assign(new Error("Failed to load chunk /opener.js"), {
        name: "ChunkLoadError",
      }),
    })
    window.dispatchEvent(rejection)
    window.dispatchEvent(rejection)

    expect(reloadPage).toHaveBeenCalledTimes(1)
  })
})
