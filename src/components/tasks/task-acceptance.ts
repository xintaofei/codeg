import type { WorkTask } from "@/lib/types"

/**
 * Whether the task's worktree is gone: it ran in one (every generation does),
 * but the worktree was since removed — detached entirely, or its folder /
 * directory no longer exists (`worktree_missing`, stamped by the backend).
 * A merge generation cannot run without the worktree, so a reviewed task in
 * this state completes instead.
 */
export function isWorktreeGone(task: WorkTask): boolean {
  return task.worktree_folder_id == null || task.worktree_missing === true
}

/**
 * Whether the task HAD a worktree that has since been deleted — the card badge
 * predicate, distinct from `isWorktreeGone` in exactly one way: a just-created
 * task that never initialized (no worktree ever minted) is not "removed", it
 * simply hasn't started. `work_branch` is the witness that a worktree once
 * existed: it is recorded together with the worktree and survives the detach
 * that clears the folder pointer.
 */
export function worktreeWasRemoved(task: WorkTask): boolean {
  const hadWorktree =
    task.work_branch != null || task.worktree_folder_id != null
  return hadWorktree && isWorktreeGone(task)
}

/**
 * Whether a reviewed task has no merge to offer, so "complete" takes the
 * primary slot instead:
 * - the run settled with an empty diff against the recorded base — only `0`
 *   counts; `null` means the engine could not read the stats, and merging
 *   stays the safe default there. The engine re-runs the same diff before it
 *   finishes the task, so a stale card cannot drop work;
 * - or the worktree is gone (see `isWorktreeGone`) — a merge could only fail,
 *   and completing is the one acceptance left. The engine preserves a work
 *   branch that still holds unlanded commits.
 */
export function hasNothingToMerge(task: WorkTask): boolean {
  if (task.status !== "review") return false
  return task.files_changed === 0 || isWorktreeGone(task)
}

/**
 * Whether this task is waiting its turn in its project's merge queue — the user
 * accepted it while another task of the same project was landing. It stays in
 * review until the engine's merge pump dispatches it, so the queue mark is what
 * tells that row apart from one nobody has decided on yet.
 */
export function isMergeQueued(task: WorkTask): boolean {
  return task.status === "review" && task.merge_queued != null
}

/**
 * Whether a merge of this task's project is running right now. Merges are
 * serial per project (one base branch, one landing at a time), so this is what
 * turns the merge button's "merge" into "queue".
 */
export function isFolderMerging(tasks: WorkTask[], folderId: number): boolean {
  return tasks.some((t) => t.folder_id === folderId && t.status === "merging")
}

/**
 * Place in line (1-based) for every queued task, per project — the order the
 * engine's pump will actually dispatch them in: oldest request first, task id
 * breaking a tie, exactly as `drain_merge_queue` sorts. A task that is not
 * queued has no entry.
 */
export function mergeQueueRanks(tasks: WorkTask[]): Map<number, number> {
  const byFolder = new Map<number, WorkTask[]>()
  for (const task of tasks) {
    if (!isMergeQueued(task)) continue
    const bucket = byFolder.get(task.folder_id)
    if (bucket) bucket.push(task)
    else byFolder.set(task.folder_id, [task])
  }
  const ranks = new Map<number, number>()
  // Compared as instants, not as strings: the backend writes RFC 3339 with a
  // variable number of fractional digits, and "…:00Z" sorts AFTER "…:00.5Z"
  // lexicographically.
  const at = (task: WorkTask): number => {
    const ms = Date.parse(task.merge_queued?.queued_at ?? "")
    return Number.isNaN(ms) ? 0 : ms
  }
  for (const bucket of byFolder.values()) {
    bucket.sort((a, b) => at(a) - at(b) || a.id - b.id)
    bucket.forEach((task, i) => ranks.set(task.id, i + 1))
  }
  return ranks
}

/**
 * Whether a reviewed task can be DELIVERED to the forge — as a new pull
 * request for an issue-sourced task, or back onto its own branch for a
 * pull-request-sourced one. Three conditions, and the backend re-checks every
 * one of them; this predicate only decides whether to offer the button.
 * - it came from a forge issue or pull request (a plain local task has no
 *   repository to push to);
 * - it has something to land — GitHub answers an empty pull request with a
 *   422, and `hasNothingToMerge` is the same test the merge button uses;
 * - its worktree is still there (folded into `hasNothingToMerge`).
 */
export function canDeliverToPr(task: WorkTask): boolean {
  return (
    task.status === "review" &&
    (task.source_kind === "forge_issue" || task.source_kind === "forge_pr") &&
    !hasNothingToMerge(task)
  )
}

/**
 * Whether this task's work belongs on the pull request it came from rather
 * than on the local base branch. The backend REFUSES a local merge for these
 * (landing it here would take the pull request's changes in behind its author's
 * back, leaving the review open and apparently unmerged), so the board must
 * not offer one — delivering back is the acceptance.
 */
export function mustDeliverToPr(task: WorkTask): boolean {
  return task.source_kind === "forge_pr"
}

/**
 * Whether this task's forge calls a proposed change a MERGE request. Only the
 * wording of the acceptance depends on it — a button that promises a pull
 * request and then opens a merge request reads like the wrong tool answered.
 * Tasks with no forge provenance (and rows stored before GitLab support) are
 * GitHub's, which is what they have always been.
 */
export function usesMergeRequests(task: WorkTask | null | undefined): boolean {
  return task?.source_meta?.provider === "gitlab"
}

/** The pull request a delivered task ended up in, if that is how it finished. */
export function deliveredPrUrl(task: WorkTask): string | null {
  if (task.completion_kind !== "delivered_pr") return null
  return task.source_meta?.result_pr ?? null
}
