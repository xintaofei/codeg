import { mergeAttributes, Node, type JSONContent } from "@tiptap/core"
import { ReactNodeViewRenderer } from "@tiptap/react"

import { referenceToMarkdown } from "../reference-text"
import {
  REFERENCE_KINDS,
  type ReferenceAttrs,
  type ReferenceKind,
  type ReferenceMeta,
} from "../types"
import { ReferenceView } from "./reference-view"

const NODE_NAME = "reference"

/** Coerce a parsed (possibly pasted) value to a known kind, defaulting to file. */
function parseRefType(raw: string | null): ReferenceKind {
  return REFERENCE_KINDS.includes(raw as ReferenceKind)
    ? (raw as ReferenceKind)
    : "file"
}

// Only schemes the composer itself emits. Reference URIs parsed from pasted
// HTML are an untrusted input, so anything else (javascript:, data:, http:, …)
// is dropped to null rather than carried into a ResourceLink on send.
const ALLOWED_URI_SCHEMES = ["file:", "codeg:"]

/** Keep a parsed reference URI only if it uses a scheme the composer emits. */
function parseUri(raw: string | null): string | null {
  if (!raw) return null
  const lower = raw.toLowerCase()
  return ALLOWED_URI_SCHEMES.some((scheme) => lower.startsWith(scheme))
    ? raw
    : null
}

/** Payload the badge view reports when an agent badge is clicked. */
export interface ReferenceBadgeClickInfo {
  attrs: ReferenceAttrs
  /** The badge's live DOM element (popover anchoring). */
  element: HTMLElement
}

export interface ReferenceNodeStorage {
  /**
   * Set by the host (message input) to receive clicks on AGENT badges — the
   * entry point for re-opening the per-mention delegation config of a mention
   * already in the composer. Other reference kinds don't report clicks.
   */
  onBadgeClick?: (info: ReferenceBadgeClickInfo) => void
  /** Localized hover hint shown on agent badges. */
  badgeTitle?: string
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    reference: {
      /** Insert an inline reference badge (file/agent/session/commit/skill). */
      insertReference: (attrs: ReferenceAttrs) => ReturnType
    }
  }

  interface Storage {
    reference: ReferenceNodeStorage
  }
}

function parseMeta(raw: string | null): ReferenceMeta | null {
  if (!raw) return null
  try {
    return JSON.parse(raw) as ReferenceMeta
  } catch {
    return null
  }
}

/**
 * Inline atom node that embeds a reference (file/agent/session/commit/skill) as
 * a single, non-editable badge. One generic node keyed on `refType` keeps the
 * schema and serialization centralized; the badge switches on `refType`.
 *
 * - `renderMarkdown` → human-readable token (see {@link referenceToMarkdown}).
 * - `parseHTML`/`renderHTML` carry `data-*` attrs so copy/paste and HTML-based
 *   round-trips reconstruct the node.
 */
export const Reference = Node.create({
  name: NODE_NAME,
  group: "inline",
  inline: true,
  atom: true,
  selectable: true,
  draggable: false,

  addStorage() {
    return { onBadgeClick: undefined } as ReferenceNodeStorage
  },

  addAttributes() {
    return {
      refType: {
        default: "file" as ReferenceKind,
        parseHTML: (el) => parseRefType(el.getAttribute("data-ref-type")),
        renderHTML: (attrs) => ({ "data-ref-type": attrs.refType }),
      },
      id: {
        default: "",
        parseHTML: (el) => el.getAttribute("data-ref-id") ?? "",
        renderHTML: (attrs) => ({ "data-ref-id": attrs.id }),
      },
      label: {
        default: "",
        parseHTML: (el) => el.getAttribute("data-label") ?? "",
        renderHTML: (attrs) => ({ "data-label": attrs.label }),
      },
      uri: {
        default: null,
        parseHTML: (el) => parseUri(el.getAttribute("data-uri")),
        renderHTML: (attrs) => (attrs.uri ? { "data-uri": attrs.uri } : {}),
      },
      meta: {
        default: null,
        parseHTML: (el) => parseMeta(el.getAttribute("data-meta")),
        renderHTML: (attrs) =>
          attrs.meta ? { "data-meta": JSON.stringify(attrs.meta) } : {},
      },
    }
  },

  parseHTML() {
    return [{ tag: "span[data-reference]" }]
  },

  renderHTML({ node, HTMLAttributes }) {
    return [
      "span",
      mergeAttributes(HTMLAttributes, { "data-reference": "" }),
      referenceToMarkdown(node.attrs as ReferenceAttrs),
    ]
  },

  renderText({ node }) {
    return referenceToMarkdown(node.attrs as ReferenceAttrs)
  },

  renderMarkdown(node: JSONContent) {
    return referenceToMarkdown(node.attrs as ReferenceAttrs)
  },

  addNodeView() {
    return ReactNodeViewRenderer(ReferenceView)
  },

  addCommands() {
    return {
      insertReference:
        (attrs: ReferenceAttrs) =>
        ({ commands }) =>
          commands.insertContent({ type: NODE_NAME, attrs }),
    }
  },
})
