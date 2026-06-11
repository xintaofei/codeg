"use client"

import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react"
import { createPortal } from "react-dom"

import { cn } from "@/lib/utils"

import { ReferenceIcon } from "../badges/reference-badge"
import type { ReferenceAttrs } from "../types"
import type { MentionRenderState } from "./mention-suggestion"
import { placeMentionPopup } from "./popup-position"
import type {
  ReferenceSearch,
  SuggestionGroup,
  SuggestionPopupHandle,
} from "./types"

const FETCH_DEBOUNCE_MS = 150

// Commit-synchronous in the browser so the panel is positioned before paint (no
// flash at a stale spot); a no-op-safe passive effect during the static-export
// prerender where `useLayoutEffect` would warn.
const useIsomorphicLayoutEffect =
  typeof window !== "undefined" ? useLayoutEffect : useEffect

/**
 * `id` of the listbox element and of each option. The editor's contentEditable
 * (which keeps DOM focus) points `aria-controls` at the listbox and
 * `aria-activedescendant` at the active option, the standard combobox pattern
 * for a popup that doesn't take focus. Only one panel is open at a time (the
 * focused editor's), so fixed ids never collide.
 */
export const MENTION_LISTBOX_ID = "mention-listbox"
export const mentionOptionId = (index: number) => `mention-option-${index}`

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
  /** Accessible name for the listbox. */
  listboxLabel?: string
  /** Builds the live-region result count announcement. */
  countLabel?: (count: number) => string
  /** Non-selectable hint shown under a group whose matches were capped. */
  moreLabel?: string
  /**
   * Reports the active option's element id (or null when nothing is
   * selectable), so the host can mirror it onto the editor's
   * `aria-activedescendant`. Must be referentially stable.
   */
  onActiveOptionChange?: (optionId: string | null) => void
}

interface FlatRow {
  item: SuggestionGroup["items"][number]
  groupIndex: number
}

