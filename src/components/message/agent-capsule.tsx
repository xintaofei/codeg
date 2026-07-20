"use client"

/**
 * Shared visual shell for sub-agent capsules. Both the history "Agent" capsule
 * (`agent-tool-call.tsx`, reconstructed from the on-disk rollout) and the live
 * codex collab capsule (`collab-agent-card.tsx`, streamed) render through this
 * same chrome so the two are visually consistent: a collapsible pill trigger
 * (chevron + shimmering title while running + an optional right-aligned suffix)
 * over a bordered, scrollable body. Each caller supplies its own body for the
 * data it actually has.
 */

import { useState, type ReactNode } from "react"
import { ChevronRightIcon } from "lucide-react"

import { Shimmer } from "@/components/ai-elements/shimmer"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/instant-collapsible"
import { ScrollArea } from "@/components/ui/scroll-area"
import { cn } from "@/lib/utils"

interface AgentCapsuleProps {
  /** Trigger label. Shimmers while `isRunning`; CSS-truncated to one line. */
  title: string
  isRunning: boolean
  isError: boolean
  /** Optional content pinned to the right of the title (duration, status icon). */
  rightSuffix?: ReactNode
  /**
   * Optional identifier (e.g. the sub-agent UUID) shown between the title and
   * the right suffix. Stays visible while collapsed and does not truncate (the
   * title truncates first), so the capsule is identifiable at a glance.
   */
  idBadge?: ReactNode
  /** Accessible label for the trigger. */
  statusLabel?: string
  /** Initial open state; defaults to open on error so failures are visible. */
  defaultOpen?: boolean
  children: ReactNode
}

export function AgentCapsule({
  title,
  isRunning,
  isError,
  rightSuffix,
  idBadge,
  statusLabel,
  defaultOpen,
  children,
}: AgentCapsuleProps) {
  const [bodyOpen, setBodyOpen] = useState(defaultOpen ?? isError)

  // Respond to prop transitions with the canonical React tracked-previous-state
  // pattern (render-phase setState) — see
  // https://react.dev/reference/react/useState#storing-information-from-previous-renders.
  // The capsule's key is stable by tool-call id, so it does NOT remount when a
  // streaming tool call changes state; these transitions are how it reacts:
  //   - non-error → error: auto-OPEN so a failure that arrives mid-stream is
  //     visible without a click (the initial `isError` seed only covers calls
  //     that mount already-failed).
  //   - running → completed (non-error): auto-COLLAPSE once (only matters if the
  //     user manually expanded during streaming).
  const [prevIsRunning, setPrevIsRunning] = useState(isRunning)
  const [prevIsError, setPrevIsError] = useState(isError)
  if (prevIsRunning !== isRunning || prevIsError !== isError) {
    setPrevIsRunning(isRunning)
    setPrevIsError(isError)
    if (!prevIsError && isError) {
      setBodyOpen(true)
    } else if (prevIsRunning && !isRunning && !isError) {
      setBodyOpen(false)
    }
  }

  return (
    <Collapsible open={bodyOpen} onOpenChange={setBodyOpen} className="w-full">
      {/* Pill trigger — matches ToolGroupPart structure with themed emphasis. */}
      <CollapsibleTrigger
        className={cn(
          "group inline-flex max-w-full items-center gap-1.5 rounded-full bg-primary/10 px-3.5 py-2 text-xs font-medium text-foreground transition-colors hover:bg-primary/15",
          isError && "text-destructive"
        )}
        aria-label={statusLabel}
      >
        <ChevronRightIcon
          aria-hidden="true"
          className={cn(
            "size-3 shrink-0 opacity-60 transition-transform",
            bodyOpen && "rotate-90"
          )}
        />
        <span className="min-w-0 truncate">
          {isRunning ? (
            <Shimmer as="span" duration={1} shineColor="var(--primary)">
              {title}
            </Shimmer>
          ) : (
            title
          )}
        </span>
        {idBadge != null && (
          <span className="shrink-0 font-mono text-[10px] font-normal text-muted-foreground/70">
            {idBadge}
          </span>
        )}
        {rightSuffix != null && (
          <span className="flex shrink-0 items-center text-muted-foreground/60">
            {rightSuffix}
          </span>
        )}
      </CollapsibleTrigger>

      {/* Body — sits below the pill. Internal sections retain their own affordances. */}
      <CollapsibleContent
        className={cn(
          "w-full outline-none",
          "data-[state=open]:animate-in data-[state=closed]:animate-out",
          "data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0",
          "data-[state=closed]:slide-out-to-top-1 data-[state=open]:slide-in-from-top-1"
        )}
      >
        <div className="mt-3 w-full overflow-hidden rounded-md border border-border/60">
          <ScrollArea className="max-h-72">
            <div className="space-y-3 px-3.5 py-2">{children}</div>
          </ScrollArea>
        </div>
      </CollapsibleContent>
    </Collapsible>
  )
}
