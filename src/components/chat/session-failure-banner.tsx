"use client"

/**
 * JetBrains AIR typed session-failure strips, docked under the composer next
 * to the Claude API-retry line (`conversation-shell`). One strip per record:
 *
 * - ACTIVE severity-"error" records use the destructive palette (matching the
 *   shell's error strip) and carry the record's suggested action buttons
 *   (`retry` / `login` / `new_session` — the vocabulary the adapters emit;
 *   unknown actions are simply not rendered). Terminal errors stay until the
 *   user acts: sending a new prompt settles them (reducer), and a recurrence
 *   re-arms via a higher revision. Every error renders its own strip.
 * - ACTIVE severity-"warning" records use the amber palette (matching the
 *   config-stale banner) and collapse to the LATEST one plus a hidden count:
 *   they are in-flight retry incidents that settle as soon as the turn makes
 *   progress, and on advertising connections codex routes what used to be the
 *   single-slot `turn_retrying` channel here — one strip per record stacked a
 *   fresh row for every reconnect of a long turn (issue #496).
 *
 * - Of the RESOLVED records only the most recent recovered WARNING renders,
 *   as one muted "recovered" line — evidence of what happened mid-turn
 *   without stacking history under the composer. It is a TRANSIENT
 *   confirmation: it self-dismisses after `RECOVERED_VISIBLE_MS`, because
 *   records are retained forever as revision watermarks and nothing else
 *   would ever take it down. Resolved records still live in the reducer table
 *   as watermarks; resolved errors (settled by the user acting) render
 *   nothing.
 *
 * EVERY strip carries a close button, recovered line included. Retry actions
 * stay off warnings (re-prompting mid-recovery would enqueue a second turn),
 * but dismissing is always the user's to do — and unlike the settle passes it
 * is available to viewers, since it only touches their own client's
 * projection.
 *
 * Adapter-authored `title`/`details` are shown verbatim (already user-facing
 * prose); a blank title falls back to the localized category label.
 */

import { useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import {
  AlertCircle,
  Ban,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Gauge,
  KeyRound,
  LogIn,
  Plus,
  RefreshCw,
  ServerCrash,
  WifiOff,
  X,
} from "lucide-react"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import type { SessionFailureRecord } from "@/lib/types"
import {
  activeSessionFailureView,
  knownSessionFailureActions,
  mostRecentRecoveredWarning,
  type SessionFailureAction,
} from "@/lib/session-failures"

/** How long the muted "recovered" confirmation stays before self-dismissing. */
const RECOVERED_VISIBLE_MS = 10_000

const CATEGORY_ICONS: Record<string, typeof AlertCircle> = {
  connection: WifiOff,
  access: KeyRound,
  limit: Gauge,
  request: Ban,
  service: ServerCrash,
  unknown: AlertCircle,
}

const ACTION_ICONS: Record<SessionFailureAction, typeof RefreshCw> = {
  retry: RefreshCw,
  login: LogIn,
  new_session: Plus,
}

/** i18n key per action (the wire vocabulary is snake_case). */
const ACTION_LABEL_KEYS = {
  retry: "action.retry",
  login: "action.login",
  new_session: "action.newSession",
} as const

/** i18n label key per rendered category (title fallback). */
const CATEGORY_LABEL_KEYS = {
  connection: "category.connection",
  access: "category.access",
  limit: "category.limit",
  request: "category.request",
  service: "category.service",
  unknown: "category.unknown",
} as const

type KnownCategory = keyof typeof CATEGORY_LABEL_KEYS

/** Fold an arbitrary wire category onto the rendered vocabulary. */
function knownCategory(category: string): KnownCategory {
  return category in CATEGORY_LABEL_KEYS
    ? (category as KnownCategory)
    : "unknown"
}

interface Props {
  failures: SessionFailureRecord[]
  /** Wires the suggested actions; when omitted the buttons are hidden (e.g.
   *  viewers, who don't own the session). */
  onAction?: (
    action: SessionFailureAction,
    failure: SessionFailureRecord
  ) => void
  /** Closes a strip (client-local), taking every id that strip stands for —
   *  the collapsed warning bar closes its hidden siblings too. Omitted only
   *  where there is no store to write back to; unlike `onAction` this is NOT
   *  gated on session ownership. */
  onDismiss?: (ids: string[]) => void
}

export function SessionFailureBanner({ failures, onAction, onDismiss }: Props) {
  const { errors, warning, hiddenWarnings, warningIds } =
    activeSessionFailureView(failures)
  const recovered = mostRecentRecoveredWarning(failures)
  const hasActive = errors.length > 0 || warning !== null
  if (!hasActive && !recovered) return null
  return (
    <>
      {errors.map((failure) => (
        <ActiveFailureStrip
          key={failure.id}
          failure={failure}
          dismissIds={[failure.id]}
          onAction={onAction}
          onDismiss={onDismiss}
        />
      ))}
      {warning && (
        <ActiveFailureStrip
          key={warning.id}
          failure={warning}
          hiddenCount={hiddenWarnings}
          dismissIds={warningIds}
          onAction={onAction}
          onDismiss={onDismiss}
        />
      )}
      {!hasActive && recovered && (
        <RecoveredStrip
          key={`${recovered.id}@${recovered.revision}`}
          failure={recovered}
          onDismiss={onDismiss}
        />
      )}
    </>
  )
}

function ActiveFailureStrip({
  failure,
  hiddenCount = 0,
  dismissIds,
  onAction,
  onDismiss,
}: {
  failure: SessionFailureRecord
  /** Older active warnings folded behind this one, shown as a count. */
  hiddenCount?: number
  /** Every record this strip stands for — what its close button dismisses. */
  dismissIds: string[]
  onAction?: Props["onAction"]
  onDismiss?: Props["onDismiss"]
}) {
  const t = useTranslations("Folder.chat.sessionFailure")
  const [expanded, setExpanded] = useState(false)
  const warning = failure.severity === "warning"
  const category = knownCategory(failure.category)
  const Icon = CATEGORY_ICONS[category]
  const title = failure.title.trim() || t(CATEGORY_LABEL_KEYS[category])
  const details = failure.details?.trim() || null
  // Warnings are in-flight retry incidents the adapter is already handling —
  // offering `retry` there would enqueue a second prompt mid-recovery, so
  // buttons render for terminal (non-warning) records only.
  const actions =
    onAction && !warning ? knownSessionFailureActions(failure) : []
  const DetailsChevron = expanded ? ChevronDown : ChevronRight
  return (
    <div
      role="alert"
      className={cn(
        "border-t px-4 py-2 text-xs",
        warning
          ? "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300"
          : "border-destructive/20 bg-destructive/5 text-destructive"
      )}
    >
      <div className="flex items-center gap-2">
        <Icon aria-hidden="true" className="h-3.5 w-3.5 shrink-0" />
        <span
          className="min-w-0 flex-1 truncate font-medium"
          title={details ?? title}
        >
          {title}
        </span>
        {hiddenCount > 0 && (
          <span className="shrink-0 text-[10px] font-medium opacity-70">
            {t("moreIncidents", { count: hiddenCount })}
          </span>
        )}
        {actions.map((action) => {
          const ActionIcon = ACTION_ICONS[action]
          return (
            <Button
              key={action}
              size="sm"
              variant="outline"
              className="h-6 shrink-0 px-2 text-xs"
              onClick={() => onAction?.(action, failure)}
            >
              <ActionIcon aria-hidden="true" className="me-1 h-3 w-3" />
              {t(ACTION_LABEL_KEYS[action])}
            </Button>
          )
        })}
        {details && (
          <button
            type="button"
            aria-label={t("toggleDetails")}
            aria-expanded={expanded}
            className="shrink-0 rounded p-0.5 opacity-70 hover:opacity-100"
            onClick={() => setExpanded((v) => !v)}
          >
            <DetailsChevron aria-hidden="true" className="h-3.5 w-3.5" />
          </button>
        )}
        {onDismiss && (
          <Button
            size="icon"
            variant="ghost"
            className={cn(
              "h-6 w-6 shrink-0",
              warning
                ? "text-amber-700/70 hover:bg-amber-500/20 hover:text-amber-800 dark:text-amber-300/70 dark:hover:text-amber-200"
                : "text-destructive/70 hover:bg-destructive/10 hover:text-destructive"
            )}
            onClick={() => onDismiss(dismissIds)}
            aria-label={t("dismiss")}
          >
            <X aria-hidden="true" className="h-3.5 w-3.5" />
          </Button>
        )}
      </div>
      {expanded && details && (
        <p className="mt-1.5 ps-[22px] text-[11px] whitespace-pre-wrap break-words opacity-80">
          {details}
        </p>
      )}
    </div>
  )
}

function RecoveredStrip({
  failure,
  onDismiss,
}: {
  failure: SessionFailureRecord
  onDismiss?: Props["onDismiss"]
}) {
  const t = useTranslations("Folder.chat.sessionFailure")
  const title =
    failure.title.trim() ||
    t(CATEGORY_LABEL_KEYS[knownCategory(failure.category)])
  // Self-expire. Records are retained forever as revision watermarks, so
  // nothing else would ever take this line down — it used to sit under the
  // composer for the rest of the session announcing a hiccup that was over
  // (field report 2026-08-17). Auto-dismiss WRITES to the store rather than
  // just hiding locally, so remounting the panel cannot resurrect it.
  const id = failure.id
  const dismiss = onDismiss
  useEffect(() => {
    if (!dismiss) return
    const timer = setTimeout(() => dismiss([id]), RECOVERED_VISIBLE_MS)
    return () => clearTimeout(timer)
  }, [dismiss, id])
  return (
    <div className="border-t border-border/50 bg-muted/30 px-4 py-1.5 text-[11px] text-muted-foreground">
      <div className="flex items-center gap-2">
        <CheckCircle2 aria-hidden="true" className="h-3 w-3 shrink-0" />
        <span className="min-w-0 flex-1 truncate">
          {t("recovered")} · {title}
        </span>
        {dismiss && (
          <Button
            size="icon"
            variant="ghost"
            className="h-5 w-5 shrink-0 text-muted-foreground/70 hover:text-foreground"
            onClick={() => dismiss([id])}
            aria-label={t("dismiss")}
          >
            <X aria-hidden="true" className="h-3 w-3" />
          </Button>
        )}
      </div>
    </div>
  )
}
