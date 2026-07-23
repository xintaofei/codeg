"use client"

import { memo, useEffect, useId, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { ChevronDown, ChevronUp } from "lucide-react"

import { cn } from "@/lib/utils"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"

import { ContentPartsRenderer } from "./content-parts-renderer"

/**
 * Caps a user message's rendered height (mirrors Codex desktop) and reveals a
 * "Show more"/"Show less" toggle once it's actually clipped. Assistant
 * messages are out of scope by design — this is only ever used from the
 * `group.role === "user"` branch in `HistoricalMessageGroup`, so `role` isn't
 * a prop here.
 */
export const CollapsibleUserMessage = memo(function CollapsibleUserMessage({
  parts,
}: {
  parts: AdaptedContentPart[]
}) {
  const t = useTranslations("Folder.chat.messageList")
  const contentRef = useRef<HTMLDivElement>(null)
  const [isOverflowing, setIsOverflowing] = useState(false)
  const [expanded, setExpanded] = useState(false)
  const contentId = useId()

  useEffect(() => {
    // Nothing useful to redetect once expanded: the clamp class below is
    // removed, so clientHeight === scrollHeight trivially and this would
    // misreport `false`, dropping the "Show less" toggle. Freeze the last
    // known value instead.
    if (expanded) return
    const el = contentRef.current
    if (!el) return
    const measure = () => {
      // Both reads are on this same, currently `max-h-60`-clamped node: no
      // numeric threshold duplicated from CSS. clientHeight is capped by the
      // class below; scrollHeight always reports the untruncated height.
      setIsOverflowing(el.scrollHeight > el.clientHeight + 1)
    }
    measure() // Synchronous initial read — doesn't depend on the
    // ResizeObserver callback firing (the jsdom test stub never invokes it).
    const observer = new ResizeObserver(measure)
    observer.observe(el)
    return () => observer.disconnect()
  }, [parts, expanded])

  const clipped = !expanded

  return (
    <>
      <div
        ref={contentRef}
        id={contentId}
        data-testid="collapsible-user-message-content"
        className={cn(
          "min-w-0",
          clipped && "max-h-60 overflow-hidden",
          clipped && isOverflowing && "collapsed-user-message-fade"
        )}
      >
        <ContentPartsRenderer parts={parts} role="user" />
      </div>
      {isOverflowing && (
        <button
          type="button"
          data-testid="collapsible-user-message-toggle"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
          aria-controls={contentId}
          className="flex shrink-0 items-center gap-1 text-xs font-medium text-muted-foreground hover:text-foreground"
        >
          {expanded ? t("showLess") : t("showMore")}
          {expanded ? (
            <ChevronUp className="size-3.5 shrink-0" />
          ) : (
            <ChevronDown className="size-3.5 shrink-0" />
          )}
        </button>
      )}
    </>
  )
})
