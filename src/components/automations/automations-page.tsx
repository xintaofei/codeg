"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  Clock,
  CirclePlay,
  Loader2,
  Pencil,
  Play,
  Plus,
  RotateCw,
  SquareArrowOutUpRight,
  Trash2,
  X,
  Zap,
} from "lucide-react"
import { useAutomationsView } from "@/contexts/automations-view-context"
import { useWorkbenchRoute } from "@/contexts/workbench-route-context"
import { useTabContext } from "@/contexts/tab-context"
import { useAppWorkspace } from "@/contexts/app-workspace-context"
import { AutomationEditor } from "./automation-editor"
import {
  templateToDraft,
  type AutomationTemplate,
} from "./automation-templates"
import { TemplateGallery } from "./template-gallery"
import { ScheduleLabel } from "./schedule-label"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Switch } from "@/components/ui/switch"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable"
import {
  automationCancelRun,
  automationCreate,
  automationDelete,
  automationMarkSeen,
  automationRunNow,
  automationRuns,
  automationSetEnabled,
  automationUpdate,
} from "@/lib/api"
import { subscribe } from "@/lib/platform"
import { cn } from "@/lib/utils"
import type { Automation, AutomationDraft, AutomationRun } from "@/lib/types"

const AUTOMATION_CHANGED_EVENT = "automation://changed"

const STATUS_STYLES: Record<string, string> = {
  running: "bg-primary/10 text-primary",
  succeeded: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
  failed: "bg-destructive/10 text-destructive",
  cancelled: "bg-muted text-muted-foreground",
  skipped: "bg-muted text-muted-foreground",
}

function StatusChip({ status }: { status: string | null }) {
  const t = useTranslations("Automations")
  if (!status) return null
  const label =
    {
      running: t("statusRunning"),
      succeeded: t("statusSucceeded"),
      failed: t("statusFailed"),
      cancelled: t("statusCancelled"),
      skipped: t("statusSkipped"),
    }[status] ?? status
  return (
    <span
      className={cn(
        "inline-flex h-5 shrink-0 items-center rounded-full px-2 text-[0.6875rem] font-medium",
        STATUS_STYLES[status] ?? "bg-muted text-muted-foreground"
      )}
    >
      {label}
    </span>
  )
}

// Compact, i18n-free relative time ("now"/"5m"/"2h"/"3d"/"2mo"/"1y"), matching
// the sidebar conversation list's style. Absolute time rides in the title attr.
function formatRelative(iso: string | null, now: number): string {
  if (!iso) return "—"
  const ts = Date.parse(iso)
  if (Number.isNaN(ts)) return "—"
  const sec = Math.max(0, Math.round((now - ts) / 1000))
  if (sec < 45) return "now"
  const min = Math.round(sec / 60)
  if (min < 60) return `${min}m`
  const hr = Math.round(min / 60)
  if (hr < 24) return `${hr}h`
  const day = Math.round(hr / 24)
  if (day < 30) return `${day}d`
  const mo = Math.round(day / 30)
  if (mo < 12) return `${mo}mo`
  return `${Math.round(mo / 12)}y`
}

// Forward-looking sibling of formatRelative ("1m"/"3h"/"2d") for the next run.
// Floors at 1m so an imminent run never renders as "0m".
function formatRelativeFuture(iso: string | null, now: number): string {
  if (!iso) return "—"
  const ts = Date.parse(iso)
  if (Number.isNaN(ts)) return "—"
  const sec = Math.max(0, Math.round((ts - now) / 1000))
  const min = Math.max(1, Math.round(sec / 60))
  if (min < 60) return `${min}m`
  const hr = Math.round(min / 60)
  if (hr < 24) return `${hr}h`
  const day = Math.round(hr / 24)
  if (day < 30) return `${day}d`
  const mo = Math.round(day / 30)
  if (mo < 12) return `${mo}mo`
  return `${Math.round(mo / 12)}y`
}

