"use client"

import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  type Dispatch,
  type RefObject,
  type SetStateAction,
} from "react"
import { useTranslations } from "next-intl"
import { ArrowDown, ArrowUp, X } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  isFoldableReply,
  splitAssistantTurnParts,
} from "@/components/message/completed-turn-content"
import type {
  ResolvedMessageGroup,
  ThreadRenderItem,
} from "@/components/message/message-list-view"
import { cn } from "@/lib/utils"

type TurnItem = Extract<ThreadRenderItem, { kind: "turn" }>

// ── Matching ─────────────────────────────────────────────────────────────────

/**
 * Case- and whitespace-fold `text` for matching WITHOUT changing its length, so
 * an offset found in the folded copy is an offset into the original — the DOM
 * pass maps matches back onto text nodes that way. A character whose lower case
 * is longer (`İ` → `i̇`) keeps its own case, and any whitespace (a soft line
 * break, a non-breaking space) reads as the plain space it renders as.
 */
export function foldForFind(text: string): string {
  const lower = text.toLowerCase()
  const folded =
    lower.length === text.length
      ? lower
      : Array.from(text, (ch) => {
          const low = ch.toLowerCase()
          return low.length === ch.length ? low : ch
        }).join("")
  return folded.replace(/\s/g, " ")
}

/** The folded query to look for, or "" when there is nothing to find. */
export function findNeedle(query: string): string {
  return query.trim().length === 0 ? "" : foldForFind(query)
}

/** Start offsets of `needle` in `folded`, non-overlapping, in order. */
export function matchOffsets(folded: string, needle: string): number[] {
  const offsets: number[] = []
  if (!needle) return offsets
  let pos = folded.indexOf(needle)
  while (pos !== -1) {
    offsets.push(pos)
    pos = folded.indexOf(needle, pos + needle.length)
  }
  return offsets
}

// A link renders as its label (an image as nothing), so its destination is not
// text on screen.
const MARKDOWN_LINK = /(!?)\[([^\]]*)\]\([^)\s]*(?:\s+"[^"]*")?\)/g

/**
 * Stand-in for the prose a row shows — one string per text part — for rows the
 * virtualizer has not mounted. A mounted row is measured from the DOM instead
 * (see `useFindHighlights`), so this only has to be close, and it mirrors what
 * the row renders by default: text parts only (tool calls, their results,
 * reasoning and chrome are not searched), and for a reply whose fold is closed
 * only the answer it keeps in view.
 *
 * `replyOpen` is whether an assistant reply's fold is open (see
 * `CompletedTurnContent`); other roles ignore it.
 */
export function findableSegments(
  item: ThreadRenderItem,
  replyOpen: boolean
): string[] {
  if (item.kind !== "turn") return []
  const { group } = item
  let parts = group.parts
  if (group.role === "assistant" && !replyOpen) {
    const split = splitAssistantTurnParts(parts)
    if (isFoldableReply(split, item.isResponseComplete)) parts = split.answer
  }
  const segments: string[] = []
  for (const part of parts) {
    if (part.type !== "text" || part.text.length === 0) continue
    // User text is shown verbatim, not rendered as Markdown.
    segments.push(
      group.role === "user"
        ? part.text
        : part.text.replace(MARKDOWN_LINK, (_m, bang: string, label: string) =>
            bang ? "" : label
          )
    )
  }
  return segments
}

// Per group: a settled row's estimate is computed once per query rather than
// on every streaming render of the thread.
const estimateCache = new WeakMap<
  ResolvedMessageGroup,
  { needle: string; replyOpen: boolean; complete: boolean; count: number }
>()

/** How many times `needle` occurs in the row's estimated prose. */
export function estimateMatchCount(
  item: TurnItem,
  replyOpen: boolean,
  needle: string
): number {
  const cached = estimateCache.get(item.group)
  if (
    cached &&
    cached.needle === needle &&
    cached.replyOpen === replyOpen &&
    cached.complete === item.isResponseComplete
  ) {
    return cached.count
  }
  let count = 0
  for (const segment of findableSegments(item, replyOpen)) {
    count += matchOffsets(foldForFind(segment), needle).length
  }
  estimateCache.set(item.group, {
    needle,
    replyOpen,
    complete: item.isResponseComplete,
    count,
  })
  return count
}

// ── Match list and cursor ────────────────────────────────────────────────────

/** A row holding matches, in thread order. */
export interface FindRow {
  key: string
  /** Index into the thread items — what `scrollToIndex` takes. */
  threadIndex: number
  count: number
}

