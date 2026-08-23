import {
  Archive,
  ArchiveRestore,
  Ban,
  CalendarClock,
  CircleCheck,
  GitMerge,
  GitPullRequestArrow,
  ListX,
  MessageSquareText,
  Pencil,
  Play,
  RotateCw,
  type LucideIcon,
} from "lucide-react"
import {
  hasNothingToMerge,
  isMergeQueued,
  mustDeliverToPr,
  usesMergeRequests,
} from "./task-acceptance"
import type { WorkTask } from "@/lib/types"

export interface TaskActionItem {
  icon: LucideIcon
  label: string
  onClick: () => void
}

/** The per-task callbacks a card or list row wires its actions to. */
export interface TaskActionHandlers {
  onStart: () => void
  onCancel: () => void
  onRetry: () => void
  onRequeue: () => void
  /** Opens the read-only live session viewer (TaskTranscriptDialog). */
  onViewSession: () => void
  onMerge: () => void
  /** Pushes the task to the forge: a new pull request for an issue-sourced
   *  task, back onto its own branch for a pull-request-sourced one — which is
   *  the ONLY acceptance those have (see `mustDeliverToPr`). */
  onDeliverPr: () => void
  /** Withdraws a merge that is waiting in the project's queue (see
   *  `isMergeQueued`) — the task stays in review. */
  onUnqueueMerge: () => void
  /** Takes the merge slot when the task changed nothing (see
   *  `hasNothingToMerge`) — accepts it outright instead. */
  onComplete: () => void
  /** Toggles by `task.archived_at` (archive ⇄ unarchive). */
  onArchive: () => void
  /** Opens the editor dialog — offered while editable (todo / failed). */
  onEdit: () => void
  /** Opens the schedule dialog — to-do tasks only (plan / re-plan / clear). */
  onSchedule: () => void
}

type ActionLabelKey =
  | "actionStart"
  | "actionCancel"
  | "actionRetry"
  | "actionRequeue"
  | "actionViewSession"
  | "actionMerge"
  | "actionDeliverPrBack"
  | "actionDeliverPrBackMr"
  | "actionUnqueueMerge"
  | "actionComplete"
  | "actionArchive"
  | "actionUnarchive"
  | "actionEdit"
  | "actionSchedule"

/**
 * The action set a task offers, shared by the board card and the list row so
 * the two can never drift: exactly one filled primary per status on the left,
 * and the secondaries (edit / archive) on the right, always ending in "查看会话"
 * when a session exists. The full set — with labels — lives in the detail
 * sheet. `merging` has no primary: it cannot be canceled.
 */
export function buildTaskActions(
  task: WorkTask,
  t: (key: ActionLabelKey) => string,
  handlers: TaskActionHandlers
): { primary: TaskActionItem | null; secondaries: TaskActionItem[] } {
  const secondaries: TaskActionItem[] = []
  let primary: TaskActionItem | null = null
  // An archived task offers exactly one way back — and still the session
  // viewer appended below, which is what archiving is usually for.
  if (task.archived_at != null) {
    primary = {
      icon: ArchiveRestore,
      label: t("actionUnarchive"),
      onClick: handlers.onArchive,
    }
  } else {
    switch (task.status) {
      case "todo":
        primary = {
          icon: Play,
          label: t("actionStart"),
          onClick: handlers.onStart,
        }
        secondaries.push({
          icon: Pencil,
          label: t("actionEdit"),
          onClick: handlers.onEdit,
        })
        // "Start" and "start later" are the same decision — the plan lives
        // next to the button that skips it.
        secondaries.push({
          icon: CalendarClock,
          label: t("actionSchedule"),
          onClick: handlers.onSchedule,
        })
        break
      case "queued":
      case "preparing":
      case "running":
      case "awaiting_input":
        primary = {
          icon: Ban,
          label: t("actionCancel"),
          onClick: handlers.onCancel,
        }
        break
      case "review":
        if (isMergeQueued(task)) {
          // Already accepted — it is waiting for the project's merge slot, and
          // the only decision left is to take it back out of the queue. Like
          // `merging`, there is no primary: nothing here needs a filled button.
          secondaries.push({
            icon: ListX,
            label: t("actionUnqueueMerge"),
            onClick: handlers.onUnqueueMerge,
          })
          break
        }
        // Nothing was changed ⟹ nothing to merge: offer the acceptance that
        // actually applies. A task that came from a pull request has its own
        // one — its work goes back to that pull request, never onto the local
        // base.
        if (hasNothingToMerge(task)) {
          primary = {
            icon: CircleCheck,
            label: t("actionComplete"),
            onClick: handlers.onComplete,
          }
        } else if (mustDeliverToPr(task)) {
          primary = {
            icon: GitPullRequestArrow,
            label: t(
              usesMergeRequests(task)
                ? "actionDeliverPrBackMr"
                : "actionDeliverPrBack"
            ),
            onClick: handlers.onDeliverPr,
          }
        } else {
          primary = {
            icon: GitMerge,
            label: t("actionMerge"),
            onClick: handlers.onMerge,
          }
        }
        break
      case "merging":
        break
      case "failed":
        primary = {
          icon: RotateCw,
          label: t("actionRetry"),
          onClick: handlers.onRetry,
        }
        secondaries.push({
          icon: Pencil,
          label: t("actionEdit"),
          onClick: handlers.onEdit,
        })
        secondaries.push({
          icon: Archive,
          label: t("actionArchive"),
          onClick: handlers.onArchive,
        })
        break
      case "done":
        primary = {
          icon: Archive,
          label: t("actionArchive"),
          onClick: handlers.onArchive,
        }
        break
      case "canceled":
        primary = {
          icon: RotateCw,
          label: t("actionRequeue"),
          onClick: handlers.onRequeue,
        }
        secondaries.push({
          icon: Archive,
          label: t("actionArchive"),
          onClick: handlers.onArchive,
        })
        break
    }
  }
  // The session viewer is offered in every status once a session exists (live
  // ones stream it in real time) and always sorts last, anchoring the corner.
  if (task.conversation_id != null) {
    secondaries.push({
      icon: MessageSquareText,
      label: t("actionViewSession"),
      onClick: handlers.onViewSession,
    })
  }
  return { primary, secondaries }
}
