import { describe, expect, it } from "vitest"

import { stripUploadedImagePayloads } from "@/lib/api"
import type { PromptInputBlock } from "@/lib/types"

const UPLOADED_IMAGE: PromptInputBlock = {
  type: "image",
  data: "QkFTRTY0",
  mime_type: "image/png",
  uri: "file:///home/me/.codeg/uploads/ab/shot.png",
}

const PASTED_IMAGE_NO_URI: PromptInputBlock = {
  type: "image",
  data: "QkFTRTY0",
  mime_type: "image/png",
  uri: null,
}

const UPLOADED_IMAGE_RESOURCE: PromptInputBlock = {
  type: "resource",
  uri: "file:///home/me/.codeg/uploads/ab/pasted.png",
  mime_type: "image/png",
  text: null,
  blob: "QkFTRTY0",
}

const TEXT: PromptInputBlock = { type: "text", text: "hello" }

describe("stripUploadedImagePayloads", () => {
  it("strips inline data from an uploaded image block", () => {
    const out = stripUploadedImagePayloads([TEXT, UPLOADED_IMAGE], true)
    expect(out[0]).toEqual(TEXT)
    expect(out[1]).toEqual({ ...UPLOADED_IMAGE, data: "" })
  })

  it("strips the blob from an uploaded image-mime embedded resource", () => {
    const out = stripUploadedImagePayloads([UPLOADED_IMAGE_RESOURCE], true)
    expect(out[0]).toEqual({ ...UPLOADED_IMAGE_RESOURCE, blob: "" })
  })

  it("keeps a path-less pasted image inline (nothing to hydrate from)", () => {
    const out = stripUploadedImagePayloads([PASTED_IMAGE_NO_URI], true)
    expect(out[0]).toBe(PASTED_IMAGE_NO_URI)
  })

  it("keeps a clipboard:// synth-uri resource inline", () => {
    const block: PromptInputBlock = {
      type: "resource",
      uri: "clipboard://pasted-image-1",
      mime_type: "image/png",
      text: null,
      blob: "QkFTRTY0",
    }
    expect(stripUploadedImagePayloads([block], true)[0]).toBe(block)
  })

  it("does not touch a non-image resource blob", () => {
    const block: PromptInputBlock = {
      type: "resource",
      uri: "file:///home/me/.codeg/uploads/ab/data.bin",
      mime_type: "application/octet-stream",
      text: null,
      blob: "QkFTRTY0",
    }
    expect(stripUploadedImagePayloads([block], true)[0]).toBe(block)
  })

  it("passes everything through untouched when shouldStrip is false (desktop local)", () => {
    const blocks = [TEXT, UPLOADED_IMAGE, UPLOADED_IMAGE_RESOURCE]
    expect(stripUploadedImagePayloads(blocks, false)).toBe(blocks)
  })

  it("leaves an already-empty payload as-is (idempotent)", () => {
    const empty: PromptInputBlock = { ...UPLOADED_IMAGE, data: "" }
    const out = stripUploadedImagePayloads([empty], true)
    expect(out[0]).toBe(empty)
  })
})