/**
 * Where the find bar points: the `occ`-th match (1-based) of row `key`. A row
 * and an occurrence rather than a flat index, because counts move underneath
 * it — a row is measured when it mounts, a reply streams, older history pages
 * in above — and none of that may move the match the bar is on.
 */
export interface FindCursor {
  key: string
  occ: number
  /** Last known position, for when the row is re-keyed or gone. */
  threadIndex: number
  /** The way the reader was stepping, for when the row has nothing left. */
  dir: 1 | -1
}

export interface FindMatch {
  key: string
  occ: number
  threadIndex: number
  /** 0-based position among all matches — the "x" of "x of y". */
  index: number
}

export function totalMatches(rows: readonly FindRow[]): number {
  let total = 0
  for (const row of rows) total += row.count
  return total
}

/** The match at flat position `index` (0-based), or null when out of range. */
export function findMatchAt(
  rows: readonly FindRow[],
  index: number
): FindMatch | null {
  if (index < 0) return null
  let base = 0
  for (const row of rows) {
    if (index < base + row.count) {
      return {
        key: row.key,
        occ: index - base + 1,
        threadIndex: row.threadIndex,
        index,
      }
    }
    base += row.count
  }
  return null
}

function matchInRow(
  rows: readonly FindRow[],
  rowIndex: number,
  occ: number
): FindMatch {
  let base = 0
  for (let i = 0; i < rowIndex; i++) base += rows[i].count
  const row = rows[rowIndex]
  const clamped = Math.min(Math.max(occ, 1), row.count)
  return {
    key: row.key,
    occ: clamped,
    threadIndex: row.threadIndex,
    index: base + clamped - 1,
  }
}

/**
 * Resolve the cursor against the current rows: the exact match while it
 * exists, the row's nearest one when the row now holds fewer, and otherwise the
 * next row in the direction the reader was stepping (wrapping) — so a row that
 * turns out, once measured, to show no match is stepped past rather than landed
 * on. A null cursor is the first match.
 *
 * `threadIndexOf` finds a key among the current thread items.
 */
export function resolveFindCursor(
  rows: readonly FindRow[],
  cursor: FindCursor | null,
  threadIndexOf: (key: string) => number | undefined
): FindMatch | null {
  if (rows.length === 0) return null
  if (!cursor) return matchInRow(rows, 0, 1)
  const own = rows.findIndex((row) => row.key === cursor.key)
  if (own !== -1) return matchInRow(rows, own, cursor.occ)

  const current = threadIndexOf(cursor.key)
  if (current === undefined) {
    // Re-keyed rather than gone — a reply settling, a refetch renaming its
    // turn: the row now at its old position is almost always the same one.
    const same = rows.findIndex((row) => row.threadIndex === cursor.threadIndex)
    if (same !== -1) return matchInRow(rows, same, cursor.occ)
  }
  const at = current ?? cursor.threadIndex
  if (cursor.dir === 1) {
    const next = rows.findIndex((row) => row.threadIndex > at)
    return matchInRow(rows, next === -1 ? 0 : next, 1)
  }
  let prev = -1
  for (let i = rows.length - 1; i >= 0; i--) {
    if (rows[i].threadIndex < at) {
      prev = i
      break
    }
  }
  const row = prev === -1 ? rows.length - 1 : prev
  return matchInRow(rows, row, rows[row].count)
}

// ── Measuring the rendered transcript ────────────────────────────────────────

interface DomSegment {
  text: string
  nodes: Text[]
  /** Offset of each node's first character in `text`. */
  starts: number[]
}

// Text inside a prose container that is not prose on screen: KaTeX's MathML
// copy (visually hidden, there for screen readers) and diagram labels.
const NOT_PROSE = ".katex-mathml, svg, style, script, template"

/** A row's prose as rendered: one segment per `[data-find-text]` container. */
function collectSegments(row: Element): DomSegment[] {
  const segments: DomSegment[] = []
  for (const container of Array.from(
    row.querySelectorAll("[data-find-text]")
  )) {
    const nodes: Text[] = []
    const starts: number[] = []
    let text = ""
    const walker = document.createTreeWalker(
      container,
      NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT,
      {
        acceptNode: (node) =>
          node.nodeType === Node.TEXT_NODE
            ? NodeFilter.FILTER_ACCEPT
            : (node as Element).matches(NOT_PROSE)
              ? NodeFilter.FILTER_REJECT
              : NodeFilter.FILTER_SKIP,
      }
    )
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const data = (node as Text).data
      if (!data) continue
      nodes.push(node as Text)
      starts.push(text.length)
      text += data
    }
    if (text) segments.push({ text, nodes, starts })
  }
  return segments
}

