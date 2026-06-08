const EXACT_TOOL_NAME_ALIASES: Record<string, string> = {
  shell_command: "bash",
  "functions.shell_command": "bash",
  exec_command: "exec_command",
  "functions.exec_command": "exec_command",
  "functions.read": "read",
  "functions.edit": "edit",
  "functions.write": "write",
  "functions.apply_patch": "apply_patch",
  change: "edit",
  "functions.change": "edit",
  changes: "edit",
  write_stdin: "bash",
  read_file: "read",
  read_text_file: "read",
  readfile: "read",
  "read file": "read",
  edit_file: "edit",
  update_file: "edit",
  write_file: "write",
  mcp__acp__read: "read",
  mcp__acp__edit: "edit",
  mcp__acp__write: "write",
  todowrite: "todowrite",
  todo_write: "todowrite",
  task_update: "taskupdate",
  task_create: "taskcreate",
  task_list: "tasklist",
  enter_plan_mode: "enterplanmode",
  exit_plan_mode: "exitplanmode",
  web_fetch: "webfetch",
  web_search: "websearch",
  context7_query_docs: "context7_query-docs",
  context7_resolve_library_id: "context7_resolve-library-id",
  agent: "agent",
  // Gemini CLI
  searchtext: "grep",
  search_text: "grep",
  writefile: "write",
  editfile: "edit",
  // Cline
  attempt_completion: "attempt_completion",
  ask_followup_question: "question",
  write_to_file: "write",
  replace_in_file: "edit",
  execute_command: "bash",
  list_files: "glob",
  search_files: "grep",
  list_code_definition_names: "grep",
  browser_action: "webfetch",
  use_mcp_tool: "tool",
  // Codex
  spawn_agent: "agent",
  wait_agent: "task",
  close_agent: "task",
  update_plan: "task",
  create_goal: "create_goal",
  "functions.create_goal": "create_goal",
  update_goal: "update_goal",
  "functions.update_goal": "update_goal",
  request_user_input: "question",
  // codeg multi-agent delegation MCP tools (server prefix varies by host)
  delegate_to_agent: "delegate_to_agent",
  "mcp__codeg-mcp__delegate_to_agent": "delegate_to_agent",
  "mcp__codeg-delegate__delegate_to_agent": "delegate_to_agent",
  mcp__codeg__delegate_to_agent: "delegate_to_agent",
  get_delegation_status: "get_delegation_status",
  cancel_delegation: "cancel_delegation",
  // OpenCode
  delegate_task: "task",
  call_omo_agent: "agent",
  ast_grep_search: "grep",
  ast_grep_replace: "edit",
  background_task: "task",
  background_cancel: "task",
  background_output: "task",
  slashcommand: "skill",
  question: "question",
  ask_user_question: "question",
  askuserquestion: "question",
  lsp_diagnostics: "lsp",
  lsp_document_symbols: "lsp",
  lsp_goto_definition: "lsp",
  lsp_servers: "lsp",
  execute: "bash",
  search: "grep",
  fetch: "webfetch",
  think: "task",
  switch_mode: "switch_mode",
  other: "tool",
}

function canonicalizeToolName(input: string): string {
  return input
    .trim()
    .toLowerCase()
    .replace(/[().]/g, "_")
    .replace(/[\s-]+/g, "_")
}

