"use client"

import { useCallback, useEffect, useRef, useState, type RefObject } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { CopyIcon, TextQuote } from "lucide-react"

import { Button } from "@/components/ui/button"
import { copyTextToClipboard } from "@/lib/utils"

/** Vertical gap between the selection box and the bubble. */
const GAP = 8
/** Minimum distance the bubble keeps from the container's left/right edges. */
const EDGE = 8
/** No room for the bubble above the selection within this many px of the
 *  container top — it flips underneath instead. */
const FLIP_BELOW_WITHIN = 40
/** A selection whose box is within this many px of a horizontal container edge
 *  counts as scrolled out of the message area, and the bubble hides. */
const OUT_OF_VIEW_SLACK = 4

/**
 * Where the bubble sits, or why it isn't showing.
 *
 * `offscreen` is deliberately distinct from `none`: the selection is still live,
 * it has just scrolled out of the message area. The position tracker keeps
 * running in that state, so scrolling back brings the bubble straight back —
 * collapsing it to `none` would stop the tracker and the bubble would never
 * return (nothing re-fires `selectionchange` on a scroll).
 */
type SelectionState =
  | { kind: "none" }
  | { kind: "offscreen"; text: string }
  | {
      kind: "visible"
      text: string
      /** Horizontal centre of the selection, container-relative px. */
      x: number
      /** Container-relative px edge the bubble is pinned to. */
      y: number
      /** Bubble hangs below the selection (no room above). */
      below: boolean
    }

const NO_SELECTION: SelectionState = { kind: "none" }

function sameState(a: SelectionState, b: SelectionState): boolean {
  if (a.kind !== b.kind) return false
  if (a.kind === "none" || b.kind === "none") return true
  if (a.text !== b.text) return false
  if (a.kind !== "visible" || b.kind !== "visible") return true
  return a.x === b.x && a.y === b.y && a.below === b.below
}

interface SelectionActionBubbleProps {
  /**
   * The element whose text selections arm the bubble. It also owns positioning:
   * the bubble is an absolutely-positioned child, so this element must be
   * `position: relative` and must NOT be the scrolling box itself (offsets are
   * derived from viewport rects).
   */
  containerRef: RefObject<HTMLElement | null>
  /**
   * Quote the selection into the conversation composer. Omitted on read-only
   * surfaces (the sub-agent transcript dialog, task transcripts) — the quote
   * action then simply isn't offered and only "copy" remains.
   */
  onQuote?: (text: string) => void
}

/**
 * Floating quick-action toolbar for a text selection inside a message
 * transcript: copy the selected text, or quote it into the composer. Either way
 * the action dismisses the toolbar and drops the selection; copy confirms with a
 * toast, since the toolbar it would otherwise confirm on is gone by then.
 *
 * Rendered IN-TREE (not portalled to `body`) on purpose. Inactive conversation
 * tabs stay mounted and are hidden with `visibility: hidden`, which is
 * inherited — an in-tree overlay disappears with its tab for free, while a
 * portalled one would keep floating over whatever the user switched to.
 */
