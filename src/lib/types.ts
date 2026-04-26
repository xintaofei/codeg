export type AgentType =
  | "claude_code"
  | "codex"
  | "open_code"
  | "gemini"
  | "generic"
  | "cline"

export type SquadRoleKind = "conductor" | "frontend" | "backend" | "worker"

export type SquadRunMode = "manual" | "conductor_dispatch" | "all_hands_review"

export type SquadRunStatus =
  | "pending"
  | "preparing"
  | "running"
  | "blocked"
  | "completed"
  | "failed"
  | "cancelled"

export type SquadRoleRunStatus =
  | "pending"
  | "preparing"
  | "connecting"
  | "connected"
  | "prompting"
  | "stopped"
  | "completed"
  | "failed"
  | "cancelled"

export type SquadTaskStatus =
  | "pending"
  | "assigned"
  | "running"
  | "blocked"
  | "completed"
  | "failed"
  | "cancelled"

export type SquadArtifactType =
  | "summary"
  | "diff"
  | "file_patch"
  | "review"
  | "terminal"
  | "plan"
  | "warning"
  | "log"

export type SquadWorkspacePolicy =
  | "read_only"
  | "write_isolated"
  | "write_shared"

export interface SquadRoleProfileInfo {
  id: number
  folderId: number
  roleKind: SquadRoleKind
  enabled: boolean
  agentType: AgentType
  registryId: string
  modelProviderId: number | null
  modelId: string | null
  envJson: string | null
  systemPrompt: string
  workspacePolicy: SquadWorkspacePolicy
  defaultRunMode: SquadRunMode
  modeId: string | null
  configOptionsJson: string | null
  createdAt: string
  updatedAt: string
}

export interface SquadRoleProfilePatch {
  enabled?: boolean
  agentType?: AgentType
  registryId?: string
  modelProviderId?: number | null
  modelId?: string | null
  envJson?: string | null
  systemPrompt?: string
  workspacePolicy?: SquadWorkspacePolicy
  defaultRunMode?: SquadRunMode
  modeId?: string | null
  configOptionsJson?: string | null
}

export interface SquadRunInfo {
  id: number
  folderId: number
  originConversationId: number | null
  mode: SquadRunMode
  status: SquadRunStatus
  goalSummary: string
  baseBranch: string | null
  isolationMode: string
  startedWithDirtyBase: boolean
  createdAt: string
  updatedAt: string
  startedAt: string | null
  completedAt: string | null
  cancelledAt: string | null
  errorMessage: string | null
}

export interface SquadRoleRunInfo {
  id: number
  squadRunId: number
  roleKind: SquadRoleKind
  roleProfileSnapshotJson: string
  connectionId: string | null
  sessionId: string | null
  conversationId: number | null
  workspacePath: string | null
  branchName: string | null
  status: SquadRoleRunStatus
  lastEventAt: string | null
  budgetStateJson: string | null
  errorMessage: string | null
  createdAt: string
  updatedAt: string
}

export interface SquadTaskInfo {
  id: number
  squadRunId: number
  assignedRoleKind: SquadRoleKind
  title: string
  description: string
  inputSummary: string | null
  status: SquadTaskStatus
  dependsOnJson: string | null
  priority: number
  createdAt: string
  updatedAt: string
  completedAt: string | null
  errorMessage: string | null
}

export interface SquadArtifactInfo {
  id: number
  squadRunId: number
  squadRoleRunId: number | null
  taskId: number | null
  roleKind: SquadRoleKind | null
  artifactType: SquadArtifactType
  title: string
  contentJson: string
  createdAt: string
}

export interface SquadRunSnapshot {
  run: SquadRunInfo
  roles: SquadRoleRunInfo[]
  tasks: SquadTaskInfo[]
  artifacts: SquadArtifactInfo[]
}

export type SquadEventType =
  | "squad_run_status_changed"
  | "squad_role_status_changed"
  | "squad_role_connection_attached"
  | "squad_task_created"
  | "squad_task_status_changed"
  | "squad_artifact_created"
  | "squad_conductor_plan_applied"
  | "squad_dispatch_round_completed"
  | (string & {})

export interface SquadEvent {
  type: SquadEventType
  squadRunId: number
  seq: number
  at: string
  roleKind?: SquadRoleKind | null
  payload?: unknown
}

export interface SquadConductorPlanAppliedPayload {
  createdCount: number
  skippedCount: number
  skippedReasons: string[]
}

export interface SquadDispatchRoundCompletedPayload {
  considered: number
  dispatched: Array<{
    taskId: number
    roleKind: SquadRoleKind
    outcome: string
    note: string | null
  }>
}

export type AppErrorCode =
  | "invalid_input"
  | "configuration_missing"
  | "configuration_invalid"
  | "not_found"
  | "not_a_git_repository"
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

export interface AgentToolCall {
  tool_name: string
  input_preview?: string | null
  output_preview?: string | null
  is_error: boolean
}

