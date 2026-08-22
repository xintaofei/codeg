import { describe, expect, it } from "vitest"

import {
  imageCardLabel,
  isImageGenerationTitle,
  pathFromToolInput,
} from "./image-tool-label"

describe("isImageGenerationTitle", () => {
  it("matches the hardcoded codex-acp title only", () => {
    expect(isImageGenerationTitle("Image generation")).toBe(true)
    expect(isImageGenerationTitle("  image generation  ")).toBe(true)
    expect(isImageGenerationTitle("Getting Started")).toBe(false)
    expect(isImageGenerationTitle("Read")).toBe(false)
    expect(isImageGenerationTitle("")).toBe(false)
    expect(isImageGenerationTitle(null)).toBe(false)
  })
})

describe("pathFromToolInput", () => {
  it("reads common path and url fields", () => {
    expect(
      pathFromToolInput(JSON.stringify({ file_path: "shots/page-capture.png" }))
    ).toBe("shots/page-capture.png")
    expect(
      pathFromToolInput(
        JSON.stringify({
          url: "https://example.com/docs/getting-started",
        })
      )
    ).toBe("https://example.com/docs/getting-started")
    expect(pathFromToolInput("not-json")).toBeNull()
  })
})

describe("imageCardLabel", () => {
  it("keeps a real tool or page title", () => {
    expect(imageCardLabel({ title: "Getting Started" })).toBe("Getting Started")
    expect(imageCardLabel({ title: "Getting Started | Example Docs" })).toBe(
      "Getting Started | Example Docs"
    )
  })

  it("does not treat the generation title as a label", () => {
    expect(imageCardLabel({ title: "Image generation" })).toBeNull()
  })

  it("falls back to a humanized filename or URL slug", () => {
    expect(
      imageCardLabel({
        title: "Image generation",
        input: JSON.stringify({ file_path: "page-capture.png" }),
      })
    ).toBe("Page Capture")
    expect(
      imageCardLabel({
        input: JSON.stringify({
          url: "https://example.com/docs/getting-started",
        }),
      })
    ).toBe("Getting Started")
  })

  it("uses the tool name when nothing else is available", () => {
    expect(imageCardLabel({ toolName: "Read" })).toBe("Read")
    expect(imageCardLabel({ toolName: "Image generation" })).toBeNull()
  })
})