/** Index of the node holding character `offset` (last start ≤ offset). */
function nodeAt(starts: number[], offset: number): number {
  let lo = 0
  let hi = starts.length - 1
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1
    if (starts[mid] <= offset) lo = mid
    else hi = mid - 1
  }
  return lo
}

/**
 * Every match of `needle` in the row's rendered prose that `shown` accepts, in
 * order. These ranges ARE the row's matches: they are what gets highlighted,
 * and their number is what the counter adds up for the row. Exported for tests.
 */
export function findRangesInRow(
  row: Element,
  needle: string,
  shown: (range: Range) => boolean
): Range[] {
  const ranges: Range[] = []
  if (!needle) return ranges
  for (const segment of collectSegments(row)) {
    for (const start of matchOffsets(foldForFind(segment.text), needle)) {
      const end = start + needle.length
      const first = nodeAt(segment.starts, start)
      const last = nodeAt(segment.starts, end - 1)
      const range = document.createRange()
      range.setStart(segment.nodes[first], start - segment.starts[first])
      range.setEnd(segment.nodes[last], end - segment.starts[last])
      if (shown(range)) ranges.push(range)
    }
  }
  return ranges
}

interface ClipBox {
  rect: DOMRect
  x: boolean
  y: boolean
}

/**
 * Whether a range inside `row` is actually on screen (scrolling aside): it has
 * a layout box, is not `visibility: hidden`, and is not cut off by an
 * `overflow: hidden` ancestor within the row — a user message clamped to its
 * first lines, a fold mid-animation, a truncated label. Scroll containers do
 * not count as cutting anything off: the reader can scroll them, and so can
 * the reveal. Style reads are cached for one pass.
 */
function rangeShownIn(row: Element): (range: Range) => boolean {
  const styles = new Map<Element, CSSStyleDeclaration>()
  const clips = new Map<Element, ClipBox | null>()
  const styleOf = (el: Element) => {
    let style = styles.get(el)
    if (!style) {
      style = getComputedStyle(el)
      styles.set(el, style)
    }
    return style
  }
  const clipOf = (el: Element) => {
    let clip = clips.get(el)
    if (clip === undefined) {
      const { overflowX, overflowY } = styleOf(el)
      const x = overflowX === "hidden" || overflowX === "clip"
      const y = overflowY === "hidden" || overflowY === "clip"
      clip = x || y ? { rect: el.getBoundingClientRect(), x, y } : null
      clips.set(el, clip)
    }
    return clip
  }
  return (range) => {
    const rects = Array.from(range.getClientRects())
    if (rects.length === 0) return false
    const parent = range.startContainer.parentElement
    if (!parent || styleOf(parent).visibility !== "visible") return false
    for (let el: Element | null = parent; el && el !== row; ) {
      const clip = clipOf(el)
      if (
        clip &&
        !rects.some(
          (r) =>
            (!clip.x ||
              (r.right > clip.rect.left && r.left < clip.rect.right)) &&
            (!clip.y || (r.bottom > clip.rect.top && r.top < clip.rect.bottom))
        )
      ) {
        return false
      }
      el = el.parentElement
    }
    return true
  }
}

// Room for the find bar, which floats over the top of the thread.
const REVEAL_MARGIN_TOP = 56
const REVEAL_MARGIN_BOTTOM = 24

// How long a match just landed on is held in view. A row reached through
// `scrollToIndex` is not settled when it mounts: the virtualizer re-centres it
// on every size change among the rows coming in (and for 150ms after the
// last), which can carry a match in a tall reply back out of view.
const REVEAL_SETTLE_MS = 400

/**
 * Scroll the active match into view: every scrolling ancestor between it and
 * the transcript frame (a code block, then the thread) centres it when it is
 * not already comfortably visible. `scrollToIndex` only centres the ROW, which
 * misses a match further down a reply taller than the viewport.
 */
