"use client"

import { useEffect, useState } from "react"

import { Bot, ChevronDown, ShieldOff, TriangleAlert } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { browserAgentGrant } from "@/lib/browser/browser-api"
import {
  useBrowserAgentActivity,
  type BrowserAgentActivity,
} from "@/lib/browser/browser-tab-store"
import { displayHostPort } from "@/lib/browser/browser-url"
import type { AgentGrant, BrowserTabState } from "@/lib/browser/types"
import { browserTabBackendId } from "@/lib/file-tab-id"
import { cn } from "@/lib/utils"

/**
 * The one place a person hands a page to an agent, and the running account of
 * what agents did with it.
 *
 * Only the `read` level is offered. The model has three, and the backend
 * understands all three, but nothing in the app can act on a page yet: a
 * "control" entry in this menu would be the interface promising something the
 * product does not do. It becomes a two-entry menu the day acting on a page
 * exists, not before.
 */

/** How long the page border stays lit after an agent touches the tab. Long
 *  enough to catch the eye of someone not looking straight at it, short
 *  enough that a run of reads reads as a flicker rather than a solid state. */
const ACTIVITY_GLOW_MS = 1200

/**
 * The colour of "an agent can read this", everywhere it appears: the chip in
 * the toolbar, the glyph in the tab strip, the border around the page.
 *
 * A fixed hue rather than the theme's `primary`, which is what this started
 * as. Every stock theme in the app sets `--primary` to a neutral (chroma 0),
 * so a primary-tinted border is a slightly darker grey line — indistinguishable
 * from the dividers on either side of it, and the border has no text or shape
 * to fall back on. Beyond that, a mark that says a page is exposed should not
 * be something the theme picker can tune down into the chrome.
 */
export const AGENT_MARK = "text-violet-600 dark:text-violet-400"

function shareableOrigin(state: BrowserTabState | null): string | null {
  // Both halves of the backend's rule (`agent::grantable_origin`), so this
  // says the same thing it does. Duplicated only to decide whether the
  // control is offered at all — the backend still decides whether the share
  // happens, and says why when it refuses.
  //
  // A document guest cannot be shared, and its address does not say so: under
  // WebView2 it is served from `https://codeg-doc.localhost/…`, a perfectly
  // ordinary-looking https origin. No surface renders this control for one
  // today (they show a local file, and have a toolbar of their own), but a
  // rule that agrees with the backend on only one of its two clauses is a
  // trap for whoever mounts it somewhere new.
  if (!state || state.kind === "document") return null
  const origin = state.origin
  if (!origin) return null
  return origin.startsWith("http://") || origin.startsWith("https://")
    ? origin
    : null
}

/**
 * How the page's border should read right now: nothing when the tab is not
 * shared, lit when an agent has just touched it, steady otherwise.
 */
export function useBrowserAgentGlow(
  tab: BrowserWorkspaceTab,
  state: BrowserTabState | null
): "none" | "steady" | "active" {
  const activity = useBrowserAgentActivity(tab.id)
  const head = activity[0]
  // Names one attempt. Every field of the head entry is in it, because any
  // one of them alone is ambiguous within a millisecond: the count moves when
  // a run grows (so an agent reading the same page twice in a row still
  // re-lights the border), and the action and outcome move when a new line is
  // pushed whose `at` and `count` happen to match the one it replaced —
  // a refusal costs no page round trip, so two attempts really can land in
  // the same millisecond.
  const stamp = head
    ? `${head.at}.${head.count}.${head.action}.${head.outcome}`
    : null
  // Lit during render rather than from an effect, so the border is already on
  // in the paint that shows the new line on the strip.
  const [seen, setSeen] = useState(stamp)
  const [lit, setLit] = useState(false)
  if (seen !== stamp) {
    setSeen(stamp)
    setLit(stamp !== null)
  }
  // `seen` is a dependency as well as `lit`: an attempt arriving while the
  // border is already on has to restart the countdown, and `lit` alone does
  // not change when it is already true.
  useEffect(() => {
    if (!lit) return
    const timer = setTimeout(() => setLit(false), ACTIVITY_GLOW_MS)
    return () => clearTimeout(timer)
  }, [lit, seen])
  if (!state?.agentGrant) return "none"
  return lit ? "active" : "steady"
}

/** The pinned program's file name — what the person would have typed —
 *  rather than the path they never look at. Null when this grant has no pin,
 *  which is every address but a loopback one. */
function pinnedProgram(grant: AgentGrant | null): string | null {
  const program = grant?.listener?.program
  if (!program) return null
  return program.split(/[/\\]/).filter(Boolean).pop() ?? null
}