function inferFromFreeformName(input: string): string | null {
  const normalized = input.trim().toLowerCase()
  if (!normalized) return null

  if (
    /\b(?:shell|bash|exec(?:ute)?)\s*[_-]?(?:command|cmd)?\b/.test(normalized)
  )
    return "bash"
  if (/apply\s*[_-]?patch/.test(normalized)) return "apply_patch"
  if (/^change(?:$|[\s:/_-])/.test(normalized)) return "edit"
  if (/^read(?:$|[\s:/-])/.test(normalized)) return "read"
  if (/^edit(?:$|[\s:/-])/.test(normalized)) return "edit"
  if (/^write(?:$|[\s:/-])/.test(normalized)) return "write"
  if (/^grep(?:\b|[_\s:-])/.test(normalized)) return "grep"
  if (/^glob(?:\b|[_\s:-])/.test(normalized)) return "glob"
  if (/^webfetch(?:\b|[_\s:-])/.test(normalized)) return "webfetch"
  if (/^websearch(?:\b|[_\s:-])/.test(normalized)) return "websearch"
  if (/\bweb[_\s-]?search\b/.test(normalized)) return "websearch"
  if (/\bgrep\b/.test(normalized)) return "grep"
  if (/\bagent\b/.test(normalized)) return "agent"
  if (/\blsp\b/.test(normalized)) return "lsp"
  if (/^todowrite(?:\b|[_\s:-])/.test(normalized)) return "todowrite"
  if (/^taskupdate(?:\b|[_\s:-])/.test(normalized)) return "taskupdate"
  if (/^taskcreate(?:\b|[_\s:-])/.test(normalized)) return "taskcreate"
  if (/^tasklist(?:\b|[_\s:-])/.test(normalized)) return "tasklist"
  if (/^task(?:\b|[_\s:-])/.test(normalized)) return "task"
  if (/\bask\s*(?:user)?\s*question\b/.test(normalized)) return "question"

  return null
}

function extractToolNameFromLiveCallTitle(input: string): string | null {
  const match = input.match(
    /^[:：'"`“”‘’\s]*([a-z0-9_.-]+)(?:\s*[:：])?\s*call[\w-]*['"`“”‘’\s]*$/i
  )
  return match?.[1] ?? null
}

const GOAL_UPDATE_TITLE_RE = /^goal updated\s*\(([^)]+)\)\s*[:：]\s*([\s\S]*)$/i

export interface ParsedGoalUpdateTitle {
  status: string
  objective: string
  toolName: "create_goal" | "update_goal"
}

function normalizeGoalStatus(status: string): string {
  return status
    .trim()
    .toLowerCase()
    .replace(/[\s-]+/g, "_")
}

function goalToolNameFromStatus(status: string): "create_goal" | "update_goal" {
  return normalizeGoalStatus(status) === "active"
    ? "create_goal"
    : "update_goal"
}

export function parseGoalUpdateTitle(
  input: string | null | undefined
): ParsedGoalUpdateTitle | null {
  const match = input?.trim().match(GOAL_UPDATE_TITLE_RE)
  if (!match) return null

  const status = normalizeGoalStatus(match[1] ?? "")
  const objective = (match[2] ?? "").trim()
  if (!status || !objective) return null

  return {
    status,
    objective,
    toolName: goalToolNameFromStatus(status),
  }
}

function tryParseInputObject(rawInput: string | null | undefined) {
  if (!rawInput) return null
  try {
    const parsed = JSON.parse(rawInput)
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return null
    }
    return parsed as Record<string, unknown>
  } catch {
    return null
  }
}

function hasAnyKey(obj: Record<string, unknown>, keys: string[]): boolean {
  return keys.some(
    (key) => key in obj && obj[key] !== null && obj[key] !== undefined
  )
}