function formatDuration(
  startIso: string | null,
  endIso: string | null
): string {
  if (!startIso || !endIso) return "—"
  const start = Date.parse(startIso)
  const end = Date.parse(endIso)
  if (Number.isNaN(start) || Number.isNaN(end) || end < start) return "—"
  const sec = Math.round((end - start) / 1000)
  if (sec < 60) return `${sec}s`
  const min = Math.floor(sec / 60)
  const rem = sec % 60
  if (min < 60) return rem ? `${min}m ${rem}s` : `${min}m`
  const hr = Math.floor(min / 60)
  return `${hr}h ${min % 60}m`
}

/** The detail pane's three states. "gallery" is the template picker shown when
 *  starting a new automation; "editor" hosts the form, seeded from a template
 *  (create) or an existing automation (edit). */
type EditingState =
  | { kind: "create"; seed: AutomationDraft | null }
  | { kind: "edit"; automation: Automation }

export function AutomationsPage() {
  const t = useTranslations("Automations")
  const { automations, unseenFailures, refetch } = useAutomationsView()
  const { folders } = useAppWorkspace()
  const [selectedId, setSelectedId] = useState<number | null>(null)
  const [mode, setMode] = useState<"detail" | "gallery" | "editor">("detail")
  const [editing, setEditing] = useState<EditingState | null>(null)

  // Clear the unseen-failure badges while the page is open — on entry and again
  // whenever a new failure arrives live (the failed run is already on screen, so
  // the sidebar badge shouldn't keep nagging). Keying on unseenFailures rather
  // than mount makes it re-fire on the automation://changed refetch; it
  // converges because markSeen drives the count to 0, after which this early
  // returns. refetch is stable.
  useEffect(() => {
    if (unseenFailures === 0) return
    void automationMarkSeen()
      .then(() => refetch())
      .catch(() => {})
  }, [unseenFailures, refetch])

  const hasAutomations = automations.length > 0
  // The shown automation: the explicit selection, else the first row, so the
  // detail pane is never blank when automations exist. Derived (no effect) so a
  // deleted selection cleanly falls back instead of dangling.
  const current =
    automations.find((a) => a.id === selectedId) ?? automations[0] ?? null
  // Frozen at mount — the page remounts on each route entry, so relative labels
  // ("Next in 3h") are anchored to when Automations was opened. Reading Date.now
  // during render is impure (react-hooks/purity); this is the RunHistory idiom.
  const [now] = useState(() => Date.now())

  const openGallery = () => {
    setEditing(null)
    setMode("gallery")
  }
  const backToGallery = () => {
    setEditing(null)
    setMode("gallery")
  }
  const closeToDetail = () => {
    setEditing(null)
    setMode("detail")
  }
  const pickTemplate = (tpl: AutomationTemplate | null) => {
    const seed = tpl
      ? templateToDraft(tpl, {
          name: t(tpl.titleKey),
          agentType: "claude_code",
          folderId: folders[0]?.id ?? null,
        })
      : null
    setEditing({ kind: "create", seed })
    setMode("editor")
  }
  const startEdit = (a: Automation) => {
    setEditing({ kind: "edit", automation: a })
    setMode("editor")
  }
  const selectAutomation = (a: Automation) => {
    setSelectedId(a.id)
    setEditing(null)
    setMode("detail")
  }

  const handleSubmit = async (draft: AutomationDraft) => {
    const saved =
      editing?.kind === "edit"
        ? await automationUpdate(editing.automation.id, draft)
        : await automationCreate(draft)
    await refetch()
    setSelectedId(saved.id)
    closeToDetail()
  }

  const editorPane =
    editing != null ? (
      <ScrollArea className="h-full">
        <div className="mx-auto w-full max-w-2xl p-4 sm:p-6">
          <AutomationEditor
            automation={
              editing.kind === "edit" ? editing.automation : editing.seed
            }
            onSubmit={handleSubmit}
            onCancel={closeToDetail}
            onBackToTemplates={
              editing.kind === "create" ? backToGallery : undefined
            }
          />
        </div>
      </ScrollArea>
    ) : null

  const picker = (onboarding: boolean) => (
    <ScrollArea className="h-full">
      <div className="mx-auto flex w-full max-w-4xl flex-col gap-6 p-4 sm:p-6">
        {onboarding ? (
          <div className="flex flex-col items-center gap-2 pt-4 text-center">
            <span className="flex size-12 items-center justify-center rounded-2xl bg-muted text-muted-foreground">
              <Zap className="size-6" aria-hidden="true" />
            </span>
            <h2 className="text-base font-semibold">{t("onboardTitle")}</h2>
            <p className="max-w-md text-sm text-muted-foreground">
              {t("onboardHint")}
            </p>
          </div>
        ) : (
          <div className="flex items-center justify-between gap-2">
            <h2 className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
              {t("startFromTemplate")}
            </h2>
            <Button size="sm" variant="ghost" onClick={closeToDetail}>
              {t("cancel")}
            </Button>
          </div>
        )}
        <TemplateGallery onPick={pickTemplate} />
      </div>
    </ScrollArea>
  )

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <header className="flex h-11 shrink-0 items-center justify-between gap-2 border-b border-border px-4">
        <div className="flex min-w-0 items-center gap-2">
          <Zap
            className="size-4 shrink-0 text-muted-foreground"
            aria-hidden="true"
          />
          <h1 className="text-sm font-semibold">{t("title")}</h1>
          <span className="hidden truncate text-xs text-muted-foreground md:inline">
            {t("headerSubtitle")}
          </span>
        </div>
        {hasAutomations && mode === "detail" ? (
          <Button size="sm" onClick={openGallery}>
            <Plus className="h-3.5 w-3.5" aria-hidden="true" />
            {t("new")}
          </Button>
        ) : null}
      </header>

      {hasAutomations ? (
        <ResizablePanelGroup direction="horizontal" className="min-h-0 flex-1">
          <ResizablePanel
            id="automations-list"
            order={1}
            defaultSize={32}
            minSize={22}
          >
            <ScrollArea className="h-full">
              <ul className="flex flex-col gap-1 p-2">
                {automations.map((a) => (
                  <AutomationListItem
                    key={a.id}
                    automation={a}
                    now={now}
                    selected={mode === "detail" && current?.id === a.id}
                    onSelect={() => selectAutomation(a)}
                  />
                ))}
              </ul>
            </ScrollArea>
          </ResizablePanel>
          <ResizableHandle withHandle />
          <ResizablePanel id="automations-detail" order={2} defaultSize={68}>
            {mode === "editor" && editing ? (
              editorPane
            ) : mode === "gallery" ? (
              picker(false)
            ) : current ? (
              <AutomationDetail
                automation={current}
                now={now}
                onEdit={() => startEdit(current)}
                refetch={refetch}
              />
            ) : (
              // Defensive only: `current` falls back to automations[0], which is
              // always present inside this hasAutomations branch, so this arm is
              // not reached in practice.
              <div className="flex h-full items-center justify-center p-8 text-center text-xs text-muted-foreground">
                {t("selectHint")}
              </div>
            )}
          </ResizablePanel>
        </ResizablePanelGroup>
      ) : (
        <div className="min-h-0 flex-1">
          {mode === "editor" && editing ? editorPane : picker(true)}
        </div>
      )}
    </div>
  )
}

