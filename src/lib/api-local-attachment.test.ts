import { beforeEach, describe, expect, it, vi } from "vitest"

const shellCallMock = vi.hoisted(() => vi.fn())

vi.mock("./transport", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./transport")>()
  return {
    ...actual,
    getActiveRemoteConnectionId: () => 7,
    getShellTransport: () => ({ call: shellCallMock }),
  }
})

import {
  readLocalImagePathForAttachment,
  readLocalPathForAttachment,
} from "./api"

describe("local attachment path readers", () => {
  beforeEach(() => {
    shellCallMock.mockReset()
    shellCallMock.mockResolvedValue({
      fileName: "attachment.bin",
      mimeType: "application/octet-stream",
      size: 3_000_000,
      dataBase64: "AA==",
    })
  })

  it("uses the 20 MB Rust image reader for remote image paths", async () => {
    await expect(
      readLocalImagePathForAttachment("/outside/image.png")
    ).resolves.toEqual(expect.objectContaining({ size: 3_000_000 }))

    expect(shellCallMock).toHaveBeenCalledWith(
      "read_local_image_for_attachment",
      { path: "/outside/image.png" }
    )
  })

  it("keeps ordinary remote uploads on the 2 MiB Rust reader", async () => {
    await readLocalPathForAttachment("/outside/notes.txt")

    expect(shellCallMock).toHaveBeenCalledWith("read_local_file_for_upload", {
      path: "/outside/notes.txt",
    })
  })
})
