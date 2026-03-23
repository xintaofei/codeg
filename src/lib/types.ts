export type AgentType =
  | "claude_code"
  | "codex"
  | "open_code"
  | "gemini"
  | "open_claw"

export type AppErrorCode =
  | "invalid_input"
  | "configuration_missing"
  | "configuration_invalid"
  | "not_found"
  | "already_exists"
  | "permission_denied"
  | "dependency_missing"
  | "network_error"
  | "authentication_failed"
  | "database_error"
  | "io_error"
  | "external_command_failed"
  | "window_operation_failed"
  | "task_execution_failed"
  | (string & {})

export interface AppCommandError {
  code: AppErrorCode
  message: string
  detail?: string | null
}

export interface ConversationSummary {
  id: string
  agent_type: AgentType
  folder_path: string | null
  folder_name: string | null
  title: string | null
  started_at: string
  ended_at: string | null
  message_count: number
  model: string | null
  git_branch: string | null
}

export type MessageRole = "user" | "assistant" | "system" | "tool"

export type ContentBlock =
  | { type: "text"; text: string }
  | {
      type: "image"
      data: string
      mime_type: string
      uri?: string | null
    }
  | {
      type: "tool_use"
      tool_use_id: string | null
      tool_name: string
      input_preview: string | null
    }
  | {
      type: "tool_result"
      tool_use_id: string | null
      output_preview: string | null
      is_error: boolean
    }
  | { type: "thinking"; text: string }

export type TurnRole = "user" | "assistant" | "system"

export interface TurnUsage {
  input_tokens: number
  output_tokens: number
  cache_creation_input_tokens: number
  cache_read_input_tokens: number
}

export interface SessionStats {
  total_usage: TurnUsage | null
  total_tokens?: number | null
  total_duration_ms: number
  context_window_used_tokens?: number | null
  context_window_max_tokens?: number | null
  context_window_usage_percent?: number | null
}

export interface MessageTurn {
  id: string
  role: TurnRole
  blocks: ContentBlock[]
  timestamp: string
  usage?: TurnUsage | null
  duration_ms?: number | null
  model?: string | null
}

export interface ConversationDetail {
  summary: ConversationSummary
  turns: MessageTurn[]
  session_stats?: SessionStats | null
}

export interface FolderInfo {
  path: string
  name: string
  agent_types: AgentType[]
  conversation_count: number
}

export interface AgentConversationCount {
  agent_type: AgentType
  conversation_count: number
}

export interface AgentStats {
  total_conversations: number
  total_messages: number
  by_agent: AgentConversationCount[]
}

export interface SidebarData {
  folders: FolderInfo[]
  stats: AgentStats
}

export interface FolderHistoryEntry {
  id: number
  path: string
  name: string
  last_opened_at: string
}

export interface FolderDetail {
  id: number
  name: string
  path: string
  git_branch: string | null
  parent_branch: string | null
  default_agent_type: AgentType | null
  last_opened_at: string
  opened_conversations: OpenedConversation[]
}

export interface OpenedConversation {
  conversation_id: number
  agent_type: AgentType
  position: number
  is_active: boolean
  is_pinned: boolean
}

export interface DbConversationSummary {
  id: number
  folder_id: number
  title: string | null
  agent_type: AgentType
  status: string
  model: string | null
  git_branch: string | null
  external_id: string | null
  message_count: number
  created_at: string
  updated_at: string
}

export interface ImportResult {
  imported: number
  skipped: number
}

export interface DbConversationDetail {
  summary: DbConversationSummary
  turns: MessageTurn[]
  session_stats?: SessionStats | null
}

export type ConversationStatus =
  | "in_progress"
  | "pending_review"
  | "completed"
  | "cancelled"

export const STATUS_ORDER: ConversationStatus[] = [
  "in_progress",
  "pending_review",
  "completed",
  "cancelled",
]

export const STATUS_LABELS: Record<ConversationStatus, string> = {
  in_progress: "In Progress",
  pending_review: "Review",
  completed: "Completed",
  cancelled: "Cancelled",
}

export const STATUS_COLORS: Record<ConversationStatus, string> = {
  in_progress: "bg-blue-500",
  pending_review: "bg-orange-500",
  completed: "bg-green-500",
  cancelled: "bg-red-500",
}

