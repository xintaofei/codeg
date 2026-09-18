"use client"

import { useEffect, useRef } from "react"
import { useTranslations } from "next-intl"
import { ArrowDown, ArrowUp, X } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import type { ThreadRenderItem } from "@/components/message/message-list-view"
import { cn } from "@/lib/utils"

/**
 * Plain-text projection of one thread item for find-in-chat. Only message
 * prose is searchable (text parts of the adapted group) — tool calls, tool
 * results, reasoning traces and chrome (typing indicator, compaction
 * divider) are excluded, mirroring what a reader scans visually.
 *
 * Mirrors the `ThreadRenderItem` contract in `message-list-view.tsx`; keep in
 * sync when new item kinds gain user-visible text.
 */
export function extractFindableText(item: ThreadRenderItem): string {
  if (item.kind !== "turn") return ""
  const texts: string[] = []
  for (const part of item.group.parts) {
    if (part.type === "text") texts.push(part.text)
  }
  return texts.join("\n")
}

interface FindInChatBarProps {
  query: string
  onQueryChange: (query: string) => void
  /** Total matches in the loaded transcript window. */
  count: number
  /** Zero-based index of the match currently in view. */
  index: number
  onNext: () => void
  onPrev: () => void
  onClose: () => void
}

/**
 * Find bar for the open conversation transcript (⌘F / Ctrl+F). A compact
 * overlay pinned by the parent — matches are jumped to by the parent via the
 * virtualizer's `scrollToIndex`, and the hit row is highlighted there too
 * (this bar stays stateless beyond its own input focus).
 */
export function FindInChatBar({
  query,
  onQueryChange,
  count,
  index,
  onNext,
  onPrev,
  onClose,
}: FindInChatBarProps) {
  const t = useTranslations("Folder.chat.messageList")
  const inputRef = useRef<HTMLInputElement>(null)

  // autoFocus misses the case where the bar mounts while the window itself
  // regains focus; re-assert on open is cheap and idempotent.
  useEffect(() => {
    inputRef.current?.focus()
    inputRef.current?.select()
  }, [])

  const hasQuery = query.trim().length > 0

  return (
    <div
      className="absolute end-4 top-3 z-30 flex items-center gap-1 rounded-lg border bg-background/95 px-2 py-1.5 shadow-md backdrop-blur"
      role="search"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault()
          onClose()
        } else if (e.key === "Enter" && hasQuery) {
          e.preventDefault()
          if (e.shiftKey) onPrev()
          else onNext()
        }
      }}
    >
      <Input
        ref={inputRef}
        value={query}
        onChange={(e) => onQueryChange(e.target.value)}
        placeholder={t("findPlaceholder")}
        className="h-7 w-52 border-none bg-transparent text-sm shadow-none focus-visible:ring-0"
        aria-label={t("findPlaceholder")}
      />
      <span
        className={cn(
          "min-w-14 text-center text-xs tabular-nums text-muted-foreground",
          hasQuery && count === 0 && "text-destructive"
        )}
      >
        {hasQuery
          ? count > 0
            ? t("findMatchOf", { index: index + 1, count })
            : t("findNoResults")
          : ""}
      </span>
      <Button
        size="icon"
        variant="ghost"
        className="size-6"
        disabled={count === 0}
        onClick={onPrev}
        aria-label={t("findPrev")}
      >
        <ArrowUp className="size-3.5" />
      </Button>
      <Button
        size="icon"
        variant="ghost"
        className="size-6"
        disabled={count === 0}
        onClick={onNext}
        aria-label={t("findNext")}
      >
        <ArrowDown className="size-3.5" />
      </Button>
      <Button
        size="icon"
        variant="ghost"
        className="size-6"
        onClick={onClose}
        aria-label={t("findClose")}
      >
        <X className="size-3.5" />
      </Button>
    </div>
  )
}
