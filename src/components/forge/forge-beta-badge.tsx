"use client"

import { cn } from "@/lib/utils"

/**
 * Marks the repository panel as still-settling work, drawn everywhere the panel
 * names itself: the page's breadcrumb title, and both entry points that lead
 * there (the sidebar row and its quick-actions mirror). Marking only the page
 * would tell people after they had already committed to the click.
 *
 * The word stays "Beta" in every locale we ship, so it is a literal rather than
 * a message key — ten identical entries plus a parity test to keep them that way
 * buys nothing. It is deliberately NOT aria-hidden either: "Repository panel
 * Beta" is exactly what a screen reader should announce, the same way the count
 * chips on the neighbouring rows read as part of their row.
 */
export function ForgeBetaBadge({ className }: { className?: string }) {
  return (
    <span
      className={cn(
        // Same chip metrics as the sidebar's shortcut and count badges, so this
        // sits on their rail instead of reading as a third kind of ornament.
        "inline-flex h-[0.9375rem] shrink-0 items-center justify-center",
        "rounded-[0.3125rem] bg-primary/10 px-[0.3125rem]",
        "text-[0.625rem] font-medium leading-none text-primary",
        className
      )}
    >
      Beta
    </span>
  )
}
