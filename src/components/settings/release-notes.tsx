"use client"

import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import { cn } from "@/lib/utils"

/** Typography for GitHub release bodies rendered in a compact panel. Extracted
 * verbatim from the settings page so the status-bar popover renders notes
 * identically without a second copy of the class list. */
const NOTES_PROSE =
  "leading-6 break-words text-muted-foreground " +
  "[&_h1]:text-sm [&_h1]:font-semibold [&_h1]:mb-2 [&_h1]:text-foreground " +
  "[&_h2]:text-sm [&_h2]:font-semibold [&_h2]:mt-3 [&_h2]:mb-2 [&_h2]:text-foreground " +
  "[&_h3]:text-xs [&_h3]:font-semibold [&_h3]:mt-2 [&_h3]:mb-1 [&_h3]:text-foreground " +
  "[&_p]:mb-2 [&_p:last-child]:mb-0 " +
  "[&_ul]:list-disc [&_ul]:pl-5 [&_ul]:mb-2 [&_ol]:list-decimal [&_ol]:pl-5 [&_ol]:mb-2 [&_li]:mb-1 " +
  "[&_code]:font-mono [&_code]:text-[11px] [&_code]:bg-muted [&_code]:rounded [&_code]:px-1 " +
  "[&_pre]:bg-muted [&_pre]:rounded-md [&_pre]:p-2 [&_pre]:overflow-x-auto [&_pre]:mb-2 " +
  "[&_a]:text-primary [&_a]:underline [&_a]:underline-offset-2 " +
  "[&_blockquote]:border-l-2 [&_blockquote]:border-border [&_blockquote]:pl-3 [&_blockquote]:text-muted-foreground/80 " +
  "[&_hr]:my-2 [&_hr]:border-border"

export interface ReleaseNotesProps {
  /** Raw markdown from the release manifest. */
  notes: string
  /** Shown when the release carries no notes. */
  emptyLabel: string
  className?: string
}

/**
 * Release notes panel. This is the module that pulls in the markdown stack, so
 * surfaces outside the settings bundle (the status-bar update popover) load it
 * lazily via `next/dynamic` rather than importing it directly.
 */
export function ReleaseNotes({
  notes,
  emptyLabel,
  className,
}: ReleaseNotesProps) {
  const trimmed = notes.trim()
  return (
    <div className={cn(NOTES_PROSE, className)}>
      {trimmed ? (
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{trimmed}</ReactMarkdown>
      ) : (
        emptyLabel
      )}
    </div>
  )
}

export default ReleaseNotes
