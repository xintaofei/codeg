export type AgentType =
  | "claude_code"
  | "codex"
  | "open_code"
  | "gemini"
  | "open_claw"
  | "cline"
  | "hermes"
  | "code_buddy"
  | "kimi_code"
  | "pi"
  | "grok"
  | "cursor"

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
      /**
       * Images returned in a tool result (e.g. Claude Code's `Read` of a
       * PNG/JPEG, or a multi-page PDF read returning one image per page).
       * Mirror of Rust `ContentBlock::ToolResult.images`. The adapter renders
       * these in-position as `generated-image` cards so the historical (JSONL
       * replay) path matches the live ACP stream — which surfaces the same
       * bytes via `ToolCallInfo.images` and an `image_generation` block.
       * Absent/empty for the common text-only tool result.
       */
      images?: ImageData[] | null
    }
  | { type: "thinking"; text: string }
  /**
   * Frontend-only, LIVE-stream synthetic block. It is NEVER persisted and
   * NEVER emitted by the Rust JSONL parsers — the persisted plan path is a
   * `TodoWrite` tool_use block. It exists purely so a live plan can survive
   * `buildStreamingTurnsFromLiveMessage` → `adaptContentBlock` without being
   * down-converted into a `thinking`/reasoning block. Mirrors the reducer's
   * `LiveContentBlock` plan variant in `acp-connections-context.tsx` (NOT the
   * `kind`-tagged snapshot type lower in this file). Because it is live-only,
   * persistence/export switches over `ContentBlock` never receive it.
   */
  | { type: "plan"; entries: PlanEntryInfo[] }

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
  /**
   * Byte length of the source transcript this parse consumed (Claude only;
   * absent elsewhere). Retires background-overlay turns whose
   * `background_activity` watermark it has caught up to — see
   * `BackgroundOverlayEntry` in the conversation runtime store.
   */
  transcript_watermark?: number | null
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
  /**
   * Root folder this one was created under (worktree folders only); null for
   * top-level folders. Flattened — a worktree of a worktree still points at the
   * original root. Drives the sidebar merge and worktree-branch detection.
   */
  parent_id: number | null
  /**
   * Folder classification. `chat` folders back folderless chat-mode
   * conversations: kept in `allFolders` so cwd / active-folder resolve, but
   * hidden from user-facing folder lists; their conversations route to the
   * sidebar "Chat" group and folder-bound chrome is hidden while one is active.
   */
  kind: FolderKind
  /**
   * User-supplied display alias, or null when unset. When present, the sidebar
   * folder header and conversation header render `alias [name]`
   * (see `formatFolderLabelWithAlias`). Display-only — never used for the
   * folder's real `path`/`id`.
   */
  alias: string | null
}

/**
 * Result of `createChatConversation`: the new conversation id plus the hidden
 * chat folder backing it, so the caller can drop the folder straight into
 * `allFolders` (resolving cwd / active-folder) without a refetch.
 */
export interface CreateChatConversationResult {
  conversationId: number
  folderId: number
  folder: FolderDetail
}

/**
 * Result of `createChatDir`: a freshly created chat-mode scratch directory
 * (filesystem only — no DB rows). Used to connect ACP at a real cwd the instant
 * "no-folder mode" is selected; the conversation is still created lazily on the
 * first send, reusing this path.
 */
export interface CreateChatDirResult {
  path: string
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
  /** True once the user renamed this conversation by hand; the backend then
   *  stops auto-deriving its title from the session file. */
  title_locked: boolean
  agent_type: AgentType
  status: string
  /** Mirrors `conversation.kind` — drives sidebar visibility and grouping. */
  kind: ConversationKind
  model: string | null
  git_branch: string | null
  external_id: string | null
  message_count: number
  /** Number of direct, non-deleted delegation children (computed by the backend
   *  `fill_child_counts` aggregate). `child_count > 0` means this conversation is
   *  expandable into its sub-session subtree — drives the sidebar chevron. */
  child_count: number
  created_at: string
  updated_at: string
  /** When the user pinned this conversation (ISO string), or null if not pinned.
   *  Drives the sidebar's "Pinned" section (sorted by this descending); a pinned
   *  conversation is shown there instead of in its folder group. */
  pinned_at: string | null
  parent_id?: number | null
  parent_tool_use_id?: string | null
  delegation_call_id?: string | null
}

/** Payload for the global `conversation://changed` side-channel that keeps
 *  every client's sidebar list/status in sync across desktop + browsers.
 *  Mirrors the Rust `ConversationChange` enum (serde `tag = "kind"`). */
export type ConversationChange =
  | { kind: "upsert"; summary: DbConversationSummary }
  | { kind: "deleted"; id: number }
  | { kind: "status"; id: number; status: string }

export const CONVERSATION_CHANGED_EVENT = "conversation://changed"

/** Payload for the global `folder://changed` side-channel. A folder created or
 *  updated headlessly — e.g. the automation engine minting a per-run worktree —
 *  reaches every client's workspace list so a conversation produced inside it can
 *  be grouped/rendered in the sidebar. Mirrors the Rust `FolderChange` enum
 *  (serde `tag = "kind"`). Distinct from `folder://open-in-workspace`, whose
 *  listener also opens + focuses a tab. */
export type FolderChange = { kind: "upsert"; folder: FolderDetail }

export const FOLDER_CHANGED_EVENT = "folder://changed"

/** Global side-channel announcing a live-feedback enable/disable (payload is
 *  `FeedbackSettings`). The settings UI runs in a separate window, so the
 *  conversation feedback bar converges on this backend broadcast rather than a
 *  frontend-only cache. Mirrors the Rust `FEEDBACK_SETTINGS_CHANGED_EVENT`. */
export const FEEDBACK_SETTINGS_CHANGED_EVENT = "feedback-settings://changed"

/** Payload for the global `tabs://changed` side-channel that keeps every
 *  client's open-tab set in sync across desktop + browsers. Mirrors the Rust
 *  `TabsChanged` struct. The full conversation-bound tab set is sent as a
 *  snapshot (idempotent apply); `is_active` marks the focused tab, which is
 *  mirrored across clients. `origin` is echoed so the originator ignores its
 *  own broadcast; the sentinel `"server"` marks cascade changes every client
 *  applies. */