function AutomationListItem({
  automation,
  now,
  selected,
  onSelect,
}: {
  automation: Automation
  now: number
  selected: boolean
  onSelect: () => void
}) {
  const t = useTranslations("Automations")
  const isSchedule = automation.trigger_kind === "schedule" && !!automation.cron
  const showNextIn =
    isSchedule && automation.enabled && !!automation.next_run_at
  const subline = showNextIn
    ? t("nextIn", { rel: formatRelativeFuture(automation.next_run_at, now) })
    : automation.last_run_at
      ? `${t("lastRun")} · ${formatRelative(automation.last_run_at, now)}`
      : null

  return (
    <li>
      <button
        type="button"
        onClick={onSelect}
        className={cn(
          "flex w-full flex-col gap-1.5 rounded-lg border border-transparent px-2.5 py-2 text-left outline-none transition-colors",
          "hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset",
          selected && "border-border bg-accent"
        )}
      >
        <div className="flex items-center gap-2">
          <span
            className={cn(
              "h-1.5 w-1.5 shrink-0 rounded-full",
              automation.enabled ? "bg-emerald-500" : "bg-muted-foreground/40"
            )}
            aria-hidden="true"
          />
          <span className="min-w-0 flex-1 truncate text-sm font-medium">
            {automation.name}
          </span>
          <StatusChip status={automation.last_run_status} />
        </div>
        <div className="flex items-center gap-1.5 pl-3.5 text-xs text-muted-foreground">
          <Badge variant="secondary" className="font-mono text-[0.625rem]">
            {automation.agent_type}
          </Badge>
          <span className="flex min-w-0 items-center gap-1 truncate">
            <Clock className="h-3 w-3 shrink-0" aria-hidden="true" />
            <span className="truncate">
              {isSchedule && automation.cron ? (
                <ScheduleLabel cron={automation.cron} />
              ) : (
                t("manual")
              )}
            </span>
          </span>
        </div>
        {subline ? (
          <div className="truncate pl-3.5 text-[0.6875rem] text-muted-foreground/80">
            {subline}
          </div>
        ) : null}
      </button>
    </li>
  )
}

