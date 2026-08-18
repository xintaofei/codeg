"use client"

import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from "react"
import { createPortal } from "react-dom"

import { isImeCompositionKey } from "@/lib/ime-composition"
import { cn } from "@/lib/utils"

import { ReferenceIcon } from "../badges/reference-badge"
import type { ReferenceAttrs, ReferenceKind } from "../types"
import type { MentionRenderState } from "./mention-suggestion"
import { placeAnchoredPopup } from "./popup-position"
import type {
  ReferenceSearch,
  SuggestionGroup,
  SuggestionPopupHandle,
} from "./types"

const FETCH_DEBOUNCE_MS = 150

// Tab order in the panel: agent first (per product decision), then the rest in
// their usual order. This is a *display* order; the search provider keeps its
// own (file-first) group order, which other code/tests depend on. `skill` is
// intentionally absent — skills, commands and experts are inserted via the `/`
// and `$` triggers, not the `@` panel.
const TAB_ORDER: readonly ReferenceKind[] = [
  "agent",
  "file",
  "session",
  "commit",
]

// English fallbacks for the tab labels; the host injects localized ones. `skill`
// is kept for type completeness (`ReferenceKind`) though it is not a shown tab.
const DEFAULT_TAB_LABELS: Record<ReferenceKind, string> = {
  agent: "Agents",
  file: "Files",
  session: "Sessions",
  commit: "Commits",
  skill: "Skills",
}

// Commit-synchronous in the browser so the panel is positioned before paint (no
// flash at a stale spot); a no-op-safe passive effect during the static-export
// prerender where `useLayoutEffect` would warn.
const useIsomorphicLayoutEffect =
  typeof window !== "undefined" ? useLayoutEffect : useEffect

/**
 * `id` of the listbox element and of each option. The editor's contentEditable
 * (which keeps DOM focus) points `aria-controls` at the listbox and
 * `aria-activedescendant` at the active option, the standard combobox pattern
 * for a popup that doesn't take focus. Option ids are namespaced by tab so the
 * id always resolves to a currently-mounted element (only the active tab's
 * options are rendered). Only one panel is open at a time, so ids never collide.
 */
export const MENTION_LISTBOX_ID = "mention-listbox"
export const mentionOptionId = (kind: ReferenceKind, index: number) =>
  `mention-option-${kind}-${index}`

export interface SuggestionPopupProps {
  /** Live trigger state (query/range/caret rect). */
  state: MentionRenderState
  /** Resolves the query into grouped suggestions. Must be referentially stable. */
  search: ReferenceSearch
  /** Insert the chosen reference, replacing the trigger range. */
  onSelect: (
    reference: ReferenceAttrs,
    range: { from: number; to: number }
  ) => void
  /** Dismiss the panel without inserting. */
  onClose: () => void
  emptyLabel?: string
  loadingLabel?: string
  /** Accessible name for the listbox / tablist. */
  listboxLabel?: string
  /** Builds the live-region result count announcement. */
  countLabel?: (count: number) => string
  /** Non-selectable hint shown under a tab whose matches were capped. */
  moreLabel?: string
  /** Localized per-kind tab labels (English fallbacks apply when omitted). */
  tabLabels?: Record<ReferenceKind, string>
  /**
   * The composer box the panel lines up with. When given, the panel adopts that
   * box's width and left edge and opens above it — the same geometry as the
   * host's own `/` command menu, so both panels read as one affordance. Without
   * it the panel keeps its fixed width and hugs the caret.
   */
  anchorRef?: RefObject<HTMLElement | null>
  /**
   * Reports the active option's element id (or null when nothing is
   * selectable), so the host can mirror it onto the editor's
   * `aria-activedescendant`. Must be referentially stable.
   */
  onActiveOptionChange?: (optionId: string | null) => void
}

/**
 * The unified `@` panel: tabbed, keyboard-navigable suggestions anchored to the
 * composer box (or, without one, to the caret). One tab per reference kind
 * (agent first); only the active tab's group is shown. Keys are forwarded from
 * the suggestion plugin via the imperative handle (the editor keeps DOM focus),
 * so selection and the active tab are tracked manually rather than relying on
 * focus-based libraries — the tab strip never takes focus (`tabIndex={-1}` +
 * mousedown `preventDefault`).
 */
export const SuggestionPopup = forwardRef<
  SuggestionPopupHandle,
  SuggestionPopupProps
