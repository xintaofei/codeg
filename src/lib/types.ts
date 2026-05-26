export type AgentType =
  | "claude_code"
  | "codex"
  | "open_code"
  | "gemini"
  | "open_claw"
  | "cline"

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
  /** Optional dotted i18n key used to render a localized message. */
  i18n_key?: string | null
  /** Optional named parameters substituted into the localized template. */
  i18n_params?: Record<string, string> | null
}

export interface RemoteWorkspaceConnection {
  id: number
  name: string
  base_url: string
  token: string
  sort_order: number
  created_at: string
  updated_at: string
}

export interface RemoteWorkspaceConnectionInput {
  name: string
  baseUrl: string
  token: string
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

/**
 * Image payload shared across `ContentBlock::Image` /
 * `ContentBlock::ImageGeneration` / ACP wire `ToolCallImageInfo`. Mirror of
 * Rust `models::message::ImageData`.
 */
export interface ImageData {
  data: string
  mime_type: string
  uri?: string | null
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
      /**
       * codex-acp v0.14+ image generation. Distinct from `image` because
       * codex-acp positions image generation as a first-class
       * `ToolCall(title="Image generation")` carrying revised_prompt + image.
       * Rendered with the dedicated `<GeneratedImagesBlock>` component, not
       * mixed with regular tool-call cards.
       *
       * Singular `image` (not array): codex-acp emits exactly one image per
       * `ToolCall`. Multi-image turns produce N separate ToolCalls. `null`
       * during the in-flight placeholder window between
       * `ImageGenerationBegin` and `ImageGenerationEnd`.
       *
       * `status` mirrors the underlying ToolCallStatus during live streaming
       * so the renderer can distinguish in-flight vs. failed when no image
       * arrives. Absent on Rust-emitted blocks (JSONL replay only emits
       * blocks with a present image, so absence is treated as success).
       */
      type: "image_generation"
      revised_prompt?: string | null
      image?: ImageData | null
      status?: ToolCallStatus | null
    }
  | {
      type: "tool_use"
      tool_use_id: string | null
      tool_name: string
      input_preview: string | null
      /**
       * ACP extensibility metadata for this tool call. Opaque pass-through
       * — both the live snapshot (`ToolCallState.meta`) and the persisted
       * message-row variant carry the same shape. Delegation writes
       * `meta["codeg.delegation"] = { status, child_connection_id,
       * child_conversation_id, error_code? }` here.
       */
      meta?: Record<string, unknown> | null
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
  /** Wall-clock completion time (ISO). Each Rust parser sets this to its
   * own end-marker (e.g. OpenCode's `time.completed`, or just the event-log
   * `timestamp` for agents that log post-generation). Notably this is NOT
   * `timestamp + duration_ms` — those two fields encode unrelated spans in
   * most parsers. */
  completed_at?: string | null
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
  last_opened_at: string
  sort_order: number
  color: string
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
  title: string | null
  agent_type: AgentType
  status: string
  model: string | null
  git_branch: string | null
  external_id: string | null
  message_count: number
  created_at: string
  updated_at: string
  parent_id?: number | null
  parent_tool_use_id?: string | null
  delegation_call_id?: string | null
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
  in_progress: "bg-yellow-400",
  pending_review: "bg-blue-500",
  completed: "bg-green-500",
  cancelled: "bg-red-500",
}

export const AGENT_DISPLAY_ORDER: AgentType[] = [
  "codex",
  "claude_code",
  "open_code",
  "gemini",
  "open_claw",
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
  "open_claw",
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
  open_claw: "OpenClaw",
  cline: "Cline",
}

export const AGENT_COLORS: Record<AgentType, string> = {
  claude_code: "bg-[#D97757]",
  codex: "bg-[#7A9DFF]",
  open_code: "bg-black",
  gemini: "bg-[#3186FF]",
  open_claw: "bg-emerald-600",
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

export interface AgentOptionsSnapshot {
  modes: SessionModeStateInfo | null
  config_options: SessionConfigOptionInfo[]
}

export interface AgentDelegationDefaults {
  mode_id?: string | null
  config_values: Record<string, string>
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

/**
 * Wire-level image attached to a tool call (e.g. codex image generation).
 * Mirrors Rust's `ToolCallImageInfo`. Reused by snapshot endpoints and
 * live `tool_call(_update)` events.
 */
export interface ToolCallImageWire {
  data: string
  mime_type: string
  uri?: string | null
}

// ACP events pushed from Rust backend (discriminated by "type" field)
export type AcpEvent =
  | { type: "content_delta"; text: string }
  | { type: "thinking"; text: string }
  | {
      type: "claude_sdk_message"
      session_id: string
      message: unknown
    }
  | {
      type: "tool_call"
      tool_call_id: string
      title: string
      kind: string
      status: string
      content: string | null
      raw_input: string | null
      raw_output: string | null
      locations?: unknown
      meta?: unknown
      /** Present iff agent attached images (e.g. codex-acp v0.14+ image gen). */
      images?: ToolCallImageWire[]
    }
  | {
      type: "tool_call_update"
      tool_call_id: string
      title: string | null
      status: string | null
      content: string | null
      raw_input: string | null
      raw_output: string | null
      raw_output_append?: boolean
      locations?: unknown
      meta?: unknown
      /**
       * Wire-level partial update: present means "replace prior images with
       * this vec", absent means "preserve prior images". Mirrors the
       * `Option<Vec<...>>` semantics on the Rust side.
       */
      images?: ToolCallImageWire[]
    }
  | {
      type: "permission_request"
      request_id: string
      tool_call: unknown
      options: PermissionOptionInfo[]
    }
  | {
      type: "permission_resolved"
      request_id: string
    }
  | {
      type: "turn_complete"
      session_id: string
      stop_reason: string
    }
  | {
      type: "session_started"
      session_id: string
    }
  | {
      type: "conversation_linked"
      conversation_id: number
      folder_id: number
    }
  | {
      type: "conversation_status_changed"
      conversation_id: number
      status: ConversationStatus
    }
  | {
      type: "session_modes"
      modes: SessionModeStateInfo
    }
  | {
      type: "session_config_options"
      config_options: SessionConfigOptionInfo[]
    }
  | {
      type: "selectors_ready"
    }
  | {
      type: "prompt_capabilities"
      prompt_capabilities: PromptCapabilitiesInfo
    }
  | {
      type: "fork_supported"
      supported: boolean
    }
  | {
      type: "mode_changed"
      mode_id: string
    }
  | {
      type: "plan_update"
      entries: PlanEntryInfo[]
    }
  | {
      type: "status_changed"
      status: ConnectionStatus
    }
  | {
      type: "error"
      message: string
      agent_type: string
      /** Stable backend error identifier for localization (e.g. "initialize_timeout"). */
      code: string | null
    }
  | {
      type: "session_load_failed"
      session_id: string
      message: string
      /** Stable backend identifier — currently `"resource_not_found"`. */
      code: string
    }
  | {
      type: "available_commands"
      commands: AvailableCommandInfo[]
    }
  | {
      type: "usage_update"
      used: number
      size: number
    }
  /**
   * A `delegate_to_agent` MCP tool call from the parent agent has spawned a
   * child sub-session and the child's prompt is in flight. Emitted as soon as
   * the broker registers the pending call. Frontend uses this to build the
   * parent ↔ child mapping for inline ToolCallBlock rendering.
   */
  | {
      type: "delegation_started"
      parent_connection_id: string
      parent_tool_use_id: string
      child_connection_id: string
      child_conversation_id: number
      agent_type: AgentType
    }
  /**
   * The child sub-session has finished (or errored / timed out / been
   * canceled). The MCP tool_result has been delivered to the parent agent;
   * frontend updates the ToolCallBlock badge from "running" to ok/err.
   */
  | {
      type: "delegation_completed"
      parent_connection_id: string
      parent_tool_use_id: string
      child_connection_id: string
      child_conversation_id: number
      result: DelegationResultSummary
    }

/**
 * Mirror of Rust `DelegationResultSummary`. `kind` discriminates Ok vs Err;
 * Ok carries `duration_ms` (broker-measured), Err carries a stable code from
 * the `DelegationError` taxonomy (e.g. `"timeout"`, `"canceled"`).
 */
export type DelegationResultSummary =
  | { kind: "ok"; duration_ms: number }
  | { kind: "err"; error_code: string }

/**
 * Wire envelope for all ACP events. JSON shape is flat via Rust's serde
 * flatten: { seq, connection_id, type, ...variant fields }. Expressed in TS
 * as an intersection that distributes over the AcpEvent discriminated union,
 * so `envelope.type` narrows the variant fields just like on AcpEvent.
 *
 * `seq` is a monotonically-increasing per-connection sequence number. Phase 0
 * always emits 0 (placeholder); Phase 1 wires it to the real counter, after
 * which clients use it as a dedup anchor between snapshot fetches and the
 * live event stream (drop events with seq <= last_event_seq from snapshot).
 *
 * 所有 ACP 事件统一通过此 envelope 发出，详见 spec phase 0/1。
 */
export type EventEnvelope = {
  seq: number
  connection_id: string
} & AcpEvent

// --- LiveSessionSnapshot wire types (mirror src-tauri/src/acp/session_state.rs) ---

export type ToolCallStatus = "pending" | "in_progress" | "completed" | "failed"

export type ToolKind =
  | "read"
  | "edit"
  | "delete"
  | "move"
  | "search"
  | "execute"
  | "think"
  | "fetch"
  | "other"

export type ToolCallOutput =
  | { kind: "text"; content: string }
  | { kind: "error"; message: string }
  | { kind: "json"; value: unknown }

export interface ToolCallState {
  id: string
  kind: ToolKind
  label: string
  status: ToolCallStatus
  input: unknown | null
  output: ToolCallOutput | null
  content: string | null
  /** File locations affected by this tool call. Opaque pass-through. */
  locations: unknown | null
  /** ACP extensibility metadata. Opaque pass-through. */
  meta: Record<string, unknown> | null
  /**
   * Images attached to this tool call (e.g. codex-acp v0.14+ image gen).
   * Persisted on the snapshot so a frontend reconnecting mid-turn / after
   * refresh sees the same image. May be absent on older snapshots.
   */
  images?: ToolCallImageWire[]
}

export type LiveContentBlock =
  | { kind: "text"; text: string }
  | { kind: "thinking"; text: string }
  | { kind: "tool_call_ref"; tool_call_id: string }
  | { kind: "plan"; entries: unknown }

export interface LiveMessage {
  id: string
  role: MessageRole
  content: LiveContentBlock[]
  started_at: string
}

export interface PendingPermissionState {
  request_id: string
  tool_call_id: string
  /**
   * Raw ACP tool_call JSON forwarded from the agent (rawInput / content /
   * locations / patch / plan all preserved). Frontend's
   * `parsePermissionToolCall` consumes this directly to render the approval
   * dialog after a refresh; flattening to a description loses everything
   * except the title.
   */
  tool_call: unknown
  options: PermissionOptionInfo[]
  created_at: string
}

export interface LiveSessionSnapshot {
  connection_id: string
  conversation_id: number | null
  folder_id: number | null
  status: ConnectionStatus
  external_id: string | null
  live_message: LiveMessage | null
  active_tool_calls: ToolCallState[]
  pending_permission: PendingPermissionState | null
  modes: SessionModeStateInfo | null
  current_mode: string | null
  config_options: SessionConfigOptionInfo[] | null
  prompt_capabilities: PromptCapabilitiesInfo | null
  usage: SessionUsageUpdateInfo | null
  fork_supported: boolean
  available_commands: AvailableCommandInfo[]
  selectors_ready: boolean
  event_seq: number
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
  description: string | null
  read_only: boolean
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

export interface SystemTerminalSettings {
  default_shell: string | null
}

export interface TerminalShellOption {
  id: string
  label_key: string
  value: string | null
  exists: boolean
  accepts_custom_path: boolean
}

export interface AvailableTerminalShells {
  options: TerminalShellOption[]
  resolved_shell: string
}

export interface SystemRenderingSettings {
  disable_hardware_acceleration: boolean
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
  | "open_claw"
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

export interface QuickMessage {
  id: number
  title: string
  content: string
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

export interface DirectoryItem {
  name: string
  path: string
  isDir: boolean
  hasChildren: boolean
  size: number | null
}

export interface UploadAttachmentResult {
  path: string
  name: string
  size: number
  mimeType: string | null
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
  agent_type: AgentType
  /**
   * Model value, interpretation depends on agent_type:
   * - claude_code: JSON string of {main, reasoning, haiku, sonnet, opus}
   * - codex / gemini / others: plain model name string
   */
  model: string | null
  created_at: string
  updated_at: string
}

export interface ClaudeProviderModel {
  main?: string
  reasoning?: string
  haiku?: string
  sonnet?: string
  opus?: string
}

export function parseClaudeProviderModel(
  raw: string | null
): ClaudeProviderModel {
  if (!raw) return {}
  try {
    const parsed = JSON.parse(raw)
    if (!parsed || typeof parsed !== "object") return {}
    const out: ClaudeProviderModel = {}
    const keys: (keyof ClaudeProviderModel)[] = [
      "main",
      "reasoning",
      "haiku",
      "sonnet",
      "opus",
    ]
    for (const k of keys) {
      const v = (parsed as Record<string, unknown>)[k]
      if (typeof v === "string" && v.trim()) out[k] = v.trim()
    }
    return out
  } catch {
    return {}
  }
}

export function serializeClaudeProviderModel(
  obj: ClaudeProviderModel
): string | null {
  const cleaned: ClaudeProviderModel = {}
  if (obj.main?.trim()) cleaned.main = obj.main.trim()
  if (obj.reasoning?.trim()) cleaned.reasoning = obj.reasoning.trim()
  if (obj.haiku?.trim()) cleaned.haiku = obj.haiku.trim()
  if (obj.sonnet?.trim()) cleaned.sonnet = obj.sonnet.trim()
  if (obj.opus?.trim()) cleaned.opus = obj.opus.trim()
  return Object.keys(cleaned).length === 0 ? null : JSON.stringify(cleaned)
}