export const AGENT_DISPLAY_ORDER: AgentType[] = [
  "codex",
  "claude_code",
  "open_code",
  "gemini",
  "open_claw",
]

const AGENT_DISPLAY_ORDER_INDEX = new Map(
  AGENT_DISPLAY_ORDER.map((agent, index) => [agent, index])
)

export function compareAgentType(a: AgentType, b: AgentType): number {
  const aIndex = AGENT_DISPLAY_ORDER_INDEX.get(a) ?? Number.MAX_SAFE_INTEGER
  const bIndex = AGENT_DISPLAY_ORDER_INDEX.get(b) ?? Number.MAX_SAFE_INTEGER
  return aIndex - bIndex
}

export const AGENT_LABELS: Record<AgentType, string> = {
  claude_code: "Claude Code",
  codex: "Codex",
  open_code: "OpenCode",
  gemini: "Gemini CLI",
  open_claw: "OpenClaw",
}

export const AGENT_COLORS: Record<AgentType, string> = {
  claude_code: "bg-orange-500",
  codex: "bg-green-500",
  open_code: "bg-blue-500",
  gemini: "bg-blue-400",
  open_claw: "bg-emerald-600",
}

// ACP connection status (matches Rust ConnectionStatus)
export type ConnectionStatus =
  | "connecting"
  | "downloading"
  | "connected"
  | "prompting"
  | "disconnected"
  | "error"

export interface PromptCapabilitiesInfo {
  image: boolean
  audio: boolean
  embedded_context: boolean
}

export type PromptInputBlock =
  | { type: "text"; text: string }
  | {
      type: "image"
      data: string
      mime_type: string
      uri?: string | null
    }
  | {
      type: "resource"
      uri: string
      mime_type?: string | null
      text?: string | null
      blob?: string | null
    }
  | {
      type: "resource_link"
      uri: string
      name: string
      mime_type?: string | null
      description?: string | null
    }

export interface PromptDraft {
  blocks: PromptInputBlock[]
  displayText: string
}

// Permission option info from agent
export interface PermissionOptionInfo {
  option_id: string
  name: string
  kind: string
}

export interface SessionModeInfo {
  id: string
  name: string
  description?: string | null
}

export interface SessionModeStateInfo {
  current_mode_id: string
  available_modes: SessionModeInfo[]
}

export interface SessionConfigSelectOptionInfo {
  value: string
  name: string
  description?: string | null
}

export interface SessionConfigSelectGroupInfo {
  group: string
  name: string
  options: SessionConfigSelectOptionInfo[]
}

export interface SessionConfigSelectInfo {
  current_value: string
  options: SessionConfigSelectOptionInfo[]
  groups: SessionConfigSelectGroupInfo[]
}

export type SessionConfigKindInfo = { type: "select" } & SessionConfigSelectInfo

export interface SessionConfigOptionInfo {
  id: string
  name: string
  description?: string | null
  category?: string | null
  kind: SessionConfigKindInfo
}

export interface PlanEntryInfo {
  content: string
  priority: string
  status: string
}

export interface AvailableCommandInfo {
  name: string
  description: string
  input_hint?: string | null
}

export interface SessionUsageUpdateInfo {
  used: number
  size: number
}

// ACP events pushed from Rust backend (discriminated by "type" field)
export type AcpEvent =
  | { type: "content_delta"; connection_id: string; text: string }
  | { type: "thinking"; connection_id: string; text: string }
  | {
      type: "tool_call"
      connection_id: string
      tool_call_id: string
      title: string
      kind: string
      status: string
      content: string | null
      raw_input: string | null
      raw_output: string | null
    }
  | {
      type: "tool_call_update"
      connection_id: string
      tool_call_id: string
      title: string | null
      status: string | null
      content: string | null
      raw_input: string | null
      raw_output: string | null
      raw_output_append?: boolean
    }
  | {
      type: "permission_request"
      connection_id: string
      request_id: string
      tool_call: unknown
      options: PermissionOptionInfo[]
    }
  | {
      type: "turn_complete"
      connection_id: string
      session_id: string
      stop_reason: string
    }
  | {
      type: "session_started"
      connection_id: string
      session_id: string
    }
  | {
      type: "session_modes"
      connection_id: string
      modes: SessionModeStateInfo
    }
  | {
      type: "session_config_options"
      connection_id: string
      config_options: SessionConfigOptionInfo[]
    }
  | {
      type: "selectors_ready"
      connection_id: string
    }
  | {
      type: "prompt_capabilities"
      connection_id: string
      prompt_capabilities: PromptCapabilitiesInfo
    }
  | {
      type: "fork_supported"
      connection_id: string
      supported: boolean
    }
  | {
      type: "mode_changed"
      connection_id: string
      mode_id: string
    }
  | {
      type: "plan_update"
      connection_id: string
      entries: PlanEntryInfo[]
    }
  | {
      type: "status_changed"
      connection_id: string
      status: ConnectionStatus
    }
  | { type: "error"; connection_id: string; message: string }
  | {
      type: "available_commands"
      connection_id: string
      commands: AvailableCommandInfo[]
    }
  | {
      type: "usage_update"
      connection_id: string
      used: number
      size: number
    }

