"use client"

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
} from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  ArrowUp,
  CopyIcon,
  Languages,
  Loader2,
  MessageCircleQuestionMark,
  StickyNote,
  TextQuote,
  X,
} from "lucide-react"

import { Button } from "@/components/ui/button"
import { useImeGuard } from "@/hooks/use-ime-guard"
import type { TranslationAttempt } from "@/hooks/use-translated-text"
import { cn, copyTextToClipboard } from "@/lib/utils"

/** Vertical gap between the selection box and the bubble. */
const GAP = 8
/** Minimum distance the bubble keeps from the container's left/right edges. */
const EDGE = 8
/** No room for the bubble above the selection within this many px of the
 *  container top — it flips underneath instead. */
const FLIP_BELOW_WITHIN = 40
/** Pointer travel that turns a press on the card's header into a drag. */
const DRAG_THRESHOLD = 4
/** A selection whose box is within this many px of a horizontal container edge
 *  counts as scrolled out of the message area, and the bubble hides. */
const OUT_OF_VIEW_SLACK = 4
/** Longest selection sent for translation. Past this the text is CUT rather
 *  than refused, and the card says so — "I selected half the message" is a
 *  reasonable thing to do, and a truncated translation still answers it. */
const MAX_SELECTION_TRANSLATE_CHARS = 2000

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

/**
 * Which face the toolbar is showing. The two panels are mutually exclusive by
 * construction: each REPLACES the button row, so there is no state where a
 * question box and a translation card fight over the same box.
 */
type BubbleMode = "actions" | "asking" | "translating"

/** The inline translation panel's contents. */
interface TranslationCardState {
  status: "loading" | "error" | "done"
  /** The text actually sent — already truncated to the cap. */
  original: string
  /** The selection was longer than the cap and got cut. */
  truncated: boolean
  /** The translation, once it has arrived. */
  text?: string
  /** Why the attempt failed, in the endpoint's own words when available. */
  error?: string
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
   * Translate the selection and hand back the attempt: `text` plus, on
   * failure, the reason (the card shows it inline with a retry). Resolving
   * `null` (or rejecting) is a legacy failure shape and still handled.
   * Omitted while translation is switched off in settings, and the action
   * then isn't offered — same rule as `onQuote`.
   */
  onTranslate?: (text: string) => Promise<TranslationAttempt | null>
  /**
   * Quote the selection into the conversation composer. Omitted on read-only
   * surfaces (the sub-agent transcript dialog, task transcripts) — the quote
   * action then simply isn't offered and only "copy" remains.
   */
  onQuote?: (text: string) => void
  /**
   * Ask a question ABOUT the selection: the host opens a fresh conversation on
   * the same agent and sends the quoted selection followed by `question`.
   * Omitted wherever a new conversation can't be opened, and the action then
   * isn't offered — same rule as `onQuote`.
   */
  onAsk?: (selection: string, question: string) => void
  /**
   * Keep the selection as a note beside the transcript. Only the canvas has
   * somewhere to put one, so everywhere else omits it and the action isn't
   * offered — same rule as `onQuote`.
   */
  onSaveAsNote?: (text: string) => void
}

/**
 * Floating quick-action toolbar for a text selection inside a message
 * transcript: copy the selected text, translate it, quote it into the composer,
 * or ask a question about it in a new conversation. Every action except
 * translation dismisses the toolbar and drops the selection; copy confirms with
 * a toast, since the toolbar it would otherwise confirm on is gone by then.
 * Translation is the exception because its result IS the toolbar — the card
 * takes the button row's place and stays until the user closes it.
 *
 * Rendered IN-TREE (not portalled to `body`) on purpose. Inactive conversation
 * tabs stay mounted and are hidden with `visibility: hidden`, which is
 * inherited — an in-tree overlay disappears with its tab for free, while a
 * portalled one would keep floating over whatever the user switched to.
 */
