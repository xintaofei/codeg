import type { JSONContent } from "@tiptap/core"

import type { ImageInputAttachment } from "@/components/chat/message-input-attachments"
import type { ComposerDraftAttachment } from "@/lib/types"

const FILE_URI_PREFIX = "file://"

/** Fingerprint used to skip a PUT that would write the same snapshot. */
export function draftSnapshotKey(
  text: string,
  attachments: ComposerDraftAttachment[]
): string {
  return JSON.stringify({
    text,
    attachments: attachments.map((item) => ({
      id: item.id,
      kind: item.kind,
      name: item.name,
      mime: item.mime ?? null,
      size: item.size ?? 0,
      path: item.path ?? null,
      uri: item.uri ?? null,
    })),
  })
}

export function attachmentsEqual(
  a: ComposerDraftAttachment[],
  b: ComposerDraftAttachment[]
): boolean {
  return draftSnapshotKey("", a) === draftSnapshotKey("", b)
}

/** Decode a `file://` uri the composer built via `buildFileUri`. */
export function fileUriToPath(uri: string | null | undefined): string | null {
  if (!uri || !uri.startsWith(FILE_URI_PREFIX)) return null
  const rest = uri.slice(FILE_URI_PREFIX.length).split("#")[0]
  let decoded: string
  try {
    decoded = decodeURIComponent(rest)
  } catch {
    return null
  }
  if (!decoded) return null
  // `file:///C:/x` → `C:/x` on Windows; unix stays `/x`.
  if (/^\/[A-Za-z]:\//.test(decoded)) return decoded.slice(1)
  return decoded
}

/** Best-effort: a path that already lives in the uploads jail. */
export function isUploadJailPath(path: string | null | undefined): boolean {
  if (!path) return false
  const normalized = path.replace(/\\/g, "/").toLowerCase()
  return (
    normalized.includes("/.codeg/uploads/") ||
    /\/uploads\/[a-z0-9_-]+\//i.test(normalized)
  )
}

/** Pull workspace `file://` badges out of a Tiptap document. */
export function collectEditorFileAttachments(
  doc: JSONContent | null | undefined
): ComposerDraftAttachment[] {
  if (!doc) return []
  const out: ComposerDraftAttachment[] = []
  const seen = new Set<string>()
  walk(doc, (node) => {
    if (node.type !== "reference") return
    const attrs = node.attrs ?? {}
    if (attrs.refType !== "file") return
    const uri = typeof attrs.uri === "string" ? attrs.uri : ""
    if (!uri.startsWith(FILE_URI_PREFIX)) return
    if (seen.has(uri)) return
    seen.add(uri)
    const name =
      (typeof attrs.label === "string" && attrs.label.trim()) ||
      fileNameFromUri(uri)
    out.push({
      id: `file:${uri}`,
      kind: "file",
      name,
      mime: null,
      size: 0,
      path: null,
      uri,
    })
  })
  return out
}

function fileNameFromUri(uri: string): string {
  const path = fileUriToPath(uri) ?? uri
  const parts = path.split(/[/\\]/)
  return parts[parts.length - 1] || "file"
}

function walk(node: JSONContent, visit: (node: JSONContent) => void): void {
  visit(node)
  for (const child of node.content ?? []) walk(child, visit)
}

export function collectDraftAttachments(
  doc: JSONContent | null | undefined,
  images: ImageInputAttachment[]
): ComposerDraftAttachment[] {
  const staged = images
    .map(imageToDraftAttachment)
    .filter((item): item is ComposerDraftAttachment => item !== null)
  const files = collectEditorFileAttachments(doc)
  const seen = new Set(staged.map((item) => item.uri ?? item.path ?? item.id))
  const extra = files.filter((item) => !seen.has(item.uri ?? item.id))
  return [...staged, ...extra]
}

export function imageToDraftAttachment(
  attachment: ImageInputAttachment
): ComposerDraftAttachment | null {
  if (attachment.uploading) return null
  const path = fileUriToPath(attachment.uri)
  if (!path || !isUploadJailPath(path)) return null
  return {
    id: attachment.id,
    kind: "image",
    name: attachment.name,
    mime: attachment.mimeType,
    size: Math.floor((attachment.data.length * 3) / 4),
    path,
    uri: null,
  }
}