/**
 * The unified `@` panel: grouped, keyboard-navigable suggestions positioned at
 * the caret. Keys are forwarded from the suggestion plugin via the imperative
 * handle (the editor keeps DOM focus), so selection is tracked manually rather
 * than relying on focus-based libraries.
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
  const [pos, setPos] = useState<{
    left: number
    top: number
    placement: "above" | "below"
  } | null>(null)
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

  // Only fresh results are selectable; selection resets to 0 on each fetch.
  const flat = useMemo<FlatRow[]>(
    () =>
      stale
        ? []
        : result.groups.flatMap((group, groupIndex) =>
            group.items.map((item) => ({ item, groupIndex }))
          ),
    [stale, result.groups]
  )

  // Scroll the active row into view.
  useEffect(() => {
    listRef.current
      ?.querySelector('[data-active="true"]')
      ?.scrollIntoView({ block: "nearest" })
  }, [selectedIndex])

  // Mirror the active option's id to the host (→ editor `aria-activedescendant`).
  // Null while nothing is selectable (loading / no matches), so the editor never
  // points at a stale or absent option.
  useEffect(() => {
    onActiveOptionChange?.(
      stale || flat.length === 0 ? null : mentionOptionId(selectedIndex)
    )
  }, [selectedIndex, flat.length, stale, onActiveOptionChange])

  // Position the caret-anchored panel within the viewport. Measure the rendered
  // panel (a `visibility:hidden` box still has layout), read the *live* caret
  // rect, then clamp/flip via the pure helper. A layout effect runs before
  // paint, so the panel never flashes at a wrong spot. `state` is a fresh object
  // each keystroke and the height tracks `stale`/`flat.length`, so this
  // re-anchors as the caret moves and results load; resize + capture-phase
  // scroll listeners re-anchor on window resize, editor scroll, or page scroll
  // while the panel is open (the caret getter returns fresh coords each call).
  useIsomorphicLayoutEffect(() => {
    if (typeof window === "undefined") return
    const reposition = () => {
      const panel = listRef.current
      if (!panel) return
      const rect = panel.getBoundingClientRect()
      const caret = state.getClientRect?.() ?? null
      setPos(
        placeMentionPopup(
          caret
            ? { left: caret.left, top: caret.top, bottom: caret.bottom }
            : null,
          { width: rect.width, height: rect.height },
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
  }, [state, stale, flat.length])

  useImperativeHandle(
    ref,
    (): SuggestionPopupHandle => ({
      onKeyDown: (event) => {
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
          case "Enter":
          case "Tab": {
            const chosen = flat[selectedIndex]
            if (chosen) onSelect(chosen.item.reference, state.range)
            // No fresh row (still loading, or no matches): consume the key
            // without inserting or submitting. Escape dismisses the panel.
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
    [flat, selectedIndex, onSelect, onClose, state.range]
  )

  const anyTruncated = !stale && result.groups.some((group) => group.truncated)
  const liveStatus = stale
    ? loadingLabel
    : flat.length === 0
      ? emptyLabel
      : anyTruncated
        ? `${countLabel(flat.length)} ${moreLabel}`
        : countLabel(flat.length)
  let rowIndex = -1

  return createPortal(
    <div
      style={{
        position: "fixed",
        left: pos?.left ?? 0,
        top: pos?.top ?? 0,
        // Hidden until the first measure positions it (avoids a flash at 0,0).
        visibility: pos ? "visible" : "hidden",
        zIndex: 50,
      }}
      data-placement={pos?.placement}
    >
      <div
        ref={listRef}
        data-testid="mention-popup"
        // Cap to the viewport (minus the 8px×2 edge margin = 1rem) so the panel
        // can always fit on small windows and scroll internally rather than
        // overflowing — the positioner clamps placement, this bounds the size.
        className="max-h-[min(18rem,calc(100dvh_-_1rem))] w-80 max-w-[calc(100vw_-_1rem)] overflow-y-auto rounded-xl border border-border bg-popover p-1 text-popover-foreground shadow-lg"
      >
        {/* Status text lives *outside* the listbox: a listbox may only own
            options/groups. (The sr-only live region below announces it to AT.) */}
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
            always resolves; holds only option/group children. */}
        <div id={MENTION_LISTBOX_ID} role="listbox" aria-label={listboxLabel}>
          {!stale &&
            result.groups.map((group) =>
              group.items.length === 0 ? null : (
                <div
                  key={group.kind}
                  role="group"
                  aria-label={group.label}
                  className="py-0.5"
                >
                  <div
                    aria-hidden
                    className="px-2 py-1 text-xs font-medium text-muted-foreground"
                  >
                    {group.label}
                  </div>
                  {group.items.map((item) => {
                    rowIndex += 1
                    const active = rowIndex === selectedIndex
                    const index = rowIndex
                    return (
                      <button
                        key={`${group.kind}:${item.reference.id}`}
                        type="button"
                        id={mentionOptionId(index)}
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
                        <ReferenceIcon data={item.reference} />
                        <span className="flex-1 truncate">
                          {item.reference.label || item.reference.id}
                        </span>
                        {item.detail && (
                          <span className="max-w-[10rem] truncate text-xs text-muted-foreground">
                            {item.detail}
                          </span>
                        )}
                      </button>
                    )
                  })}
                  {group.truncated && (
                    // aria-hidden: a visual "refine" affordance, not an option —
                    // keeps the listbox owning only options (the live region
                    // conveys truncation to AT). Never enters `flat`, so Enter
                    // can't select it.
                    <div
                      aria-hidden
                      className="px-2 py-1 text-xs italic text-muted-foreground"
                    >
                      {moreLabel}
                    </div>
                  )}
                </div>
              )
            )}
        </div>
      </div>
      {/* Announce loading / result count / empty state to screen readers; the
          listbox keeps no focus, so AT relies on this polite live region. */}
      <div role="status" aria-live="polite" className="sr-only">
        {liveStatus}
      </div>
    </div>,
    document.body
  )
})
