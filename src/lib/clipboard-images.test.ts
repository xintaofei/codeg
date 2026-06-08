import { afterEach, describe, expect, it } from "vitest"
import {
  clipboardHasText,
  filesFromClipboard,
  imageFilesFromClipboardApi,
} from "./clipboard-images"

// jsdom's DataTransfer doesn't model `items`/`getAsFile()` the way browsers do,
// so build exactly the shape the helpers read.
function makeDataTransfer(opts: {
  files?: File[]
  text?: string
  items?: Array<{ kind: string; file: File | null }>
}): DataTransfer {
  return {
    files: (opts.files ?? []) as unknown as FileList,
    getData: (type: string) => (type === "text/plain" ? (opts.text ?? "") : ""),
    items: (opts.items ?? []).map((item) => ({
      kind: item.kind,
      getAsFile: () => item.file,
    })),
  } as unknown as DataTransfer
}

function pngFile(name = "shot.png"): File {
  return new File(["x"], name, { type: "image/png" })
}

describe("filesFromClipboard", () => {
  it("returns null dataTransfer as empty", () => {
    expect(filesFromClipboard(null)).toEqual([])
  })

  it("returns clipboardData.files directly when present", () => {
    const file = pngFile()
    expect(filesFromClipboard(makeDataTransfer({ files: [file] }))).toEqual([
      file,
    ])
  })

  it("falls back to file-kind items when files is empty (Linux X11/Wayland)", () => {
    const file = pngFile()
    const result = filesFromClipboard(
      makeDataTransfer({ items: [{ kind: "file", file }] })
    )
    expect(result).toEqual([file])
  })

  it("prefers text over an image item for mixed clipboards", () => {
    // Copying a spreadsheet cell exposes both text and an image item; the text
    // paste must win so the cell isn't hijacked into an attachment.
    const result = filesFromClipboard(
      makeDataTransfer({
        text: "A1\tB1",
        items: [{ kind: "file", file: pngFile() }],
      })
    )
    expect(result).toEqual([])
  })

  it("drops items whose getAsFile() returns null", () => {
    const result = filesFromClipboard(
      makeDataTransfer({
        items: [
          { kind: "file", file: null },
          { kind: "string", file: null },
        ],
      })
    )
    expect(result).toEqual([])
  })
})

describe("clipboardHasText", () => {
  it("is false for null and whitespace-only text", () => {
    expect(clipboardHasText(null)).toBe(false)
    expect(clipboardHasText(makeDataTransfer({ text: "   " }))).toBe(false)
  })

  it("is true when plain text is present", () => {
    expect(clipboardHasText(makeDataTransfer({ text: "hello" }))).toBe(true)
  })
})

type FakeClipboardItem = {
  types: string[]
  getType: (type: string) => Promise<Blob>
}

function stubClipboardRead(
  read: (() => Promise<FakeClipboardItem[]>) | undefined
) {
  Object.defineProperty(navigator, "clipboard", {
    value: read ? { read } : undefined,
    configurable: true,
    writable: true,
  })
}

describe("imageFilesFromClipboardApi", () => {
  afterEach(() => {
    stubClipboardRead(undefined)
  })

  it("returns [] when the Clipboard API is unavailable", async () => {
    stubClipboardRead(undefined)
    expect(await imageFilesFromClipboardApi()).toEqual([])
  })

  it("returns [] when read() rejects (permission denied / no activation)", async () => {
    stubClipboardRead(() => Promise.reject(new Error("denied")))
    expect(await imageFilesFromClipboardApi()).toEqual([])
  })

  it("converts image items into Files and skips non-image types", async () => {
    const png = new Blob(["png"], { type: "image/png" })
    stubClipboardRead(() =>
      Promise.resolve([
        {
          types: ["text/html", "image/png"],
          getType: (type: string) =>
            Promise.resolve(
              type === "image/png" ? png : new Blob([], { type })
            ),
        },
      ])
    )
    const files = await imageFilesFromClipboardApi()
    expect(files).toHaveLength(1)
    expect(files[0].type).toBe("image/png")
    expect(files[0].name).toBe("clipboard-image-1.png")
  })

  it("emits one File per item even when an item has several image encodings", async () => {
    // A single screenshot item often advertises both png and jpeg; it must
    // produce one attachment, not one per encoding.
    const png = new Blob(["png"], { type: "image/png" })
    const calls: string[] = []
    stubClipboardRead(() =>
      Promise.resolve([
        {
          types: ["image/png", "image/jpeg"],
          getType: (type: string) => {
            calls.push(type)
            return Promise.resolve(png)
          },
        },
      ])
    )
    const files = await imageFilesFromClipboardApi()
    expect(files).toHaveLength(1)
    expect(files[0].name).toBe("clipboard-image-1.png")
    expect(calls).toEqual(["image/png"])
  })

  it("names multiple images uniquely", async () => {
    const jpeg = new Blob(["a"], { type: "image/jpeg" })
    const png = new Blob(["b"], { type: "image/png" })
    stubClipboardRead(() =>
      Promise.resolve([
        { types: ["image/jpeg"], getType: () => Promise.resolve(jpeg) },
        { types: ["image/png"], getType: () => Promise.resolve(png) },
      ])
    )
    const files = await imageFilesFromClipboardApi()
    expect(files.map((f) => f.name)).toEqual([
      "clipboard-image-1.jpeg",
      "clipboard-image-2.png",
    ])
  })
})