export interface AgentExecutionStats {
  agent_type?: string | null
  status?: string | null
  total_duration_ms?: number | null
  total_tokens?: number | null
  total_tool_use_count?: number | null
  read_count?: number | null
  search_count?: number | null
  bash_count?: number | null
  edit_file_count?: number | null
  lines_added?: number | null
  lines_removed?: number | null
  other_tool_count?: number | null
  tool_calls?: AgentToolCall[]
}

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
      agent_stats?: AgentExecutionStats | null
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
  default_agent_type: AgentType | null
  workspace_preset: WorkspacePreset | null
  last_opened_at: string
  sort_order: number
}

export interface WorkspacePreset {
  default_agent_type: AgentType | null
  model_provider_id: number | null
  approval_policy: string | null
  skill_ids: string[]
  mcp_server_ids: string[]
  env_overrides: Record<string, string>
}

export interface OpenedTab {
  id: number
  folder_id: number
  conversation_id: number | null
  agent_type: AgentType
  position: number
  is_active: boolean
  is_pinned: boolean
}

export interface DbConversationSummary {
  id: number
  folder_id: number
  folder_name: string | null
  folder_path: string | null
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

export const STATUS_ICON_COLORS: Record<ConversationStatus, string> = {
  in_progress: "text-blue-500",
  pending_review: "text-orange-500",
  completed: "text-green-500",
  cancelled: "text-red-500",
}

export const AGENT_DISPLAY_ORDER: AgentType[] = [
  "codex",
  "claude_code",
  "open_code",
  "gemini",
  "generic",
  "cline",
]

const AGENT_DISPLAY_ORDER_INDEX = new Map(
  AGENT_DISPLAY_ORDER.map((agent, index) => [agent, index])
)

export function compareAgentType(a: AgentType, b: AgentType): number {
  const aIndex = AGENT_DISPLAY_ORDER_INDEX.get(a) ?? Number.MAX_SAFE_INTEGER
  const bIndex = AGENT_DISPLAY_ORDER_INDEX.get(b) ?? Number.MAX_SAFE_INTEGER
  return aIndex - bIndex
}

export const ALL_AGENT_TYPES: AgentType[] = [
  "claude_code",
  "codex",
  "open_code",
  "gemini",
  "generic",
  "cline",
]

export const MODEL_PROVIDER_AGENT_TYPES: AgentType[] = [
  "claude_code",
  "codex",
  "gemini",
]

export const AGENT_LABELS: Record<AgentType, string> = {
  claude_code: "Claude Code",
  codex: "Codex",
  open_code: "OpenCode",
  gemini: "Gemini CLI",
  generic: "Generic",
  cline: "Cline",
}

export const AGENT_COLORS: Record<AgentType, string> = {
  claude_code: "bg-[#D97757]",
  codex: "bg-[#7A9DFF]",
  open_code: "bg-black",
  gemini: "bg-[#3186FF]",
  generic: "bg-slate-600",
  cline: "bg-purple-500",
}

// ACP connection status (matches Rust ConnectionStatus)
export type ConnectionStatus =
  | "connecting"
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
      type: "claude_sdk_message"
      connection_id: string
      session_id: string
      message: unknown
    }
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
      locations?: unknown
      meta?: unknown
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
      locations?: unknown
      meta?: unknown
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
  | {
      type: "error"
      connection_id: string
      message: string
      agent_type: string
      /** Stable backend error identifier for localization (e.g. "initialize_timeout"). */
      code: string | null
    }
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
  owner_window_label: string
  working_dir: string | null
  created_at: string
  updated_at: string
  session_id: string | null
  last_error: string | null
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
  cline_secrets_json: string | null
  model_provider_id: number | null
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

/**
 * Built-in expert skills, sourced from obra/superpowers and bundled into
 * the codeg binary. Experts live in a central store at `~/.codeg/skills/`
 * and are linked into agent skill directories on demand.
 */
export interface ExpertMetadata {
  id: string
  category: string
  icon: string | null
  sort_order: number
  display_name: Record<string, string>
  description: Record<string, string>
  bundled_hash: string
}

export interface ExpertListItem {
  metadata: ExpertMetadata
  installed_centrally: boolean
  user_modified: boolean
  central_path: string
}

export type ExpertLinkState =
  | "not_linked"
  | "linked_to_codeg"
  | "linked_elsewhere"
  | "blocked_by_real_directory"
  | "broken"

