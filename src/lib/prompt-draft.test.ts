import { describe, expect, it } from "vitest"

import type { PromptDraft, PromptInputBlock } from "@/lib/types"

import {
  extractUserImagesFromDraft,
  extractUserResourcesFromDraft,
} from "./prompt-draft"

function draft(blocks: PromptInputBlock[]): PromptDraft {
  return { blocks, displayText: "" }
}

const grokImageResource: PromptInputBlock = {
  type: "resource",
  uri: "clipboard://image.png-abc",
  mime_type: "image/png",
  text: null,
  blob: "QUJD",
}

const textResource: PromptInputBlock = {
  type: "resource",
  uri: "clipboard://notes.txt",
  mime_type: "text/plain",
  text: "hi",
  blob: null,
}

describe("extractUserImagesFromDraft", () => {
  it("includes native image blocks", () => {
    const images = extractUserImagesFromDraft(
      draft([
        { type: "image", data: "QUJD", mime_type: "image/png", uri: null },
      ])
    )
    expect(images).toEqual([
      { name: "image.png", data: "QUJD", mime_type: "image/png", uri: null },
    ])
  })

  it("promotes an image-mime embedded resource to a thumbnail (Grok's encoding)", () => {
    const images = extractUserImagesFromDraft(draft([grokImageResource]))
    expect(images).toHaveLength(1)
    // Bytes come from `blob`, and the origin uri is preserved.
    expect(images[0]).toMatchObject({
      data: "QUJD",
      mime_type: "image/png",
      uri: "clipboard://image.png-abc",
    })
  })

  it("ignores a non-image embedded resource", () => {
    expect(extractUserImagesFromDraft(draft([textResource]))).toEqual([])
  })
})

describe("extractUserResourcesFromDraft", () => {
  it("excludes an image-mime embedded resource (it renders as a thumbnail)", () => {
    expect(extractUserResourcesFromDraft(draft([grokImageResource]))).toEqual(
      []
    )
  })

  it("keeps a non-image embedded resource as a chip", () => {
    expect(extractUserResourcesFromDraft(draft([textResource]))).toEqual([
      {
        name: "notes.txt",
        uri: "clipboard://notes.txt",
        mime_type: "text/plain",
      },
    ])
  })
})
