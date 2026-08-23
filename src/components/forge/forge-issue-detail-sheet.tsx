"use client"

import { useTranslations } from "next-intl"
import {
  CirclePlay,
  ExternalLink,
  ListTodo,
  MessageSquare,
  RotateCcw,
} from "lucide-react"
import { MessageResponse } from "@/components/ai-elements/message"
import { formatRelative } from "@/components/conversations/sidebar-conversation-grouping"
import {
  CHIP_FILL,
  ForgeLabelChip,
  ROW_ACTION,
  ROW_ACTION_GLYPH,
  stateGlyph,
} from "@/components/forge/forge-issue-row"
import { statusLabelKey } from "@/components/tasks/task-card"
import { BrowserLink } from "@/components/ui/browser-link"
import { Button } from "@/components/ui/button"
import {
  Drawer,
  DrawerContent,
  DrawerDescription,
  DrawerHeader,
  DrawerTitle,
  SIDE_PANEL_CONTENT_CLASS,
} from "@/components/ui/drawer"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useWorkbenchRoute } from "@/contexts/workbench-route-context"
import { chipStateForLink } from "@/lib/forge-task-chip"
import { cn } from "@/lib/utils"
import type { ForgeIssueRow, ForgeTaskLink } from "@/lib/types"

/**
 * Typography for the item's Markdown body at the panel's scale.
 *
 * Streamdown sizes its own elements for the full-width chat column — `h1` at
 * `text-3xl`, 24px above every heading — which in a 32rem panel turns a
 * three-heading issue into a page of titles. A descendant selector outranks the
 * class Streamdown puts on the element itself, so these win without
 * `!important`. Lists and the first/last block's collapsed margin already come
 * from `MessageResponse`; `prose` is deliberately absent, as the repo has no
 * typography plugin and those classes would generate nothing.
 *
 * Deliberately NOT the task sheet's `RESULT_MARKDOWN`, which is tuned a notch
 * smaller: there the Markdown is a summary sitting among other sections, here it
 * is the whole reason the panel opened and has to stay comfortable to read at
 * length. Images are capped because an issue body is full of screenshots and
 * the forge writes them at their natural width.
 */
const BODY_MARKDOWN =
  "[&_h1]:text-sm [&_h2]:text-sm [&_h3]:text-[0.8125rem] [&_h4]:text-[0.8125rem] " +
  "[&_h1]:font-semibold [&_h2]:font-semibold [&_h3]:font-semibold [&_h4]:font-semibold " +
  "[&_h1]:mt-4 [&_h2]:mt-4 [&_h3]:mt-3 [&_h4]:mt-3 " +
  "[&_h1]:mb-1.5 [&_h2]:mb-1.5 [&_h3]:mb-1 [&_h4]:mb-1 " +
  "[&_p]:mt-0 [&_p]:mb-2.5 [&_ul]:my-2 [&_ol]:my-2 [&_li]:my-0.5 " +
  "[&_blockquote]:my-2.5 [&_hr]:my-4 [&_table]:my-2.5 " +
  "[&_img]:max-w-full [&_img]:rounded-lg"

/** Render-time "now", as on the row: the panel re-renders with its list. */
function relative(iso: string): string {
  return formatRelative(iso, Date.now())
}

/**
 * The full date behind a relative one. The list says "3 days ago" because that
 * is what a triage scan wants; the panel is where someone asks "three days from
 * WHEN", and a title attribute answers it without spending a line.
 */
function absolute(iso: string): string | undefined {
  const at = new Date(iso)
  return Number.isNaN(at.getTime()) ? undefined : at.toLocaleString()
}

/**
 * Right-side detail panel for one issue / pull request.
 *
 * It replaces what the row's title used to do — leave the app for the forge's
 * own web page — because everything a triage pass needs is already in the list
 * payload: the body rides along with every row (see `ForgeIssueRow::body`), so
 * opening this costs no request, and the list underneath keeps its filters,
 * its page and its scroll position. The panel is the same drawer the task board
 * uses, at the same width, for the same reason those all share
 * `SIDE_PANEL_CONTENT_CLASS`: they stack on one another.
 *
 * What it deliberately does NOT have is the discussion. Comments are not in the
 * list payload and would need a per-item request against the same quota the
 * list is already spending, so the count in the header is honest about there
 * being a conversation and the footer's link is how you go and read it.
 */
