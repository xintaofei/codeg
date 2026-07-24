import { describe, expect, it } from "vitest"

import {
  partitionAttachmentFiles,
  partitionAttachmentPaths,
} from "./attachment-routing"

describe("attachment routing", () => {
  it("recognizes image File objects by declared MIME type", () => {
    const image = new File(["image"], "capture.bin", { type: "image/png" })
    const text = new File(["text"], "notes.txt", { type: "text/plain" })

    expect(partitionAttachmentFiles([image, text], true)).toEqual({
      images: [image],
      resources: [text],
    })
  })

  it("falls back to the file extension when MIME type is empty", () => {
    const image = new File(["image"], "capture.PNG")

    expect(partitionAttachmentFiles([image], true)).toEqual({
      images: [image],
      resources: [],
    })
  })

  it("keeps images as resources when image capability is disabled", () => {
    const image = new File(["image"], "capture.png", { type: "image/png" })

    expect(partitionAttachmentFiles([image], false)).toEqual({
      images: [],
      resources: [image],
    })
  })

  it("classifies POSIX and Windows paths case-insensitively", () => {
    expect(
      partitionAttachmentPaths(["/outside/a.PNG", "C:\\x\\b.txt"], true)
    ).toEqual({
      images: ["/outside/a.PNG"],
      resources: ["C:\\x\\b.txt"],
    })
  })
})