function DetailField({
  label,
  children,
}: {
  label: string
  children: React.ReactNode
}) {
  return (
    <div className="flex flex-col gap-0.5">
      <dt className="text-[0.6875rem] font-medium uppercase tracking-wide text-muted-foreground">
        {label}
      </dt>
      <dd className="text-sm">{children}</dd>
    </div>
  )
}

function SectionCard({
  title,
  children,
}: {
  title: string
  children: React.ReactNode
}) {
  return (
    <section className="rounded-xl border border-border bg-card p-4">
      <h3 className="mb-3 text-[0.6875rem] font-medium uppercase tracking-wide text-muted-foreground">
        {title}
      </h3>
      {children}
    </section>
  )
}

function AutomationDetail({
  automation,
  now,
  onEdit,
  refetch,
}: {
  automation: Automation
  now: number
  onEdit: () => void
  refetch: () => Promise<void>
}) {
  const t = useTranslations("Automations")
  const { folders } = useAppWorkspace()
  const [busy, setBusy] = useState(false)

  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await fn()
      await refetch()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const folderName =
    folders.find((f) => f.id === automation.root_folder_id)?.name ?? "—"
  const labels = automation.config.label_snapshot
  const configEntries = Object.entries(automation.config.config_values || {})
  const isSchedule = automation.trigger_kind === "schedule" && !!automation.cron
  const showNextIn =
    isSchedule && automation.enabled && !!automation.next_run_at

  return (
    <ScrollArea className="h-full">
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 p-4 sm:p-6">
        <div className="flex items-start justify-between gap-3">
          <div className="flex min-w-0 flex-col gap-1">
            <div className="flex min-w-0 items-center gap-2">
              <h2 className="truncate text-lg font-semibold">
                {automation.name}
              </h2>
              <StatusChip status={automation.last_run_status} />
            </div>
            <div className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-xs text-muted-foreground">
              <Clock className="h-3 w-3 shrink-0" aria-hidden="true" />
              {isSchedule && automation.cron ? (
                <ScheduleLabel cron={automation.cron} />
              ) : (
                t("manual")
              )}
              {showNextIn ? (
                <span>
                  {"· "}
                  {t("nextIn", {
                    rel: formatRelativeFuture(automation.next_run_at, now),
                  })}
                </span>
              ) : null}
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            <Switch
              checked={automation.enabled}
              disabled={busy}
              onCheckedChange={(v) =>
                run(() => automationSetEnabled(automation.id, v))
              }
              aria-label={t("enabled")}
            />
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => run(() => automationRunNow(automation.id))}
            >
              <Play className="h-3.5 w-3.5" aria-hidden="true" />
              {t("runNow")}
            </Button>
            <Button size="sm" variant="ghost" disabled={busy} onClick={onEdit}>
              <Pencil className="h-3.5 w-3.5" aria-hidden="true" />
              {t("edit")}
            </Button>
            <AlertDialog>
              <AlertDialogTrigger asChild>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  className="text-muted-foreground hover:text-destructive"
                  aria-label={t("delete")}
                >
                  <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                </Button>
              </AlertDialogTrigger>
              <AlertDialogContent>
                <AlertDialogHeader>
                  <AlertDialogTitle>{t("deleteTitle")}</AlertDialogTitle>
                  <AlertDialogDescription>
                    {t("deleteDescription")}
                  </AlertDialogDescription>
                </AlertDialogHeader>
                <AlertDialogFooter>
                  <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
                  <AlertDialogAction
                    onClick={() => run(() => automationDelete(automation.id))}
                  >
                    {t("delete")}
                  </AlertDialogAction>
                </AlertDialogFooter>
              </AlertDialogContent>
            </AlertDialog>
          </div>
        </div>

        <SectionCard title={t("sectionSchedule")}>
          <dl className="grid grid-cols-2 gap-x-6 gap-y-3">
            <DetailField label={t("agent")}>
              <Badge variant="secondary" className="font-mono text-[0.625rem]">
                {labels?.agent_label ?? automation.agent_type}
              </Badge>
            </DetailField>
            <DetailField label={t("trigger")}>
              {isSchedule && automation.cron ? (
                <span className="flex flex-wrap items-center gap-1.5">
                  <ScheduleLabel cron={automation.cron} />
                  <span className="font-mono text-xs text-muted-foreground">
                    {automation.cron}
                  </span>
                </span>
              ) : (
                t("manual")
              )}
            </DetailField>
            <DetailField label={t("folder")}>
              <span className="truncate">
                {labels?.folder_label ?? folderName}
              </span>
            </DetailField>
            <DetailField label={t("isolation")}>
              {automation.isolation === "worktree_per_run"
                ? t("isolationWorktree")
                : t("isolationShared")}
              {automation.isolation === "shared_in_root" &&
              automation.branch ? (
                <span className="ml-1 font-mono text-xs text-muted-foreground">
                  {automation.branch}
                </span>
              ) : null}
            </DetailField>
            {isSchedule ? (
              <DetailField label={t("nextRun")}>
                {automation.next_run_at
                  ? new Date(automation.next_run_at).toLocaleString()
                  : "—"}
              </DetailField>
            ) : null}
            {automation.config.mode_id || configEntries.length > 0 ? (
              <DetailField label={t("config")}>
                <div className="flex flex-wrap gap-1">
                  {automation.config.mode_id ? (
                    <Badge variant="outline" className="text-[0.625rem]">
                      {labels?.mode_label ?? automation.config.mode_id}
                    </Badge>
                  ) : null}
                  {configEntries.map(([k, v]) => (
                    <Badge
                      key={k}
                      variant="outline"
                      className="text-[0.625rem]"
                    >
                      {labels?.config_labels?.[k] ?? v}
                    </Badge>
                  ))}
                </div>
              </DetailField>
            ) : null}
          </dl>
        </SectionCard>

        <SectionCard title={t("sectionPrompt")}>
          <p className="whitespace-pre-wrap text-sm text-foreground/90">
            {automation.config.display_text || "—"}
          </p>
        </SectionCard>

        <RunHistory
          key={automation.id}
          automation={automation}
          onChanged={refetch}
        />
      </div>
    </ScrollArea>
  )
}