export interface ExpertInstallStatus {
  expertId: string
  agentType: AgentType
  state: ExpertLinkState
  linkPath: string
  targetPath: string | null
  expectedTargetPath: string
  copyMode: boolean
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

export interface BackendBuildInfo {
  version: string
  git_commit: string | null
  built_at: string | null
  profile: string
}

export interface FrontendBuildManifest {
  version: string
  git_commit: string | null
  built_at: string | null
}

export interface BuildConsistencyInfo {
  status: string
  message: string
  backend: BackendBuildInfo
  frontend: FrontendBuildManifest | null
  manifest_path: string
}

export interface RuntimeSecurityInfo {
  mode: string
  host: string | null
  auth_enabled: boolean
  allow_remote_access: boolean
  insecure: boolean
  override_active: boolean
  static_dir: string | null
  data_dir: string | null
}

export interface RuntimeLogEntry {
  timestamp: string
  level: string
  scope: string
  message: string
  data?: unknown
}

export interface ConnectionLimitSnapshot {
  current_total: number
  global_limit: number
  per_owner_limit: number
  per_agent_limit: number
  by_agent: Record<string, number>
  recent_failures: number
  breaker_threshold: number
  breaker_window_seconds: number
  breaker_cooldown_seconds: number
  breaker_open: boolean
  breaker_open_until: string | null
  orphan_grace_seconds: number
  connecting_timeout_seconds: number
}

export interface RuntimeDiagnostics {
  build: BuildConsistencyInfo | null
  security: RuntimeSecurityInfo | null
  connections: ConnectionInfo[]
  terminals: TerminalInfo[]
  web_clients: WebClientInfo[]
  recent_logs: RuntimeLogEntry[]
  connection_limits: ConnectionLimitSnapshot
}

export interface WebClientInfo {
  client_id: string
  active_sockets: number
  connected_at: string
  updated_at: string
}

export interface AcpCacheEntry {
  agent_type: AgentType
  label: string
  path: string
  size_bytes: number
  file_count: number
  versions: string[]
  installed_version: string | null
  recommended_version: string | null
}

export interface AcpCacheInventory {
  generated_at: string
  root_path: string
  total_size_bytes: number
  entries: AcpCacheEntry[]
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

export interface PackageManagerInfo {
  name: string
  installed: boolean
  version: string | null
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

export type McpAppType =
  | "claude_code"
  | "codex"
  | "gemini"
  | "generic"
  | "open_code"
  | "cline"

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

export type GitResetMode = "soft" | "mixed" | "hard" | "keep"

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

export interface GitPushInfo {
  branch: string
  remotes: GitRemote[]
  tracking_remote: string | null
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

export interface DirectoryEntry {
  name: string
  path: string
  hasChildren: boolean
}

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

export interface WorkspaceGitEntry {
  path: string
  status: string
  additions: number
  deletions: number
}

export type WorkspaceDelta =
  | { kind: "tree_replace"; nodes: FileTreeNode[] }
  | { kind: "git_replace"; entries: WorkspaceGitEntry[] }
  | { kind: "meta"; reason: string }

export interface WorkspaceDeltaEnvelope {
  seq: number
  kind: "fs_delta" | "git_delta" | "meta" | "resync_hint" | string
  payload: WorkspaceDelta[]
  requires_resync: boolean
  changed_paths?: string[]
}

export interface WorkspaceStateEvent {
  root_path: string
  seq: number
  version: number
  kind: "fs_delta" | "git_delta" | "meta" | "resync_hint" | string
  payload: WorkspaceDelta[]
  requires_resync: boolean
  changed_paths?: string[]
}

export interface WorkspaceSnapshotResponse {
  root_path: string
  seq: number
  version: number
  full: boolean
  tree_snapshot: FileTreeNode[] | null
  git_snapshot: WorkspaceGitEntry[] | null
  deltas: WorkspaceDeltaEnvelope[]
  degraded: boolean
  is_git_repo: boolean
}

export interface GitLogResult {
  entries: GitLogEntry[]
  has_upstream: boolean
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
  working_dir: string
  owner_window_label: string
  created_at: string
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
  | "install_opencode_plugins"

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

// ─── OpenCode Plugins ───

export type PluginStatus = "installed" | "missing"

export interface PluginInfo {
  name: string
  declared_spec: string
  installed_version: string | null
  status: PluginStatus
}

export interface PluginCheckSummary {
  config_path: string
  cache_dir: string
  plugins: PluginInfo[]
  has_project_config_hint: boolean
}

export type PluginInstallEventKind = "started" | "log" | "completed" | "failed"

export interface PluginInstallEvent {
  task_id: string
  kind: PluginInstallEventKind
  payload: string
}

export type AgentInstallEventKind = "started" | "log" | "completed" | "failed"

export interface AgentInstallEvent {
  task_id: string
  kind: AgentInstallEventKind
  payload: string
}

// ─── Chat Channels ───

export type ChannelType = "lark" | "telegram" | "weixin"

export type ChannelConnectionStatus =
  | "connected"
  | "connecting"
  | "disconnected"
  | "error"

export interface ChatChannelInfo {
  id: number
  name: string
  channel_type: ChannelType
  enabled: boolean
  config_json: string
  event_filter_json: string | null
  daily_report_enabled: boolean
  daily_report_time: string | null
  created_at: string
  updated_at: string
}

export interface ChannelStatusInfo {
  channel_id: number
  name: string
  channel_type: ChannelType
  status: ChannelConnectionStatus
}

export interface ChatChannelMessageLog {
  id: number
  channel_id: number
  direction: "outbound" | "inbound"
  message_type: string
  content_preview: string
  status: "sent" | "failed"
  error_detail: string | null
  created_at: string
}

export interface ModelProviderInfo {
  id: number
  name: string
  api_url: string
  api_key: string
  api_key_masked: string
  agent_types: AgentType[]
  created_at: string
  updated_at: string
}