function inferFromInput(
  rawInput: string | null | undefined,
  kind: string | null | undefined,
  title: string | null | undefined
): string | null {
  if (!rawInput) return null

  const normalizedKind = normalizeToolName(kind ?? "")
  const normalizedTitle = normalizeToolName(title ?? "")

  if (rawInput.includes("*** Begin Patch")) {
    return "apply_patch"
  }

  const trimmed = rawInput.trim()
  if (
    trimmed.length > 0 &&
    !trimmed.startsWith("{") &&
    !trimmed.startsWith("[") &&
    (normalizedKind === "bash" ||
      normalizedKind === "exec_command" ||
      normalizedKind === "tool" ||
      normalizedTitle === "bash" ||
      normalizedTitle === "exec_command")
  ) {
    return "bash"
  }

  const parsed = tryParseInputObject(rawInput)
  if (!parsed) return null

  if (
    hasAnyKey(parsed, [
      "command",
      "cmd",
      "script",
      "args",
      "argv",
      "command_args",
    ])
  )
    return "bash"
  if (hasAnyKey(parsed, ["old_string", "new_string", "replace_all"]))
    return "edit"
  if (hasAnyKey(parsed, ["changes"])) return "edit"
  if (hasAnyKey(parsed, ["todos"])) return "todowrite"
  if (hasAnyKey(parsed, ["query"])) return "websearch"
  if (hasAnyKey(parsed, ["url"])) return "webfetch"

  const hasPattern = hasAnyKey(parsed, ["pattern"])
  const hasGlob = hasAnyKey(parsed, ["glob"])
  if (hasPattern) return hasGlob ? "glob" : "grep"
  if (hasGlob) return "glob"

  if (hasAnyKey(parsed, ["question"])) return "question"

  if (hasAnyKey(parsed, ["subagent_type"])) {
    return "agent"
  }
  if (hasAnyKey(parsed, ["taskId", "task_id", "subject"])) {
    return "task"
  }

  const hasPath = hasAnyKey(parsed, ["file_path", "notebook_path", "path"])
  if (hasPath) {
    // Check write-specific input keys first — they take priority over
    // kind/title because ACP ToolKind::Edit ("edit") is a category that
    // covers both Edit and Write tools. Without this, a Write tool call
    // (with {content, file_path}) would be classified as "edit" due to
    // its kind, then rendered with EditToolInput which expects
    // old_string/new_string and produces blank output for new files.
    if (
      hasAnyKey(parsed, ["content", "new_source", "cell_type", "edit_mode"])
    ) {
      return "write"
    }
    if (
      normalizedKind === "read" ||
      normalizedKind === "edit" ||
      normalizedKind === "write" ||
      normalizedKind === "delete" ||
      normalizedKind === "move"
    ) {
      return normalizedKind
    }
    if (
      normalizedTitle === "read" ||
      normalizedTitle === "edit" ||
      normalizedTitle === "write"
    ) {
      return normalizedTitle
    }
    return "read"
  }

  return null
}