// Connection info returned by acp_list_connections
export interface ConnectionInfo {
  id: string
  agent_type: AgentType
  status: ConnectionStatus
}

// ACP agent info returned by acp_list_agents
export interface AcpAgentInfo {
  agent_type: AgentType
  registry_id: string
  registry_version: string | null
  name: string
  description: string
  available: boolean
  distribution_type: string
  enabled: boolean
  sort_order: number
  installed_version: string | null
  env: Record<string, string>
  config_json: string | null
  config_file_path: string | null
  opencode_auth_json: string | null
  codex_auth_json: string | null
  codex_config_toml: string | null
}

// Lightweight agent status returned by acp_get_agent_status
export interface AcpAgentStatus {
  agent_type: AgentType
  available: boolean
  enabled: boolean
  installed_version: string | null
}

export type AgentSkillScope = "global" | "project"
export type AgentSkillLayout = "markdown_file" | "skill_directory"

export interface AgentSkillLocation {
  scope: AgentSkillScope
  path: string
  exists: boolean
}

export interface AgentSkillItem {
  id: string
  name: string
  scope: AgentSkillScope
  layout: AgentSkillLayout
  path: string
}

export interface AgentSkillsListResult {
  supported: boolean
  message: string | null
  locations: AgentSkillLocation[]
  skills: AgentSkillItem[]
}

export interface AgentSkillContent {
  skill: AgentSkillItem
  content: string
}

export interface SystemProxySettings {
  enabled: boolean
  proxy_url: string | null
}

export type AppLocale =
  | "en"
  | "zh_cn"
  | "zh_tw"
  | "ja"
  | "ko"
  | "es"
  | "de"
  | "fr"
  | "pt"
  | "ar"
export type LanguageMode = "system" | "manual"

export interface SystemLanguageSettings {
  mode: LanguageMode
  language: AppLocale
}

// --- Version Control ---

export interface GitCredentials {
  username: string
  password: string
}

export interface GitDetectResult {
  installed: boolean
  version: string | null
  path: string | null
}

export interface GitSettings {
  custom_path: string | null
}

export interface GitHubAccount {
  id: string
  server_url: string
  username: string
  scopes: string[]
  avatar_url: string | null
  is_default: boolean
  created_at: string
}

export interface GitHubAccountsSettings {
  accounts: GitHubAccount[]
}

export interface GitHubTokenValidation {
  success: boolean
  username: string | null
  scopes: string[]
  avatar_url: string | null
  message: string | null
}

export type McpAppType = "claude_code" | "codex" | "open_code"

export interface LocalMcpServer {
  id: string
  spec: Record<string, unknown>
  apps: McpAppType[]
}

export interface McpMarketplaceProvider {
  id: string
  name: string
  description: string
}

export interface McpMarketplaceItem {
  provider_id: string
  server_id: string
  name: string
  description: string
  homepage: string | null
  remote: boolean
  verified: boolean
  icon_url: string | null
  latest_version: string | null
  protocols: string[]
  owner: string | null
  namespace: string | null
  downloads: number | null
  score: number | null
  is_deployed: boolean | null
}

export interface McpMarketplaceInstallParameter {
  key: string
  label: string
  description: string | null
  required: boolean
  secret: boolean
  kind: string
  default_value: unknown | null
  placeholder: string | null
  enum_values: string[]
  location: string | null
}

export interface McpMarketplaceInstallOption {
  id: string
  protocol: string
  label: string
  description: string | null
  spec: Record<string, unknown>
  parameters: McpMarketplaceInstallParameter[]
}

