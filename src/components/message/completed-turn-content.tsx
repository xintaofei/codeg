"use client"

import { memo, useCallback, useMemo, useState } from "react"
import { ChevronRightIcon } from "lucide-react"
import { useTranslations } from "next-intl"

import {
  splitTrailingAnswerParts,
  type AdaptedContentPart,
} from "@/lib/adapters/ai-elements-adapter"
import { formatElapsedLabel } from "@/lib/format-elapsed"
import { cn } from "@/lib/utils"
import { Shimmer } from "@/components/ai-elements/shimmer"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/instant-collapsible"
import { ContentPartsRenderer } from "./content-parts-renderer"

export interface SplitAssistantTurnParts {
  progress: AdaptedContentPart[]
  answer: AdaptedContentPart[]
}

/**
 * Split a completed assistant reply at its last progress item. Text before or
 * between tool/reasoning work is intermediate commentary; trailing response
 * content is the final answer and must remain visible. A text-only response is
 * left untouched because there is no reliable signal that any of it is
 * progress rather than the answer.
 *
 * The progress/answer taxonomy is the adapter's (`isTurnAnswerPart`), shared
 * with the Goal capsule's trailing-answer lift — the same question ("what may
 * a collapsed chip swallow?") must not get two answers.
 */
export function splitAssistantTurnParts(
  parts: AdaptedContentPart[]
): SplitAssistantTurnParts {
  const { body, trailing } = splitTrailingAnswerParts(parts)
  return { progress: body, answer: trailing }
}

/**
 * Does the split leave anything for the reader once the progress is folded
 * away? Whitespace-only text is not an answer: it renders as an empty markdown
 * block, so a turn "kept visible" by it still reads as a blank reply.
 */
function hasVisibleAnswer(answer: AdaptedContentPart[]): boolean {
  return answer.some(
    (part) => part.type !== "text" || part.text.trim().length > 0
  )
}

/**
 * Manual fold overrides for turns OUTSIDE the current round, keyed by the
 * group's `parts` array and stamped with the fold epoch.
 *
 * Weak on `parts` because the thread is virtualized: scrolling a turn past the
 * overscan buffer unmounts it, and an uncontrolled Collapsible would forget the
 * expansion — so scrolling away from a turn you opened and back would re-hide
 * its work. For settled history `parts` is exactly as stable as the turn's
 * identity (it comes from the per-turn adapter cache and the merged-run cache
 * in `message-list-view`), and being weak it is collected with the turn rather
 * than accumulating per conversation.
 *
 * The epoch stamp is what makes "sending a new message folds everything above
 * it" a single number bump rather than a walk over the thread: an entry written
 * under an earlier epoch simply stops matching.
 *
 * The CURRENT round deliberately does NOT live here. Its `parts` array is
 * replaced twice on the way into history (the stream settling into a promoted
 * local turn, then the authoritative detail refetch), so anything keyed on it
 * would drop the expansion mid-read — which is exactly the "the reply folds
 * itself up the moment it finishes" behaviour this replaces. `message-list-view`
 * owns that one state positionally and passes it down controlled.
 */
const manualFold = new WeakMap<
  AdaptedContentPart[],
  { epoch: number; open: boolean }
>()

/**
 * Shared between the interactive trigger and the static (nothing-to-fold) row
 * so a turn's header keeps the same shape whether or not it can be folded.
 *
 * `w-full` with the chevron sitting right after the label (not pushed to the
 * far edge): the rule underneath is a section divider and spans the reply,
 * while the control it belongs to reads as one unit. No corner radius — a
 * radius curls the ends of a lone `border-b` up into little hooks.
 *
 * The rule is tinted from `--foreground` rather than taking `--border`, which
 * it cannot use at any opacity: `--border` is a near-white `oklch(0.922)` in
 * light and `white/10%` in dark, so the usual `border-border/50` came out at
 * roughly `oklch(0.96)` on white and white at 5% on near-black — invisible in
 * both, and worst in dark. A foreground tint inverts with the theme instead —
 * the same derivation as task-card's outline and `--ws-chrome-border`, which
 * both reach for `--foreground` for exactly this reason.
 *
 * The TINT is what buys the legibility here, not the strength: at 10% this
 * lands about where `--border` would if it were used undiluted, except it now
 * holds up in dark and over a workspace background image, where the token
 * washes out. Deliberately no heavier — the header is a quiet label the reader
 * scans past, and a rule spanning the full width of every reply in the thread
 * carries far more weight than a lone boxed card's outline does.
 */
