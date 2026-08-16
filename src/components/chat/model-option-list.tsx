"use client"

import { useCallback, useId, useMemo, useRef, useState } from "react"
import { Check, Search } from "lucide-react"
import { Virtualizer, type VirtualizerHandle } from "virtua"
import { useImeGuard } from "@/hooks/use-ime-guard"
import { cn } from "@/lib/utils"
import { ScrollArea } from "@/components/ui/scroll-area"
import { DropdownRadioItemContent } from "@/components/chat/dropdown-radio-item-content"
import {
  filterModelGroups,
  flattenModelGroups,
  type ModelOptionGroup,
} from "@/lib/model-config-groups"

interface ModelOptionListProps {
  groups: ModelOptionGroup[]
  currentValue: string
  onSelect: (value: string) => void
  searchPlaceholder: string
  searchAriaLabel: string
  listAriaLabel: string
  emptyLabel: string
  /** Focus the search box on mount (the wide popover opens straight into it). */
  autoFocus?: boolean
}

// Coarse per-row viewport estimate (headers are shorter, two-line options
// taller) — only sizes the scroll window; virtua measures real rows itself.
const ROW_ESTIMATE_PX = 44
const MAX_LIST_HEIGHT_PX = 320
export const CURSOR_CURRENT_UNAVAILABLE_VALUE =
  "__codeg_cursor_current_unavailable__"

function isSelectableModelValue(value: string): boolean {
  return value !== CURSOR_CURRENT_UNAVAILABLE_VALUE
}

