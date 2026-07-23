"use client"

import { memo } from "react"
import {
  ChevronRight,
  Download,
  FolderGit2,
  FolderOpenDot,
  SquarePen,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { cn } from "@/lib/utils"

/**
 * Collapsible heading for one of the two top-level sidebar sections: "pinned"
 * (shown only when there are pinned conversations) and "folders" (wraps the
 * whole folder list). One flat row in the virtualized list, height-matched to
 * every other row (`h-[2rem]`) so `virtua`'s fixed item-size estimate stays
 * accurate.
 *
 * The label sits first and the disclosure chevron trails it, revealed only on
 * hover/focus (and always on touch, which has no hover). The chevron rotates via
 * the React `expanded` prop, not a Radix `data-*` variant — this repo's Radix
 * only emits `data-state`, so a bare `data-open:` style would be a no-op. Own
 * the translations here (rather than receiving `t`) so next-intl's
 * fresh-per-render `t` never defeats the memo.
 */
export const SidebarSectionHeader = memo(function SidebarSectionHeader({
  section,
  expanded,
  onToggle,
  onNewChat,
  onOpenFolder,
  onCloneRepository,
  onImportSessions,
  topGap = false,
}: {
  section: "pinned" | "folders" | "chats"
  expanded: boolean
  onToggle: (section: "pinned" | "folders" | "chats") => void
  /**
   * When provided on the "chats" section, renders a New-chat action button at
   * the row's right edge, revealed only while the row is hovered/focused (and
   * always on touch, which has no hover). A sibling of — not nested in — the
   * toggle button (nesting buttons is invalid HTML), so clicking it never
   * toggles the section. Must be referentially stable to preserve the memo.
   */
  onNewChat?: () => void
  /**
   * When provided on the "folders" section, render two right-edge action
   * buttons — Open Folder and Clone Repository — the same "add a folder"
   * actions as the top-of-page NewFolderDropdown, revealed on hover/focus (and
   * always on touch). Siblings of — not nested in — the toggle button, so
   * clicking never toggles the section. Must be referentially stable to
   * preserve the memo.
   */
  onOpenFolder?: () => void
  onCloneRepository?: () => void
  /**
   * When provided on the "folders" section, renders the global "Import local
   * sessions" action (opens the import-picker window, no folder anchor) in the
   * same right-edge cluster. Must be referentially stable to preserve the memo.
   */
  onImportSessions?: () => void
  /**
   * Adds breathing room above the header so the "Folders" section reads as
   * visually separated from the "Pinned" section above it. Implemented as
   * padding (not margin) on a wrapper so the row's measured border-box grows —
   * `virtua` reads the real height via ResizeObserver, so the extra space is
   * accounted for instead of overlapping the previous row.
   */
  topGap?: boolean
}) {
  const t = useTranslations("Folder.sidebar")
  // Tooltips/aria-labels for the folders-section actions reuse the existing
  // top-of-page dropdown strings (Open Folder / Clone Repository), so no new
  // locale keys are introduced. Owned here (not received) to preserve the memo,
  // same as `t`.
  const tDropdown = useTranslations("Folder.folderNameDropdown")
  const label =
    section === "pinned"
      ? t("sectionPinned")
      : section === "chats"
        ? t("sectionChats")
        : t("sectionFolders")
  const showNewChat = section === "chats" && onNewChat != null
  // The folders section mirrors the chats section's right-edge affordance, but
  // with two buttons (Open Folder / Clone Repository) — the same "add a folder"
  // actions the top-of-page NewFolderDropdown offers.
  const showFolderActions =
    section === "folders" &&
    (onOpenFolder != null ||
      onCloneRepository != null ||
      onImportSessions != null)
  // Shared styling for the right-edge hover-revealed action buttons (New chat on
  // the chats section; Open Folder / Clone Repository on the folders section).
  // Revealed only while the row is hovered (group/header lives on the row
  // container, NOT the toggle button — so moving onto a button to click it keeps
  // the row hovered and the button from flickering). Stays shown on keyboard
  // focus and on touch (no hover). /90 clears the 3:1 non-text bar; hover deepens
  // to full foreground. h-6/w-6 + 14px icon match the folder rows' right-edge
  // action icons so every affordance reads as one family. justify-end (not
  // -center) flushes the 14px glyph to the button's right edge: the box is
  // transparent, so a centred icon would read ~5px further in than the
  // conversation time / shortcut badges (which fill to their own right edge),
  // breaking the shared 0.75rem right alignment.
  const actionButtonClassName = cn(
    "flex h-6 w-6 items-center justify-end rounded-[0.375rem]",
    "cursor-pointer text-muted-foreground/90 outline-none",
    "opacity-0 group-hover/header:opacity-100 focus-visible:opacity-100",
    "[@media(hover:none)]:opacity-100",
    "transition-[color,opacity] duration-150 hover:text-sidebar-foreground",
    "focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
  )
  return (
    <div className={cn(topGap && "pt-[0.75rem]")}>
      <div className="group/header relative h-[2rem]">
        <button
          type="button"
          onClick={() => onToggle(section)}
          aria-expanded={expanded}
          className={cn(
            "group flex h-full w-full items-center gap-[0.375rem] px-[0.5rem]",
            "rounded-md outline-none select-none",
            // Lighter than the folder name, but on the SAME base token
            // (`sidebar-foreground`) — not `muted-foreground`. Both labels are
            // 0.875rem/normal, so an earlier "looks a different size" was pure
            // contrast: a lighter/lower-contrast token reads as smaller. Same
            // family keeps perceived size matched.
            //
            // /50 ≈ 3.7:1 (light) / ~5.1:1 (dark). In light mode this is BELOW the
            // 4.5:1 WCAG AA bar for 14px body text, but clears the 3:1 large-text /
            // UI-component bar. Deliberate, user-approved: these are redundant
            // secondary section labels (the list beneath them is self-evident), so
            // the 3:1 bar is the one held here. /60 was the AA floor; the user
            // asked for lighter still and accepted the 3:1 tradeoff. Don't drop
            // below /45 (~3.1:1) without revisiting — that breaches 3:1 too. Hover
            // deepens to /80 for a clear interactive affordance.
            "text-sidebar-foreground/50 transition-colors duration-150",
            "hover:text-sidebar-foreground/80",
            "focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
          )}
        >
          <span className="text-[0.875rem] font-normal">{label}</span>
          <ChevronRight
            aria-hidden
            className={cn(
              "h-3 w-3 shrink-0 transition-[transform,opacity] duration-200 ease-out",
              // Collapsed: always show the chevron (the only affordance that the
              // section can be reopened). Expanded: reveal on hover/focus only.
              expanded
                ? "rotate-90 opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100 [@media(hover:none)]:opacity-100"
                : "opacity-100"
            )}
          />
        </button>
        {showNewChat && (
          <button
            type="button"
            // Stop the click from reaching the row (defensive — the button is a
            // sibling, not nested, so it never triggers the toggle anyway).
            onClick={(e) => {
              e.stopPropagation()
              onNewChat?.()
            }}
            title={t("newChatAction")}
            aria-label={t("newChatAction")}
            // Sized to match the folder rows' right-edge ⋯ action icon
            // (`h-[0.875rem]`, 14px) so the two affordances read as one family —
            // a hair smaller than the default `h-4` glyph.
            className={cn(
              "absolute top-1/2 right-[0.375rem] -translate-y-1/2",
              actionButtonClassName
            )}
          >
            <SquarePen className="h-[0.875rem] w-[0.875rem]" />
          </button>
        )}
        {showFolderActions && (
          <div className="absolute top-1/2 right-[0.375rem] flex -translate-y-1/2 items-center gap-px">
            {onImportSessions != null && (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation()
                  onImportSessions()
                }}
                title={t("importLocalSessions")}
                aria-label={t("importLocalSessions")}
                className={actionButtonClassName}
              >
                <Download className="h-[0.875rem] w-[0.875rem]" />
              </button>
            )}
            {onCloneRepository != null && (
              <button
                type="button"
                // Sibling of the toggle button, so it never toggles the section;
                // stop propagation defensively anyway.
                onClick={(e) => {
                  e.stopPropagation()
                  onCloneRepository()
                }}
                title={tDropdown("cloneRepository")}
                aria-label={tDropdown("cloneRepository")}
                className={actionButtonClassName}
              >
                <FolderGit2 className="h-[0.875rem] w-[0.875rem]" />
              </button>
            )}
            {onOpenFolder != null && (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation()
                  onOpenFolder()
                }}
                title={tDropdown("openFolder")}
                aria-label={tDropdown("openFolder")}
                className={actionButtonClassName}
              >
                <FolderOpenDot className="h-[0.875rem] w-[0.875rem]" />
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  )
})