const HEADER_CLASS =
  "flex w-full items-center gap-1 border-b border-foreground/10 pb-1.5 text-xs font-medium text-muted-foreground/70"

export const CompletedTurnContent = memo(function CompletedTurnContent({
  parts,
  durationMs,
  completed,
  currentRound = false,
  roundOpen = true,
  onRoundOpenChange,
  foldEpoch = 0,
}: {
  parts: AdaptedContentPart[]
  durationMs?: number | null
  completed: boolean
  /** This reply is the thread's current round — the newest assistant run, from
   *  the moment the agent started replying until the next user send. Its fold
   *  state is owned by `message-list-view` (see `manualFold`). */
  currentRound?: boolean
  /** Current-round fold state. Only read when `currentRound`. */
  roundOpen?: boolean
  onRoundOpenChange?: (open: boolean) => void
  /** Bumped by `message-list-view` on every user send. */
  foldEpoch?: number
}) {
  const t = useTranslations("Folder.chat.messageList")
  const tElapsed = useTranslations("Folder.chat.liveTurnStats")
  const split = useMemo(() => splitAssistantTurnParts(parts), [parts])

  const [localOpen, setLocalOpen] = useState(() => {
    const entry = manualFold.get(parts)
    if (entry?.epoch === foldEpoch) return entry.open
    // A reply still being written is never folded by default — folding it is
    // an explicit act. Normally `currentRound` covers the live reply, but a
    // host that tracks no rounds at all (the delegation-child viewer) leans on
    // this, and it keeps the component honest on its own.
    return !completed
  })

  // Derived-state-during-render, not an effect: the fold has to be settled in
  // the same render that reads it, or sending a message would paint one frame
  // of the previous round still expanded before collapsing it.
  const [foldMark, setFoldMark] = useState({ epoch: foldEpoch, currentRound })
  if (foldMark.epoch !== foldEpoch || foldMark.currentRound !== currentRound) {
    setFoldMark({ epoch: foldEpoch, currentRound })
    if (foldMark.epoch !== foldEpoch) {
      // A new user message folds everything above it, including whatever the
      // reader had opened by hand. Everything above is settled by definition;
      // the `!completed` case is steering (a send lands mid-reply), where the
      // reply being written must stay open.
      setLocalOpen(!completed)
    } else if (foldMark.currentRound && !currentRound) {
      // A newer round took over this position without a send in between (a
      // background/loop turn). Carry the outgoing round's expansion into local
      // state so it doesn't snap shut under the reader.
      setLocalOpen(roundOpen)
    }
  }

  const open = currentRound ? roundOpen : localOpen

  // The unfold animation belongs to a real closed→open TOGGLE, never to a mount
  // that starts open. This component remounts constantly while staying open:
  // the row key flips from `streaming-…` to `persisted-…` the instant a reply
  // settles, the authoritative detail refetch renames the turn and flips it
  // again, and the virtualizer recycles any row scrolled past its overscan
  // buffer. Without this gate each of those replays a 200ms unfold, so a
  // finished reply appears to collapse and re-open by itself — precisely the
  // behaviour the round model exists to prevent.
  //
  // Only the ENTER side is gated. The exit animation must always run: it is
  // what unmounts the content (see the presence check in `instant-collapsible`).
  const [openMark, setOpenMark] = useState({ open, animateEnter: false })
  if (openMark.open !== open) setOpenMark({ open, animateEnter: open })
  const animateEnter = openMark.open === open && openMark.animateEnter

  const handleOpenChange = useCallback(
    (next: boolean) => {
      if (currentRound) {
        onRoundOpenChange?.(next)
        return
      }
      manualFold.set(parts, { epoch: foldEpoch, open: next })
      setLocalOpen(next)
    },
    [currentRound, foldEpoch, onRoundOpenChange, parts]
  )

  const elapsed =
    typeof durationMs === "number" && durationMs > 0
      ? formatElapsedLabel(durationMs, tElapsed)
      : null

  // Folding trades the process away to keep the answer. With no answer left
  // over there is nothing to keep, and the reply would fold to a lone header —
  // which is exactly the shape of the turns a reader most needs to see: one
  // stopped mid-tool-call (agents leave no closing prose), a Cline reply whose
  // `attempt_completion` card IS the answer, a plan-mode turn that ends on
  // ExitPlanMode with the plan inside that card. Those settle un-foldable.
  //
  // A live reply is exempt from the answer half of that rule: its closing prose
  // has not been written yet, so applying it would withhold the toggle for the
  // whole stream and hand it over one beat before the turn ends. Folding a live
  // reply is then an explicit choice; the round settling re-applies the rule.
  const foldable =
    split.progress.length > 0 && (!completed || hasVisibleAnswer(split.answer))

  const label = !completed
    ? t("working")
    : elapsed
      ? t("workedFor", { duration: elapsed })
      : t("worked")

  // Every assistant reply with content carries a header. Gating it on "has work
  // to fold or a duration to show" made it blink out at the worst moment: a
  // reply settles BEFORE the post-turn reparse backfills `duration_ms`, so a
  // text-only reply went "Working…" → no header at all → "Worked for 3s" a
  // second later. The settled-no-duration label holds that slot.
  const labelNode = completed ? (
    <span className="min-w-0 truncate tabular-nums">{label}</span>
  ) : (
    <Shimmer
      as="span"
      className="min-w-0 truncate"
      duration={1}
      shineColor="var(--primary)"
    >
      {label}
    </Shimmer>
  )

  // Streamdown's incomplete-markdown repair (remend) may only run on text that
  // is still being written. On settled text it appends a closer after spans
  // that are ALREADY complete — a glob inside code, an identifier like `_meta`
  // — leaving a stray `*` / `_`; landing after a final code fence, that closer
  // reopens the block (#555). Every branch below renders some slice of this one
  // turn, so they all get the same answer.
  const isStreaming = !completed

  if (!foldable) {
    // Parsers leave empty placeholder turns between tool exchanges;
    // `mergeConsecutiveAssistantTurns` only swallows them mid-run, so a lone
    // one reaches here with nothing in it at all. It has no content for a
    // header to head — heading it would turn an invisible turn into a visible
    // empty one. Settled turns only: a live reply's header is the point, even
    // before its first token lands.
    const blank =
      completed &&
      split.progress.length === 0 &&
      !hasVisibleAnswer(split.answer)
    if (blank) {
      return (
        <ContentPartsRenderer
          parts={parts}
          role="assistant"
          isStreaming={isStreaming}
        />
      )
    }
    return (
      <div className="space-y-3">
        <div className={HEADER_CLASS}>{labelNode}</div>
        <ContentPartsRenderer
          parts={parts}
          role="assistant"
          isStreaming={isStreaming}
        />
      </div>
    )
  }

  return (
    <div className="space-y-4">
      <Collapsible
        className="w-full"
        open={open}
        onOpenChange={handleOpenChange}
      >
        {/* No hover treatment at all — the header is a quiet label the reader
            scans past, and the chevron carries the affordance. Closed points
            along the reading direction, open points down at what it revealed:
            the same disclosure triangle every other fold in the thread uses. */}
        <CollapsibleTrigger
          className={cn(
            HEADER_CLASS,
            "group focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background"
          )}
        >
          {labelNode}
          <ChevronRightIcon
            aria-hidden="true"
            className="size-3.5 shrink-0 opacity-50 transition-transform group-data-[state=open]:rotate-90"
          />
        </CollapsibleTrigger>
        {/* `reply-fold-body` (globals.css) slides the body open and shut on a
            grid track. The inner div is the clipped grid item — it must stay a
            single child, and the spacing has to live INSIDE it or the closed
            track never reaches zero. */}
        <CollapsibleContent
          className={cn(
            "reply-fold-body w-full outline-none",
            animateEnter && "reply-fold-enter"
          )}
        >
          <div>
            <div className="pt-3">
              <ContentPartsRenderer
                parts={split.progress}
                role="assistant"
                isStreaming={isStreaming}
              />
            </div>
          </div>
        </CollapsibleContent>
      </Collapsible>
      {split.answer.length > 0 && (
        <ContentPartsRenderer
          parts={split.answer}
          role="assistant"
          isStreaming={isStreaming}
        />
      )}
    </div>
  )
})