function revealRange(range: Range, row: Element, frame: Element): void {
  for (
    let el = range.startContainer.parentElement;
    el && el !== frame;
    el = el.parentElement
  ) {
    const style = getComputedStyle(el)
    const scrollsY =
      /(auto|scroll|overlay)/.test(style.overflowY) &&
      el.scrollHeight > el.clientHeight
    const scrollsX =
      /(auto|scroll|overlay)/.test(style.overflowX) &&
      el.scrollWidth > el.clientWidth
    if (!scrollsY && !scrollsX) continue
    const outer = el.contains(row)
    const r = range.getBoundingClientRect()
    const box = el.getBoundingClientRect()
    const top = box.top + (outer ? REVEAL_MARGIN_TOP : 0)
    const bottom = box.bottom - (outer ? REVEAL_MARGIN_BOTTOM : 0)
    if (scrollsY && (r.top < top || r.bottom > bottom)) {
      el.scrollTop += (r.top + r.bottom) / 2 - (top + bottom) / 2
    }
    if (scrollsX && (r.left < box.left || r.right > box.right)) {
      el.scrollLeft += (r.left + r.right) / 2 - (box.left + box.right) / 2
    }
  }
}

// ── Painting ─────────────────────────────────────────────────────────────────

const MATCH_HIGHLIGHT = "find-match"
const ACTIVE_HIGHLIGHT = "find-match-active"
const HIGHLIGHT_STYLE_ID = "find-in-chat-highlights"

// The ::highlight() rules live here rather than in globals.css: Lightning CSS,
// which processes the stylesheet at build time, does not recognise the
// pseudo-element. The two layers stack, so the active one is near-opaque to
// read over the regular one.
function ensureHighlightStyles(): void {
  if (document.getElementById(HIGHLIGHT_STYLE_ID)) return
  const style = document.createElement("style")
  style.id = HIGHLIGHT_STYLE_ID
  style.textContent = `
::highlight(${MATCH_HIGHLIGHT}) { background-color: rgba(251, 191, 36, 0.3); color: inherit; }
::highlight(${ACTIVE_HIGHLIGHT}) { background-color: rgba(251, 146, 60, 0.85); color: #1c1917; }`
  document.head.appendChild(style)
}

// Every searching transcript's ranges, by owner. The highlight registry is one
// per document, and more than one transcript can be searching at once (canvas
// cards), so each publishes its share and the registry shows the union.
const paintedByOwner = new Map<
  string,
  { matches: Range[]; active: Range | null }
>()

function publishHighlights(
  owner: string,
  painted: { matches: Range[]; active: Range | null } | null
): void {
  if (painted) paintedByOwner.set(owner, painted)
  else paintedByOwner.delete(owner)
  if (
    typeof CSS === "undefined" ||
    CSS.highlights == null ||
    typeof Highlight !== "function"
  ) {
    return
  }
  const matches = new Highlight()
  const active = new Highlight()
  for (const share of paintedByOwner.values()) {
    for (const range of share.matches) matches.add(range)
    if (share.active) active.add(share.active)
  }
  if (matches.size > 0) CSS.highlights.set(MATCH_HIGHLIGHT, matches)
  else CSS.highlights.delete(MATCH_HIGHLIGHT)
  if (active.size > 0) CSS.highlights.set(ACTIVE_HIGHLIGHT, active)
  else CSS.highlights.delete(ACTIVE_HIGHLIGHT)
}

// ── Measured counts ──────────────────────────────────────────────────────────

interface MeasuredRow {
  group: ResolvedMessageGroup
  epoch: number
  count: number
  /** In the DOM as of the latest pass, which re-measures it on any change. */
  mounted: boolean
}

export interface MeasuredRows {
  needle: string
  rows: ReadonlyMap<string, MeasuredRow>
}

export const NO_MEASURED_ROWS: MeasuredRows = { needle: "", rows: new Map() }

/**
 * Fold one DOM pass into the measured counts — `pass` holds every row that is
 * mounted now — returning `prev` itself when nothing moved, so the state
 * update is a no-op. Exported for tests.
 */
export function mergeMeasuredRows(
  prev: MeasuredRows,
  needle: string,
  epoch: number,
  pass: ReadonlyMap<string, { group: ResolvedMessageGroup; count: number }>
): MeasuredRows {
  const fresh = prev.needle !== needle
  const base = fresh ? new Map<string, MeasuredRow>() : prev.rows
  let next: Map<string, MeasuredRow> | null = fresh ? new Map(base) : null
  const write = (key: string, row: MeasuredRow) => {
    if (!next) next = new Map(base)
    next.set(key, row)
  }
  for (const [key, { group, count }] of pass) {
    const old = base.get(key)
    if (
      !old ||
      !old.mounted ||
      old.count !== count ||
      old.group !== group ||
      old.epoch !== epoch
    ) {
      write(key, { group, epoch, count, mounted: true })
    }
  }
  for (const [key, old] of base) {
    if (old.mounted && !pass.has(key)) write(key, { ...old, mounted: false })
  }
  return next ? { needle, rows: next } : prev
}