export function ForgeIssueDetailSheet({
  row,
  link,
  onOpenChange,
  onStart,
}: {
  /** The item on show, or `null` when the panel is closed. Held by the page so
   *  a list refresh re-renders the panel with the item's fresh copy. */
  row: ForgeIssueRow | null
  /** Latest task for this item, if any — the footer's action depends on it. */
  link: ForgeTaskLink | null
  onOpenChange: (open: boolean) => void
  /** Opens the page's trigger dialog on this item. */
  onStart: () => void
}) {
  const t = useTranslations("Forge")
  const tTasks = useTranslations("Tasks")
  const { setRoute } = useWorkbenchRoute()

  if (row == null) return null

  const chip = chipStateForLink(link)
  const active = chip === "active"
  const terminal = chip === "terminal"
  const { Icon, className: glyphClass, labelKey } = stateGlyph(row)
  const stateLabel = t(labelKey)
  const body = row.body?.trim()

  return (
    <Drawer open onOpenChange={onOpenChange} swipeDirection="right">
      <DrawerContent className={SIDE_PANEL_CONTENT_CLASS}>
        <DrawerHeader className="shrink-0 gap-0 border-b border-border px-5 py-4">
          {/* `pr-8` clears the close button in the corner. */}
          <div className="flex items-start gap-3 pr-8">
            {/* The list's own state glyph, given the framed tile the task
                sheet's agent icon has — at panel scale a bare 14px mark beside
                a two-line title reads as a stray bullet. */}
            <span className="mt-0.5 inline-flex size-9 shrink-0 items-center justify-center rounded-xl border border-border bg-muted/40">
              {/* Decoration here, unlike on the row: the state is spelled out
                  in the meta line below, and labelling both would read the
                  word twice to a screen reader. */}
              <Icon className={cn("size-[1.125rem]", glyphClass)} aria-hidden />
            </span>
            <div className="flex min-w-0 flex-1 flex-col gap-1.5">
              <DrawerTitle className="min-w-0 break-words text-[0.9375rem] font-semibold leading-5">
                {row.title}
              </DrawerTitle>
              {/* The row's own meta line, with the state spelled out: the list
                  can lean on a column of glyphs to carry the state, a single
                  item on its own cannot. */}
              <div className="flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1 text-[0.6875rem] text-muted-foreground">
                <span className={cn("font-medium", glyphClass)}>
                  {stateLabel}
                </span>
                <span className="font-mono">· #{row.number}</span>
                {row.author ? <span>· {row.author}</span> : null}
                {row.updated_at ? (
                  <span title={absolute(row.updated_at)}>
                    · {t("detailUpdated", { time: relative(row.updated_at) })}
                  </span>
                ) : null}
                {row.comments > 0 ? (
                  <span className="inline-flex items-center gap-1 tabular-nums">
                    <span aria-hidden>·</span>
                    <MessageSquare className="size-3" aria-hidden />
                    {t("commentCount", { count: row.comments })}
                  </span>
                ) : null}
              </div>
              {/* EVERY label, unlike the row — the panel is where the ones the
                  row had to drop finally show. */}
              {row.labels.length > 0 ? (
                <div className="flex flex-wrap items-center gap-1">
                  {row.labels.map((label) => (
                    <ForgeLabelChip
                      key={label.name}
                      label={label}
                      className="h-5 px-2 text-[0.6875rem]"
                    />
                  ))}
                </div>
              ) : null}
            </div>
          </div>
          <DrawerDescription className="sr-only">
            {t("detailDescription")}
          </DrawerDescription>
        </DrawerHeader>

        <ScrollArea className="min-h-0 flex-1">
          <div className="px-5 py-4">
            {body ? (
              // The forge's own Markdown, through the same renderer the chat
              // uses — headings, task lists, tables, fenced code and images all
              // come out as the author wrote them, and link clicks go through
              // the app's link-safety routing rather than the webview.
              <div
                className={cn(
                  "break-words text-[0.8125rem] leading-relaxed",
                  BODY_MARKDOWN
                )}
              >
                <MessageResponse>{body}</MessageResponse>
              </div>
            ) : (
              <p className="py-6 text-center text-xs text-muted-foreground">
                {t("detailNoBody")}
              </p>
            )}
          </div>
        </ScrollArea>

        {/* The way out to the forge on one side, what to DO about the item on
            the other. Same pills as the row, so an item's action does not
            change shape on the way into the panel — only the fill does: here
            "Start" is the one thing the panel is asking for, and gets the
            filled treatment a column of rows could not afford. */}
        <div className="flex shrink-0 items-center gap-2 border-t border-border px-5 py-3">
          {/* A real anchor wearing the pill, not a button that calls `openUrl`:
              `href` is what gives it "copy link address", the status-bar
              preview and a screen reader that says "link". `BrowserLink` is
              what keeps the click working in the desktop webview. */}
          <BrowserLink
            href={row.html_url}
            className={cn(
              ROW_ACTION,
              "inline-flex items-center text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            )}
          >
            <ExternalLink className={ROW_ACTION_GLYPH} aria-hidden />
            {t("openItem")}
          </BrowserLink>

          <div className="ms-auto flex items-center gap-1.5">
            {link == null ? (
              <Button
                type="button"
                size="sm"
                className={ROW_ACTION}
                onClick={onStart}
              >
                <CirclePlay className={ROW_ACTION_GLYPH} aria-hidden />
                {t("start")}
              </Button>
            ) : (
              // Siblings, never nested — same reason as on the row: a button
              // inside a button folds its text into the outer one's accessible
              // name and leaves keyboard activation to the browser.
              <>
                {terminal ? (
                  <button
                    type="button"
                    onClick={onStart}
                    className="inline-flex items-center gap-1 text-[0.6875rem] text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
                  >
                    <RotateCcw className="size-3" aria-hidden />
                    {t("retrigger")}
                  </button>
                ) : null}
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  onClick={() => setRoute("tasks")}
                  title={t("viewTask")}
                  className={cn(
                    ROW_ACTION,
                    active ? CHIP_FILL.active : CHIP_FILL.settled
                  )}
                >
                  <ListTodo className={ROW_ACTION_GLYPH} aria-hidden />
                  {tTasks(statusLabelKey(link.status))}
                </Button>
              </>
            )}
          </div>
        </div>
      </DrawerContent>
    </Drawer>
  )
}