>(function SuggestionPopup(
  {
    state,
    search,
    onSelect,
    onClose,
    emptyLabel = "No matches",
    loadingLabel = "Searching…",
    listboxLabel = "Mentions",
    countLabel = (count) => `${count} results`,
    moreLabel = "More results — keep typing to filter",
    tabLabels = DEFAULT_TAB_LABELS,
    anchorRef,
    onActiveOptionChange,
  },
  ref
) {
  // Results are tagged with the query they answer. While that tag doesn't match
  // the live query (initial mount, or mid-debounce after the query changed) the
  // panel is "stale": it shows loading and nothing is selectable, so Enter can
  // never insert a row from a previous query.
  const [result, setResult] = useState<{
    // null until the first fetch resolves, so results read as "stale"
    // (and the panel shows loading) before any search has answered.
    query: string | null
    groups: SuggestionGroup[]
  }>({ query: null, groups: [] })
  const [selectedIndex, setSelectedIndex] = useState(0)
  // The tab the user explicitly chose (via Tab/click), or null to auto-follow
  // the first non-empty tab. Pinning survives subsequent keystrokes within this
  // open session; reopening the panel remounts and resets it to null.
  const [pinnedTab, setPinnedTab] = useState<ReferenceKind | null>(null)
  const [pos, setPos] = useState<{
    left: number
    top: number
    placement: "above" | "below"
  } | null>(null)
  // Width adopted from the anchor box (0 = no anchor / not measured yet, where
  // the panel keeps its own `w-80`).
  const [boxWidth, setBoxWidth] = useState(0)
  const listRef = useRef<HTMLDivElement>(null)
  const stale = result.query !== state.query

  // Debounced, abortable fetch on every query change. All state updates run
  // inside the (async) timer callback, never synchronously in the effect body.
  useEffect(() => {
    const abort = new AbortController()
    let active = true
    const timer = setTimeout(() => {
      Promise.resolve(search(state.query, abort.signal))
        .then((groups) => {
          if (!active || abort.signal.aborted) return
          setResult({ query: state.query, groups })
          setSelectedIndex(0)
        })
        .catch(() => {
          if (!active || abort.signal.aborted) return
          setResult({ query: state.query, groups: [] })
          setSelectedIndex(0)
        })
    }, FETCH_DEBOUNCE_MS)
    return () => {
      active = false
      abort.abort()
      clearTimeout(timer)
    }
  }, [state.query, search])

  const groupByKind = useMemo(
    () => new Map(result.groups.map((group) => [group.kind, group])),
    [result.groups]
  )
  // Auto-target the first non-empty tab (agent-first) until the user pins one,
  // so a file/session/… query never strands the user on an empty agent tab.
  const firstNonEmpty = useMemo(
    () =>
      TAB_ORDER.find(
        (kind) => (groupByKind.get(kind)?.items.length ?? 0) > 0
      ) ?? TAB_ORDER[0],
    [groupByKind]
  )
  const activeTab = pinnedTab ?? firstNonEmpty
  const activeGroup = useMemo(
    () => (stale ? null : (groupByKind.get(activeTab) ?? null)),
    [stale, groupByKind, activeTab]
  )
  // Only the active tab's fresh items are selectable; selection resets to 0 on
  // each fetch and on every tab switch.
  const flat = useMemo(
    () => (stale || !activeGroup ? [] : activeGroup.items),
    [stale, activeGroup]
  )

  // Scroll the active option into view (scoped to options so it never targets
  // the active tab button, which also carries an active marker via class only).
  useEffect(() => {
    listRef.current
      ?.querySelector('[role="option"][data-active="true"]')
      ?.scrollIntoView({ block: "nearest" })
  }, [selectedIndex, activeTab])

  // Mirror the active option's id to the host (→ editor `aria-activedescendant`).
  // Null while nothing is selectable (loading / no matches in the active tab).
  useEffect(() => {
    onActiveOptionChange?.(
      stale || flat.length === 0
        ? null
        : mentionOptionId(activeTab, selectedIndex)
    )
  }, [activeTab, selectedIndex, flat.length, stale, onActiveOptionChange])

  // Position the panel within the viewport. Measure the rendered panel (a
  // `visibility:hidden` box still has layout), read the *live* anchor — the
  // composer box when the host named one, else the caret rect — then clamp/flip
  // via the pure helper. A layout effect runs before paint, so the panel never
  // flashes at a wrong spot. `state` is a fresh object each keystroke and the
  // height tracks `stale`/`flat.length`/`activeTab`, so this re-anchors as the
  // caret moves, results load, and tabs switch; resize + capture-phase scroll
  // listeners re-anchor on window resize, editor scroll, or page scroll while
  // the panel is open.
  //
  // With an anchor box the width is adopted BEFORE the height is measured, in
  // two passes: a fixed panel with no width shrinks to its content, and a row
  // that fits on one line there can wrap once the (different) composer width
  // lands — measuring first would place the panel using a height it is about to
  // outgrow. The panel stays hidden through both passes.
  useIsomorphicLayoutEffect(() => {
    if (typeof window === "undefined") return
    const reposition = () => {
      const panel = listRef.current
      if (!panel) return
      const box = anchorRef?.current?.getBoundingClientRect() ?? null
      if (box && Math.abs(box.width - boxWidth) > 0.5) {
        // Pass one. `boxWidth` is a dep, so this effect re-runs against the
        // re-laid-out panel and falls through below.
        setBoxWidth(box.width)
        return
      }
      const rect = panel.getBoundingClientRect()
      const caret = state.getClientRect?.() ?? null
      const anchor = box
        ? { left: box.left, top: box.top, bottom: box.bottom }
        : caret
          ? { left: caret.left, top: caret.top, bottom: caret.bottom }
          : null
      setPos(
        placeAnchoredPopup(
          anchor,
          { width: box ? box.width : rect.width, height: rect.height },
          { width: window.innerWidth, height: window.innerHeight }
        )
      )
    }
    reposition()
    window.addEventListener("resize", reposition)
    window.addEventListener("scroll", reposition, true)
    return () => {
      window.removeEventListener("resize", reposition)
      window.removeEventListener("scroll", reposition, true)
    }
  }, [state, stale, flat.length, activeTab, anchorRef, boxWidth])

  useImperativeHandle(
    ref,
    (): SuggestionPopupHandle => ({
      onKeyDown: (event) => {
        // Mid-composition the key is the IME's: on WebKit the Enter that picks
        // a CJK candidate arrives after `compositionend`, so the plugin's
        // `allow` gate has already reopened and only the event itself says so.
        if (isImeCompositionKey(event)) return false
        switch (event.key) {
          case "ArrowDown":
            if (flat.length > 0) {
              setSelectedIndex((index) => (index + 1) % flat.length)
            }
            return true
          case "ArrowUp":
            if (flat.length > 0) {
              setSelectedIndex(
                (index) => (index - 1 + flat.length) % flat.length
              )
            }
            return true
          case "Tab": {
            // Tab / Shift+Tab move between tabs (pinning the choice); Enter still
            // selects. Wraps around the five tabs.
            const dir = event.shiftKey ? -1 : 1
            const at = TAB_ORDER.indexOf(activeTab)
            setPinnedTab(
              TAB_ORDER[(at + dir + TAB_ORDER.length) % TAB_ORDER.length]
            )
            setSelectedIndex(0)
            return true
          }
          case "Enter": {
            const chosen = flat[selectedIndex]
            if (chosen) onSelect(chosen.reference, state.range)
            // No fresh row (still loading, or empty tab): consume without
            // inserting or submitting. Escape dismisses the panel.
            return true
          }
          case "Escape":
            onClose()
            return true
          default:
            return false
        }
      },
    }),
    [flat, selectedIndex, activeTab, onSelect, onClose, state.range]
  )

  const activeLabel = tabLabels[activeTab]
  const truncated = !stale && activeGroup?.truncated === true
  const liveStatus = stale
    ? loadingLabel
    : flat.length === 0
      ? `${activeLabel}: ${emptyLabel}`
      : truncated
        ? `${activeLabel}: ${countLabel(flat.length)} ${moreLabel}`
        : `${activeLabel}: ${countLabel(flat.length)}`

  return createPortal(
    <div
      style={{
        position: "fixed",
        left: pos?.left ?? 0,
        top: pos?.top ?? 0,
        // Hidden until the first measure positions it (avoids a flash at 0,0).
        visibility: pos ? "visible" : "hidden",
        zIndex: 50,
        // The panel portals to `body`, and a modal Radix layer (a Dialog or
        // Sheet hosting the composer) sets `pointer-events: none` on `body` —
        // only the layer itself is re-enabled. Without this the panel is
        // click-dead there and the press lands on the document instead, which
        // the layer reads as an outside press and closes itself. Radix's
        // outside test walks the REACT tree, so a press that does reach the
        // panel is correctly seen as inside the host.
        pointerEvents: "auto",
      }}
      data-placement={pos?.placement}
    >
      <div
        ref={listRef}
        data-testid="mention-popup"
        style={{
          // Match the composer box when one was named; `w-80` below is the
          // no-anchor fallback (and covers the pre-measure frame, which is
          // hidden anyway).
          width: boxWidth || undefined,
        }}
        // Cap to the viewport (minus the 8px×2 edge margin = 1rem) so the panel
        // always fits on small windows; the tab strip stays pinned and only the
        // option list scrolls. The positioner clamps placement, this bounds size.
        className="flex max-h-[min(18rem,calc(100dvh_-_1rem))] w-80 max-w-[calc(100vw_-_1rem)] flex-col overflow-hidden rounded-xl border border-border bg-popover text-popover-foreground shadow-lg"
      >
        {/* Tab strip: pointer-/key-driven only (tabIndex=-1 keeps editor focus).
            Each tab controls the single listbox below (no role=tabpanel, which
            cannot legally wrap a listbox). */}
        <div
          role="tablist"
          aria-label={listboxLabel}
          aria-orientation="horizontal"
          className="flex shrink-0 gap-0.5 overflow-x-auto border-b border-border p-1"
        >
          {TAB_ORDER.map((kind) => {
            const isActive = kind === activeTab
            const count = stale ? 0 : (groupByKind.get(kind)?.items.length ?? 0)
            return (
              <button
                key={kind}
                type="button"
                role="tab"
                tabIndex={-1}
                aria-selected={isActive}
                aria-controls={MENTION_LISTBOX_ID}
                // mousedown only prevents the focus shift (keeps the editor
                // focused so aria-activedescendant stays valid); the switch runs
                // on click so AT / synthetic activation (which fires click, not
                // mousedown) works too.
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => {
                  setPinnedTab(kind)
                  setSelectedIndex(0)
                }}
                className={cn(
                  "flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-xs font-medium",
                  isActive
                    ? "bg-accent text-accent-foreground"
                    : "text-muted-foreground hover:bg-accent/50"
                )}
              >
                <span>{tabLabels[kind]}</span>
                {!stale && count > 0 && (
                  <span className="rounded bg-muted px-1 text-[0.7rem] tabular-nums text-muted-foreground">
                    {count}
                  </span>
                )}
              </button>
            )
          })}
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-1">
          {/* Status text lives *outside* the listbox: a listbox may only own
              options. (The sr-only live region below announces it to AT.) */}
          {stale ? (
            <div className="px-2 py-3 text-sm text-muted-foreground">
              {loadingLabel}
            </div>
          ) : flat.length === 0 ? (
            <div className="px-2 py-3 text-sm text-muted-foreground">
              {emptyLabel}
            </div>
          ) : null}
          {/* Always rendered (even empty) so the editor's `aria-controls` target
              always resolves; holds only option children for the active tab. */}
          <div
            id={MENTION_LISTBOX_ID}
            role="listbox"
            aria-label={`${listboxLabel}: ${activeLabel}`}
          >
            {!stale &&
              activeGroup?.items.map((item, index) => {
                const active = index === selectedIndex
                return (
                  <button
                    key={`${activeGroup.kind}:${item.reference.id}`}
                    type="button"
                    id={mentionOptionId(activeGroup.kind, index)}
                    role="option"
                    aria-selected={active}
                    data-active={active}
                    className={cn(
                      "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm",
                      active
                        ? "bg-accent text-accent-foreground"
                        : "hover:bg-accent/50"
                    )}
                    onMouseDown={(event) => {
                      // Keep editor focus; insert on click.
                      event.preventDefault()
                      onSelect(item.reference, state.range)
                    }}
                    onMouseEnter={() => setSelectedIndex(index)}
                  >
                    <ReferenceIcon data={item.reference} variant="option" />
                    <span
                      className="flex-1 truncate"
                      title={item.reference.label || item.reference.id}
                    >
                      {item.reference.label || item.reference.id}
                    </span>
                    {item.detail && (
                      <span
                        className="max-w-[10rem] truncate text-xs text-muted-foreground"
                        title={item.detail}
                      >
                        {item.detail}
                      </span>
                    )}
                  </button>
                )
              })}
          </div>
          {truncated && (
            // aria-hidden: a visual "refine" affordance, not an option — keeps
            // the listbox owning only options (the live region conveys
            // truncation to AT). Never enters `flat`, so Enter can't select it.
            <div
              aria-hidden
              className="px-2 py-1 text-xs italic text-muted-foreground"
            >
              {moreLabel}
            </div>
          )}
        </div>
      </div>
      {/* Announce loading / active tab + result count / empty state to screen
          readers; the listbox keeps no focus, so AT relies on this live region. */}
      <div role="status" aria-live="polite" className="sr-only">
        {liveStatus}
      </div>
    </div>,
    document.body
  )
})
