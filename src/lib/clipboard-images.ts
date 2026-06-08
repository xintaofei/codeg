// Clipboard helpers for extracting pasted files/images. Split out from
// `message-input.tsx` so the platform-specific quirks (Linux/Tauri WebKitGTK)
// can be unit-tested without mounting the whole input component.

// Extract pasted files from a clipboard event. On macOS/Windows pasted images
// land in `clipboardData.files`, but on Linux (X11/Wayland) the same image is
// only exposed through `clipboardData.items` as a file-kind DataTransferItem,
// so fall back to `getAsFile()` when `files` is empty.
export function filesFromClipboard(dataTransfer: DataTransfer | null): File[] {
  if (!dataTransfer) return []
  const files = Array.from(dataTransfer.files ?? [])
  if (files.length > 0) return files
  // Mixed text+image clipboards (spreadsheet cells, rich web content) expose
  // both a text/plain string and an image file item. Prefer the text and let
  // the default paste run, so copying a cell doesn't get hijacked into an
  // image attachment. Pure image pastes (screenshots) carry no text.
  if (clipboardHasText(dataTransfer)) return []
  const items = dataTransfer.items ? Array.from(dataTransfer.items) : []
  return items
    .filter((item) => item.kind === "file")
    .map((item) => item.getAsFile())
    .filter((file): file is File => file !== null)
}

// Whether the clipboard carries any plain text. Used to keep the async image
// fallback below from hijacking a text+image paste — see `filesFromClipboard`.
export function clipboardHasText(dataTransfer: DataTransfer | null): boolean {
  if (!dataTransfer) return false
  return dataTransfer.getData("text/plain").trim().length > 0
}

// Linux/Tauri (WebKitGTK) screenshot tools (e.g. WeChat) write an image to the
// system clipboard in a form the synchronous DataTransfer API can't read:
// `clipboardData.files` is empty and `DataTransferItem.getAsFile()` returns
// null. The async Clipboard API can still read the raw image blobs, so callers
// retry through here when `filesFromClipboard` comes up empty.
//
// Returns the images as `File`s, or [] when the API is unavailable, blocked by
// permissions / missing transient user activation, or holds no image. Callers
// must already have ruled out a text paste (see `clipboardHasText`) so a mixed
// text+image clipboard isn't hijacked into an attachment.
//
// Must be invoked synchronously from the paste handler so the underlying
// `navigator.clipboard.read()` still runs inside the paste user gesture;
// awaiting other work first can drop the transient activation it requires.
export async function imageFilesFromClipboardApi(): Promise<File[]> {
  if (!navigator.clipboard?.read) return []
  try {
    const items = await navigator.clipboard.read()
    const files: File[] = []
    for (const item of items) {
      // A single ClipboardItem may advertise one image under several encodings
      // (e.g. image/png and image/jpeg of the same screenshot). Take only the
      // first image representation per item so one pasted image yields one
      // attachment — matching the one-file-per-item behavior of `getAsFile()`.
      const type = item.types.find((t) => t.startsWith("image/"))
      if (!type) continue
      const blob = await item.getType(type)
      const ext = type.split("/")[1] || "png"
      files.push(
        new File([blob], `clipboard-image-${files.length + 1}.${ext}`, { type })
      )
    }
    return files
  } catch {
    // Unsupported, permission-denied, or no transient user activation — let the
    // default paste behavior stand.
    return []
  }
}