/** The share control in a browser tab's toolbar. */
export function BrowserAgentShareControl({
  tab,
  state,
}: {
  tab: BrowserWorkspaceTab
  state: BrowserTabState | null
}) {
  const t = useTranslations("Browser.agent")
  const backendId = browserTabBackendId(tab.id)
  const grant = state?.agentGrant ?? null
  const origin = shareableOrigin(state)

  const share = (level: "read" | "none") => {
    if (!backendId) return
    void browserAgentGrant(backendId, level)
      .then(() => {
        if (level === "read" && origin) {
          toast.success(t("sharedToast", { origin: displayOrigin(origin) }))
        }
      })
      .catch((error: unknown) => {
        toast.error(t("shareFailed"), { description: String(error) })
      })
  }

  if (!grant) {
    return (
      <button
        type="button"
        className={cn(
          "flex h-7 w-7 shrink-0 items-center justify-center rounded text-muted-foreground",
          "transition-colors hover:bg-primary/8 hover:text-foreground",
          "disabled:pointer-events-none disabled:opacity-40"
        )}
        title={
          origin
            ? t("share", { origin: displayOrigin(origin) })
            : t("notShareable")
        }
        aria-label={t("shareLabel")}
        disabled={!backendId || !origin}
        onClick={() => share("read")}
      >
        <Bot className="h-4 w-4" />
      </button>
    )
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className={cn(
            "flex h-7 shrink-0 items-center gap-1 rounded px-1.5 text-xs font-medium",
            "bg-violet-500/12 transition-colors hover:bg-violet-500/20",
            AGENT_MARK
          )}
          title={t("sharedWith", { origin: displayOrigin(grant.origin) })}
          aria-label={t("sharedWith", { origin: displayOrigin(grant.origin) })}
        >
          <Bot className="h-3.5 w-3.5 shrink-0" />
          <span>{t("shared")}</span>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-56">
        <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">
          {t("sharedWith", { origin: displayOrigin(grant.origin) })}
        </DropdownMenuLabel>
        {/* What the grant is actually bound to, in the words that matter: it
            outlives this page and ends at the edge of this site. */}
        <DropdownMenuLabel className="pt-0 text-xs font-normal text-muted-foreground/80">
          {t("sharedScope")}
        </DropdownMenuLabel>
        {/* And for a loopback address, the edge of the site is not the whole
            boundary: `localhost:3000` is a port number, so the grant is also
            bound to the program behind it. Saying which one here is what
            keeps the notice from arriving out of nowhere later. */}
        {pinnedProgram(grant) ? (
          <DropdownMenuLabel className="pt-0 text-xs font-normal text-muted-foreground/80">
            {t("sharedScopeProgram", { program: pinnedProgram(grant) })}
          </DropdownMenuLabel>
        ) : null}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => share("none")}>
          <ShieldOff className="h-3.5 w-3.5" />
          <span>{t("stopSharing")}</span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** `https://example.com:8443` → `example.com:8443`; anything unparseable as
 *  it stands. */
function displayOrigin(origin: string): string {
  return displayHostPort(origin) ?? origin
}

function activityLabel(
  t: ReturnType<typeof useTranslations<"Browser.agent">>,
  entry: BrowserAgentActivity
): string {
  if (entry.outcome === "refused") return t("activityRefused")
  if (entry.outcome === "failed") return t("activityFailed")
  return t("activityRead")
}

/**
 * What agents have done to this tab, newest first: the latest line always,
 * the rest behind a disclosure.
 *
 * Shown whenever there is anything to show — including on a tab nobody
 * shared, where every line is a refusal. That is the case worth surfacing
 * most: an agent reaching for a page it was never given is invisible
 * otherwise, since the only party told about it is the agent.
 */
export function BrowserAgentStrip({ tab }: { tab: BrowserWorkspaceTab }) {
  const t = useTranslations("Browser.agent")
  const activity = useBrowserAgentActivity(tab.id)
  const [expanded, setExpanded] = useState(false)
  const latest = activity[0]
  if (!latest) return null
  const rest = activity.slice(1)
  return (
    <div className="flex shrink-0 flex-col border-b border-border/60 bg-muted/40">
      <div className="flex h-7 items-center gap-2 px-3 text-xs text-muted-foreground">
        {latest.outcome === "refused" ? (
          <ShieldOff className="h-3.5 w-3.5 shrink-0 text-amber-600" />
        ) : latest.outcome === "failed" ? (
          <TriangleAlert className="h-3.5 w-3.5 shrink-0 text-amber-600" />
        ) : (
          <Bot className={cn("h-3.5 w-3.5 shrink-0", AGENT_MARK)} />
        )}
        <ActivityLine t={t} entry={latest} className="min-w-0 flex-1" />
        {rest.length > 0 ? (
          <button
            type="button"
            className="flex shrink-0 items-center gap-1 rounded px-1 py-0.5 hover:bg-primary/8 hover:text-foreground"
            aria-expanded={expanded}
            onClick={() => setExpanded((open) => !open)}
          >
            {expanded ? t("collapse") : t("expand", { count: rest.length })}
            <ChevronDown
              className={cn(
                "h-3 w-3 transition-transform",
                expanded && "rotate-180"
              )}
            />
          </button>
        ) : null}
      </div>
      {expanded ? (
        <div className="max-h-40 overflow-y-auto border-t border-border/40">
          {rest.map((entry, index) => (
            <ActivityLine
              // Entries are append-only at the head and collapse in place, so
              // an index below the head names the same attempt for as long as
              // the list lives.
              key={`${entry.at}-${index}`}
              t={t}
              entry={entry}
              className="flex h-6 items-center px-3 pl-8 text-xs text-muted-foreground"
            />
          ))}
        </div>
      ) : null}
    </div>
  )
}

function ActivityLine({
  t,
  entry,
  className,
}: {
  t: ReturnType<typeof useTranslations<"Browser.agent">>
  entry: BrowserAgentActivity
  className?: string
}) {
  // Clock time, not "3s ago": no ticking to keep it honest, and "when
  // exactly" is the question a record of what touched your page is for.
  const when = new Date(entry.at).toLocaleTimeString()
  return (
    <div className={cn("truncate", className)}>
      {activityLabel(t, entry)}
      {entry.count > 1 ? (
        <span className="ml-1.5 tabular-nums">
          {t("activityCount", { count: entry.count })}
        </span>
      ) : null}
      <span className="ml-1.5 text-muted-foreground/70 tabular-nums">
        {when}
      </span>
    </div>
  )
}