export function SelectionActionBubble({
  containerRef,
  onTranslate,
  onQuote,
  onAsk,
  onSaveAsNote,
}: SelectionActionBubbleProps) {
  const t = useTranslations("Folder.chat.messageList")
  const ime = useImeGuard()
  const [state, setState] = useState<SelectionState>(NO_SELECTION)
  const stateRef = useRef<SelectionState>(NO_SELECTION)
  const bubbleRef = useRef<HTMLDivElement | null>(null)
  // A panel is open over the button row — the question composer or the
  // translation card. While it is, the toolbar FREEZES: the selection text is
  // already captured in `state`, and every tracker below stands down. It has to
  // — focusing the input collapses the page selection (and the card exists
  // precisely after the selection has been handed off), so a live tracker would
  // measure nothing and tear the panel down under the user mid-sentence. The
  // ref is the synchronous copy the document-level handlers and the frame loop
  // read.
  const [mode, setMode] = useState<BubbleMode>("actions")
  const modeRef = useRef<BubbleMode>("actions")
  const [question, setQuestion] = useState("")
  const [translation, setTranslation] = useState<TranslationCardState | null>(
    null
  )
  // Translation is the one async action that doesn't dismiss the toolbar, so a
  // result can outlive the card that asked for it: the user closes the card (or
  // presses Escape) while the request is still in flight. Every candidate
  // result is stamped with the sequence number of the card that requested it,
  // and a stale one is dropped instead of resurrecting a dismissed card.
  const translateSeqRef = useRef(0)
  const inputRef = useRef<HTMLInputElement | null>(null)
  // The translation card pins itself where the selection was, which can sit
  // right on top of the text the user wants to read next. The card's header is
  // a drag handle: these track the manual offset the user drags it to. A ref
  // keeps the drag's base immutable across moves; the state copy rerenders.
  const dragOffsetRef = useRef({ x: 0, y: 0 })
  const [cardOffset, setCardOffset] = useState({ x: 0, y: 0 })
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
    // A new selection (a new `text`, or a re-selection after the bubble was
    // dismissed) starts over positionally: the drag offset belonged to the
    // PREVIOUS translation card, and keeping it pins the fresh button row to
    // wherever the user dragged that card — far from the new selection. It is
    // only cleared on a genuine state change, so the frame loop re-measuring
    // the SAME selection (scroll follow) never fights an active drag.
    const prevText =
      stateRef.current.kind === "none" ? null : stateRef.current.text
    const nextText = next.kind === "none" ? null : next.text
    if (prevText !== nextText) {
      dragOffsetRef.current = { x: 0, y: 0 }
      setCardOffset({ x: 0, y: 0 })
    }
    stateRef.current = next
    setState(next)
  }, [])

  /**
   * The one freeze predicate, read by ALL THREE trackers below (the
   * `selectionchange` handler, the deferred `pointerup` read, and the frame
   * loop). It is a single function on purpose: when this was three inlined
   * reads, a mode that only taught two of them to stand down left the third
   * measuring a selection that was no longer there, and the panel hung at stale
   * coordinates.
   */
  const isFrozen = useCallback(() => modeRef.current !== "actions", [])

  /**
   * Go back to the button row, throwing away whatever the open panel held: the
   * typed question, the translation card, and any in-flight translation's right
   * to land.
   */
  const closeModes = useCallback(() => {
    modeRef.current = "actions"
    setMode("actions")
    setQuestion("")
    setTranslation(null)
    translateSeqRef.current += 1
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
    // Vertical placement reads the panel's OWN height, not just how far down
    // the selection sits. The button row needs `FLIP_BELOW_WITHIN`; the
    // translation card is an order of magnitude taller, and hanging it above a
    // selection 120px down the container put most of its box past the top edge,
    // where the surrounding overflow shears it — the "card is covered" report.
    // `offsetHeight` is 0 before the panel has rendered (and in jsdom), and the
    // floor keeps that first pass behaving exactly as the button row always
    // did; the re-clamp below re-measures once the real height exists.
    const top = rect.top - box.top
    const bottom = rect.bottom - box.top
    const height = bubbleRef.current?.offsetHeight ?? 0
    const below = top < Math.max(height + GAP, FLIP_BELOW_WITHIN)
    // Hanging below, `y` is the panel's top edge; hanging above (translate
    // -100%) it is the bottom one. Either way the clamp keeps the far edge
    // inside the container — a panel taller than the container itself pins to
    // the near edge and scrolls internally rather than being sheared.
    const y = Math.round(
      below
        ? Math.min(
            Math.min(bottom + GAP, box.height - FLIP_BELOW_WITHIN),
            Math.max(box.height - height - EDGE, EDGE)
          )
        : Math.max(top - GAP, Math.min(height + EDGE, box.height - EDGE))
    )
    return { kind: "visible", text, x, y, below }
  }, [containerRef])

  useEffect(() => {
    const insideBubble = (target: EventTarget | null) =>
      target instanceof Node && bubbleRef.current?.contains(target) === true

    const handleSelectionChange = () => {
      // Mid-drag the selection is still growing and the bubble would chase the
      // cursor; `pointerup` takes the final reading. While a panel is open the
      // toolbar is frozen — and the very act of focusing the ask input fires
      // this with an empty selection.
      if (draggingRef.current || pressedInsideRef.current || isFrozen()) {
        return
      }
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
      // A press anywhere outside is the dismissal gesture for an open panel too
      // — it abandons the question or the translation card, same as pressing
      // Escape.
      closeModes()
      apply(NO_SELECTION)
    }
    const handlePointerUp = (event: PointerEvent) => {
      draggingRef.current = false
      if (insideBubble(event.target)) return
      // The browser finalises the selection after dispatching pointerup (a
      // double/triple click in particular), so read it on the next task.
      window.setTimeout(() => {
        if (draggingRef.current || isFrozen()) return
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
  }, [apply, closeModes, measure, isFrozen])

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
      if (draggingRef.current || pressedInsideRef.current || isFrozen()) {
        return
      }
      apply(measure())
    })
    return () => cancelAnimationFrame(frame)
  }, [live, apply, measure, isFrozen])

  // The action handlers read the selection through `stateRef` rather than
  // `state`, so they stay referentially stable while the frame loop repositions
  // the bubble.

  /**
   * Drop the selection and take the bubble down. Every action ends this way
   * (translation's card included — the X button and Escape land here): the work
   * is done, so the toolbar gets out of the way instead of hovering over text
   * the user is finished with. Clearing the selection (rather than only hiding)
   * is what makes the dismissal stick — the frame loop re-measures every frame
   * and would put the bubble straight back otherwise.
   */
  const dismiss = useCallback(() => {
    pressedInsideRef.current = false
    closeModes()
    window.getSelection()?.removeAllRanges()
    apply(NO_SELECTION)
  }, [apply, closeModes])

  // Escape closes the translation card. The ask composer handles its own on the
  // input it has focused; the card focuses nothing, so the key has to be caught
  // at the document — in capture, and swallowed, so the conversation pane and
  // any surrounding overlay don't take it as their own dismissal first.
  useEffect(() => {
    if (mode !== "translating") return
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      event.stopPropagation()
      dismiss()
    }
    document.addEventListener("keydown", handleKeyDown, true)
    return () => document.removeEventListener("keydown", handleKeyDown, true)
  }, [dismiss, mode])

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

  const handleSaveAsNote = useCallback(() => {
    const current = stateRef.current
    if (current.kind === "none" || !onSaveAsNote) return
    onSaveAsNote(current.text)
    dismiss()
  }, [onSaveAsNote, dismiss])

  const handleQuote = useCallback(() => {
    const current = stateRef.current
    if (current.kind === "none" || !onQuote) return
    onQuote(current.text)
    dismiss()
  }, [onQuote, dismiss])

  /**
   * Swap the buttons for the question input. `modeRef` is set synchronously
   * (not just via state) because the frame loop and the document handlers read
   * it, and the very next thing that happens is the input taking focus — which
   * collapses the page selection and would otherwise dismiss us.
   */
  const handleAskOpen = useCallback(() => {
    if (stateRef.current.kind !== "visible" || !onAsk) return
    pressedInsideRef.current = false
    modeRef.current = "asking"
    setMode("asking")
  }, [onAsk])

  /**
   * Swap the buttons for the translation card. Like `handleAskOpen` this
   * freezes the trackers synchronously, and it deliberately does NOT dismiss
   * the bubble or clear the selection: the card is about to replace the button
   * row, and the result has to have somewhere to land. The card's own geometry
   * is the selection's (measured while it still exists), so the card can't
   * chase text that is gone.
   */
  const handleTranslate = useCallback(() => {
    const current = stateRef.current
    if (current.kind !== "visible" || !onTranslate) return
    pressedInsideRef.current = false
    const seq = ++translateSeqRef.current
    const raw = current.text
    const truncated = raw.length > MAX_SELECTION_TRANSLATE_CHARS
    const original = truncated
      ? raw.slice(0, MAX_SELECTION_TRANSLATE_CHARS)
      : raw
    modeRef.current = "translating"
    setMode("translating")
    // A fresh card starts where the selection was, un-dragged.
    dragOffsetRef.current = { x: 0, y: 0 }
    setCardOffset({ x: 0, y: 0 })
    setTranslation({ status: "loading", original, truncated })
    // Fire and forget on purpose: awaiting here would just delay the card's
    // render by a tick for nothing. The result below re-checks both the mode
    // and the sequence before it is allowed to touch state.
    void onTranslate(original)
      .then((attempt) => {
        if (
          modeRef.current !== "translating" ||
          translateSeqRef.current !== seq
        )
          return
        if (attempt === null || attempt.text === null) {
          setTranslation({
            status: "error",
            original,
            truncated,
            error: attempt?.error,
          })
          return
        }
        setTranslation({
          status: "done",
          original,
          truncated,
          text: attempt.text,
        })
      })
      .catch(() => {
        if (
          modeRef.current !== "translating" ||
          translateSeqRef.current !== seq
        )
          return
        setTranslation({ status: "error", original, truncated })
      })
  }, [onTranslate])

  // Re-clamp for whichever panel just replaced the button row — the ask row is
  // much wider than the buttons, the translation card taller and wider still —
  // and THEN take focus for the ask input. In that order, because focusing
  // collapses the page selection and `measure` would have nothing left to read.
  // This is the last measurement the toolbar takes before it freezes, so getting
  // it wrong here strands the panel hanging over the container edge, where the
  // panel's overflow-hidden shears it.
  //
  // The card re-runs this when its status widens it (loading spinner → full
  // text): `translation?.status` is a dependency, so "done" re-clamps once more.
  //
  // A measurement that no longer finds the selection is DISCARDED rather than
  // applied: on touch there is no mousedown to preventDefault, so the tap that
  // opened the composer has already dropped the selection — applying that would
  // unmount the input the user is about to type into.
  useEffect(() => {
    if (mode === "actions") return
    const next = measure()
    if (next.kind === "visible") apply(next)
    if (mode === "asking") inputRef.current?.focus()
  }, [apply, mode, measure, translation?.status])

  const handleAskSubmit = useCallback(() => {
    const current = stateRef.current
    const trimmed = question.trim()
    if (current.kind === "none" || !onAsk || !trimmed) return
    // The selection goes over verbatim; turning it into a quote is the host's
    // job, exactly as for `onQuote`.
    onAsk(current.text, trimmed)
    dismiss()
  }, [dismiss, onAsk, question])

  /**
   * Drag the translation card by its header. The card is frozen (mode !==
   * "actions"), so nothing else writes its position while the drag runs, and
   * the accumulated offset is applied on top of the measured position in the
   * style below.
   *
   * Pointer capture is taken only once the pointer has actually travelled —
   * never on the press itself. Capturing up front retargets `pointerup` to the
   * handle, so the `click` the browser synthesizes fires on the common
   * ancestor of press and release (the header) instead of on the button the
   * press landed on: that is exactly why the card's X button did nothing.
   * Interactive children bail out entirely, so a press on the button is a
   * click and nothing else.
   */
  const handleCardDragStart = useCallback((event: ReactPointerEvent) => {
    if (event.button !== 0) return
    if (
      event.target instanceof Element &&
      event.target.closest("button, a, input, textarea")
    ) {
      return
    }
    const handle = event.currentTarget as HTMLElement
    const start = { x: event.clientX, y: event.clientY }
    const base = dragOffsetRef.current
    let dragging = false
    const onMove = (move: PointerEvent) => {
      const dx = move.clientX - start.x
      const dy = move.clientY - start.y
      if (!dragging) {
        if (Math.abs(dx) < DRAG_THRESHOLD && Math.abs(dy) < DRAG_THRESHOLD) {
          return
        }
        dragging = true
        // From here the gesture is a drag, so the move stream has to survive
        // the cursor leaving the card.
        handle.setPointerCapture(move.pointerId)
        handle.style.cursor = "grabbing"
      }
      const next = { x: base.x + dx, y: base.y + dy }
      dragOffsetRef.current = next
      setCardOffset(next)
    }
    const onEnd = () => {
      handle.removeEventListener("pointermove", onMove)
      handle.removeEventListener("pointerup", onEnd)
      handle.removeEventListener("pointercancel", onEnd)
      handle.style.cursor = ""
    }
    handle.addEventListener("pointermove", onMove)
    handle.addEventListener("pointerup", onEnd)
    handle.addEventListener("pointercancel", onEnd)
  }, [])

  if (state.kind !== "visible") return null

  return (
    <div
      ref={bubbleRef}
      role="toolbar"
      aria-label={t("selectionActions")}
      className={cn(
        "absolute z-30 flex items-center gap-0.5 rounded-full border border-border bg-popover p-0.5 shadow-md select-none",
        // Only the panels can outgrow a narrow tiled column. Capping them
        // against the container (the bubble's containing block) lets the input
        // shrink instead of being sheared off by the panel's overflow-hidden;
        // the button row is left to size itself, where a cap would squeeze
        // labels.
        mode !== "actions" && "max-w-[calc(100%-1rem)]",
        // The translation card stacks: it is a column (header, source, result)
        // filling the capped width, not a row of buttons. Its height is capped
        // against the container too — a long translation would otherwise grow
        // a card taller than the message area, which no placement can fit.
        mode === "translating" &&
          "max-h-[calc(100%-1rem)] flex-col items-stretch overflow-hidden",
        // The pill shape belongs to the button row; a card with a header and a
        // body reads as a popover, not a pill.
        mode !== "actions" && "rounded-lg"
      )}
      style={{
        left: state.x + cardOffset.x,
        top: state.y + cardOffset.y,
        transform: `translate(-50%, ${state.below ? "0" : "-100%"})`,
      }}
      // Keep the selection (and the page's focus) intact while a button is
      // pressed. Without this the press collapses the selection, the resulting
      // `selectionchange` tears the toolbar down, and the button unmounts before
      // its `click` is ever dispatched — the action would simply never run.
      //
      // The ask input and the translated text are the exceptions: both NEED the
      // default (focus and caret placement for one, drag-selecting the result
      // for the other), and by then the toolbar is frozen and no longer cares
      // about the page selection.
      onMouseDown={(event) => {
        if (
          event.target instanceof Element &&
          event.target.closest("input, textarea, [data-selectable]")
        ) {
          return
        }
        event.preventDefault()
      }}
      // Touch/pen presses arm the conversation panel's long-press context menu.
      // Stop them here so a slow tap on a bubble button doesn't pop that menu.
      onPointerDown={(event) => {
        if (event.pointerType !== "mouse") event.stopPropagation()
      }}
      onContextMenu={(event) => event.stopPropagation()}
    >
      {mode === "translating" && translation ? (
        <TranslationCard
          state={translation}
          onClose={dismiss}
          onRetry={handleTranslate}
          onDragStart={handleCardDragStart}
        />
      ) : mode === "asking" ? (
        <>
          <input
            ref={inputRef}
            type="text"
            value={question}
            onChange={(event) => setQuestion(event.target.value)}
            placeholder={t("selectionAskPlaceholder")}
            aria-label={t("selectionAskPlaceholder")}
            // `select-text` undoes the toolbar's `select-none`, which some
            // engines otherwise inherit into the field and make untouchable.
            className="h-6 w-56 min-w-0 bg-transparent px-2 text-xs outline-none select-text placeholder:text-muted-foreground"
            {...ime.props}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                // Swallow it: the conversation pane and any surrounding overlay
                // treat Escape as their own dismissal.
                event.preventDefault()
                event.stopPropagation()
                dismiss()
                return
              }
              if (event.key !== "Enter") return
              // Mid-composition Enter belongs to the IME (it commits the
              // candidate); submitting on it would send a half-typed question,
              // which is the common case for every CJK input method.
              if (ime.isComposing(event)) return
              event.preventDefault()
              event.stopPropagation()
              handleAskSubmit()
            }}
          />
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            disabled={question.trim().length === 0}
            onClick={handleAskSubmit}
            aria-label={t("selectionAskSubmit")}
          >
            <ArrowUp />
          </Button>
        </>
      ) : (
        <>
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
          {onSaveAsNote && (
            <Button
              type="button"
              variant="ghost"
              size="xs"
              onClick={handleSaveAsNote}
              aria-label={t("selectionSaveAsNote")}
            >
              <StickyNote />
              {t("selectionSaveAsNote")}
            </Button>
          )}
          {onAsk && (
            <Button
              type="button"
              variant="ghost"
              size="xs"
              onClick={handleAskOpen}
              aria-label={t("selectionAsk")}
            >
              <MessageCircleQuestionMark />
              {t("selectionAsk")}
            </Button>
          )}
          {/* Rightmost: the user reads left-to-right actions then translates. */}
          {onTranslate && (
            <Button
              type="button"
              variant="ghost"
              size="xs"
              onClick={handleTranslate}
              aria-label={t("selectionTranslate")}
            >
              <Languages />
              {t("selectionTranslate")}
            </Button>
          )}
        </>
      )}
    </div>
  )
}