export interface TabsChanged {
  version: number
  origin: string
  tabs: OpenedTab[]
}

export const TABS_CHANGED_EVENT = "tabs://changed"

/** Response of `list_opened_tabs`: the persisted set + current workspace tab
 *  version (clients seed their compare-and-set / echo logic from it). */
export interface OpenedTabsSnapshot {
  items: OpenedTab[]
  version: number
}

/** Response of the `save_opened_tabs` compare-and-set. When `accepted` is false
 *  the save was stale (another client won) and `tabs` is the current truth to
 *  reconcile against. */
export interface SaveTabsOutcome {
  accepted: boolean
  version: number
  tabs: OpenedTab[]
}

export interface ImportResult {
  imported: number
  updated: number
  skipped: number
}

export interface DbConversationDetail {
  summary: DbConversationSummary
  turns: MessageTurn[]
  session_stats?: SessionStats | null
  /** See `ConversationDetail.transcript_watermark` (threaded through the DB
   *  fetch path from the same parser detail). */
  transcript_watermark?: number | null
  /**
   * Id of the persisted user turn the backend identified as the in-flight prompt
   * (present only while a turn is running on this conversation's connection). The
   * timeline uses it to locate — and, while the live reply is in hand, hide — the
   * partial assistant turn some agents (OpenCode, Gemini) persist after the prompt
   * mid-stream, which would otherwise double-render against the live reply.
   */
  in_flight_user_turn_id?: string | null
}

export type ConversationStatus =
  | "in_progress"
  | "pending_review"
  | "completed"
  | "cancelled"

/** Mirrors Rust `ConversationKind` (src-tauri/src/db/entities/conversation.rs).
 *  `loop` rows belong to the Loop Engineering workbench and never appear in
 *  the sidebar list; `delegate` rows nest under their parent's tool-call view. */
export type ConversationKind = "regular" | "chat" | "loop" | "delegate"

/** Mirrors Rust `FolderKind` (src-tauri/src/db/entities/folder.rs).
 *  `loop_worktree` is reserved for M2+ — add it here when the variant lands. */
export type FolderKind = "regular" | "chat"

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
  "hermes",
  "code_buddy",
  "kimi_code",
  "pi",
  "grok",
  "cursor",
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
  "hermes",
  "code_buddy",
  "kimi_code",
  "pi",
  "grok",
  "cursor",
]

export const MODEL_PROVIDER_AGENT_TYPES: AgentType[] = [
  "claude_code",
  "codex",
  "gemini",
]

/**
 * How a Hermes provider's credentials are supplied:
 * - `apiKey`: codeg writes the key to `~/.hermes/.env`.
 * - `oauth`: set through the terminal `--setup` flow (no API-key field).
 * - `aws`: resolved from the AWS SDK credential chain (no API-key field).
 */
export type HermesProviderKind = "apiKey" | "oauth" | "aws"

/**
 * Curated Hermes providers the settings panel edits via structured fields.
 * Mirrors the backend `HERMES_PROVIDERS` table (commands/acp.rs), whose ids and
 * `.env` key vars come from Hermes' own `hermes_cli/auth.py` PROVIDER_REGISTRY.
 * The provider choice drives the linkage between the API key (~/.hermes/.env)
 * and the general config (~/.hermes/config.yaml `model.provider`/`base_url`).
 */
export interface HermesProviderOption {
  /** Canonical `model.provider` id written to config.yaml. */
  id: string
  /** Brand display name shown in the provider dropdown (not localized). */
  label: string
  /** Whether the provider takes a user-supplied base URL (OpenAI-compatible). */
  needsBaseUrl: boolean
  kind: HermesProviderKind
}