/**
 * A row's measured count, or undefined when its estimate has to stand in: never
 * measured under this query, or measured before its content or fold state last
 * changed while it was off screen. A row still mounted is trusted as is — the
 * pass re-measures it the moment its DOM changes, and falling back to the
 * estimate in between would flicker the counter on every streamed token.
 */
export function measuredCount(
  measured: MeasuredRows,
  needle: string,
  epoch: number,
  item: TurnItem
): number | undefined {
  if (measured.needle !== needle) return undefined
  const row = measured.rows.get(item.key)
  if (!row) return undefined
  if (row.mounted) return row.count
  return row.group === item.group && row.epoch === epoch ? row.count : undefined
}

/**
 * The DOM half of find-in-chat. While a query is set it walks the mounted rows
 * (`[data-find-key]`) of the transcript under `frameRef`, finds the matches in
 * their rendered prose, paints them with the CSS Custom Highlight API — text
 * nodes and Ranges, so the rendered Markdown is never mutated — and reports each
 * row's count through `setMeasured` as the source of truth for that row (read
 * back with `measuredCount`). Rows the virtualizer has not mounted keep their
 * estimate until they are.
 *
 * The walk repeats on any DOM change (rows mounting as the thread scrolls, a
 * reply streaming, a fold opening), on the end of an animation (a fold sliding
 * open) and on resize (text reflowing under a clamp). The active match is
 * scrolled into view whenever it changes.
 *
 * `epoch` is the reply-fold epoch: a send folds every earlier reply, which
 * invalidates what was measured while one was open.
 */
export function useFindHighlights(
  frameRef: RefObject<HTMLElement | null>,
  {
    enabled,
    needle,
    epoch,
    items,
    activeKey,
    activeOcc,
    setMeasured,
  }: {
    enabled: boolean
    needle: string
    epoch: number
    items: ThreadRenderItem[]
    activeKey: string | null
    activeOcc: number
    setMeasured: Dispatch<SetStateAction<MeasuredRows>>
  }
): void {
  const owner = useId()
  const groups = useMemo(() => {
    const byKey = new Map<string, ResolvedMessageGroup>()
    for (const item of items) {
      if (item.kind === "turn") byKey.set(item.key, item.group)
    }
    return byKey
  }, [items])
  // Read by the pass, which runs after commit — the rows it finds in the DOM
  // are the ones this render committed.
  const groupsRef = useRef(groups)
  useLayoutEffect(() => {
    groupsRef.current = groups
  }, [groups])
  const revealedRef = useRef<string | null>(null)

  // A layout effect, so a new query or a step to another match is painted and
  // counted in the same frame as the render that asked for it.
  useLayoutEffect(() => {
    const frame = frameRef.current
    if (!frame || !enabled || !needle) {
      revealedRef.current = null
      publishHighlights(owner, null)
      return
    }
    ensureHighlightStyles()
    let scheduled = 0
    // Until when a match just landed on is held in view (see REVEAL_SETTLE_MS).
    let settleUntil = 0
    const pass = () => {
      scheduled = 0
      const counts = new Map<
        string,
        { group: ResolvedMessageGroup; count: number }
      >()
      const matches: Range[] = []
      let active: { range: Range; row: Element; id: string } | null = null
      for (const row of Array.from(
        frame.querySelectorAll<HTMLElement>("[data-find-key]")
      )) {
        const key = row.dataset.findKey
        const group = key ? groupsRef.current.get(key) : undefined
        if (!key || !group) continue
        const ranges = findRangesInRow(row, needle, rangeShownIn(row))
        counts.set(key, { group, count: ranges.length })
        for (const range of ranges) matches.push(range)
        if (key === activeKey && ranges.length > 0) {
          const occ = Math.min(Math.max(activeOcc, 1), ranges.length)
          active = {
            range: ranges[occ - 1],
            row,
            id: `${needle}\u0000${key}\u0000${occ}`,
          }
        }
      }
      publishHighlights(owner, { matches, active: active?.range ?? null })
      setMeasured((prev) => mergeMeasuredRows(prev, needle, epoch, counts))
      // Brought into view once, and kept there only while the thread
      // settles: after that, scrolling away from it is the reader's call.
      if (active && revealedRef.current !== active.id) {
        revealedRef.current = active.id
        settleUntil = performance.now() + REVEAL_SETTLE_MS
        revealRange(active.range, active.row, frame)
      } else if (active && performance.now() < settleUntil) {
        revealRange(active.range, active.row, frame)
      }
    }
    const schedule = () => {
      if (!scheduled) scheduled = requestAnimationFrame(pass)
    }
    const onScroll = () => {
      if (performance.now() < settleUntil) schedule()
    }
    const mutations = new MutationObserver(schedule)
    mutations.observe(frame, {
      childList: true,
      subtree: true,
      characterData: true,
      attributes: true,
      // What can hide or reveal prose: fold and clamp state, `hidden`, a
      // `<details>` opening. Not `style`, which the virtualizer rewrites on
      // every scroll frame.
      attributeFilter: [
        "class",
        "hidden",
        "open",
        "data-state",
        "aria-expanded",
      ],
    })
    const resize = new ResizeObserver(schedule)
    resize.observe(frame)
    frame.addEventListener("animationend", schedule)
    // Capture: scroll events do not bubble up from the thread's scroller.
    frame.addEventListener("scroll", onScroll, true)
    pass()
    return () => {
      mutations.disconnect()
      resize.disconnect()
      frame.removeEventListener("animationend", schedule)
      frame.removeEventListener("scroll", onScroll, true)
      cancelAnimationFrame(scheduled)
    }
  }, [
    frameRef,
    owner,
    enabled,
    needle,
    epoch,
    activeKey,
    activeOcc,
    setMeasured,
  ])

  // The layout effect above keeps its paint across re-runs (a step to the next
  // match must not flash the highlights off); dropping it is left to disabling
  // and to this.
  useEffect(() => () => publishHighlights(owner, null), [owner])
}