export function SelectionActionBubble({
  containerRef,
  onQuote,
}: SelectionActionBubbleProps) {
  const t = useTranslations("Folder.chat.messageList")
  const [state, setState] = useState<SelectionState>(NO_SELECTION)
  const stateRef = useRef<SelectionState>(NO_SELECTION)
  const bubbleRef = useRef<HTMLDivElement | null>(null)
  // A pointer is down somewhere: the user is (probably) dragging out a
  // selection, so hold the bubble back until they let go.
  const draggingRef = useRef(false)
  // A press has landed on the bubble and its `click` hasn't been dispatched yet.
  // On touch there is no `mousedown` to preventDefault, so the tap CLEARS the
  // selection — and tearing the bubble down on that `selectionchange` would
  // unmount the button before its click ever lands. Frozen until the next press
  // outside the bubble.
  const pressedInsideRef = useRef(false)

  const apply = useCallback((next: SelectionState) => {
    if (sameState(stateRef.current, next)) return
    stateRef.current = next
    setState(next)
  }, [])

  const measure = useCallback((): SelectionState => {
    const container = containerRef.current
    if (!container) return NO_SELECTION
    const selection = window.getSelection()
    if (!selection || selection.isCollapsed || selection.rangeCount === 0) {
      return NO_SELECTION
    }
    const range = selection.getRangeAt(0)
    // Only OUR transcript. A selection that starts here and ends outside (or
    // lives in another tab / the composer) has a common ancestor above the
    // container, so it is correctly rejected.
    if (!container.contains(range.commonAncestorContainer)) return NO_SELECTION
    const text = selection.toString()
    if (!text.trim()) return NO_SELECTION

    const rect = range.getBoundingClientRect()
    const box = container.getBoundingClientRect()
    if (rect.width === 0 && rect.height === 0)
      return { kind: "offscreen", text }
    if (
      rect.bottom < box.top + OUT_OF_VIEW_SLACK ||
      rect.top > box.bottom - OUT_OF_VIEW_SLACK
    ) {
      return { kind: "offscreen", text }
    }

    // Clamp the toolbar's BOX inside the container, not just its anchor point:
    // it is centred on `x` by a -50% transform, so clamping the anchor alone
    // still lets half the toolbar hang past the edge — where the panel's
    // `overflow-hidden` shears it off (measured: 35px of a button gone in a
    // 300px-wide tiled column). `offsetWidth` is 0 on the very first measure,
    // before the toolbar has rendered; the frame loop below re-measures with the
    // real width on the next frame, and since the width doesn't depend on `x`
    // that settles in one step. A toolbar wider than its container can't fit
    // either way, so it just centres.
    const centre = rect.left + rect.width / 2 - box.left
    const half = (bubbleRef.current?.offsetWidth ?? 0) / 2
    const minX = EDGE + half
    const maxX = box.width - EDGE - half
    const x = Math.round(
      minX > maxX ? box.width / 2 : Math.min(Math.max(centre, minX), maxX)
    )
    const top = rect.top - box.top
    const below = top < FLIP_BELOW_WITHIN
    const y = Math.round(
      below
        ? Math.min(rect.bottom - box.top + GAP, box.height - FLIP_BELOW_WITHIN)
        : top - GAP
    )
    return { kind: "visible", text, x, y, below }
  }, [containerRef])

  useEffect(() => {
    const insideBubble = (target: EventTarget | null) =>
      target instanceof Node && bubbleRef.current?.contains(target) === true

    const handleSelectionChange = () => {
      // Mid-drag the selection is still growing and the bubble would chase the
      // cursor; `pointerup` takes the final reading.
      if (draggingRef.current || pressedInsideRef.current) return
      apply(measure())
    }
    const handlePointerDown = (event: PointerEvent) => {
      // Pressing our own buttons must not tear the bubble down before `click`.
      // Only the PRIMARY button is armed: a right-click inside the bubble is
      // followed by `contextmenu`, never by `click`, so arming there would
      // freeze the bubble with nothing left to release it. `button` is read
      // with the DOM's own default (0 = primary) because jsdom has no
      // `PointerEvent` and synthesizes these without the property.
      if (insideBubble(event.target)) {
        if ((event.button ?? 0) === 0) pressedInsideRef.current = true
        return
      }
      pressedInsideRef.current = false
      draggingRef.current = true
      apply(NO_SELECTION)
    }
    const handlePointerUp = (event: PointerEvent) => {
      draggingRef.current = false
      if (insideBubble(event.target)) return
      // The browser finalises the selection after dispatching pointerup (a
      // double/triple click in particular), so read it on the next task.
      window.setTimeout(() => {
        if (draggingRef.current) return
        apply(measure())
      }, 0)
    }
    // A cancelled press (scroll takeover, palm rejection) is never followed by a
    // `click`, so the guard below would stay armed forever and strand the
    // bubble. Release it here — this is the cancel path's whole job.
    const handlePointerCancel = (event: PointerEvent) => {
      pressedInsideRef.current = false
      handlePointerUp(event)
    }
    // The press-inside freeze ends the moment its `click` is dispatched — that
    // is the whole window it exists to bridge. Leaving it armed would strand the
    // bubble: the position tracker below honours the same flag, so after a copy
    // it would stop following the text it points at.
    const handleClick = () => {
      pressedInsideRef.current = false
    }

    // Capture phase throughout: a handler deeper in the tree may stop
    // propagation (Radix menus and the composer both do), and this bookkeeping
    // has to see every press regardless.
    document.addEventListener("selectionchange", handleSelectionChange)
    document.addEventListener("pointerdown", handlePointerDown, true)
    document.addEventListener("pointerup", handlePointerUp, true)
    document.addEventListener("pointercancel", handlePointerCancel, true)
    document.addEventListener("click", handleClick, true)
    return () => {
      document.removeEventListener("selectionchange", handleSelectionChange)
      document.removeEventListener("pointerdown", handlePointerDown, true)
      document.removeEventListener("pointerup", handlePointerUp, true)
      document.removeEventListener("pointercancel", handlePointerCancel, true)
      document.removeEventListener("click", handleClick, true)
    }
  }, [apply, measure])

  // Keep the bubble glued to the selection while anything moves it: thread
  // scrolling, the window resizing, a sidebar animating, or new streamed content
  // reflowing the transcript above it. None of those fire `selectionchange`, and
  // a scroll listener alone misses the reflow cases — a frame loop covers them
  // all, and only runs while a selection is actually live.
  const live = state.kind !== "none"
  useEffect(() => {
    if (!live) return
    let frame = requestAnimationFrame(function tick() {
      frame = requestAnimationFrame(tick)
      if (draggingRef.current || pressedInsideRef.current) return
      apply(measure())
    })
    return () => cancelAnimationFrame(frame)
  }, [live, apply, measure])

  // The action handlers read the selection through `stateRef` rather than
  // `state`, so they stay referentially stable while the frame loop repositions
  // the bubble.

  /**
   * Drop the selection and take the bubble down. Every action ends this way:
   * the work is done, so the toolbar gets out of the way instead of hovering
   * over text the user is finished with. Clearing the selection (rather than
   * only hiding) is what makes the dismissal stick — the frame loop re-measures
   * every frame and would put the bubble straight back otherwise.
   */
  const dismiss = useCallback(() => {
    pressedInsideRef.current = false
    window.getSelection()?.removeAllRanges()
    apply(NO_SELECTION)
  }, [apply])

  const handleCopy = useCallback(() => {
    const current = stateRef.current
    if (current.kind === "none") return
    const { text } = current
    // Dismiss BEFORE the write, not in its `.then()`. The clipboard write is
    // async (and on the non-secure-context fallback path it focuses a hidden
    // textarea, which churns the page selection), so waiting on it would leave
    // a stale toolbar hovering over text that is already being taken away.
    // Clearing first also means the fallback's snapshot-and-restore of the page
    // selection has nothing to put back.
    dismiss()
    void copyTextToClipboard(text).then((ok) => {
      toast[ok ? "success" : "error"](
        ok ? t("selectionCopied") : t("selectionCopyFailed")
      )
    })
  }, [dismiss, t])

  const handleQuote = useCallback(() => {
    const current = stateRef.current
    if (current.kind === "none" || !onQuote) return
    onQuote(current.text)
    dismiss()
  }, [onQuote, dismiss])

  if (state.kind !== "visible") return null

  return (
    <div
      ref={bubbleRef}
      role="toolbar"
      aria-label={t("selectionActions")}
      className="absolute z-30 flex items-center gap-0.5 rounded-full border border-border bg-popover p-0.5 shadow-md select-none"
      style={{
        left: state.x,
        top: state.y,
        transform: `translate(-50%, ${state.below ? "0" : "-100%"})`,
      }}
      // Keep the selection (and the page's focus) intact while a button is
      // pressed. Without this the press collapses the selection, the resulting
      // `selectionchange` tears the toolbar down, and the button unmounts before
      // its `click` is ever dispatched — the action would simply never run.
      onMouseDown={(event) => event.preventDefault()}
      // Touch/pen presses arm the conversation panel's long-press context menu.
      // Stop them here so a slow tap on a bubble button doesn't pop that menu.
      onPointerDown={(event) => {
        if (event.pointerType !== "mouse") event.stopPropagation()
      }}
      onContextMenu={(event) => event.stopPropagation()}
    >
      <Button
        type="button"
        variant="ghost"
        size="xs"
        onClick={handleCopy}
        aria-label={t("selectionCopy")}
      >
        <CopyIcon />
        {t("selectionCopy")}
      </Button>
      {onQuote && (
        <Button
          type="button"
          variant="ghost"
          size="xs"
          onClick={handleQuote}
          aria-label={t("selectionQuote")}
        >
          <TextQuote />
          {t("selectionQuote")}
        </Button>
      )}
    </div>
  )
}
