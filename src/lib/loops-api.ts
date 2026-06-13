import { getTransport } from "./transport"
import type {
  IssueConfig,
  LoopArtifactDetail,
  LoopArtifactRow,
  LoopDagView,
  LoopInboxItemRow,
  LoopInboxStatus,
  LoopIssueDetail,
  LoopIssuePriority,
  LoopIssueRow,
  LoopIssueStatus,
  LoopIterationRow,
  LoopMemoryKind,
  LoopMemoryRow,
  LoopMemoryStatus,
  LoopSpaceSummary,
} from "./types"

// ─── Spaces ──────────────────────────────────────────────────────────────

export function listLoopSpaces() {
  return getTransport().call<LoopSpaceSummary[]>("list_loop_spaces", {})
}

export function createLoopSpace(name: string, folderId: number) {
  return getTransport().call<LoopSpaceSummary>("create_loop_space", {
    name,
    folderId,
  })
}

export function updateLoopSpace(id: number, name: string) {
  return getTransport().call<LoopSpaceSummary>("update_loop_space", {
    id,
    name,
  })
}

export function deleteLoopSpace(id: number) {
  return getTransport().call<void>("delete_loop_space", { id })
}

// ─── Issues ──────────────────────────────────────────────────────────────

export function listLoopIssues(spaceId: number, statuses?: LoopIssueStatus[]) {
  return getTransport().call<LoopIssueRow[]>("list_loop_issues", {
    spaceId,
    statuses: statuses ?? null,
  })
}

export function getLoopIssue(id: number) {
  return getTransport().call<LoopIssueDetail | null>("get_loop_issue", { id })
}

export function createLoopIssue(params: {
  spaceId: number
  title: string
  description: string
  priority: LoopIssuePriority
  config?: IssueConfig
}) {
  return getTransport().call<LoopIssueDetail>("create_loop_issue", {
    spaceId: params.spaceId,
    title: params.title,
    description: params.description,
    priority: params.priority,
    config: params.config ?? null,
  })
}

export function deleteLoopIssue(id: number) {
  return getTransport().call<void>("delete_loop_issue", { id })
}

export function updateLoopIssueConfig(
  id: number,
  config: IssueConfig,
  tokenBudget: number | null
) {
  return getTransport().call<void>("update_loop_issue_config", {
    id,
    config,
    tokenBudget,
  })
}

// ─── Engine actions (trigger / pause / resume / cancel) ─────────────────────

export function triggerLoopIssue(id: number) {
  return getTransport().call<void>("trigger_loop_issue", { id })
}

export function pauseLoopIssue(id: number) {
  return getTransport().call<void>("pause_loop_issue", { id })
}

export function resumeLoopIssue(id: number) {
  return getTransport().call<void>("resume_loop_issue", { id })
}

export function cancelLoopIssue(id: number) {
  return getTransport().call<void>("cancel_loop_issue", { id })
}

/** Retry a blocked issue: re-arm its blocked tasks and resume driving. */
export function retryLoopIssue(id: number) {
  return getTransport().call<void>("retry_loop_issue", { id })
}

/** Add `additional` tokens to a budget-paused issue's budget and resume it. */
export function addLoopIssueBudget(id: number, additional: number) {
  return getTransport().call<void>("add_loop_issue_budget", { id, additional })
}

// ─── Approval gates (design / merge) ────────────────────────────────────────

export function approveLoopDesign(id: number) {
  return getTransport().call<void>("approve_loop_design", { id })
}

export function rejectLoopDesign(id: number, comment?: string) {
  return getTransport().call<void>("reject_loop_design", {
    id,
    comment: comment ?? null,
  })
}

export function approveLoopMerge(id: number) {
  return getTransport().call<void>("approve_loop_merge", { id })
}

export function rejectLoopMerge(id: number, comment?: string) {
  return getTransport().call<void>("reject_loop_merge", {
    id,
    comment: comment ?? null,
  })
}

// ─── Artifacts / DAG ───────────────────────────────────────────────────────

export function getLoopDag(issueId: number) {
  return getTransport().call<LoopDagView>("get_loop_dag", { issueId })
}

export function listLoopArtifacts(spaceId: number) {
  return getTransport().call<LoopArtifactRow[]>("list_loop_artifacts", {
    spaceId,
  })
}

export function getLoopArtifact(id: number) {
  return getTransport().call<LoopArtifactDetail | null>("get_loop_artifact", {
    id,
  })
}

// ─── Iterations ────────────────────────────────────────────────────────────

export function listLoopIterations(spaceId: number, issueId?: number) {
  return getTransport().call<LoopIterationRow[]>("list_loop_iterations", {
    spaceId,
    issueId: issueId ?? null,
  })
}

// ─── Inbox ─────────────────────────────────────────────────────────────────

export function listLoopInbox(spaceId: number, status?: LoopInboxStatus) {
  return getTransport().call<LoopInboxItemRow[]>("list_loop_inbox", {
    spaceId,
    status: status ?? null,
  })
}

// ─── Memory ────────────────────────────────────────────────────────────────

export function listLoopMemory(spaceId: number) {
  return getTransport().call<LoopMemoryRow[]>("list_loop_memory", { spaceId })
}

export function createLoopMemory(params: {
  spaceId: number
  kind: LoopMemoryKind
  title: string
  content: string
}) {
  return getTransport().call<LoopMemoryRow>("create_loop_memory", params)
}

export function updateLoopMemory(params: {
  spaceId: number
  id: number
  title: string
  content: string
  status: LoopMemoryStatus
}) {
  return getTransport().call<void>("update_loop_memory", params)
}

export function deleteLoopMemory(spaceId: number, id: number) {
  return getTransport().call<void>("delete_loop_memory", { spaceId, id })
}
