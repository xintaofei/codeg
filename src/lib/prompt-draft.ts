import type {
  AdaptedContentPart,
  UserImageDisplay,
  UserResourceDisplay,
} from "@/lib/adapters/ai-elements-adapter"
import type { PromptDraft, PromptInputBlock } from "@/lib/types"

function isResourceLinkBlock(
  block: PromptInputBlock
): block is Extract<PromptInputBlock, { type: "resource_link" }> {
  return block.type === "resource_link"
}

function isEmbeddedResourceBlock(
  block: PromptInputBlock
): block is Extract<PromptInputBlock, { type: "resource" }> {
  return block.type === "resource"
}

function isImageBlock(
  block: PromptInputBlock
): block is Extract<PromptInputBlock, { type: "image" }> {
  return block.type === "image"
}

/**
 * An embedded `resource` block that actually carries image bytes — an `image/*`
 * mime + a `blob`. This is how an agent with `image:false` but
 * `embedded_context:true` (e.g. Grok) encodes a pasted image (see
 * `imageAttachmentToPromptBlock`). It must render as a thumbnail like a native
 * image, not as a content-less resource chip.
 */
function isImageResourceBlock(block: PromptInputBlock): block is Extract<
  PromptInputBlock,
  { type: "resource" }
> & {
  blob: string
  mime_type: string
} {
  return (
    block.type === "resource" &&
    typeof block.blob === "string" &&
    block.blob.length > 0 &&
    (block.mime_type?.startsWith("image/") ?? false)
  )
}

function deriveResourceNameFromUri(uri: string): string {
  const fallback = "resource"
  const normalized = uri.trim()
  if (!normalized) return fallback
  const withoutQuery = normalized.split(/[?#]/, 1)[0]
  const candidate = withoutQuery.split(/[\\/]/).pop() ?? ""
  let decoded = ""
  if (candidate) {
    try {
      decoded = decodeURIComponent(candidate)
    } catch {
      decoded = candidate
    }
  }
  return decoded || fallback
}

export function getPromptDraftDisplayText(
  draft: PromptDraft,
  attachedResourcesFallback: string
): string {
  const trimmed = draft.displayText.trim()
  return trimmed || attachedResourcesFallback
}

export function buildUserMessageTextPartsFromDraft(
  draft: PromptDraft,
  attachedResourcesFallback: string
): AdaptedContentPart[] {
  return [
    {
      type: "text",
      text: getPromptDraftDisplayText(draft, attachedResourcesFallback),
    },
  ]
}

export function extractUserResourcesFromDraft(
  draft: PromptDraft
): UserResourceDisplay[] {
  const linked = draft.blocks.filter(isResourceLinkBlock).map((resource) => ({
    name: resource.name,
    uri: resource.uri,
    mime_type: resource.mime_type ?? null,
  }))
  const embedded = draft.blocks
    .filter(isEmbeddedResourceBlock)
    // An image-mime embedded resource surfaces as a thumbnail (via
    // `extractUserImagesFromDraft`), not a resource chip.
    .filter((resource) => !isImageResourceBlock(resource))
    .map((resource) => ({
      name: deriveResourceNameFromUri(resource.uri),
      uri: resource.uri,
      mime_type: resource.mime_type ?? null,
    }))
  return [...linked, ...embedded]
}

function deriveImageName(
  uri: string | null | undefined,
  mimeType: string
): string {
  if (uri && uri.trim().length > 0) {
    const name = deriveResourceNameFromUri(uri)
    if (name !== "resource") return name
  }
  const ext = mimeType.split("/")[1]?.split("+")[0] ?? "image"
  return `image.${ext}`
}

export function extractUserImagesFromDraft(
  draft: PromptDraft
): UserImageDisplay[] {
  const native = draft.blocks.filter(isImageBlock).map((image) => ({
    name: deriveImageName(image.uri, image.mime_type),
    data: image.data,
    mime_type: image.mime_type,
    uri: image.uri ?? null,
  }))
  // Grok-style images ride as embedded `resource` blocks (image mime + blob);
  // surface them as thumbnails too, reading the bytes from `blob`.
  const embedded = draft.blocks
    .filter(isImageResourceBlock)
    .map((resource) => ({
      name: deriveImageName(resource.uri, resource.mime_type),
      data: resource.blob,
      mime_type: resource.mime_type,
      uri: resource.uri,
    }))
  return [...native, ...embedded]
}