function RunHistory({
  automation,
  onChanged,
}: {
  automation: Automation
  onChanged: () => Promise<void>
}) {
  const t = useTranslations("Automations")
  const { openTab } = useTabContext()
  const { openConversations } = useWorkbenchRoute()
  const [runs, setRuns] = useState<AutomationRun[]>([])
  const [loading, setLoading] = useState(true)
  const [now, setNow] = useState(() => Date.now())
  const reqRef = useRef(0)

  const load = useCallback(async () => {
    const id = ++reqRef.current
    try {
      const list = await automationRuns(automation.id)
      if (id === reqRef.current) {
        setRuns(list)
        setNow(Date.now())
      }
    } catch {
      // keep the previous list on transient error
    } finally {
      if (id === reqRef.current) setLoading(false)
    }
  }, [automation.id])

  useEffect(() => {
    setLoading(true)
    void load()
    let unsub: (() => void) | undefined
    let cancelled = false
    void subscribe(AUTOMATION_CHANGED_EVENT, () => {
      void load()
    }).then((u: () => void) => {
      if (cancelled) u()
      else unsub = u
    })
    return () => {
      cancelled = true
      unsub?.()
    }
  }, [load])

  const viewConversation = (r: AutomationRun) => {
    // Worktree runs live in their own folder; shared runs in the automation's
    // root. Bail rather than open folderId 0 (a structurally broken tab) if
    // neither resolves. openConversations() also covers re-selecting the
    // already-active tab, which wouldn't change activeTabId.
    const folderId = r.worktree_folder_id ?? automation.root_folder_id
    if (r.conversation_id == null || folderId == null) return
    openConversations()
    openTab(folderId, r.conversation_id, automation.agent_type)
  }

  const cancel = async (r: AutomationRun) => {
    try {
      await automationCancelRun(r.id)
      await load()
      await onChanged()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between">
        <span className="text-[0.6875rem] font-medium uppercase tracking-wide text-muted-foreground">
          {t("runHistory")}
        </span>
        <Button
          size="icon"
          variant="ghost"
          className="h-6 w-6 text-muted-foreground"
          onClick={() => void load()}
          title={t("refresh")}
          aria-label={t("refresh")}
        >
          <RotateCw className="h-3.5 w-3.5" aria-hidden="true" />
        </Button>
      </div>

      {loading && runs.length === 0 ? (
        <div className="flex items-center gap-2 py-4 text-xs text-muted-foreground">
          <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
        </div>
      ) : runs.length === 0 ? (
        <p className="py-4 text-xs text-muted-foreground">{t("noRuns")}</p>
      ) : (
        <ul className="flex flex-col divide-y divide-border rounded-lg border border-border">
          {runs.map((r) => (
            <li key={r.id} className="flex items-center gap-2 px-3 py-2">
              {r.trigger === "manual" ? (
                <CirclePlay
                  className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
                  aria-hidden="true"
                />
              ) : (
                <Clock
                  className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
                  aria-hidden="true"
                />
              )}
              <StatusChip status={r.status} />
              <div className="flex min-w-0 flex-1 flex-col">
                <span
                  className="truncate text-xs"
                  title={
                    r.started_at
                      ? new Date(r.started_at).toLocaleString()
                      : undefined
                  }
                >
                  {formatRelative(r.started_at, now)}
                  {r.ended_at ? (
                    <span className="text-muted-foreground">
                      {" · "}
                      {formatDuration(r.started_at, r.ended_at)}
                    </span>
                  ) : r.status === "running" ? (
                    <span className="text-muted-foreground">
                      {" · "}
                      {t("running")}
                    </span>
                  ) : null}
                </span>
                {r.error ? (
                  <span className="truncate text-[0.6875rem] text-destructive">
                    {r.error}
                  </span>
                ) : r.summary ? (
                  <span className="truncate text-[0.6875rem] text-muted-foreground">
                    {r.summary}
                  </span>
                ) : null}
              </div>
              {r.status === "running" ? (
                <Button
                  size="icon"
                  variant="ghost"
                  className="h-6 w-6 text-muted-foreground hover:text-destructive"
                  onClick={() => void cancel(r)}
                  title={t("cancelRun")}
                  aria-label={t("cancelRun")}
                >
                  <X className="h-3.5 w-3.5" aria-hidden="true" />
                </Button>
              ) : null}
              {r.conversation_id != null ? (
                <Button
                  size="icon"
                  variant="ghost"
                  className="h-6 w-6 text-muted-foreground"
                  onClick={() => viewConversation(r)}
                  title={t("viewConversation")}
                  aria-label={t("viewConversation")}
                >
                  <SquareArrowOutUpRight
                    className="h-3.5 w-3.5"
                    aria-hidden="true"
                  />
                </Button>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