export interface McpMarketplaceServerDetail {
  provider_id: string
  server_id: string
  name: string
  description: string
  homepage: string | null
  remote: boolean
  verified: boolean
  icon_url: string | null
  latest_version: string | null
  protocols: string[]
  owner: string | null
  namespace: string | null
  downloads: number | null
  score: number | null
  is_deployed: boolean | null
  default_option_id: string | null
  install_options: McpMarketplaceInstallOption[]
  spec: Record<string, unknown>
}

export interface FolderCommand {
  id: number
  folder_id: number
  name: string
  command: string
  sort_order: number
  created_at: string
  updated_at: string
}

export interface GitStatusEntry {
  status: string
  file: string
}

export interface GitBranchList {
  local: string[]
  remote: string[]
  worktree_branches: string[]
}

export interface GitConflictInfo {
  has_conflicts: boolean
  conflicted_files: string[]
  operation: string
  upstream_commit?: string | null
}

export interface GitPullResult {
  updated_files: number
  conflict?: GitConflictInfo | null
}

export interface GitPushResult {
  pushed_commits: number
  upstream_set: boolean
}

export interface GitMergeResult {
  merged_commits: number
  conflict?: GitConflictInfo | null
}

export interface GitRebaseResult {
  message: string
  conflict?: GitConflictInfo | null
}

export interface GitConflictFileVersions {
  base: string
  ours: string
  theirs: string
  merged: string
}

export interface GitCommitResult {
  committed_files: number
}

export interface GitRemote {
  name: string
  url: string
}

export interface GitStashEntry {
  index: number
  message: string
  branch: string
  date: string
  ref_name: string
}

export type FileTreeNode =
  | { kind: "file"; name: string; path: string }
  | { kind: "dir"; name: string; path: string; children: FileTreeNode[] }

export interface FilePreviewContent {
  path: string
  content: string
}

export interface FileEditContent {
  path: string
  content: string
  etag: string
  mtime_ms: number | null
  readonly: boolean
  line_ending: "lf" | "crlf" | "mixed" | "none"
}

export interface FileSaveResult {
  path: string
  etag: string
  mtime_ms: number | null
  readonly: boolean
  line_ending: "lf" | "crlf" | "mixed" | "none"
}

export interface FileTreeChangedEvent {
  root_path: string
  changed_paths: string[]
  kind: "create" | "modify" | "remove" | "access" | "any" | "other"
  full_reload: boolean
  refresh_git_status: boolean
}

export interface GitLogEntry {
  hash: string
  full_hash: string
  author: string
  date: string
  message: string
  files: GitLogFileChange[]
  pushed: boolean | null
}

export interface GitLogFileChange {
  path: string
  status: string
  additions: number
  deletions: number
}

// Terminal types
export interface TerminalInfo {
  id: string
  title: string
}

export interface TerminalEvent {
  terminal_id: string
  data: string
}

export interface TokenBreakdown {
  input: number
  output: number
  cache_input: number
  cache_output: number
}

export interface DailyTokenStats {
  date: string
  breakdown: TokenBreakdown
  total: number
}

// Preflight check types
export type FixActionKind =
  | "open_url"
  | "redownload_binary"
  | "retry_connection"
  | "open_agents_settings"

export interface FixAction {
  label: string
  kind: FixActionKind
  payload: string
}

export type CheckStatus = "pass" | "fail" | "warn"

export interface CheckItem {
  check_id: string
  label: string
  status: CheckStatus
  message: string
  fixes: FixAction[]
}

export interface PreflightResult {
  agent_type: AgentType
  agent_name: string
  passed: boolean
  checks: CheckItem[]
}

// LSP Server types

export type LspDistributionType =
  | "npm"
  | "binary"
  | "cargo_install"
  | "pip_install"

export interface LspServerInfo {
  id: string
  name: string
  description: string
  language: string
  distribution_type: LspDistributionType
  registry_version: string | null
  enabled: boolean
  sort_order: number
  installed_version: string | null
  config_json: string | null
}

export interface LspServerStatus {
  id: string
  installed: boolean
  installed_version: string | null
  update_available: boolean
}

export interface LspPreflightResult {
  server_id: string
  server_name: string
  passed: boolean
  checks: CheckItem[]
}