export function normalizeToolName(toolName: string): string {
  const trimmed = toolName
    .trim()
    .replace(/^[:：'"`“”‘’\s]+/, "")
    .replace(/['"`“”‘’\s]+$/, "")
  if (!trimmed) return "tool"

  const exact = EXACT_TOOL_NAME_ALIASES[trimmed.toLowerCase()]
  if (exact) return exact

  const goalUpdate = parseGoalUpdateTitle(trimmed)
  if (goalUpdate) return goalUpdate.toolName

  const canonical = canonicalizeToolName(trimmed)
  const alias = EXACT_TOOL_NAME_ALIASES[canonical]
  if (alias) return alias

  // Multi-agent delegation MCP tools. Server prefix AND separator both
  // vary by host: Claude Code uses `mcp__<server>__<tool>`, Codex live ACP
  // exposes `<server>/<tool>`, others use `.` or `:`. Match the bare tool
  // name after any non-alphanumeric separator so every form collapses to
  // the same canonical name the renderer dispatches on.
  if (/[^a-z0-9]delegate_to_agent$/.test(canonical)) return "delegate_to_agent"
  if (/[^a-z0-9]get_delegation_status$/.test(canonical))
    return "get_delegation_status"
  if (/[^a-z0-9]cancel_delegation$/.test(canonical)) return "cancel_delegation"
  if (/[^a-z0-9]create_goal$/.test(canonical)) return "create_goal"
  if (/[^a-z0-9]update_goal$/.test(canonical)) return "update_goal"

  const freeform = inferFromFreeformName(trimmed)
  if (freeform) return freeform

  const liveTitleToolName = extractToolNameFromLiveCallTitle(trimmed)
  if (liveTitleToolName) {
    const fromLiveTitle = normalizeToolName(liveTitleToolName)
    if (fromLiveTitle !== "tool") return fromLiveTitle
  }

  return trimmed
}

// Canonical names of the codeg-mcp delegation companion tools. Each has a
// dedicated card renderer, so its identity must win over input-shape
// heuristics during live streaming (see `inferLiveToolName`).
const DELEGATION_COMPANION_TOOLS: ReadonlySet<string> = new Set([
  "delegate_to_agent",
  "get_delegation_status",
  "cancel_delegation",
])

export function inferLiveToolName(params: {
  title?: string | null
  kind?: string | null
  rawInput?: string | null
  meta?: Record<string, unknown> | null
}): string {
  // The backend (e.g. ACP connection layer for OpenCode sub-agent task
  // calls) may set `title="agent"` as an *authoritative* sentinel after
  // running agent-specific detection. This must win over `inferFromInput`'s
  // input-shape heuristics, which otherwise classify sub-agent payloads
  // as "bash" / "edit" / etc. when their input objects happen to carry a
  // `command`/`args`/`changes`/... key alongside the real `subagent_type`
  // marker.
  //
  // Match the sentinel by *literal* equality after trimming/lowercasing —
  // NOT via `normalizeToolName`, whose freeform `\bagent\b` matcher would
  // misclassify any title containing the word "agent" (e.g. "Inspect agent
  // config") as an Agent card before raw_input is even consulted.
  if ((params.title ?? "").trim().toLowerCase() === "agent") return "agent"

  // The codeg-mcp delegation companion tools carry their authoritative identity
  // in `meta.claudeCode.toolName` — claude-agent-acp sets it to the raw
  // `mcp__<server>__<tool>` name for every MCP call. Resolve them FIRST, ahead
  // of `inferFromInput`, so the live stream routes into the same delegation
  // cards the historical path resolves from the raw tool name. Without this,
  // `cancel_delegation` (input `{task_id}`) gets misclassified by
  // `inferFromInput` as the generic "task" tool (shown as "任务" with no detail),
  // and `get_delegation_status` (input `{task_ids}`) falls through unclassified —
  // both need meta to resolve to the canonical companion tool name.
  // Scoped to these three so the documented input-shape-first ordering below
  // (notably Claude Code's `Task` → "agent" via `subagent_type`, whose meta
  // name is "Task" — not a delegation tool) is preserved for everything else.
  const metaToolName = extractClaudeCodeToolName(params.meta)
  if (metaToolName) {
    const normalizedMeta = normalizeToolName(metaToolName)
    if (DELEGATION_COMPANION_TOOLS.has(normalizedMeta)) return normalizedMeta
  }

  // Input-shape detection runs FIRST so cross-agent heuristics (Claude Code
  // `Task` tool routed via `subagent_type`, OpenCode sub-agent calls, etc.)
  // keep priority. The meta-tool-name override below only kicks in when the
  // input shape is silent — i.e. synthesized events with no `rawInput`.
  const byInput = inferFromInput(params.rawInput, params.kind, params.title)
  if (byInput) return byInput

  // Claude-Code override: claude-agent-acp embeds the SDK tool name under
  // `_meta.claudeCode.toolName`. We need it for synthesized events like
  // `memory_recall` (kind="read" + title="Recalled N memories"), where neither
  // the input shape nor the human title carries the real identity. Placed below
  // `inferFromInput` so the more specific subagent_type / patch / command
  // heuristics keep winning when present.
  //
  // Lower-case it so the canonical name matches the rest of this function's
  // returns (all lower-case). The SDK reports the Agent/Task tool as `Agent`
  // (capitalised); before `rawInput` streams in, that is the only signal we
  // have, and the live agent-card nesting check (`getToolName(...) === "agent"`
  // in conversation-runtime-context) is case-sensitive — returning `"Agent"`
  // there left child tool calls un-nested and the card stuck on its fallback
  // title. We deliberately do NOT run `normalizeToolName` here: its live-title
  // heuristic rewrites `memory_recall` to `memory_re`.
  if (metaToolName) return metaToolName.toLowerCase()

  const byTitle = normalizeToolName(params.title ?? "")
  if (byTitle !== "tool") return byTitle

  const byKind = normalizeToolName(params.kind ?? "")
  if (byKind !== "tool") return byKind

  return "tool"
}

function extractClaudeCodeToolName(
  meta: Record<string, unknown> | null | undefined
): string | null {
  if (!meta || typeof meta !== "object") return null
  const cc = (meta as Record<string, unknown>).claudeCode
  if (!cc || typeof cc !== "object") return null
  const tn = (cc as Record<string, unknown>).toolName
  if (typeof tn !== "string") return null
  const trimmed = tn.trim()
  return trimmed.length > 0 ? trimmed : null
}
