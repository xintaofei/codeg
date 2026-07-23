"use client"

import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react"
import {
  Archive,
  ArchiveRestore,
  ChevronRight,
  CloudDownload,
  CloudSync,
  CloudUpload,
  Folder,
  FolderGit2,
  FolderOpen,
  GitBranch,
  GitBranchPlus,
  GitCommitHorizontal,
  GitMerge,
  GitPullRequestArrow,
  Globe,
  Loader2,
  Search,
  Trash2,
  type LucideIcon,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { Virtualizer, type VirtualizerHandle } from "virtua"
import { cn } from "@/lib/utils"
import { ScrollArea } from "@/components/ui/scroll-area"
import { branchRowPaddingLeft } from "@/components/layout/branch-tree-collapsible"
import {
  buildBranchRows,
  isNavigableRow,
  type BranchLeafAction,
  type BranchOperationMeta,
  type BranchRow,
} from "@/lib/branch-selector-rows"
import { collectGroupKeys } from "@/lib/branch-tree"
import type { BranchTreeNode, RemoteBranchSection } from "@/lib/branch-tree"

interface BranchSelectorListProps {
  /** Operations with pre-translated labels (search matches on these). */
  operations: BranchOperationMeta[]
  localNodes: BranchTreeNode[]
  remoteSections: RemoteBranchSection[]
  localCount: number
  remoteCount: number
  branch: string | null
  worktreeBranchSet: Set<string>
  /** First-load fetch in flight and nothing cached yet — show a spinner. */
  branchLoading: boolean
  /** A git task is running — disable operation + branch-action rows. */
  loading: boolean
  onRunOperation: (opId: string) => void
  onLeafAction: (
    action: BranchLeafAction,
    fullName: string,
    isRemote: boolean
  ) => void
}

// Coarse per-row estimate — only seeds the scroll window's height on the first
// paint, before the real content height is measured (see `measuredHeight`). A
// too-small estimate would otherwise leave a spurious scrollbar on a narrowed
// list, a too-big one dead space; virtua measures real rows itself either way.
const ROW_ESTIMATE_PX = 34
const MAX_LIST_HEIGHT_PX = 480
// Approximate width of the right-side action bubble (`w-56`) — used only to
// decide whether to flip it to the left when the popover hugs the screen edge.
const BUBBLE_WIDTH_PX = 224
// Coarse per-action height + container padding, used to clamp the bubble's top
// so a leaf near the bottom doesn't push its last actions past the list root.
// Slightly over-estimated on purpose so the whole bubble always fits.
const BUBBLE_ITEM_PX = 34
const BUBBLE_PAD_PX = 8

const OP_ICONS: Record<string, LucideIcon> = {
  pull: CloudDownload,
  fetch: CloudSync,
  commit: GitCommitHorizontal,
  push: CloudUpload,
  newBranch: GitBranchPlus,
  newWorktree: FolderGit2,
  stash: Archive,
  stashPop: ArchiveRestore,
  manageRemotes: Globe,
  init: GitBranch,
}

const ACTION_ICONS: Record<BranchLeafAction, LucideIcon> = {
  switch: GitBranch,
  merge: GitMerge,
  rebase: GitPullRequestArrow,
  delete: Trash2,
  deleteRemote: Trash2,
}

// The per-branch actions shown in the right-side bubble. Delete is hidden for the
// remote branch the current local branch tracks (deleting it is nonsensical).
function leafActions(
  isRemote: boolean,
  isTracking: boolean
): BranchLeafAction[] {
  const actions: BranchLeafAction[] = ["switch", "merge", "rebase"]
  if (!isTracking) actions.push(isRemote ? "deleteRemote" : "delete")
  return actions
}

interface ActionBubble {
  leafKey: string
  fullName: string
  isRemote: boolean
  actions: BranchLeafAction[]
  /** Nav-cursor index of the owner leaf, so hovering the bubble re-highlights it. */
  ownerNavIndex: number
  /** Top offset (px) of the bubble relative to the list root (clamped to fit). */
  top: number
  /** Flip to the left of the popover when there's no room on the right. */
  flipLeft: boolean
}

// Searchable + virtualized branch panel: operations and the local/remote branch
// tree in ONE flat list (modeled on `ModelOptionList` — a plain search box
// driving a virtua `Virtualizer` scrolled by an OverlayScrollbars `ScrollArea`,
// NOT cmdk). Sections default open, prefix groups default collapsed (see the
// `overrides`/`defaultCollapsedGroups` split below); rows toggle either.
// Clicking a leaf opens a right-side action bubble — a plain DOM element inside
// this single Popover layer, never a nested Radix layer (that silently drops
// selection on WKWebView; see session-selectors-panel.tsx).
export function BranchSelectorList({
  operations,
  localNodes,
  remoteSections,
  localCount,
  remoteCount,
  branch,
  worktreeBranchSet,
  branchLoading,
  loading,
  onRunOperation,
  onLeafAction,
}: BranchSelectorListProps) {
  const t = useTranslations("Folder.branchDropdown")

  const [query, setQuery] = useState("")
  // Prefix groups default to COLLAPSED, sections to OPEN. That's modeled as a
  // default-collapsed set (every prefix-group key) plus `overrides`, the user's
  // explicit toggles (true = force-open, false = force-collapse). The popover
  // unmounts on close, so both reset on each open.
  const [overrides, setOverrides] = useState<Map<string, boolean>>(
    () => new Map()
  )
  const [activeIndex, setActiveIndex] = useState(0)
  const [bubble, setBubble] = useState<ActionBubble | null>(null)
  const [bubbleActiveIndex, setBubbleActiveIndex] = useState(0)
  // Real rendered height of the row list, measured once the virtua viewport
  // exists (null until then → the coarse estimate seeds the first paint).
  const [measuredHeight, setMeasuredHeight] = useState<number | null>(null)

  const rootRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const listboxRef = useRef<HTMLDivElement>(null)
  const virtualizerRef = useRef<VirtualizerHandle>(null)
  const viewportRef = useRef<HTMLElement | null>(null)
  const [viewportEl, setViewportEl] = useState<HTMLElement | null>(null)
  const handleViewportRef = useCallback((element: HTMLElement | null) => {
    viewportRef.current = element
    setViewportEl(element)
  }, [])

  const baseId = useId()
  const listId = `${baseId}-list`
  const optionId = useCallback(
    (index: number) => `${baseId}-opt-${index}`,
    [baseId]
  )

  const closeBubble = useCallback(() => {
    setBubble((b) => (b ? null : b))
  }, [])

  // Type-ahead: while the popup is open, typing should filter even if the search
  // box doesn't hold focus (Radix focus management, or focus drifting onto a
  // branch row after a click on WebKit). A document-level listener catches keys
  // wherever focus landed — including body / the PopoverContent wrapper, which a
  // handler on an inner div would miss. It no-ops when the input IS focused, so
  // the controlled onChange stays the single writer there (no double-insert).
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const input = inputRef.current
      const active = document.activeElement
      if (!input || active === input) return
      // Never hijack another editable field (e.g. the composer) that happens to
      // hold focus while the popup is still open — only re-home keys typed with
      // focus on a non-editable element (a branch row, body, the popover shell).
      if (
        active instanceof HTMLElement &&
        (active.tagName === "INPUT" ||
          active.tagName === "TEXTAREA" ||
          active.isContentEditable)
      )
        return
      if (event.isComposing || event.key === "Process") return
      const isPrintable =
        event.key.length === 1 &&
        !event.metaKey &&
        !event.ctrlKey &&
        !event.altKey
      if (isPrintable) {
        event.preventDefault()
        input.focus()
        setQuery((q) => q + event.key)
        setActiveIndex(0)
        closeBubble()
      } else if (event.key === "Backspace") {
        event.preventDefault()
        input.focus()
        setQuery((q) => q.slice(0, -1))
        setActiveIndex(0)
        closeBubble()
      }
    }
    document.addEventListener("keydown", onKeyDown)
    return () => document.removeEventListener("keydown", onKeyDown)
  }, [closeBubble])

  // Track the list's true rendered height so the scroll window is exactly as
  // tall as its rows (capped at the max), instead of trusting a per-row
  // estimate that under-/over-shoots. Attaching only after the virtua viewport
  // exists means the row content is already mounted, so we skip the padding-only
  // pre-mount state (which would briefly collapse the popup). The observer then
  // keeps firing as filtering grows/shrinks the list.
  useEffect(() => {
    const el = listboxRef.current
    if (!viewportEl || !el) return
    const observer = new ResizeObserver((entries) => {
      const box = entries[0]?.borderBoxSize?.[0]
      // Round up so the window is never a sub-pixel shorter than its content —
      // that fractional gap is exactly what makes the stray scrollbar appear.
      const next = Math.ceil(
        box ? box.blockSize : el.getBoundingClientRect().height
      )
      setMeasuredHeight((prev) =>
        prev != null && Math.abs(prev - next) < 1 ? prev : next
      )
    })
    observer.observe(el)
    return () => observer.disconnect()
  }, [viewportEl])

  // Prefix groups start folded: seed the default-collapsed set with every group
  // key in the current trees (sections + multi-remote wrappers are keyed outside
  // the tree, so they stay open).
  const defaultCollapsedGroups = useMemo(
    () =>
      new Set([
        ...collectGroupKeys(localNodes),
        ...remoteSections.flatMap((s) => collectGroupKeys(s.nodes)),
      ]),
    [localNodes, remoteSections]
  )

  // The set fed to buildBranchRows: defaults minus force-open overrides, plus
  // force-collapse overrides.
  const effectiveCollapsed = useMemo(() => {
    const set = new Set(defaultCollapsedGroups)
    for (const [key, expanded] of overrides) {
      if (expanded) set.delete(key)
      else set.add(key)
    }
    return set
  }, [defaultCollapsedGroups, overrides])

  const toggleCollapse = useCallback(
    (key: string) => {
      setOverrides((prev) => {
        const prevOverride = prev.get(key)
        const currentlyCollapsed =
          prevOverride !== undefined
            ? !prevOverride
            : defaultCollapsedGroups.has(key)
        const next = new Map(prev)
        // New expanded state = the negation of the current one = whether it was
        // collapsed.
        next.set(key, currentlyCollapsed)
        return next
      })
    },
    [defaultCollapsedGroups]
  )

  const rows = useMemo(
    () =>
      buildBranchRows({
        operations,
        localNodes,
        remoteSections,
        localCount,
        remoteCount,
        branch,
        worktreeBranchSet,
        collapsed: effectiveCollapsed,
        query,
      }),
    [
      operations,
      localNodes,
      remoteSections,
      localCount,
      remoteCount,
      branch,
      worktreeBranchSet,
      effectiveCollapsed,
      query,
    ]
  )

  // Flat indices of rows the keyboard cursor can land on (skips separators +
  // empty rows), plus the reverse lookup so each row resolves its cursor slot.
  const navigableRowIndices = useMemo(
    () => rows.flatMap((row, index) => (isNavigableRow(row) ? [index] : [])),
    [rows]
  )
  const navigableCount = navigableRowIndices.length
  const navigableIndexByRow = useMemo(() => {
    const map = new Map<number, number>()
    navigableRowIndices.forEach((rowIndex, navIndex) =>
      map.set(rowIndex, navIndex)
    )
    return map
  }, [navigableRowIndices])

  const activeIndexClamped =
    navigableCount === 0 ? 0 : Math.min(activeIndex, navigableCount - 1)

  const moveActiveTo = useCallback(
    (next: number) => {
      if (navigableCount === 0) return
      const clamped = Math.max(0, Math.min(navigableCount - 1, next))
      setActiveIndex(clamped)
      virtualizerRef.current?.scrollToIndex(navigableRowIndices[clamped], {
        align: "nearest",
      })
    },
    [navigableCount, navigableRowIndices]
  )

  const leafActionLabel = useCallback(
    (action: BranchLeafAction, fullName: string): string => {
      switch (action) {
        case "switch":
          return t("switchToBranch")
        case "merge":
          return t("mergeBranchIntoCurrent", {
            branchName: fullName,
            currentBranch: branch ?? "-",
          })
        case "rebase":
          return t("rebaseCurrentToBranch", {
            currentBranch: branch ?? "-",
            branchName: fullName,
          })
        case "delete":
        case "deleteRemote":
          return t("deleteBranch")
      }
    },
    [t, branch]
  )

  // Open (or toggle) the action bubble anchored to a leaf row element. The top
  // is clamped so the whole bubble stays inside the list root — a leaf near the
  // bottom would otherwise push its last actions past the edge and clip them.
  function openLeafBubble(
    row: Extract<BranchRow, { kind: "leaf" }>,
    el: HTMLElement | null,
    navIndex: number
  ) {
    if (bubble?.leafKey === row.key) {
      setBubble(null)
      return
    }
    const root = rootRef.current
    if (!el || !root) return
    const rowRect = el.getBoundingClientRect()
    const rootRect = root.getBoundingClientRect()
    const actions = leafActions(row.isRemote, row.isTracking)
    const bubbleHeight = actions.length * BUBBLE_ITEM_PX + BUBBLE_PAD_PX
    const top = Math.min(
      rowRect.top - rootRect.top,
      Math.max(0, rootRect.height - bubbleHeight)
    )
    setBubble({
      leafKey: row.key,
      fullName: row.fullName,
      isRemote: row.isRemote,
      actions,
      ownerNavIndex: navIndex,
      top,
      flipLeft: rootRect.right + 8 + BUBBLE_WIDTH_PX > window.innerWidth,
    })
    setBubbleActiveIndex(0)
    // Highlight the owner leaf (covers the keyboard path, where no hover fires).
    setActiveIndex(navIndex)
  }

  function activateRow(
    row: BranchRow,
    el: HTMLElement | null,
    navIndex: number
  ) {
    switch (row.kind) {
      case "operation":
        if (!loading) onRunOperation(row.opId)
        break
      case "section":
      case "group":
        closeBubble()
        toggleCollapse(row.key)
        break
      case "leaf":
        if (!row.isCurrent) openLeafBubble(row, el, navIndex)
        break
      default:
        break
    }
  }

  function selectBubbleAction(index: number) {
    if (!bubble) return
    const action = bubble.actions[index]
    if (!action || loading) return
    onLeafAction(action, bubble.fullName, bubble.isRemote)
    setBubble(null)
  }

  function handleKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    // Don't steal Enter/arrows while an IME composition is in flight (CJK).
    if (event.nativeEvent.isComposing || event.key === "Process") return

    // When the action bubble is open, arrows/Enter drive it; Escape/Left close
    // it; any other key closes it and falls through to normal list handling.
    if (bubble) {
      switch (event.key) {
        case "ArrowDown":
          event.preventDefault()
          setBubbleActiveIndex((i) =>
            Math.min(bubble.actions.length - 1, i + 1)
          )
          return
        case "ArrowUp":
          event.preventDefault()
          setBubbleActiveIndex((i) => Math.max(0, i - 1))
          return
        case "Enter":
          event.preventDefault()
          selectBubbleAction(bubbleActiveIndex)
          return
        case "Escape":
        case "ArrowLeft":
          event.preventDefault()
          closeBubble()
          return
        default:
          closeBubble()
      }
    }

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
        moveActiveTo(navigableCount - 1)
        break
      case "Enter": {
        const rowIndex = navigableRowIndices[activeIndexClamped]
        const row = rowIndex != null ? rows[rowIndex] : undefined
        if (row) {
          event.preventDefault()
          activateRow(
            row,
            document.getElementById(optionId(activeIndexClamped)),
            activeIndexClamped
          )
        }
        break
      }
      default:
        break
    }
  }

  // Prefer the real measured content height; fall back to the coarse per-row
  // estimate only until the first measurement lands.
  const listHeight = Math.min(
    MAX_LIST_HEIGHT_PX,
    measuredHeight ?? Math.max(rows.length, 1) * ROW_ESTIMATE_PX
  )
  const activeFlatIndex = navigableRowIndices[activeIndexClamped]
  const showSpinner = branchLoading && localCount === 0 && remoteCount === 0
  const isEmpty = !showSpinner && rows.length === 0

  const renderRow = (row: BranchRow, flatIndex: number) => {
    if (row.kind === "separator") {
      return (
        <div
          key={row.key}
          role="presentation"
          className="mx-2 my-1 h-px bg-border"
        />
      )
    }
    if (row.kind === "empty") {
      return (
        <div
          key={row.key}
          role="presentation"
          className="py-1.5 pr-3 text-sm text-muted-foreground/70"
          style={{ paddingLeft: branchRowPaddingLeft("dropdown", 1) }}
        >
          {row.scope === "local" ? t("noLocalBranches") : t("noRemoteBranches")}
        </div>
      )
    }

    const navIndex = navigableIndexByRow.get(flatIndex) ?? 0
    const active = navIndex === activeIndexClamped
    const rowClass = cn(
      "flex w-full select-none items-center gap-2 rounded-md py-1.5 pr-2 text-left text-sm outline-none transition-colors",
      active ? "bg-accent text-accent-foreground" : "hover:bg-accent/60"
    )

    if (row.kind === "operation") {
      const Icon = OP_ICONS[row.opId] ?? GitBranch
      return (
        <button
          key={row.key}
          type="button"
          role="option"
          id={optionId(navIndex)}
          aria-selected={active}
          disabled={loading}
          onMouseMove={() => setActiveIndex(navIndex)}
          onClick={(e) => activateRow(row, e.currentTarget, navIndex)}
          className={cn(
            rowClass,
            "disabled:pointer-events-none disabled:opacity-50",
            row.destructive && "text-destructive"
          )}
          style={{ paddingLeft: branchRowPaddingLeft("dropdown", 0) }}
        >
          <Icon className="size-3.5 shrink-0 opacity-70" />
          <span className="min-w-0 flex-1 truncate">{row.label}</span>
        </button>
      )
    }

    if (row.kind === "section" || row.kind === "group") {
      // Sections anchor the tree at depth 0; groups carry their own depth.
      const depth = row.kind === "section" ? 0 : row.depth
      const label =
        row.kind === "section"
          ? row.scope === "local"
            ? t("localBranches", { count: row.count })
            : t("remoteBranches", { count: row.count })
          : row.label
      return (
        <button
          key={row.key}
          type="button"
          role="option"
          id={optionId(navIndex)}
          aria-selected={active}
          onMouseMove={() => setActiveIndex(navIndex)}
          onClick={(e) => activateRow(row, e.currentTarget, navIndex)}
          className={rowClass}
          style={{ paddingLeft: branchRowPaddingLeft("dropdown", depth) }}
        >
          {/* Sections use a chevron; prefix groups use a folder icon. Icons run
              a touch lighter than the label (opacity, not a fixed muted color,
              so they stay legible on the active accent background too). */}
          {row.kind === "section" ? (
            <ChevronRight
              className={cn(
                "size-3.5 shrink-0 opacity-70 transition-transform",
                row.expanded && "rotate-90"
              )}
            />
          ) : row.expanded ? (
            <FolderOpen className="size-3.5 shrink-0 opacity-70" />
          ) : (
            <Folder className="size-3.5 shrink-0 opacity-70" />
          )}
          <span className="min-w-0 flex-1 truncate">{label}</span>
          {row.kind === "group" && (
            <span className="shrink-0 pl-2 text-xs text-muted-foreground/70">
              {row.count}
            </span>
          )}
        </button>
      )
    }

    // leaf. The active background is driven solely by `activeIndex` (which the
    // bubble keeps pointed at its owner leaf) — no separate "bubble open"
    // highlight, so hovering another row cleanly moves the single highlight.
    const LeafIcon = row.isWorktree ? FolderGit2 : GitBranch
    return (
      <button
        key={row.key}
        type="button"
        role="option"
        id={optionId(navIndex)}
        aria-selected={active}
        title={row.fullName}
        onMouseMove={() => setActiveIndex(navIndex)}
        onClick={(e) => activateRow(row, e.currentTarget, navIndex)}
        className={cn(rowClass, row.isCurrent && "opacity-50")}
        style={{ paddingLeft: branchRowPaddingLeft("dropdown", row.depth) }}
      >
        <LeafIcon className="size-3.5 shrink-0 opacity-70" />
        <span className="min-w-0 flex-1 truncate">{row.label}</span>
        {row.isCurrent ? (
          <span className="shrink-0 pl-2 text-xs text-muted-foreground">
            {t("current")}
          </span>
        ) : (
          <ChevronRight className="size-3 shrink-0 text-muted-foreground/50" />
        )}
      </button>
    )
  }

  return (
    <div ref={rootRef} className="relative flex min-w-0 flex-col">
      {/* Inner shell clips content to the popover rounding; the action bubble is
          a sibling of it so it can overflow past the popover's right edge (the
          PopoverContent is overflow-visible). */}
      <div className="flex min-w-0 flex-col overflow-hidden rounded-[inherit]">
        {/* pl-4 (16px) + size-3.5 icon + gap-2 puts the icon and input text at
            the same x as the rows below (listbox p-1 + 0.75rem row padding). */}
        <div className="flex items-center gap-2 border-b py-2 pl-4 pr-2.5">
          <Search className="size-3.5 shrink-0 text-muted-foreground" />
          <input
            ref={inputRef}
            type="text"
            value={query}
            autoFocus
            spellCheck={false}
            autoComplete="off"
            role="combobox"
            aria-expanded
            aria-controls={listId}
            aria-activedescendant={
              navigableCount > 0 ? optionId(activeIndexClamped) : undefined
            }
            aria-label={t("searchAriaLabel")}
            placeholder={t("searchPlaceholder")}
            onChange={(event) => {
              setQuery(event.target.value)
              setActiveIndex(0)
              closeBubble()
            }}
            onKeyDown={handleKeyDown}
            className="w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground"
          />
        </div>

        {showSpinner ? (
          <div className="flex items-center justify-center py-8">
            <Loader2 className="size-4 animate-spin text-muted-foreground" />
          </div>
        ) : isEmpty ? (
          <div className="px-3 py-6 text-center text-sm text-muted-foreground">
            {t("noMatches")}
          </div>
        ) : (
          <div style={{ height: listHeight }}>
            <ScrollArea onViewportRef={handleViewportRef} className="h-full">
              <div
                ref={listboxRef}
                role="listbox"
                id={listId}
                aria-label={t("branchListLabel")}
                className="p-1"
              >
                {viewportEl ? (
                  <Virtualizer
                    ref={virtualizerRef}
                    scrollRef={viewportRef}
                    onScroll={closeBubble}
                    keepMounted={
                      activeFlatIndex != null ? [activeFlatIndex] : undefined
                    }
                  >
                    {rows.map((row, flatIndex) => renderRow(row, flatIndex))}
                  </Virtualizer>
                ) : null}
              </div>
            </ScrollArea>
          </div>
        )}
      </div>

      {bubble ? (
        <div
          role="menu"
          aria-label={bubble.fullName}
          // Hovering the bubble points the single highlight back at its owner
          // leaf, so the source branch re-lights while you're over its actions.
          onMouseEnter={() => setActiveIndex(bubble.ownerNavIndex)}
          // Flush against the popup's edge (no margin) so it reads as attached.
          className="absolute z-50 w-56 rounded-xl border bg-popover p-1 shadow-lg"
          style={{
            top: bubble.top,
            ...(bubble.flipLeft ? { right: "100%" } : { left: "100%" }),
          }}
        >
          {bubble.actions.map((action, index) => {
            const ActionIcon = ACTION_ICONS[action]
            const destructive = action === "delete" || action === "deleteRemote"
            const bubbleActive = index === bubbleActiveIndex
            return (
              <button
                key={action}
                type="button"
                role="menuitem"
                disabled={loading}
                onMouseMove={() => setBubbleActiveIndex(index)}
                onClick={() => selectBubbleAction(index)}
                className={cn(
                  "flex w-full select-none items-center gap-2 rounded-md px-1.5 py-1.5 text-left text-sm outline-none transition-colors",
                  "disabled:pointer-events-none disabled:opacity-50",
                  bubbleActive && "bg-accent text-accent-foreground",
                  destructive && "text-destructive"
                )}
              >
                <ActionIcon className="size-3.5 shrink-0 opacity-70" />
                <span className="min-w-0 flex-1 truncate">
                  {leafActionLabel(action, bubble.fullName)}
                </span>
              </button>
            )
          })}
        </div>
      ) : null}
    </div>
  )
}
