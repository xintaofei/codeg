/**
 * Follow-up scenarios for a reviewed work task.
 *
 * The board offers ONE neutral "follow up" action on a reviewed task; the
 * intent picks the wording the backend wraps around whatever the user typed.
 * That distinction is the point: the same sentence means "fix this", "also do
 * this" or "explain this" depending on which they meant, and an agent told its
 * work was *returned* starts editing files either way — including when the user
 * only wanted an answer.
 *
 * Mirrors `FollowUpIntent` in `src-tauri/src/models/work_task.rs`. The list
 * order is the chip order.
 */

import {
  CircleQuestionMark,
  ListChecks,
  PenLine,
  ShieldCheck,
  type LucideIcon,
} from "lucide-react"

import type { AgentType, FolderDetail, WorkTask } from "./types"

export type FollowUpIntent = "revise" | "continue" | "question" | "verify"

/**
 * `as const` and no widening annotation on purpose: next-intl types `t()` by
 * literal key, so `labelKey: string` would make every lookup a type error.
 */
export const FOLLOW_UP_SCENARIOS = [
  {
    // First, and the default: this is what the action did before it grew
    // scenarios, down to the prompt the agent receives.
    intent: "revise",
    icon: PenLine,
    labelKey: "followUpIntentRevise",
    placeholderKey: "followUpPlaceholderRevise",
    allowsEmpty: false,
  },
  {
    intent: "continue",
    icon: ListChecks,
    labelKey: "followUpIntentContinue",
    placeholderKey: "followUpPlaceholderContinue",
    allowsEmpty: false,
  },
  {
    intent: "question",
    icon: CircleQuestionMark,
    labelKey: "followUpIntentQuestion",
    placeholderKey: "followUpPlaceholderQuestion",
    allowsEmpty: false,
  },
  {
    intent: "verify",
    icon: ShieldCheck,
    labelKey: "followUpIntentVerify",
    placeholderKey: "followUpPlaceholderVerify",
    allowsEmpty: true,
  },
] as const satisfies readonly {
  intent: FollowUpIntent
  icon: LucideIcon
  /** `Tasks` message key for the chip label. */
  labelKey: string
  /** `Tasks` message key for the textarea placeholder. */
  placeholderKey: string
  /**
   * Whether the scenario is a complete instruction without user text. Only the
   * self-check is: "look it over before I accept it" needs no elaboration, and
   * making it one click is most of its value.
   */
  allowsEmpty: boolean
}[]

export type FollowUpScenario = (typeof FOLLOW_UP_SCENARIOS)[number]

export const DEFAULT_FOLLOW_UP_INTENT: FollowUpIntent = "revise"

export function followUpScenario(intent: FollowUpIntent): FollowUpScenario {
  return (
    FOLLOW_UP_SCENARIOS.find((s) => s.intent === intent) ??
    FOLLOW_UP_SCENARIOS[0]
  )
}

/**
 * Whether this scenario can be sent with what is currently in the box.
 *
 * An attachment counts as content on its own: a screenshot of the bug with no
 * sentence is a complete instruction, and it reaches the agent as its own
 * prompt block — refusing it because the prose is empty would drop it silently.
 */
export function canSubmitFollowUp(
  intent: FollowUpIntent,
  text: string,
  hasAttachments = false
): boolean {
  return (
    text.trim().length > 0 ||
    hasAttachments ||
    followUpScenario(intent).allowsEmpty
  )
}

/** What the follow-up composer resolves its commands / references against. */
export interface FollowUpComposerTarget {
  /** Whose slash commands and skills are offered. */
  agentType: AgentType
  /** Working directory `@` references and the command probe run in. */
  folderPath: string | null
  /** Worktree whose project-scoped skills are available to the task. */
  skillWorkspacePath: string | null
}

/** The subset of a folder this resolution needs. */
type FolderLike = Pick<FolderDetail, "id" | "path" | "default_agent_type">

/**
 * Where the follow-up box should look for files and commands.
 *
 * Path: the task's own WORKTREE when it has one. That is the checkout the agent
 * has been working in, so it is what the user means by "the file I'm pointing
 * at" — the project root would offer the pre-task state under paths the agent
 * never sees. Falls back to the project folder (a task that never started has
 * no worktree yet).
 *
 * Agent: the conversation's, which is what actually ran; then the task's own
 * override, which is all we have before the conversation detail lands; then the
 * folder default. `claude_code` is the last resort, mirroring the task editor —
 * a wrong guess costs an empty command list, not a wrong launch (the follow-up
 * resumes the task's existing session either way).
 */
export function followUpComposerTarget(args: {
  task: WorkTask
  /** Agent of the task's conversation, once its detail has loaded. */
  conversationAgentType: AgentType | null
  folders: FolderLike[]
}): FollowUpComposerTarget {
  const { task, conversationAgentType, folders } = args
  const byId = (id: number | null) =>
    id == null ? undefined : folders.find((f) => f.id === id)
  const projectFolder = byId(task.folder_id)
  const worktreeFolder = byId(task.worktree_folder_id)
  return {
    agentType:
      conversationAgentType ??
      task.config?.agent_type ??
      projectFolder?.default_agent_type ??
      "claude_code",
    folderPath: worktreeFolder?.path ?? projectFolder?.path ?? null,
    skillWorkspacePath: worktreeFolder?.path ?? null,
  }
}
