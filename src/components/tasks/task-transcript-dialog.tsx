"use client"

/**
 * Live session viewer for a work task — the same read-only streaming surface
 * as the delegation sub-agent viewer (`LiveTranscriptView`), without opening
 * the conversation in the workbench. Every "查看会话" affordance on the board
 * (card secondary button, detail-sheet action zone) lands here.
 *
 * Unlike delegation children (attached by the delegation provider), a
 * headless work-task connection is invisible to the frontend until attached:
 * on desktop the global acp://event router drops envelopes with no reverse-map
 * entry, and on web there is no per-connection stream at all. So while the
 * task is in a live status this viewer owns an
 * `attachDelegationChild`/`detachDelegationChild` pair for the task's
 * connection (identity parent mapping — there is no real parent tool call).
 * For settled tasks the DB row's connection_id is stale and the connection is
 * gone; we skip the attach and the viewer renders the persisted transcript.
 *
 * A side drawer, like the delegation viewer it shares `LiveTranscriptView`
 * with: non-modal so the board stays readable behind it, no pointer dismissal
 * so working in the board doesn't take it down, and — the reason it matters
 * here — it STACKS. Opened from the detail sheet (itself a drawer) it mounts
 * inside that sheet's React tree and Base UI slides it over the top; the
 * transcript's own `delegate_to_agent` cards then open a third layer.
 */

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { Loader2 } from "lucide-react"

import { AgentIcon } from "@/components/agent-icon"
import { LiveTranscriptView } from "@/components/message/live-transcript-view"
import { type ResolvedMessageGroup } from "@/components/message/message-list-view"
import { StatusChip } from "./task-card"
import {
  Drawer,
  DrawerContent,
  DrawerDescription,
  DrawerTitle,
  SIDE_PANEL_CONTENT_CLASS,
} from "@/components/ui/drawer"
import { useAcpActions } from "@/contexts/acp-connections-context"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { getFolderConversation, workTaskEvents } from "@/lib/api"
import { FOLLOW_UP_SCENARIOS } from "@/lib/task-follow-up"
import {
  extractRounds,
  firstTextOfParts,
  matchRound,
  type TaskRound,
} from "@/lib/task-rounds"
import { type AgentType, type WorkTask } from "@/lib/types"

interface TaskTranscriptDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  task: WorkTask | null
}

/** Statuses in which the engine holds a live connection worth attaching
 *  (`merging` included — the merge is an agent turn too). */
function isLive(task: WorkTask): boolean {
  return (
    task.status === "running" ||
    task.status === "awaiting_input" ||
    task.status === "merging"
  )
}

export function TaskTranscriptDialog({
  open,
  onOpenChange,
  task,
}: TaskTranscriptDialogProps) {
  const t = useTranslations("Tasks")

  return (
    <Drawer open={open} onOpenChange={onOpenChange} swipeDirection="right">
      {/* Exactly the detail sheet's width — it stacks directly over it. */}
      <DrawerContent
        closeButtonClassName="top-2.5 right-3"
        className={SIDE_PANEL_CONTENT_CLASS}
      >
        <DrawerTitle className="sr-only">{t("transcriptTitle")}</DrawerTitle>
        <DrawerDescription className="sr-only">
          {t("transcriptDescription")}
        </DrawerDescription>
        {open && task != null && task.conversation_id != null ? (
          <TaskAgentResolver task={task} />
        ) : null}
      </DrawerContent>
    </Drawer>
  )
}

/**
 * Resolve the session's agent BEFORE the viewer mounts. It selects the parser
 * every live chunk is rendered with, and it is a per-task setting (stable
 * across the task's generations), so resolving it once up front — rather than
 * guessing and refining later — is what keeps chunks off the wrong renderer.
 * A per-task override is definitive and instant; otherwise one conversation
 * read returns the agent recorded at dispatch, with the folder's default agent
 * covering a failed read.
 */
function TaskAgentResolver({ task }: { task: WorkTask }) {
  const folders = useAppWorkspaceStore((s) => s.folders)
  const override = task.config?.agent_type ?? null
  const folderDefault =
    folders.find((f) => f.id === task.folder_id)?.default_agent_type ?? null
  const conversationId = task.conversation_id as number // gated by the host
  const [resolved, setResolved] = useState<AgentType | null>(override)
  useEffect(() => {
    if (override != null) return
    let cancelled = false
    getFolderConversation(conversationId)
      .then((detail) => {
        if (cancelled) return
        setResolved(detail.summary.agent_type ?? folderDefault ?? "claude_code")
      })
      .catch(() => {
        // Transcript unreadable (agent gone, file pruned) — best-effort.
        if (!cancelled) setResolved(folderDefault ?? "claude_code")
      })
    return () => {
      cancelled = true
    }
  }, [override, conversationId, folderDefault])

  if (resolved == null) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2
          className="size-5 animate-spin text-muted-foreground"
          aria-hidden="true"
        />
      </div>
    )
  }
  return <TaskTranscriptBody task={task} agentType={resolved} />
}

