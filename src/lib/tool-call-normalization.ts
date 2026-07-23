import { COLLAB_AGENT_TOOL_NAME, isCodexCollabInput } from "@/lib/collab-tool"

const EXACT_TOOL_NAME_ALIASES: Record<string, string> = {
  shell_command: "bash",
  "functions.shell_command": "bash",
  // Grok Build (xAI) names its terminal tool `run_terminal_command`. History
  // parses the raw name (`parsers/grok.rs`), so without this alias the reload
  // path misses the "bash" classification the live path infers from
  // `rawInput.command` — the command card would fall through to the generic
  // tool renderer (raw ANSI, no terminal title) instead of the Terminal card.
  run_terminal_command: "bash",
  // Cursor's history parser (`parsers/cursor.rs`) emits the CLI's own tool
  // identifiers; `shell` carries `rawInput.command`, so alias it to the
  // Terminal card. Cursor's other tool names (read/edit/grep/glob/ls) already
  // match their canonical kinds verbatim.
  shell: "bash",
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
  // codeg-mcp live-feedback poll (server prefix varies by host; the suffix rule
  // in `normalizeToolName` covers the other separators). Codex persists it under
  // the bare `check_user_feedback` name, dropping the `mcp__codeg_mcp` namespace.
  check_user_feedback: "check_user_feedback",
  "mcp__codeg-mcp__check_user_feedback": "check_user_feedback",
  mcp__codeg__check_user_feedback: "check_user_feedback",
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
  // codeg-mcp ask-user-question companion tool (server prefix varies by host;
  // the suffix rule in `normalizeToolName` covers the other separators)
  "mcp__codeg-mcp__ask_user_question": "question",
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

  // Cursor live MCP calls (`mcpToolCall`): rawInput carries the provider and
  // tool identity. Resolve to `<provider>__<tool>` — the same shape the
  // history parser emits — so MCP-routed tools (the delegation companions
  // et al) reach their dedicated cards, and before the `args` key below
  // misreads the payload as a terminal command.
  const mcpProvider = parsed.providerIdentifier
  const mcpTool = parsed.toolName
  if (
    typeof mcpProvider === "string" &&
    typeof mcpTool === "string" &&
    mcpTool
  ) {
    return normalizeToolName(
      mcpProvider ? `${mcpProvider}__${mcpTool}` : mcpTool
    )
  }

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

  // `question` (singular) covers Cline/Codex follow-up tools; `questions`
  // (plural) is the codeg-mcp `ask_user_question` payload shape, so the live
  // stream resolves to "question" before the tool result arrives.
  if (hasAnyKey(parsed, ["question", "questions"])) return "question"

  // `subagent_type` is the Claude Code Task shape; `subagentType` is Cursor's
  // task tool (a protobuf-es oneof object on the live wire).
  if (hasAnyKey(parsed, ["subagent_type", "subagentType"])) {
    return "agent"
  }
  if (hasAnyKey(parsed, ["taskId", "task_id", "subject"])) {
    return "task"
  }

  // Cursor stamps the semantic tool identity in `_toolName` for calls whose
  // args may not have streamed yet. `task` is Cursor's sub-agent tool — route
  // it to the Agent card even when the input snapshot is otherwise empty (the
  // live tool_call is announced before its args are populated). Other values
  // (`createPlan`, `generateImage`, …) collapse via camelCase → snake_case to
  // the canonical names the history parser emits.
  const hinted = parsed._toolName
  if (typeof hinted === "string" && hinted) {
    if (hinted === "task") return "agent"
    return normalizeToolName(
      hinted.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase()
    )
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

  // codeg-mcp ask-user-question companion tool. Same host-prefix story as the
  // delegation tools above (`mcp__<server>__ask_user_question`,
  // `<server>/ask_user_question`, …) — the bare `ask_user_question` alias only
  // catches the unprefixed form, so collapse every separator here. Note the
  // freeform matcher below intentionally does NOT catch the underscore form.
  if (/[^a-z0-9]ask_user_question$/.test(canonical)) return "question"

  // codeg-mcp live-feedback poll. Same host-prefix story as the delegation tools
  // (`mcp__<server>__check_user_feedback`, `<server>/check_user_feedback`, …) —
  // collapse every separator to the canonical name the renderer dispatches on.
  if (/[^a-z0-9]check_user_feedback$/.test(canonical))
    return "check_user_feedback"

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

  // codex collab / sub-agent activity (codex-acp 1.0.1 #223). The live ACP
  // tool_call's title is the bare, free-form collab op (`spawn_agent`/
  // `wait_agent`/`close_agent`/…), but its rawInput carries inter-agent fields
  // no other tool emits. Detect by that shape so the call routes to the
  // dedicated collab card regardless of the title (and ahead of `inferFromInput`,
  // which returns null for this shape anyway, and the title alias that would
  // otherwise collapse `spawn_agent`→"agent" / `wait_agent`→"task").
  if (isCodexCollabInput(params.rawInput)) return COLLAB_AGENT_TOOL_NAME

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

  // The delegation broker stamps `meta["codeg.delegation"]` onto the parent's
  // `delegate_to_agent` tool call (meta_writer.rs) — an authoritative,
  // codeg-minted marker no other tool ever carries. It is the ONLY live
  // identity signal on hosts whose wire loses the MCP tool name entirely:
  // Cursor announces MCP calls as title "MCP: tool" with empty rawInput and
  // never resends either, so when the broker claims the call and writes the
  // running meta, this is what flips the card to the delegation renderer
  // mid-run (the title-sniff rewrite only lands at completion).
  if (
    params.meta &&
    typeof params.meta === "object" &&
    "codeg.delegation" in params.meta
  ) {
    return "delegate_to_agent"
  }

  // Delegation companion tools also carry their identity in the TITLE on hosts
  // that don't set `claudeCode` meta — notably Grok, whose backend unwraps the
  // `use_tool` envelope so the title becomes the raw `<server>__<tool>` name.
  // Resolve them here, ahead of `inferFromInput`, so `cancel_delegation` (input
  // `{task_id}`) isn't misclassified as the generic "task" tool (and
  // get_delegation_status / delegate_to_agent stay consistent). Scoped to the
  // companion set, so the input-shape-first ordering below is preserved for
  // everything else.
  const titleCompanion = normalizeToolName(params.title ?? "")
  if (DELEGATION_COMPANION_TOOLS.has(titleCompanion)) return titleCompanion

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