// Searchable, virtualized model list shared by both selector forms (the wide
// popover and the collapsed cog panel). Deliberately NOT a Radix menu and NOT
// cmdk: a Radix menu's roving focus over hundreds of items is the scroll jank we
// are fixing, and cmdk + virtua was the combination that previously broke item
// clicks. Instead: a plain search box drives a virtua `Virtualizer` (scrolled by
// an OverlayScrollbars `ScrollArea` — a real DOM scrollbar the popover contains,
// so grabbing it keeps Radix's pointer path clean; the focus bounce it still
// triggers on WebKit is handled by `useScrollbarSafeDismiss`) of plain option
// buttons, with arrow/Enter keyboard handled here and listbox a11y on the list.
export function ModelOptionList({
  groups,
  currentValue,
  onSelect,
  searchPlaceholder,
  searchAriaLabel,
  listAriaLabel,
  emptyLabel,
  autoFocus = false,
}: ModelOptionListProps) {
  const ime = useImeGuard()
  const [query, setQuery] = useState("")
  const [activeIndex, setActiveIndex] = useState(0)
  const virtualizerRef = useRef<VirtualizerHandle>(null)
  // virtua scrolls the real OverlayScrollbars viewport (surfaced by ScrollArea's
  // `onViewportRef` once OS initializes). We keep both a ref (for the Virtualizer
  // `scrollRef` prop) and a state flag so the Virtualizer only mounts after that
  // viewport exists — reading it synchronously at mount is racy (OS defers init).
  const viewportRef = useRef<HTMLElement | null>(null)
  const [viewportEl, setViewportEl] = useState<HTMLElement | null>(null)
  const handleViewportRef = useCallback((element: HTMLElement | null) => {
    viewportRef.current = element
    setViewportEl(element)
  }, [])
  const baseId = useId()
  const listId = `${baseId}-list`
  const optionId = useCallback(
    (optionIndex: number) => `${baseId}-opt-${optionIndex}`,
    [baseId]
  )

  const rows = useMemo(
    () => flattenModelGroups(filterModelGroups(groups, query)),
    [groups, query]
  )
  // Flat row indices that are options (skipping headers) — the keyboard cursor
  // walks these, and they map an option position back to its flat row index.
  const optionRowIndices = useMemo(
    () =>
      rows.flatMap((row, index) =>
        row.kind === "option" && isSelectableModelValue(row.option.value)
          ? [index]
          : []
      ),
    [rows]
  )
  const optionCount = optionRowIndices.length
  // Reverse lookup (flat row index → keyboard option index) so each option row
  // can resolve its cursor position during render without a mutable counter.
  const optionIndexByRow = useMemo(() => {
    const map = new Map<number, number>()
    optionRowIndices.forEach((rowIndex, optionIndex) =>
      map.set(rowIndex, optionIndex)
    )
    return map
  }, [optionRowIndices])

  // Clamp on read so a shrinking filtered set (or a live groups update) can never
  // leave the cursor out of range — avoids a setState-in-effect just to re-clamp.
  const activeIndexClamped =
    optionCount === 0 ? 0 : Math.min(activeIndex, optionCount - 1)

  const moveActiveTo = useCallback(
    (next: number) => {
      if (optionCount === 0) return
      const clamped = Math.max(0, Math.min(optionCount - 1, next))
      setActiveIndex(clamped)
      virtualizerRef.current?.scrollToIndex(optionRowIndices[clamped], {
        align: "nearest",
      })
    },
    [optionCount, optionRowIndices]
  )

  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLInputElement>) => {
      // Don't steal Enter/arrows while an IME composition is in flight (CJK
      // input): Enter there confirms the candidate, it must not pick a model.
      if (ime.isComposing(event)) return
      switch (event.key) {
        case "ArrowDown":
          event.preventDefault()
          moveActiveTo(activeIndexClamped + 1)
          break
        case "ArrowUp":
          event.preventDefault()
          moveActiveTo(activeIndexClamped - 1)
          break
        case "Home":
          event.preventDefault()
          moveActiveTo(0)
          break
        case "End":
          event.preventDefault()
          moveActiveTo(optionCount - 1)
          break
        case "Enter": {
          const rowIndex = optionRowIndices[activeIndexClamped]
          const row = rowIndex != null ? rows[rowIndex] : undefined
          if (row && row.kind === "option") {
            event.preventDefault()
            onSelect(row.option.value)
          }
          break
        }
        default:
          break
      }
    },
    [
      activeIndexClamped,
      ime,
      moveActiveTo,
      onSelect,
      optionCount,
      optionRowIndices,
      rows,
    ]
  )

  const listHeight = Math.min(
    MAX_LIST_HEIGHT_PX,
    Math.max(rows.length, 1) * ROW_ESTIMATE_PX
  )

  // Always keep the active row mounted so `aria-activedescendant` resolves to a
  // real element even after wheel-scrolling / filtering unmounts it off-screen.
  const activeFlatIndex = optionRowIndices[activeIndexClamped]

  return (
    <div className="flex min-w-0 flex-col">
      <div className="flex items-center gap-2 border-b px-2.5 py-2">
        <Search className="size-4 shrink-0 text-muted-foreground" />
        <input
          type="text"
          value={query}
          autoFocus={autoFocus}
          spellCheck={false}
          autoComplete="off"
          role="combobox"
          aria-expanded
          aria-controls={listId}
          aria-activedescendant={
            optionCount > 0 ? optionId(activeIndexClamped) : undefined
          }
          aria-label={searchAriaLabel}
          placeholder={searchPlaceholder}
          onChange={(event) => {
            setQuery(event.target.value)
            setActiveIndex(0)
          }}
          {...ime.props}
          onKeyDown={handleKeyDown}
          className="w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground"
        />
      </div>

      {optionCount === 0 ? (
        <div className="px-3 py-6 text-center text-sm text-muted-foreground">
          {emptyLabel}
        </div>
      ) : (
        // A real DOM scrollbar (OverlayScrollbars `ScrollArea`) that lives INSIDE
        // the Radix popover, replacing virtua's native `VList` scrollbar. Because
        // the content `contains()` it, Radix never reads grabbing it as an
        // outside *pointer* interaction. (The remaining WebKit dismiss — the grab
        // blurring focus to an outside element — is handled separately by
        // `useScrollbarSafeDismiss` on the popover.) The `Virtualizer` scrolls the
        // OS viewport (bound via `scrollRef`) and mounts only once that viewport
        // exists (surfaced through `onViewportRef`).
        <div style={{ height: listHeight }}>
          <ScrollArea onViewportRef={handleViewportRef} className="h-full">
            <div
              role="listbox"
              id={listId}
              aria-label={listAriaLabel}
              className="p-1"
            >
              {viewportEl ? (
                <Virtualizer
                  ref={virtualizerRef}
                  scrollRef={viewportRef}
                  keepMounted={
                    activeFlatIndex != null ? [activeFlatIndex] : undefined
                  }
                >
                  {rows.map((row, flatIndex) => {
                    if (row.kind === "header") {
                      return (
                        <div
                          key={row.key}
                          role="presentation"
                          className="truncate px-2 pt-2 pb-0.5 text-xs font-medium text-muted-foreground"
                        >
                          {row.name}
                        </div>
                      )
                    }
                    const optionIndex = optionIndexByRow.get(flatIndex)
                    const selectable = optionIndex !== undefined
                    const selected = row.option.value === currentValue
                    const active =
                      selectable && optionIndex === activeIndexClamped
                    return (
                      <button
                        key={row.key}
                        type="button"
                        role="option"
                        id={
                          selectable
                            ? optionId(optionIndex)
                            : `${baseId}-display-${flatIndex}`
                        }
                        aria-selected={selected}
                        disabled={!selectable}
                        title={row.option.name}
                        onMouseMove={() => {
                          if (selectable) setActiveIndex(optionIndex)
                        }}
                        onClick={() => {
                          if (selectable) onSelect(row.option.value)
                        }}
                        className={cn(
                          "flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left text-sm transition-colors",
                          active && "bg-accent text-accent-foreground",
                          selected && !active && "bg-accent/60",
                          !selectable && "cursor-not-allowed opacity-70"
                        )}
                      >
                        <span className="flex size-4 shrink-0 items-center justify-center pt-0.5">
                          {selected ? <Check className="size-4" /> : null}
                        </span>
                        <DropdownRadioItemContent
                          label={row.option.name}
                          description={row.option.description}
                        />
                      </button>
                    )
                  })}
                </Virtualizer>
              ) : null}
            </div>
          </ScrollArea>
        </div>
      )}
    </div>
  )
}