// ── Keyboard ownership ───────────────────────────────────────────────────────

const TRANSCRIPT_FRAME = "[data-transcript]"

/**
 * Whether a keystroke aimed at `target` — the find shortcut, or the Escape that
 * closes the bar — belongs to the transcript whose frame is `frame`. It does
 * not while a full-page route or the maximized file column covers the
 * transcript (`inert`), inside a terminal (the chord may be the multiplexer's,
 * the same precedent as the tab-switch chords in `workspace-chrome-controller`),
 * inside a dialog or drawer the transcript is not part of (the command palette,
 * the side-panel browser), or when another transcript is nearer the focus
 * (canvas cards, side-by-side groups). Focus on nothing in particular leaves it
 * to the caller's notion of the active transcript.
 */
export function transcriptOwnsKeystroke(
  target: EventTarget | null,
  frame: Element | null
): boolean {
  if (!frame || frame.closest("[inert]")) return false
  if (!(target instanceof Element)) return true
  if (target.closest('[data-terminal-panel-region="true"]')) return false
  const dialog = target.closest('[role="dialog"], [role="alertdialog"]')
  if (dialog && !dialog.contains(frame)) return false
  const body = target.ownerDocument.body
  for (let el: Element | null = target; el && el !== body; ) {
    if (el.contains(frame)) return true
    if (el.matches(TRANSCRIPT_FRAME) || el.querySelector(TRANSCRIPT_FRAME)) {
      return false
    }
    el = el.parentElement
  }
  return true
}

// ── Find bar ─────────────────────────────────────────────────────────────────

interface FindInChatBarProps {
  query: string
  onQueryChange: (query: string) => void
  /** Matches in the loaded transcript window. */
  count: number
  /** 0-based position of the active match. */
  index: number
  /** Bumped every time the shortcut asks for the bar; re-focuses an open one. */
  focusToken: number
  onNext: () => void
  onPrev: () => void
  onClose: () => void
}

/**
 * Find bar for the open conversation transcript. A compact overlay placed by
 * the parent, which owns the query, the matches and the stepping; the bar only
 * collects the query and reports keys and clicks.
 */
export function FindInChatBar({
  query,
  onQueryChange,
  count,
  index,
  focusToken,
  onNext,
  onPrev,
  onClose,
}: FindInChatBarProps) {
  const t = useTranslations("Folder.chat.messageList")
  const inputRef = useRef<HTMLInputElement>(null)

  // Opening — or pressing the shortcut again with the bar already open —
  // selects what is there, so a new search replaces the old one by typing,
  // the way a browser's find bar does.
  useEffect(() => {
    inputRef.current?.focus()
    inputRef.current?.select()
  }, [focusToken])

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