export const HERMES_PROVIDERS: HermesProviderOption[] = [
  // API-key providers — codeg writes the key var to ~/.hermes/.env.
  {
    id: "openrouter",
    label: "OpenRouter",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "openai-api",
    label: "OpenAI / Compatible",
    needsBaseUrl: true,
    kind: "apiKey",
  },
  // Hermes' built-in `custom` provider: a user-supplied OpenAI-compatible
  // endpoint. Unlike `openai-api` (key in ~/.hermes/.env), `custom` stores its
  // key + endpoint INLINE in config.yaml (`model.api_key`/`model.base_url`);
  // the backend routes them there. Shows both API Key + API URL fields.
  {
    id: "custom",
    label: "Custom (OpenAI-compatible)",
    needsBaseUrl: true,
    kind: "apiKey",
  },
  {
    id: "anthropic",
    label: "Anthropic",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "gemini",
    label: "Google AI Studio",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "deepseek",
    label: "DeepSeek",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "xai",
    label: "xAI Grok",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "zai",
    label: "Z.AI / GLM",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "minimax",
    label: "MiniMax",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "minimax-cn",
    label: "MiniMax (China)",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "kimi-coding",
    label: "Kimi / Moonshot",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "kimi-coding-cn",
    label: "Kimi / Moonshot (China)",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "nvidia",
    label: "NVIDIA NIM",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "alibaba",
    label: "Qwen (DashScope)",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "alibaba-coding-plan",
    label: "Alibaba Coding Plan",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "copilot",
    label: "GitHub Copilot",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "lmstudio",
    label: "LM Studio",
    needsBaseUrl: true,
    kind: "apiKey",
  },
  {
    id: "azure-foundry",
    label: "Azure Foundry",
    needsBaseUrl: true,
    kind: "apiKey",
  },
  {
    id: "stepfun",
    label: "StepFun",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "arcee",
    label: "Arcee AI",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "gmi",
    label: "GMI Cloud",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "huggingface",
    label: "Hugging Face",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "kilocode",
    label: "Kilo Code",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "opencode-zen",
    label: "OpenCode Zen",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "opencode-go",
    label: "OpenCode Go",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "xiaomi",
    label: "Xiaomi MiMo",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "tencent-tokenhub",
    label: "Tencent TokenHub",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "ollama-cloud",
    label: "Ollama Cloud",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  {
    id: "novita",
    label: "Novita AI",
    needsBaseUrl: false,
    kind: "apiKey",
  },
  // OAuth / external providers — credentials set via the terminal `--setup` flow.
  {
    id: "nous",
    label: "Nous Portal",
    needsBaseUrl: false,
    kind: "oauth",
  },
  {
    id: "openai-codex",
    label: "OpenAI Codex",
    needsBaseUrl: false,
    kind: "oauth",
  },
  {
    id: "minimax-oauth",
    label: "MiniMax",
    needsBaseUrl: false,
    kind: "oauth",
  },
  {
    id: "xai-oauth",
    label: "xAI Grok",
    needsBaseUrl: false,
    kind: "oauth",
  },
  {
    id: "qwen-oauth",
    label: "Qwen",
    needsBaseUrl: false,
    kind: "oauth",
  },
  {
    id: "google-gemini-cli",
    label: "Gemini CLI",
    needsBaseUrl: false,
    kind: "oauth",
  },
  {
    id: "copilot-acp",
    label: "GitHub Copilot ACP",
    needsBaseUrl: false,
    kind: "oauth",
  },
  // AWS Bedrock — credentials from the AWS SDK chain.
  {
    id: "bedrock",
    label: "AWS Bedrock",
    needsBaseUrl: false,
    kind: "aws",
  },
]

/**
 * Normalized Hermes config projection returned in `AcpAgentInfo.config_json`
 * for `agent_type === "hermes"` (parsed from ~/.hermes/.env + config.yaml).
 */
export interface HermesLocalConfig {
  provider?: string
  model?: string
  baseUrl?: string
  apiKey?: string
  hermesHome?: string
  setupCommand?: string
  modelCommand?: string
}

export const AGENT_LABELS: Record<AgentType, string> = {
  claude_code: "Claude Code",
  codex: "Codex",
  open_code: "OpenCode",
  gemini: "Gemini CLI",
  open_claw: "OpenClaw",
  cline: "Cline",
  hermes: "Hermes Agent",
  code_buddy: "CodeBuddy",
  kimi_code: "Kimi Code",
  pi: "Pi",
  grok: "Grok",
  cursor: "Cursor",
}

export const AGENT_COLORS: Record<AgentType, string> = {
  claude_code: "bg-[#D97757]",
  codex: "bg-[#7A9DFF]",
  open_code: "bg-black",
  gemini: "bg-[#3186FF]",
  open_claw: "bg-emerald-600",
  cline: "bg-purple-500",
  hermes: "bg-amber-500",
  code_buddy: "bg-[#0052D9]",
  kimi_code: "bg-[#1783FF]",
  pi: "bg-[#0D9488]",
  grok: "bg-neutral-900",
  cursor: "bg-zinc-800",
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

// --- ask_user_question (mirror of Rust `crate::acp::question`) ---

/** One selectable choice in an `ask_user_question` (mirror of `QuestionOption`). */
export interface QuestionOption {
  label: string
  description: string
}

/** A single multiple-choice question (mirror of Rust `QuestionSpec`). `id` is
 *  the backend-minted correlation key the answer is submitted against. */
export interface QuestionSpec {
  id: string
  question: string
  header: string
  multi_select: boolean
  options: QuestionOption[]
}

/** Awaiting-answer question set on the session (mirror of `PendingQuestionState`). */
export interface PendingQuestionState {
  question_id: string
  questions: QuestionSpec[]
  created_at: string
}

/** One question's answer submitted to `acp_answer_question`. `labels` carries
 *  the selected option labels plus any free-text "Other" the user typed. */
export interface QuestionAnswerItem {
  questionId: string
  labels: string[]
}

/** The full submission to `acp_answer_question`. `declined` is set when the
 *  user dismissed the card without choosing. */
export interface QuestionAnswer {
  answers: QuestionAnswerItem[]
  declined: boolean
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
  /** Slash commands captured during the same transient probe as modes/config
   *  (empty when the agent advertises none in the probe window). */
  available_commands: AvailableCommandInfo[]
}

export interface AgentDelegationDefaults {
  mode_id?: string | null
  config_values: Record<string, string>
}

// ─── Automations ───────────────────────────────────────────────────────────
// Mirrors src-tauri/src/models/automation.rs. Wire form is snake_case (serde
// default), matching AgentDelegationDefaults.

export type AutomationTriggerKind = "schedule" | "manual"
export type AutomationIsolation = "worktree_per_run" | "shared_in_root"
export type AutomationRunStatus =
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "skipped"

/** Display-only cache so the editor can render a value the live agent no longer
 *  offers (marked unavailable) instead of silently dropping it. */
export interface AutomationLabelSnapshot {
  agent_label?: string
  mode_label?: string
  config_labels?: Record<string, string>
  folder_label?: string
  branch_label?: string
}

/** The captured composer snapshot stored in `automation.config`. `mode_id` +
 *  `config_values` are exactly AgentDelegationDefaults; the model rides inside
 *  `config_values["model"]`, never as its own field. */
export interface AutomationConfig {
  prompt_blocks: PromptInputBlock[]
  display_text: string
  mode_id?: string | null
  config_values: Record<string, string>
  label_snapshot?: AutomationLabelSnapshot | null
}

export interface Automation {
  id: number
  name: string
  enabled: boolean
  trigger_kind: AutomationTriggerKind
  cron: string | null
  timezone: string
  next_run_at: string | null
  agent_type: AgentType
  root_folder_id: number | null
  isolation: AutomationIsolation
  branch: string | null
  is_remote_branch: boolean
  // Serialized from an opaque JSON column; the backend falls back to `null`
  // when a stored blob fails to parse, so readers must guard against it.
  config: AutomationConfig | null
  last_run_at: string | null
  last_run_status: string | null
  last_run_conversation_id: number | null
  unseen_failures: number
  created_at: string
  updated_at: string
}

export interface AutomationRun {
  id: number
  automation_id: number
  status: AutomationRunStatus
  trigger: string
  scheduled_for: string | null
  started_at: string | null
  ended_at: string | null
  conversation_id: number | null
  worktree_folder_id: number | null
  stop_reason: string | null
  error: string | null
  summary: string | null
  created_at: string
}

/** Full create/update payload — the editor saves the whole automation wholesale. */
export interface AutomationDraft {
  name: string
  enabled: boolean
  trigger_kind: AutomationTriggerKind
  cron: string | null
  timezone: string
  agent_type: AgentType
  root_folder_id: number | null
  isolation: AutomationIsolation
  branch: string | null
  is_remote_branch: boolean
  config: AutomationConfig
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
/**
 * One background task settled by a `<task-notification>` transcript record
 * (mirror of Rust `BackgroundSettledInfo`). `task_id` is the launch ack's
 * `agentId` (async sub-agent) or `backgroundTaskId` (background shell);
 * `status` is the notification's `<status>` verbatim (`"completed"` on
 * success). The same id may settle more than once (a resumed sub-agent
 * notifies again).
 *
 * `tool_use_id`/`result` come from the same notification's `<tool-use-id>`/
 * `<result>` tags. The `background_activity` handler uses them to flip the
 * launch card in-memory (rewriting its `[[codeg-background-task]]` marker via
 * `resolveBackgroundTask`) instead of a `refetchDetail` — which double-rendered
 * the #870-held turn and raced the transcript's last write. `tool_use_id` is
 * the launching tool call's id (`toolu_…`), NOT `task_id`; absent for a
 * background shell (no marker card to flip).
 */
export interface BackgroundSettledInfo {
  task_id: string
  status: string
  summary?: string | null
  tool_use_id?: string | null
  result?: string | null
  /**
   * True when this task's reply is/was rendered live on the ACP wire as the
   * tail of a #870-held turn (the backend derives this from its launched-id
   * set, which outlives the turn's own status flip). The handler uses it to
   * skip arming the "Syncing background results…" hint for such a settle — the
   * reply is already on screen, so there's no gap to bridge. Absent/false for a
   * genuinely out-of-turn settle (reply arrives later as its own overlay turn).
   */
  wire_visible?: boolean
}

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
      // Synthetic notification-only event (chat-channel "user message" push).
      // The frontend reducer has no case for it — it is consumed backend-side.
      type: "user_prompt_sent"
      text_preview: string
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
   * Out-of-turn activity surfaced from the agent's own session transcript by
   * the backend watcher (Claude only): async sub-agent / background-shell
   * `<task-notification>` completions, the agent's continued work after them,
   * and cron//loop autonomous turns (which produce no wire events at all).
   * `turns` are UPSERTs keyed by `MessageTurn.id` into the conversation
   * runtime store's background overlay; `settled` entries each raise one OS
   * notification; `outstanding` mirrors into the connection for the idle-sweep
   * exemption and the "background tasks running" chip.
   */
  | {
      type: "background_activity"
      session_id: string
      turns?: MessageTurn[]
      outstanding: number
      settled?: BackgroundSettledInfo[]
      watermark: number
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
      /** Bounded preview of the delegated task text. Labels the card on
       *  hosts whose parent tool call never carries the arguments in
       *  `raw_input` (Cursor). Optional for older-backend tolerance. */
      task_preview?: string | null
      /** Broker-minted task id (the `task_id=` embedded in the running ack). */
      task_id?: string | null
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
      /** Child agent type. Carried so a frontend that missed the
       *  `delegation_started` event (mounted mid-flight, reconnect, or
       *  snapshot replay) can bind the correct agent instead of a default. */
      agent_type: AgentType
      result: DelegationResultSummary
    }
  /**
   * The user's submitted prompt, broadcast on the connection stream so OTHER
   * clients viewing this conversation synthesize the user turn in real time.
   * The sending client renders its own optimistic turn and ignores this echo.
   * Emitted only for root sends (delegation children synthesize kickoff text
   * separately).
   */
  | {
      type: "user_message"
      message_id: string
      blocks: UserMessageBlock[]
    }
  /**
   * The user submitted a live-feedback note while the agent is mid-turn (the
   * `check_user_feedback` steering path). Broadcast so every client viewing
   * this conversation renders the pending note; also captured in the snapshot.
   */
  | {
      type: "feedback_submitted"
      item: FeedbackItem
    }
  /**
   * The agent read one or more pending feedback notes via `check_user_feedback`.
   * Carries the note ids + the delivery instant; clients flip those notes to
   * `delivered` (they already hold the text from `feedback_submitted` / snapshot).
   */
  | {
      type: "feedback_consumed"
      ids: string[]
      delivered_at: string
    }
  /**
   * An agent called `ask_user_question`: a blocking multiple-choice prompt the
   * user must answer. Broadcast so every client renders the interactive card
   * above the input box; also captured in the snapshot for mid-turn attach.
   */
  | {
      type: "question_request"
      question_id: string
      questions: QuestionSpec[]
    }
  /**
   * A pending question was answered (from any client) or canceled (tool call
   * aborted / connection drained). Clients clear the matching card.
   */
  | {
      type: "question_resolved"
      question_id: string
    }
  /**
   * The agent's effective settings (env vars / model provider / native config)
   * changed AFTER this connection spawned, so the running process is still on
   * its launch-time config. The frontend shows a "restart to apply" banner.
   * `stale: false` means a prior drift was reverted (the setting was changed
   * back) and the banner should clear. Mirrored into `LiveSessionSnapshot` so a
   * snapshot attach (reconnect, refresh, new tile) recovers the state.
   */
  | {
      type: "session_config_stale"
      stale: boolean
      kind: ConfigStaleKind
    }

/** Which settings surface drifted (mirror of Rust `ConfigStaleKind`), used to
 *  word the "restart to apply" banner. */
export type ConfigStaleKind = "agent_config" | "model_provider"

/** A block of a broadcast user prompt (mirror of Rust `UserMessageBlock`).
 *  Narrower than the persisted `ContentBlock`: only what a viewer needs to
 *  render the user turn. Resource/resource-link prompt blocks are folded into
 *  `text` markdown links backend-side. */
export type UserMessageBlock =
  | { type: "text"; text: string }
  | { type: "image"; data: string; mime_type: string }

/**
 * Mirror of Rust `DelegationResultSummary`. `kind` discriminates Ok vs Err;
 * Ok carries `duration_ms` (broker-measured) and an optional `text_preview`
 * (≤ ~2 KiB of the child's final assistant text, so the parent card can render
 * the result inline without re-fetching the child session); Err carries a
 * stable code from the `DelegationError` taxonomy (e.g. `"timeout"`,
 * `"canceled"`).
 */
export type DelegationResultSummary =
  | { kind: "ok"; duration_ms: number; text_preview?: string | null }
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

/**
 * Snapshot-recoverable record of an in-flight (running) sub-agent delegation,
 * keyed by `parent_tool_use_id`. Mirror of Rust `ActiveDelegationState`. Only
 * running delegations are carried here — completed ones are removed (recovered
 * instead from the child's persisted DB row via `inject_delegation_meta`, or
 * the live `DelegationProvider` binding). Unlike `active_tool_calls`, these
 * survive the parent's `TurnComplete`, so a web/server client can recover the
 * running parent↔child binding from the snapshot on any attach — even when it
 * missed the transient `delegation_started` event.
 */
export interface ActiveDelegationState {
  parent_tool_use_id: string
  child_connection_id: string
  child_conversation_id: number
  agent_type: AgentType
  /** Task label + broker task id mirrored from `delegation_started` so a
   *  snapshot re-attach reseeds the binding WITH its label (required on
   *  hosts whose tool call `raw_input` never carries the arguments). */
  task_preview?: string | null
  task_id?: string | null
}

/** Lifecycle of a live-feedback note (mirror of Rust `FeedbackStatus`). */
export type FeedbackStatus = "pending" | "delivered"

/**
 * A user-submitted live-feedback ("steering") note (mirror of Rust
 * `FeedbackItem`). Turn-scoped: the backend clears the set when the next turn's
 * `user_message` arrives. `delivered_at` is set once the agent reads it.
 */
export interface FeedbackItem {
  id: string
  text: string
  created_at: string
  status: FeedbackStatus
  delivered_at?: string | null
}

/** Snapshot of the most recent ACP runtime error. */
export interface SessionLastError {
  message: string
  code?: string | null
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
  /** Awaiting-answer `ask_user_question`, recoverable on mid-turn attach.
   *  Absent (omitted) when no question is pending. */
  pending_question?: PendingQuestionState | null
  /** In-flight user prompt for the current turn — lets a client attaching
   *  mid-turn render the user turn. Absent (omitted) when no turn is in flight. */
  pending_user_message?: {
    message_id: string
    blocks: UserMessageBlock[]
  } | null
  /** Live sub-agent delegations recoverable from the snapshot. May be absent
   *  on older server payloads (then treated as `[]`). */
  active_delegations?: ActiveDelegationState[]
  /** Live-feedback notes for the current turn. Absent on older payloads /
   *  when empty (then treated as `[]`). */
  feedback?: FeedbackItem[]
  /** Launched-but-unresolved background tasks (async sub-agents / background
   *  shells) accounted from the transcript. Lets a client attaching
   *  mid-episode recover the pending count the one-shot `background_activity`
   *  events won't replay. Absent / omitted when zero. */
  background_outstanding?: number
  /** Whether this agent has the `check_user_feedback` tool (fixed at launch).
   *  The frontend gates the feedback bar on this — the agent's real capability —
   *  not the (possibly later-toggled) global setting. Absent → `false`. */
  feedback_tool_available?: boolean
  modes: SessionModeStateInfo | null
  current_mode: string | null
  config_options: SessionConfigOptionInfo[] | null
  prompt_capabilities: PromptCapabilitiesInfo | null
  usage: SessionUsageUpdateInfo | null
  fork_supported: boolean
  available_commands: AvailableCommandInfo[]
  selectors_ready: boolean
  /** Whether the running session is on stale (launch-time) config after a later
   *  settings save. Absent on older server payloads (then treated as `false`). */
  config_stale?: boolean
  /** Which settings surface drifted; present only while `config_stale`. */
  config_stale_kind?: ConfigStaleKind | null
  /** Latest agent/runtime error recoverable after reconnect. */
  last_error?: SessionLastError | null
  event_seq: number
}

// Connection info returned by acp_list_connections
export interface ConnectionInfo {
  id: string
  agent_type: AgentType
  status: ConnectionStatus
}

// Live connection bound to a conversation, returned by
// acp_find_connection_for_conversation. `null` means no live connection (read
// persisted detail instead of attaching). `event_seq` is the connection's
// progress at discovery time — informational only; viewers always cold-attach
// (full snapshot, no cursor), since they've applied no prior events.
export interface ConversationConnectionInfo {
  connection_id: string
  event_seq: number
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
  /** Compact structured codex model-catalog source (the custom-model list),
   *  round-tripped into the settings editor. Codex + api-key mode only. */
  codex_model_catalog: string | null
  cline_secrets_json: string | null
  /** Raw ~/.hermes/config.yaml text, for the Hermes panel's advanced editor. */
  hermes_config_yaml: string | null
  /** Raw ~/.grok/config.toml text, for the Grok panel's config-file editor. */
  grok_config_toml: string | null
  /** Parsed scalar settings backing the Grok panel's structured controls. Only
   * populated for the Grok agent; derived from grok_config_toml. */
  grok_settings: GrokSettings | null
  /** Raw ~/.cursor/cli-config.json text, for the Cursor panel's advanced view. */
  cursor_cli_config_json: string | null
  /** Parsed scalar settings backing the Cursor panel's structured controls
   * (sandbox / permission rules; the Run Everything permission mode is a
   * launch flag, not a config key). Cursor agent only. */
  cursor_settings: CursorSettings | null
  model_provider_id: number | null
}

/** Parsed keys from ~/.grok/config.toml. `null` means the key is absent.
 * Serialized snake_case to match AcpAgentInfo. The stock per-session model is
 * NOT here — it's chosen from the composer. But a codeg-managed custom (BYO
 * endpoint) model IS: it's the `[model.<id>]` block whose id equals
 * [models].default, read back through the custom_* fields. */
export interface GrokSettings {
  default_reasoning_effort: string | null
  permission_mode: string | null
  /** The codeg-managed custom model id ([model.<id>] == [models].default). */
  custom_model_id: string | null
  /** [model.<id>].base_url — null ⇒ Grok's official xAI endpoint. */
  custom_base_url: string | null
  /** [model.<id>].api_key — inline key scoped to the custom endpoint. */
  custom_api_key: string | null
  /** [model.<id>].api_backend — chat_completions | responses | messages. */
  custom_api_backend: string | null
  /** [model.<id>].context_window — context size in tokens. */
  custom_context_window: number | null
  /** [session].auto_compact_threshold_percent — 0–100 (Grok default 85). */
  auto_compact_threshold_percent: number | null
}

/** Structured-control values the Grok settings panel sends on save. Each
 * non-null value sets its config.toml key; each null removes it. camelCase on
 * the wire to match the request body. A non-empty customModelId writes (or
 * renames to) [model.<id>] + [models].default; empty/null removes the managed
 * block. Within an active model, an empty sub-field omits its key. */
export interface GrokStructuredConfig {
  defaultReasoningEffort: string | null
  permissionMode: string | null
  customModelId: string | null
  customBaseUrl: string | null
  customApiKey: string | null
  customApiBackend: string | null
  customContextWindow: number | null
  autoCompactThresholdPercent: number | null
}

/** Parsed keys from ~/.cursor/cli-config.json (shared with the Cursor CLI's
 * own /config UI). Only the codeg-managed subset is projected; everything
 * else is preserved verbatim on write. */
export interface CursorSettings {
  /** sandbox.mode — "enabled" | "disabled". */
  sandbox_mode: string | null
  /** permissions.allow rules, e.g. Shell(ls). */
  permissions_allow: string[]
  /** permissions.deny rules. */
  permissions_deny: string[]
}

/** Structured-control values the Cursor settings panel sends on save. Null
 * fields leave the key untouched; non-null fields replace it (lists
 * wholesale; an empty-string scalar removes the key). camelCase on the wire
 * to match the request body. */
export interface CursorStructuredConfig {
  sandboxMode?: string | null
  permissionsAllow?: string[] | null
  permissionsDeny?: string[] | null
}

/** Result of probing `cursor-agent status --format json` (auth card). */
export interface CursorAuthStatus {
  installed: boolean
  is_authenticated: boolean
  raw_status: string | null
  email: string | null
  membership: string | null
  error: string | null
  /** Absolute path to the cursor-agent binary codeg would launch; the panel
   * builds a copy-pasteable `"<binary_path>" login` command from it (the
   * managed binary isn't on PATH). Null when not installed. */
  binary_path?: string | null
}

/** One `cursor-agent models` entry: `<id> - <label> [(default)]`. The picker
 * shows `label` (falling back to `id`) and passes `id` to the CLI as --model. */
export interface CursorModelInfo {
  id: string
  label: string
  is_default: boolean
}

/** Result of `cursor-agent models` (model picker). */
export interface CursorModelsResult {
  models: CursorModelInfo[]
  default_model: string | null
  error: string | null
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

/** A single enable/disable request for one (skill, agent) pair. For office
 *  tools, `expertId` carries the office skill id. */
export interface LinkOp {
  expertId: string
  agentType: AgentType
  enable: boolean
}

/** Per-op outcome of a batch apply. A failed op never aborts the rest. */
export interface LinkOpResult {
  expertId: string
  agentType: AgentType
  ok: boolean
  /** Present on a successful enable; null for disables and failures. */
  status: ExpertInstallStatus | null
  error: string | null
}

/**
 * A user-authored "custom" skill. The fourth skill pack: unlike the bundled
 * experts/science/office packs, these are created/edited/imported/deleted by
 * the user, but live in the SAME central store (`~/.codeg/skills/<id>/`) and
 * reuse the experts link primitives. A skill is "custom" iff its central-store
 * directory id is not claimed by any bundled pack. Link statuses reuse
 * `ExpertInstallStatus`/`LinkOp`/`LinkOpResult` (the `expertId` field carries
 * the custom skill id).
 */
export interface CustomSkillItem {
  id: string
  /** Frontmatter `name:` if present, else the id. */
  name: string
  /** Best-effort one-line description from the SKILL.md frontmatter. */
  description: string | null
  central_path: string
}

/** Per-skill outcome of a batch delete (delete is skill-scoped, not per-agent). */
export interface CustomDeleteResult {
  id: string
  ok: boolean
  error: string | null
}

/**
 * Per-skill outcome of importing an agent's own skills into the central store.
 * `skipped` means the skill is already in the shared store (a linked built-in
 * skill or one imported earlier) — an idempotent no-op, not a failure.
 */
export interface CustomImportResult {
  id: string
  name: string
  ok: boolean
  skipped: boolean
  error: string | null
}

/**
 * Built-in scientific-research skills, curated from
 * K-Dense-AI/scientific-agent-skills and bundled into the codeg binary. They
 * share the central store (`~/.codeg/skills/`) and link primitives with
 * experts; link statuses reuse `ExpertInstallStatus`/`LinkOp`/`LinkOpResult`
 * (the `expertId` field carries the science skill id).
 */
export interface ScienceMetadata {
  id: string
  category: string
  icon: string | null
  sort_order: number
  /** Surface as a card in the new-session "Scientific Research" tab. */
  featured: boolean
  /** Color key indexing the ACCENTS map in quick-actions.tsx (featured only). */
  accent: string | null
  /** Primary workflow requires an external API key. */
  needs_key: boolean
  /** Ships scripts that may need a Python/uv environment. */
  needs_env: boolean
  display_name: Record<string, string>
  description: Record<string, string>
  bundled_hash: string
}

export interface ScienceListItem {
  metadata: ScienceMetadata
  installed_centrally: boolean
  user_modified: boolean
  central_path: string
}

export interface OfficecliInfo {
  installed: boolean
  version: string | null
  path: string | null
  // Set when the binary file is present (`installed = true`) but running it
  // failed — e.g. a missing system library (libicu) on a slim Linux server.
  // Carries an actionable diagnostic; null when officecli runs fine.
  runtimeError: string | null
}

export interface OfficecliSkill {
  id: string
  category: string
  icon: string
  sortOrder: number
  displayName: Record<string, string>
  description: Record<string, string>
  installedCentrally: boolean
}

export interface SkillSyncReport {
  synced: number
  errors: string[]
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

// --- Logging ---

export type LogLevel = "off" | "error" | "warn" | "info" | "debug" | "trace"

/** A per-target level override, e.g. `codeg_lib::acp` at `debug` while the
 * global level stays `info`. `target` is a tracing target (a Rust module path). */
export interface TargetDirective {
  target: string
  level: LogLevel
}

export interface LogSettings {
  level: LogLevel
  /** Omitted by the backend when empty; treat `undefined` as `[]`. */
  targets?: TargetDirective[]
}

/** What the Logs settings UI reads: the persisted level + per-target overrides,
 * plus whether an env var (CODEG_LOG/RUST_LOG) currently locks the controls
 * (env owns the live level). */
export interface LogSettingsView {
  level: LogLevel
  targets: TargetDirective[]
  env_locked: boolean
}

/** One enclosing span in an event's scope: its name + recorded fields. Ordered
 * root→leaf in `LogRecord.spans`. */
export interface SpanInfo {
  name: string
  fields: Record<string, string>
}

/** One captured log event. `level` is tracing's uppercase string
 * ("ERROR".."TRACE"); `target` is the emitting module path. `fields` holds the
 * event's own key-value fields and `spans` the enclosing span chain; both are
 * empty for plain-message logs. */
export interface LogRecord {
  seq: number
  timestamp_ms: number
  level: string
  target: string
  message: string
  fields: Record<string, string>
  spans: SpanInfo[]
}

export interface LogFileInfo {
  name: string
  size_bytes: number
  modified_ms: number
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

/** Per-agent install status of the HyperFrames agent skills. */
export interface HyperframesSkillAgent {
  agent: string
  installed: boolean
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
  | "hermes"
  | "code_buddy"
  | "kimi_code"
  | "grok"
  | "cursor"

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

/**
 * State of a working tree's HEAD (mirrors Rust `GitHeadInfo`). Distinguishes a
 * non-repo, a detached HEAD, and being on a branch — the branch-only
 * `getGitBranch` contract collapsed the last two into `null`, hiding git
 * operations for detached repos (issue #279).
 */
export interface GitHeadInfo {
  is_repo: boolean
  /** Branch name when on a branch (incl. unborn); null when detached or non-repo. */
  branch: string | null
  detached: boolean
  /** Short commit hash, present when detached. */
  short_sha: string | null
}

/**
 * Where a branch is checked out, resolved against registered folders (mirrors
 * Rust `WorktreeResolution`). `path` is the canonical worktree/main-tree path
 * hosting the branch, or null when it is not checked out in any worktree.
 * `folder_id` is the registered folder owning that path, or null for an
 * external/unregistered worktree. Drives the branch selector's navigate-vs-
 * checkout decision.
 */
export interface WorktreeResolution {
  path: string | null
  folder_id: number | null
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
  | "install_uv"

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

// ─── OpenCode Provider Catalog (models.dev) ───

/** A model entry under a catalog provider, normalized from models.dev. */
export interface OpenCodeCatalogModel {
  id: string
  name: string
  reasoning: boolean
  tool_call: boolean
  context: number | null
  cost_in: number | null
  cost_out: number | null
}

/**
 * One provider from the models.dev catalog (the same registry OpenCode reads).
 * `auth_kind` is `"oauth"` for providers OpenCode signs into via a browser flow
 * (ChatGPT, GitHub Copilot, GitLab Duo), `"api"` otherwise.
 */
export interface OpenCodeCatalogProvider {
  id: string
  name: string
  npm: string | null
  env: string[]
  doc: string | null
  auth_kind: "api" | "oauth"
  models: OpenCodeCatalogModel[]
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

export type OfficecliInstallEventKind =
  | "started"
  | "log"
  | "completed"
  | "failed"

export interface OfficecliInstallEvent {
  task_id: string
  kind: OfficecliInstallEventKind
  payload: string
}

// ─── Chat Channels ───

export type ChannelType = "lark" | "telegram" | "weixin"

/** One configured event-notification webhook sink. */
export interface WebhookConfig {
  url: string
  enabled: boolean
}

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

/** Result of `updateModelProvider` (mirror of Rust `UpdateModelProviderResult`):
 *  the updated provider plus how many running sessions the credential/model
 *  cascade left on stale (launch-time) config — for the settings-side
 *  "N sessions need restart" toast. */
export interface UpdateModelProviderResult {
  provider: ModelProviderInfo
  affectedRunningSessions: number
}

export interface ClaudeProviderModel {
  main?: string
  reasoning?: string
  haiku?: string
  sonnet?: string
  opus?: string
  /** ANTHROPIC_CUSTOM_MODEL_OPTION — id of a custom entry appended to the
   *  in-session /model picker (e.g. a model the gateway serves). */
  customOption?: string
  /** ANTHROPIC_CUSTOM_MODEL_OPTION_NAME — display name for that entry. */
  customOptionName?: string
  /** ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION — description for that entry. */
  customOptionDescription?: string
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
      "customOption",
      "customOptionName",
      "customOptionDescription",
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
  if (obj.customOption?.trim()) cleaned.customOption = obj.customOption.trim()
  if (obj.customOptionName?.trim())
    cleaned.customOptionName = obj.customOptionName.trim()
  if (obj.customOptionDescription?.trim())
    cleaned.customOptionDescription = obj.customOptionDescription.trim()
  return Object.keys(cleaned).length === 0 ? null : JSON.stringify(cleaned)
}

// ── Codex structured model catalog ──
//
// Codex custom models are stored as a compact list (each entry = a snapshot
// `base` slug + sparse `overrides`) inside the same single `model` string
// column used for Claude. The backend expands each entry into a full codex
// `ModelInfo` (cloning `base` from the bundled snapshot, forcing
// `visibility:"list"` + `supported_in_api:true`) and writes a
// `model_catalog_json` file. See src-tauri/src/acp/codex_model_catalog.rs.

/** A codex `ModelInfo` entry (from `codex debug models`). Friendly fields are
 *  typed; the rest stay opaque for the advanced editor + catalog cloning. */
export interface CodexModelInfo {
  slug: string
  display_name?: string
  description?: string | null
  context_window?: number | null
  max_context_window?: number | null
  visibility?: string
  [key: string]: unknown
}

/** One user-configured **custom** codex model, stored compactly. Heavy
 *  ModelInfo fields are cloned from `base` (a live-catalog slug) at
 *  catalog-generation time; `overrides` holds only fields the user changed. */
export interface CodexCustomEntry {
  slug: string
  displayName?: string
  contextWindow?: number
  base: string
  overrides?: Record<string, unknown>
}

/** The compact codex model config stored in a provider's `model` column / the
 *  codex agent's catalog source sidecar. Mirrors the Rust `CodexModelConfig`.
 *  Official models are auto-included from the live catalog, so only the user's
 *  deviations (custom additions + removed officials) are persisted. */
export interface CodexModelConfig {
  customs: CodexCustomEntry[]
  excludedOfficials?: string[]
  default?: string
}

/** Recursively sort object keys so serialized `overrides` are byte-stable (the
 *  edit dialog diffs `provider.model !== serialize(state)`). */
function sortJsonValue(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(sortJsonValue)
  if (v && typeof v === "object") {
    const out: Record<string, unknown> = {}
    for (const k of Object.keys(v as Record<string, unknown>).sort()) {
      out[k] = sortJsonValue((v as Record<string, unknown>)[k])
    }
    return out
  }
  return v
}

/** Parse one custom entry (new `customs` shape or legacy `models` shape — they
 *  are structurally identical). Returns null for a slug-less entry. */
function parseCustomEntry(m: unknown): CodexCustomEntry | null {
  if (!m || typeof m !== "object") return null
  const e = m as Record<string, unknown>
  const slug = typeof e.slug === "string" ? e.slug.trim() : ""
  if (!slug) return null
  const entry: CodexCustomEntry = {
    slug,
    base: typeof e.base === "string" && e.base.trim() ? e.base.trim() : slug,
  }
  if (typeof e.displayName === "string" && e.displayName.trim())
    entry.displayName = e.displayName.trim()
  if (typeof e.contextWindow === "number" && Number.isFinite(e.contextWindow))
    entry.contextWindow = e.contextWindow
  if (
    e.overrides &&
    typeof e.overrides === "object" &&
    !Array.isArray(e.overrides) &&
    Object.keys(e.overrides as object).length > 0
  ) {
    entry.overrides = e.overrides as Record<string, unknown>
  }
  return entry
}

function legacyBareSlug(raw: string): CodexModelConfig {
  const slug = raw.trim()
  return slug
    ? { customs: [{ slug, base: slug }], default: slug }
    : { customs: [] }
}

/** Parse the compact codex model config, with migration:
 *  - new shape `{customs,excludedOfficials,default}` → parsed;
 *  - legacy `{models}` → each model migrated to a custom;
 *  - a bare slug string → a single custom (matches the Rust `parse_model_config`
 *    back-compat so pre-existing providers keep working). */
export function parseCodexModelConfig(raw: string | null): CodexModelConfig {
  if (!raw || !raw.trim()) return { customs: [] }
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return legacyBareSlug(raw)
  }
  if (typeof parsed === "string") return legacyBareSlug(parsed)
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return legacyBareSlug(raw)
  }
  const obj = parsed as Record<string, unknown>
  const rawList = Array.isArray(obj.customs)
    ? obj.customs
    : Array.isArray(obj.models)
      ? obj.models
      : null
  const customs: CodexCustomEntry[] = []
  for (const m of rawList ?? []) {
    const entry = parseCustomEntry(m)
    if (entry) customs.push(entry)
  }
  const result: CodexModelConfig = { customs }
  if (Array.isArray(obj.excludedOfficials)) {
    const excluded = obj.excludedOfficials
      .filter((s): s is string => typeof s === "string" && !!s.trim())
      .map((s) => s.trim())
    if (excluded.length) result.excludedOfficials = excluded
  }
  if (typeof obj.default === "string" && obj.default)
    result.default = obj.default
  return result
}

/** Serialize the compact codex model config to canonical JSON (fixed key order,
 *  sorted `overrides` + `excludedOfficials`), or `null` when the user has made
 *  no deviations (no customs, no removed officials). `serialize(parse(x)) === x`
 *  for any canonical `x`, so an unedited form never reports a spurious change. */
export function serializeCodexModelConfig(
  obj: CodexModelConfig
): string | null {
  const customs = (obj.customs ?? [])
    .filter((m) => m.slug && m.slug.trim())
    .map((m) => {
      const entry: Record<string, unknown> = { slug: m.slug.trim() }
      if (m.displayName?.trim()) entry.displayName = m.displayName.trim()
      if (
        typeof m.contextWindow === "number" &&
        Number.isFinite(m.contextWindow)
      )
        entry.contextWindow = m.contextWindow
      entry.base = m.base?.trim() || m.slug.trim()
      if (m.overrides && Object.keys(m.overrides).length > 0)
        entry.overrides = sortJsonValue(m.overrides)
      return entry
    })
  const excluded = Array.from(
    new Set(
      (obj.excludedOfficials ?? [])
        .filter((s) => s && s.trim())
        .map((s) => s.trim())
    )
  ).sort()
  // No deviations from codex's own catalog → feature off.
  if (customs.length === 0 && excluded.length === 0) return null
  const out: Record<string, unknown> = { customs }
  if (excluded.length) out.excludedOfficials = excluded
  // Preserve the user's `default` verbatim (it may name an official the
  // serializer can't see); the backend validates it against the live catalog at
  // expand time and falls back if it names no listed model.
  if (obj.default && obj.default.trim()) out.default = obj.default.trim()
  return JSON.stringify(out)
}