/**
 * Map a failure's machine reason to something the card can show. The gate
 * codes come from `requestTranslationDetailed`; anything else (the backend's
 * own message: rate limit, connection refused, HTTP status…) is shown
 * verbatim — it is already user-readable.
 */
/**
 * Map a failure's machine reason to something the card can show. The gate
 * codes come from `requestTranslationDetailed`; anything else (the backend's
 * own message: rate limit, connection refused, HTTP status…) is shown
 * verbatim — it is already user-readable. The translation lives in the
 * component's message namespace, so the lookup runs there.
 */
function translateFailureReason(
  error: string,
  t: (key: never) => string
): string {
  const GATE_COPY: Record<string, string> = {
    DISABLED: "failureDisabled",
    SELECTION_TOO_LONG: "failureSelectionTooLong",
    BAD_BATCH: "failureBadBatch",
    EMPTY_REPLY: "failureEmptyReply",
    INVENTED_CONTENT: "failureInventedContent",
    ECHO_OR_REFUSAL: "failureEchoOrRefusal",
    PLACEHOLDERS_LOST: "failurePlaceholdersLost",
  }
  const known = GATE_COPY[error]
  return known ? t(known as never) : error
}

/**
 * The inline translation panel: the (possibly truncated) source up top for
 * context, and the result underneath — a spinner while it runs, an inline
 * failure (reason + retry) if it didn't make it (a toast would be absurd: the
 * panel it belongs to is still on screen), the translation otherwise.
 */
