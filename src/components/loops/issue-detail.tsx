"use client"

import { useCallback, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { Ban, Loader2, Pause, Play, Settings2 } from "lucide-react"
import { toast } from "sonner"

import {
  cancelLoopIssue,
  getLoopDag,
  getLoopIssue,
  listLoopInbox,
  pauseLoopIssue,
  resumeLoopIssue,
  triggerLoopIssue,
} from "@/lib/loops-api"
import type {
  AgentType,
  IssueConfig,
  LoopArtifactRow,
  LoopInboxItemRow,
  LoopIssueDetail,
  LoopIterationOutcome,
  LoopIterationRow,
  LoopLinkRow,
  LoopStage,
} from "@/lib/types"
import { toErrorMessage } from "@/lib/app-error"
import { buildAttentionMap } from "@/lib/loop-attention"
import { useLoopResource } from "@/hooks/use-loop-resource"
import { useLoopNav } from "@/hooks/use-loop-nav"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from "@/components/ui/resizable"
import type {
  ArtifactIterationRef,
  PhasePending,
} from "@/lib/loop-process-graph"
import { DagGraph } from "@/components/loops/dag-graph"
import { StagePipelineRail } from "@/components/loops/stage-pipeline-rail"
import { IssueSettingsPanel } from "@/components/loops/issue-settings-dialog"
import { BoardView } from "@/components/loops/board-view"
import { IterationList } from "@/components/loops/iteration-list"
import { useLoopOverlays } from "@/components/loops/loop-overlays-context"
import { ArtifactList } from "@/components/loops/artifact-list"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import {
  IssuePriorityBadge,
  IssueRouteBadge,
  IssueStatusBadge,
} from "@/components/loops/issue-badges"

interface IssueDetailData {
  detail: LoopIssueDetail | null
  artifacts: LoopArtifactRow[]
  links: LoopLinkRow[]
  liveIterations: LoopIterationRow[]
  inbox: LoopInboxItemRow[]
  /** P3 agent facet. Left `undefined` on older servers (field absent) so the
   *  graph's `Array.isArray` probe reads the facet as unavailable. */
  artifactIterationRefs?: ArtifactIterationRef[]
}

const EMPTY_ISSUE_DETAIL: IssueDetailData = {
  detail: null,
  artifacts: [],
  links: [],
  liveIterations: [],
  inbox: [],
}

export function IssueDetail({
  issueId,
  spaceDefaultConfig = null,
}: {
  issueId: number | null
  /** The space's default config, threaded into the per-issue settings dialog so
   *  an inheriting issue can preview what it resolves to. */
  spaceDefaultConfig?: IssueConfig | null
}) {
  const t = useTranslations("Loops.issueDetail")
  const tList = useTranslations("Loops.issueList")
  const tCommon = useTranslations("Loops.common")
  const tToasts = useTranslations("Loops.toasts")

  const { nav, openArtifact, openSettings, closeSettings, clearFocus } =
    useLoopNav()
  const { openIteration } = useLoopOverlays()
  const settingsOpen = nav.settings
  const [actionBusy, setActionBusy] = useState(false)
  const [cancelOpen, setCancelOpen] = useState(false)

  // Issue detail + DAG + this issue's iterations, kept live by the realtime
  // provider. Scope is the STABLE `issueId` prop — never derived from the async
  // detail — and the match keys on this issue's id (every engine event carries
  // it), structurally eliminating the old "subscribe filtered by a value loaded
  // asynchronously" bug that left the board frozen.
  const { data, loading } = useLoopResource<IssueDetailData>(
    async () => {
      if (issueId == null) return EMPTY_ISSUE_DETAIL
      const [detail, dag] = await Promise.all([
        getLoopIssue(issueId),
        getLoopDag(issueId),
      ])
      if (!detail) return EMPTY_ISSUE_DETAIL
      // This issue's pending inbox cards drive the per-node attention rings (D8).
      // Fetched from the space pane and narrowed to this issue; a failed fetch
      // just yields no rings (the cards still surface in the space inbox).
      const inbox = await listLoopInbox(detail.space_id, "pending")
        .then((rows) => rows.filter((r) => r.issue_id === issueId))
        .catch(() => [] as LoopInboxItemRow[])
      // In-flight iterations ride on the DAG view (single authoritative fetch):
      // they drive the "executing now" highlight + the real-time ghost nodes and
      // stage rail. Read-stage artifacts land done/pending, so status alone can't
      // show a live node.
      return {
        detail,
        artifacts: dag.artifacts,
        links: dag.links,
        liveIterations: dag.live_iterations,
        inbox,
        artifactIterationRefs: dag.artifact_iteration_refs,
      }
    },
    {
      match: (e) => e.issue_id === issueId,
      initial: EMPTY_ISSUE_DETAIL,
      deps: [issueId],
    }
  )
  const issue = data.detail
  const { artifacts, links, liveIterations, artifactIterationRefs } = data

  // Pending inbox cards grouped onto the nodes they concern (D8) — drives the
  // amber attention rings on the graph and board.
  const attentionMap = useMemo(
    () => buildAttentionMap(data.inbox),
    [data.inbox]
  )

  // Namespaced executing keys (`artifact:{id}`) for nodes with a live iteration:
  // the issue root while triage runs, and the target task while implement/review
  // runs. Read / finalize / reflect stages are shown as ghost nodes (not by
  // highlighting an input node), so they're deliberately excluded here.
  const executingIds = useMemo(() => {
    const ids = new Set<string>()
    const root = artifacts.find((a) => a.kind === "issue")
    for (const it of liveIterations) {
      if (it.status !== "queued" && it.status !== "running") continue
      if (it.stage === "triage") {
        if (root) ids.add(`artifact:${root.id}`)
      } else if (it.stage === "implement" || it.stage === "review") {
        if (it.target_artifact_id != null)
          ids.add(`artifact:${it.target_artifact_id}`)
      }
    }
    return ids
  }, [liveIterations, artifacts])

  // Open a ghost's live iteration session in the shared viewer (the engine binds
  // a conversation when it sends the briefing; queued ghosts have none yet, so
  // the card stays inert until then). Issue context labels the viewer.
  const onOpenIteration = useCallback(
    (pending: PhasePending) => {
      if (issue == null || pending.conversationId == null) return
      openIteration({
        conversationId: pending.conversationId,
        issueContext: {
          spaceId: issue.space_id,
          issueId: issue.id,
          issueSeq: issue.seq_no,
          stage: pending.stage,
        },
      })
    },
    [openIteration, issue]
  )

  // Open a live artifact-less session (Issue triage / Result finalize chip) in
  // the shared viewer, labeled with this issue's context.
  const onOpenSession = useCallback(
    (session: {
      conversationId: number
      agentType?: AgentType | null
      outcome?: LoopIterationOutcome | null
      stage?: LoopStage
    }) => {
      if (issue == null) return
      openIteration({
        conversationId: session.conversationId,
        agentType: session.agentType,
        outcome: session.outcome,
        issueContext: {
          spaceId: issue.space_id,
          issueId: issue.id,
          issueSeq: issue.seq_no,
          stage: session.stage,
        },
      })
    },
    [openIteration, issue]
  )

  // Run an engine action; the resulting `loop://changed` event refreshes the
  // view. `onOk` carries any success-only side effect (e.g. a toast).
  const runAction = useCallback(
    async (action: () => Promise<void>, onOk?: () => void) => {
      setActionBusy(true)
      try {
        await action()
        onOk?.()
      } catch (err) {
        toast.error(tToasts("actionFailed", { message: toErrorMessage(err) }))
      } finally {
        setActionBusy(false)
      }
    },
    [tToasts]
  )

  if (issueId == null) {
    return (
      <div className="flex h-full items-center justify-center px-6 text-center text-sm text-muted-foreground">
        {t("selectPrompt")}
      </div>
    )
  }

  if (loading && !issue) {
    return (
      <div className="flex h-full items-center justify-center text-muted-foreground">
        <Loader2 className="h-5 w-5 animate-spin" />
      </div>
    )
  }

  if (!issue) return null

  const budget = issue.token_budget
  const tokenText =
    budget != null
      ? t("tokenWithBudget", {
          used: issue.token_used.toLocaleString(),
          budget: budget.toLocaleString(),
        })
      : issue.token_used.toLocaleString()

  return (
    <div className="relative flex h-full min-h-0 flex-col overflow-hidden">
      {/* Row ① — title + token usage + actions */}
      <div className="flex shrink-0 items-start justify-between gap-3 px-5 pt-4 pb-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="shrink-0 font-mono text-xs text-muted-foreground">
              #{issue.seq_no}
            </span>
            <h2 className="truncate text-base font-semibold">{issue.title}</h2>
          </div>
          <div className="mt-1.5 flex flex-wrap items-center gap-1">
            <IssueStatusBadge status={issue.status} />
            <IssuePriorityBadge priority={issue.priority} />
            <IssueRouteBadge route={issue.route} />
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          <div className="text-right text-xs text-muted-foreground">
            <div>{t("tokenUsage")}</div>
            <div className="font-mono text-sm text-foreground">{tokenText}</div>
          </div>
          {issue.status === "pending" && (
            <Button
              size="sm"
              className="h-8"
              disabled={actionBusy}
              onClick={() =>
                runAction(
                  () => triggerLoopIssue(issue.id),
                  () =>
                    toast.success(
                      tToasts("issueTriggered", { title: issue.title })
                    )
                )
              }
            >
              {actionBusy ? (
                <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
              ) : (
                <Play className="mr-1 h-3.5 w-3.5" />
              )}
              {tList("trigger")}
            </Button>
          )}
          {issue.status === "running" && (
            <Button
              size="sm"
              variant="outline"
              className="h-8"
              disabled={actionBusy}
              onClick={() => runAction(() => pauseLoopIssue(issue.id))}
            >
              <Pause className="mr-1 h-3.5 w-3.5" />
              {tList("pause")}
            </Button>
          )}
          {issue.status === "paused" && (
            <Button
              size="sm"
              className="h-8"
              disabled={actionBusy}
              onClick={() => runAction(() => resumeLoopIssue(issue.id))}
            >
              <Play className="mr-1 h-3.5 w-3.5" />
              {tList("resume")}
            </Button>
          )}
          {(issue.status === "running" ||
            issue.status === "paused" ||
            issue.status === "blocked") && (
            <Button
              size="sm"
              variant="ghost"
              className="h-8 text-destructive hover:text-destructive"
              disabled={actionBusy}
              onClick={() => setCancelOpen(true)}
            >
              <Ban className="mr-1 h-3.5 w-3.5" />
              {tList("cancel")}
            </Button>
          )}
          <Button
            size="icon"
            variant="ghost"
            className="h-8 w-8"
            onClick={() => openSettings()}
          >
            <Settings2 className="h-4 w-4" />
            <span className="sr-only">{t("settings")}</span>
          </Button>
        </div>
      </div>

      {/* Rows ② + ③ — resizable: graph/board over iterations/artifacts */}
      <ResizablePanelGroup
        direction="vertical"
        autoSaveId="loop:issue:rows"
        className="min-h-0 flex-1 border-t"
      >
        <ResizablePanel
          defaultSize={68}
          minSize={30}
          className="flex min-h-0 flex-col"
        >
          <div className="shrink-0 px-4 pt-2">
            <StagePipelineRail
              route={issue.route}
              artifacts={artifacts}
              liveIterations={liveIterations}
            />
          </div>
          <Tabs defaultValue="graph" className="flex min-h-0 flex-1 flex-col">
            <TabsList className="mx-auto mt-2 self-center">
              <TabsTrigger value="graph">{t("subtabGraph")}</TabsTrigger>
              <TabsTrigger value="board">{t("subtabBoard")}</TabsTrigger>
            </TabsList>
            <TabsContent
              value="graph"
              className="min-h-0 flex-1 overflow-auto p-5 data-[state=inactive]:hidden"
            >
              <DagGraph
                artifacts={artifacts}
                links={links}
                liveIterations={liveIterations}
                executingIds={executingIds}
                attentionMap={attentionMap}
                focus={nav.focus}
                onFocusConsumed={clearFocus}
                onSelect={openArtifact}
                onOpenIteration={onOpenIteration}
                artifactIterationRefs={artifactIterationRefs}
                onOpenSession={onOpenSession}
              />
              {artifacts.length <= 1 && liveIterations.length === 0 && (
                <p className="mt-4 text-center text-xs text-muted-foreground">
                  {t("graphPlaceholder")}
                </p>
              )}
            </TabsContent>
            <TabsContent
              value="board"
              className="min-h-0 flex-1 overflow-auto p-5 data-[state=inactive]:hidden"
            >
              <BoardView
                artifacts={artifacts}
                liveIterations={liveIterations}
                attentionMap={attentionMap}
                onSelect={openArtifact}
              />
            </TabsContent>
          </Tabs>
        </ResizablePanel>
        <ResizableHandle />
        <ResizablePanel defaultSize={32} minSize={12} className="min-h-0">
          <Tabs
            defaultValue="iterations"
            className="flex h-full min-h-0 flex-col"
          >
            <TabsList className="mx-5 mt-2 self-start">
              <TabsTrigger value="iterations">
                {t("subtabIterations")}
              </TabsTrigger>
              <TabsTrigger value="artifacts">
                {t("subtabArtifacts")}
              </TabsTrigger>
            </TabsList>
            <TabsContent
              value="iterations"
              className="min-h-0 flex-1 overflow-y-auto px-5 py-2 data-[state=inactive]:hidden"
            >
              <IterationList spaceId={issue.space_id} issueId={issue.id} />
            </TabsContent>
            <TabsContent
              value="artifacts"
              className="min-h-0 flex-1 overflow-y-auto px-5 py-2 data-[state=inactive]:hidden"
            >
              <ArtifactList
                artifacts={artifacts}
                onSelect={openArtifact}
                showIssue={false}
              />
            </TabsContent>
          </Tabs>
        </ResizablePanel>
      </ResizablePanelGroup>

      <AlertDialog
        open={cancelOpen}
        onOpenChange={(o) => !o && setCancelOpen(false)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{tList("cancelConfirmTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {tList("cancelConfirmDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={actionBusy}>
              {tCommon("cancel")}
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                e.preventDefault()
                void runAction(
                  () => cancelLoopIssue(issue.id),
                  () => setCancelOpen(false)
                )
              }}
              disabled={actionBusy}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              {actionBusy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {tList("cancelConfirmAction")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {settingsOpen && (
        <IssueSettingsPanel
          issue={issue}
          spaceDefaultConfig={spaceDefaultConfig}
          onClose={closeSettings}
        />
      )}
    </div>
  )
}
