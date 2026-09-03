import { NodeViewWrapper, type ReactNodeViewProps } from "@tiptap/react"
import type { MouseEvent } from "react"

import { ReferenceBadge } from "../badges/reference-badge"
import type { ReferenceAttrs } from "../types"
import type { ReferenceNodeStorage } from "./reference-node"

/**
 * React node view for the `reference` atom. Renders the inline badge and marks
 * the surface non-editable so the caret treats the whole reference as one unit.
 *
 * Clicking an AGENT badge additionally reports through the node's storage
 * callback (`onBadgeClick`, registered by the host) — the re-entry point for
 * the per-mention delegation config. The click still selects the node
 * (ProseMirror's own handling is untouched); the callback is a side channel.
 */
export function ReferenceView({ node, editor }: ReactNodeViewProps) {
  const attrs = node.attrs as ReferenceAttrs

  const handleClick = (event: MouseEvent<HTMLSpanElement>) => {
    if (attrs.refType !== "agent") return
    const storage = editor?.storage.reference as
      | ReferenceNodeStorage
      | undefined
    storage?.onBadgeClick?.({ attrs, element: event.currentTarget })
  }

  return (
    <NodeViewWrapper
      as="span"
      className="codeg-reference"
      contentEditable={false}
      onClick={attrs.refType === "agent" ? handleClick : undefined}
      // Cursor affordance: an agent badge opens its delegation config.
      title={
        attrs.refType === "agent"
          ? (editor?.storage.reference as ReferenceNodeStorage | undefined)
              ?.badgeTitle
          : undefined
      }
    >
      <ReferenceBadge data={attrs} />
    </NodeViewWrapper>
  )
}