function TranslationCard({
  state,
  onClose,
  onRetry,
  onDragStart,
}: {
  state: TranslationCardState
  onClose: () => void
  /** Re-run the translation with the exact original text of this card. */
  onRetry: () => void
  /** Pointer down on the header row starts dragging the card. */
  onDragStart: (event: ReactPointerEvent) => void
}) {
  const t = useTranslations("Folder.chat.messageList")

  return (
    <div className="flex w-80 min-w-0 flex-col p-2">
      <div
        data-drag-handle
        onPointerDown={onDragStart}
        className="flex shrink-0 touch-none cursor-grab select-none items-center justify-between gap-2 active:cursor-grabbing"
      >
        <div className="flex min-w-0 items-baseline gap-1.5">
          <span className="shrink-0 text-xs font-medium text-muted-foreground">
            {t("selectionTranslateOriginal")}
          </span>
          {state.truncated && (
            <span className="truncate text-2xs text-amber-600 dark:text-amber-400">
              {t("selectionTranslateTruncated", {
                limit: MAX_SELECTION_TRANSLATE_CHARS,
              })}
            </span>
          )}
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          onClick={onClose}
          aria-label={t("selectionTranslateClose")}
        >
          <X />
        </Button>
      </div>
      <p
        className="mt-1 line-clamp-2 shrink-0 text-xs leading-relaxed text-muted-foreground break-words"
        title={state.original}
      >
        {state.original}
      </p>
      {/* The result is the one part that can be arbitrarily long, so it is the
          part that scrolls — but only once the toolbar's max-h cap actually
          binds. It must NOT be `flex-1`: the card's height is auto, and a
          basis-0 item contributes zero to an auto-height flex container, which
          collapses this area to 0px and hides the spinner, the translation,
          and the error alike. Natural sizing keeps the card at its content
          height; `min-h-0` makes it the one shrink point when the cap clamps
          the card, and `overflow-y-auto` turns that shrink into scrolling. */}
      <div className="mt-1.5 min-h-0 space-y-2 overflow-y-auto border-t border-border pt-1.5">
        {state.status === "loading" && (
          <div
            className="flex items-center justify-center py-2 text-muted-foreground"
            role="status"
            aria-label={t("selectionTranslating")}
          >
            <Loader2 className="size-4 animate-spin" />
          </div>
        )}
        {state.status === "error" && (
          <div className="space-y-1.5 py-1">
            <p className="text-xs text-destructive">
              {t("selectionTranslateFailed")}
            </p>
            {/* The reason, in the endpoint's own words when it gave one: a
                rate-limit message and a config error demand different
                reactions from the user, and "翻译失败" alone says nothing. */}
            {state.error && (
              <p className="text-2xs leading-relaxed text-muted-foreground break-words">
                {translateFailureReason(state.error, t)}
              </p>
            )}
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7 text-xs"
              onClick={onRetry}
            >
              <Loader2 className="mr-1 h-3 w-3" aria-hidden="true" />
              {t("selectionTranslateRetry")}
            </Button>
          </div>
        )}
        {state.status === "done" &&
          // The model mirrors the source's paragraph breaks, but a single
          // `whitespace-pre-wrap` block renders the blank lines as cramped
          // half-empties. Splitting on them gives real paragraph spacing, and
          // each paragraph keeps its single newlines (list items, wrapped
          // lines) via `whitespace-pre-line`.
          state.text
            ?.split(/\n{2,}/)
            .filter((paragraph) => paragraph.trim())
            .map((paragraph, index) => (
              <p
                key={index}
                data-selectable
                // `select-text` undoes the toolbar's `select-none`, so the
                // translation itself can be copied by selection like any text.
                className="text-xs leading-relaxed whitespace-pre-line break-words select-text"
              >
                {paragraph}
              </p>
            ))}
      </div>
    </div>
  )
}
