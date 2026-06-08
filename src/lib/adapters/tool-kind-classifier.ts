export type ToolKindLabel =
  | "search"
  | "command"
  | "read"
  | "memory"
  | "edit"
  | "fetch"
  | "think"
  | "todo"
  | "task"
  | "other"

export const TOOL_KIND_ORDER: ToolKindLabel[] = [
  "search",
  "read",
  "memory",
  "edit",
  "command",
  "fetch",
  "task",
  "todo",
  "think",
  "other",
]

/**
 * Identify agent-like tool calls that own their own card-style rendering
 * (e.g. AgentToolCallPart, DelegatedSubThread). These should not be folded
 * into a tool-group; they each break the run and render standalone.
 *
 * The delegation MCP tools surface with a host-specific server prefix and
 * separator: Claude Code emits `mcp__<server>__<tool>`, Codex live ACP
 * exposes them as `<server>/<tool>`, and other hosts may use `.` or `:`.
 * Match the suffix after any non-alphanumeric separator so every form lands
 * on the same code path. The bare-name comparisons cover the live-streaming
 * path (where `inferLiveToolName` has already collapsed to the canonical
 * name); the suffix regexes cover the historical path (raw tool name).
 */
const DELEGATE_TO_AGENT_SUFFIX_RE = /[^a-z0-9]delegate_to_agent$/
const GET_DELEGATION_STATUS_SUFFIX_RE = /[^a-z0-9]get_delegation_status$/
const CANCEL_DELEGATION_SUFFIX_RE = /[^a-z0-9]cancel_delegation$/
const CREATE_GOAL_SUFFIX_RE = /[^a-z0-9]create_goal$/
const UPDATE_GOAL_SUFFIX_RE = /[^a-z0-9]update_goal$/

export function isAgentLikeToolName(toolName: string): boolean {
  const name = toolName.toLowerCase().trim()
  if (name === "agent") return true
  if (
    name === "delegate_to_agent" ||
    name === "get_delegation_status" ||
    name === "cancel_delegation" ||
    name === "create_goal" ||
    name === "update_goal"
  )
    return true
  if (DELEGATE_TO_AGENT_SUFFIX_RE.test(name)) return true
  if (GET_DELEGATION_STATUS_SUFFIX_RE.test(name)) return true
  if (CANCEL_DELEGATION_SUFFIX_RE.test(name)) return true
  if (CREATE_GOAL_SUFFIX_RE.test(name)) return true
  if (UPDATE_GOAL_SUFFIX_RE.test(name)) return true
  return false
}

/**
 * Specifically the `get_delegation_status` companion tool, in either the bare
 * canonical form (live streaming, post-`inferLiveToolName`) or any host-
 * prefixed form (historical raw name: `mcp__<server>__get_delegation_status`,
 * `<server>/get_delegation_status`, …). Used to collapse a run of consecutive
 * status polls into a single merged card.
 */
export function isDelegationStatusToolName(toolName: string): boolean {
  const name = toolName.toLowerCase().trim()
  return (
    name === "get_delegation_status" ||
    GET_DELEGATION_STATUS_SUFFIX_RE.test(name)
  )
}

export function classifyToolKind(toolName: string): ToolKindLabel {
  const name = toolName.toLowerCase().trim()

  if (
    name === "grep" ||
    name === "glob" ||
    name === "search" ||
    name === "find" ||
    name === "list_files" ||
    name === "list_code_definition_names"
  ) {
    return "search"
  }

  if (
    name === "bash" ||
    name === "exec_command" ||
    name === "shell" ||
    name === "execute_command" ||
    name === "run_command"
  ) {
    return "command"
  }

  if (
    name === "read" ||
    name === "read file" ||
    name === "read_file" ||
    name === "view"
  ) {
    return "read"
  }

  if (name === "memory_recall") {
    return "memory"
  }

  if (
    name === "edit" ||
    name === "write" ||
    name === "notebookedit" ||
    name === "apply_patch" ||
    name === "str_replace" ||
    name === "create_file" ||
    name === "write_to_file" ||
    name === "replace_in_file"
  ) {
    return "edit"
  }

  if (
    name === "webfetch" ||
    name === "websearch" ||
    name === "fetch" ||
    name === "browser" ||
    name === "browser_action" ||
    name === "web_search"
  ) {
    return "fetch"
  }

  if (
    name === "think" ||
    name === "sequentialthinking" ||
    name === "enterplanmode" ||
    name === "exitplanmode" ||
    name === "switch_mode"
  ) {
    return "think"
  }

  if (
    name === "todowrite" ||
    name === "tasklist" ||
    name === "taskcreate" ||
    name === "taskupdate" ||
    name === "update_todo_list"
  ) {
    return "todo"
  }

  if (
    name === "task" ||
    name === "agent" ||
    name === "skill" ||
    name === "new_task" ||
    name === "attempt_completion"
  ) {
    return "task"
  }

  return "other"
}