function TaskTranscriptBody({
  task,
  agentType,
}: {
  task: WorkTask
  agentType: AgentType
}) {
  const t = useTranslations("Tasks")
  const { attachDelegationChild, detachDelegationChild } = useAcpActions()

  // Round markers → phase dividers above the matching user turns. Refetched
  // when a new generation dispatches (run_seq moves) so a merge started while
  // watching gets its divider too.
  const [rounds, setRounds] = useState<TaskRound[]>([])
  const runSeq = task.run_seq
  useEffect(() => {
    let cancelled = false
    workTaskEvents(task.id, 500)
      .then((events) => {
        if (!cancelled) setRounds(extractRounds(events))
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [task.id, runSeq])
  const userTurnHeader = useCallback(
    (group: ResolvedMessageGroup) => {
      const round = matchRound(rounds, firstTextOfParts(group.parts))
      switch (round?.kind) {
        case "work":
          return t("phaseWork")
        case "retry":
          return t("phaseRetry")
        case "return": {
          // Name the actual scenario — "rework" and "answer my question" are
          // both `return` rounds. Rounds from before scenarios existed carry no
          // intent and keep the generic label.
          const scenario = FOLLOW_UP_SCENARIOS.find(
            (s) => s.intent === round.intent
          )
          return scenario ? t(scenario.labelKey) : t("phaseReturn")
        }
        case "merge":
          return t("phaseMerge")
        default:
          return null
      }
    },
    [rounds, t]
  )

  // The connection to stream from, tracked FORWARD ONLY.
  //
  // This used to be latched at mount, which silently downgraded the viewer to a
  // persisted-transcript reader for its whole lifetime whenever the latch came
  // up empty — and then every unfinished tool call of the running turn rendered
  // as settled (a `get_delegation_status` blocking on its sub-agent showed a
  // green ✓ for the entire wait). Three ways it came up empty, all while the
  // board still shows the task as 进行中 or otherwise live:
  //   - the 进行中 column is `preparing | running` but `isLive` is
  //     `running | awaiting_input | merging`, so a re-run generation sitting in
  //     `preparing` (its conversation_id survives from the previous run, so
  //     "查看会话" IS offered) latched null;
  //   - `begin_merge` clears `connection_id` in the same update that sets
  //     `merging`, so opening in that interval latched null;
  //   - a viewer held open across a generation boundary kept the previous
  //     connection, which `on_turn_complete` has already disconnected.
  //
  // Plain re-derivation is NOT the fix either — that was the reason for the
  // latch: the moment the provider flips the task to review we would detach and
  // race the final turn-complete the bridge needs to promote the live reply,
  // and a long-settled task must never attach at all (on web that opens a
  // per-connection stream for a connection the backend no longer has). So the
  // id only ever moves FORWARD onto a new live connection and never falls back
  // to null. `attachId` starts null for a task that is not live, so a settled
  // task still attaches to nothing.
  const liveConnectionId = isLive(task) ? task.connection_id : null
  const [attachId, setAttachId] = useState<string | null>(liveConnectionId)
  if (liveConnectionId !== null && liveConnectionId !== attachId) {
    // Adjusting state during render (the React-sanctioned form) rather than in
    // an effect: the attach below must see the new id on this very render, not
    // one commit later.
    setAttachId(liveConnectionId)
  }
  const taskId = task.id
  useEffect(() => {
    if (attachId == null) return
    attachDelegationChild({
      connectionId: attachId,
      parentConnectionId: attachId,
      parentToolUseId: `work-task-${taskId}`,
      // Agent as resolved before this body mounted. It is a per-task setting,
      // stable across generations, so re-attaching never needs a fresh read.
      agentType,
      // Unlike a real delegation child (attached the moment it spawns), this
      // viewer opens onto a turn already in progress — hydrate its state
      // before routing or the desktop firehose would only show whatever the
      // agent happens to emit next.
      hydrate: true,
    })
    // Detaches the PREVIOUS connection when the id moves to a new generation,
    // and the current one when the viewer closes.
    return () => detachDelegationChild(attachId)
  }, [
    attachId,
    taskId,
    agentType,
    attachDelegationChild,
    detachDelegationChild,
  ])

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* Aligned with the transcript's own 16px row inset below, as in the
          delegation viewer. `pr-12` clears the close button. */}
      <div className="flex items-center gap-3 border-b border-border px-4 py-2.5 pr-12">
        <span className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-border bg-background text-foreground">
          <AgentIcon agentType={agentType} className="h-4 w-4" />
        </span>
        <span className="min-w-0 flex-1 truncate text-sm font-semibold text-foreground">
          {task.title}
        </span>
        <StatusChip task={task} />
      </div>
      <LiveTranscriptView
        conversationId={task.conversation_id as number}
        connectionId={attachId}
        agentType={agentType}
        kickoffText={task.config?.display_text ?? null}
        userTurnHeader={userTurnHeader}
      />
    </div>
  )
}
