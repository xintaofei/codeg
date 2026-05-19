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
  request_user_input: "question",
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

  const canonical = canonicalizeToolName(trimmed)
  const alias = EXACT_TOOL_NAME_ALIASES[canonical]
  if (alias) return alias

  const freeform = inferFromFreeformName(trimmed)
  if (freeform) return freeform

  const liveTitleToolName = extractToolNameFromLiveCallTitle(trimmed)
  if (liveTitleToolName) {
    const fromLiveTitle = normalizeToolName(liveTitleToolName)
    if (fromLiveTitle !== "tool") return fromLiveTitle
  }

  return trimmed
}

export function inferLiveToolName(params: {
  title?: string | null
  kind?: string | null
  rawInput?: string | null
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

  const byInput = inferFromInput(params.rawInput, params.kind, params.title)
  if (byInput) return byInput

  const byTitle = normalizeToolName(params.title ?? "")
  if (byTitle !== "tool") return byTitle

  const byKind = normalizeToolName(params.kind ?? "")
  if (byKind !== "tool") return byKind

  return "tool"
}
