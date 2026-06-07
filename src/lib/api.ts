import {
  getActiveRemoteConnectionId,
  getShellTransport,
  getTransport,
  isDesktop,
  isRemoteDesktopMode,
  notifyRemoteDesktopUnauthorized,
} from "./transport"
import { getCodegToken } from "./transport/web-auth"
import { notifyWebUnauthorized } from "./transport/web-connection-store"
import { getCurrentEffectiveAppLocale } from "./i18n"
import { TurnBusyError, isTurnInProgressRejection } from "./turn-busy"
import type { FolderThemeColor } from "./theme-presets"
import type {
  AgentType,
  AgentDelegationDefaults,
  AgentOptionsSnapshot,
  ConversationSummary,
  ConversationDetail,
  DbConversationDetail,
  FolderInfo,
  AgentStats,
  SidebarData,
  ConnectionInfo,
  ConversationConnectionInfo,
  LiveSessionSnapshot,
  FeedbackItem,
  AcpAgentInfo,
  AcpAgentStatus,
  AgentSkillScope,
  AgentSkillLayout,
  AgentSkillItem,
  AgentSkillsListResult,
  AgentSkillContent,
  ExpertListItem,
  ExpertInstallStatus,
  FolderHistoryEntry,
  FolderDetail,
  WorktreeResolution,
  DbConversationSummary,
  ImportResult,
  OpenedTab,
  OpenedTabsSnapshot,
  SaveTabsOutcome,
  GitStatusEntry,
  GitBranchList,
  GitPullResult,
  GitPushResult,
  GitPushInfo,
  GitMergeResult,
  GitRebaseResult,
  GitResetMode,
  GitConflictFileVersions,
  GitCommitResult,
  GitRemote,
  GitStashEntry,
  PreflightResult,
  FolderCommand,
  TerminalInfo,
  PromptInputBlock,
  FileTreeNode,
  DirectoryEntry,
  DirectoryItem,
  UploadAttachmentResult,
  FilePreviewContent,
  FileEditContent,
  FileSaveResult,
  WorkspaceSnapshotResponse,
  GitLogResult,
  AvailableTerminalShells,
  SystemLanguageSettings,
  SystemProxySettings,
  SystemRenderingSettings,
  SystemTerminalSettings,
  GitCredentials,
  GitDetectResult,
  PackageManagerInfo,
  HyperframesSkillAgent,
  GitSettings,
  GitHubAccountsSettings,
  GitHubTokenValidation,
  McpAppType,
  LocalMcpServer,
  McpMarketplaceProvider,
  McpMarketplaceItem,
  McpMarketplaceServerDetail,
  ChatChannelInfo,
  ChannelStatusInfo,
  ChatChannelMessageLog,
  WebhookConfig,
  ModelProviderInfo,
  PluginCheckSummary,
  QuickMessage,
} from "./types"

export async function listConversations(params?: {
  agent_type?: AgentType | null
  search?: string | null
  sort_by?: string | null
  folder_path?: string | null
}): Promise<ConversationSummary[]> {
  return getTransport().call("list_conversations", {
    agentType: params?.agent_type ?? null,
    search: params?.search ?? null,
    sortBy: params?.sort_by ?? null,
    folderPath: params?.folder_path ?? null,
  })
}

export async function getConversation(
  agentType: AgentType,
  conversationId: string
): Promise<ConversationDetail> {
  return getTransport().call("get_conversation", { agentType, conversationId })
}

export async function listFolders(): Promise<FolderInfo[]> {
  return getTransport().call("list_folders")
}

export async function getStats(): Promise<AgentStats> {
  return getTransport().call("get_stats")
}

export async function getSidebarData(): Promise<SidebarData> {
  return getTransport().call("get_sidebar_data")
}

// ACP commands

export async function acpConnect(
  agentType: AgentType,
  workingDir?: string,
  sessionId?: string,
  preferredModeId?: string | null,
  preferredConfigValues?: Record<string, string> | null
): Promise<string> {
  return getTransport().call("acp_connect", {
    agentType,
    workingDir: workingDir ?? null,
    sessionId: sessionId ?? null,
    preferredModeId: preferredModeId ?? null,
    preferredConfigValues: preferredConfigValues ?? null,
  })
}

export async function acpPrompt(
  connectionId: string,
  blocks: PromptInputBlock[],
  folderId: number | null = null,
  conversationId: number | null = null,
  clientMessageId: string | null = null
): Promise<void> {
  try {
    await getTransport().call("acp_prompt", {
      connectionId,
      blocks,
      folderId,
      conversationId,
      clientMessageId,
    })
  } catch (e) {
    if (isTurnInProgressRejection(e)) throw new TurnBusyError()
    throw e
  }
}

export async function acpSetMode(
  connectionId: string,
  modeId: string
): Promise<void> {
  return getTransport().call("acp_set_mode", { connectionId, modeId })
}

export async function acpSetConfigOption(
  connectionId: string,
  configId: string,
  valueId: string
): Promise<void> {
  return getTransport().call("acp_set_config_option", {
    connectionId,
    configId,
    valueId,
  })
}

export async function acpCancel(connectionId: string): Promise<void> {
  return getTransport().call("acp_cancel", { connectionId })
}

export interface ForkResult {
  forkedSessionId: string
  originalSessionId: string
  siblingConversationId: number
}

export async function acpFork(connectionId: string): Promise<ForkResult> {
  try {
    return await getTransport().call("acp_fork", { connectionId })
  } catch (e) {
    // A fork is serialized with prompts on the backend: it returns
    // TurnInProgress while a turn is in flight. Surface it as TurnBusyError so
    // callers can treat it as transient (re-queue) rather than a fork failure.
    if (isTurnInProgressRejection(e)) throw new TurnBusyError()
    throw e
  }
}

export async function acpRespondPermission(
  connectionId: string,
  requestId: string,
  optionId: string
): Promise<void> {
  return getTransport().call("acp_respond_permission", {
    connectionId,
    requestId,
    optionId,
  })
}

export async function acpDisconnect(connectionId: string): Promise<void> {
  return getTransport().call("acp_disconnect", { connectionId })
}

export async function acpTouchConnection(
  connectionId: string
): Promise<boolean> {
  return getTransport().call("acp_touch_connection", { connectionId })
}

export async function acpListConnections(): Promise<ConnectionInfo[]> {
  return getTransport().call("acp_list_connections")
}

export async function acpGetSessionSnapshot(
  connectionId: string
): Promise<LiveSessionSnapshot | null> {
  return getTransport().call("acp_get_session_snapshot", { connectionId })
}

export async function acpGetSessionSnapshotByConversation(
  conversationId: number
): Promise<LiveSessionSnapshot | null> {
  return getTransport().call("acp_get_session_snapshot_by_conversation", {
    conversationId,
  })
}

export async function acpFindConnectionForConversation(
  conversationId: number,
  sessionId: string | undefined,
  agentType: AgentType
): Promise<ConversationConnectionInfo | null> {
  return getTransport().call("acp_find_connection_for_conversation", {
    conversationId,
    sessionId,
    agentType,
  })
}

export async function acpListAgents(): Promise<AcpAgentInfo[]> {
  return getTransport().call("acp_list_agents")
}

export async function acpGetAgentStatus(
  agentType: AgentType
): Promise<AcpAgentStatus> {
  return getTransport().call("acp_get_agent_status", { agentType })
}

export async function acpClearBinaryCache(agentType: AgentType): Promise<void> {
  return getTransport().call("acp_clear_binary_cache", { agentType })
}

export async function acpDownloadAgentBinary(
  agentType: AgentType,
  taskId: string,
  version?: string | null
): Promise<void> {
  return getTransport().call("acp_download_agent_binary", {
    agentType,
    version: version ?? null,
    taskId,
  })
}

export async function acpDetectAgentLocalVersion(
  agentType: AgentType
): Promise<string | null> {
  return getTransport().call("acp_detect_agent_local_version", { agentType })
}

export async function acpPrepareNpxAgent(
  agentType: AgentType,
  registryVersion: string | null | undefined,
  taskId: string,
  cleanFirst: boolean = false,
  version?: string | null
): Promise<string> {
  return getTransport().call("acp_prepare_npx_agent", {
    agentType,
    registryVersion: registryVersion ?? null,
    version: version ?? null,
    cleanFirst,
    taskId,
  })
}

export async function acpUninstallAgent(
  agentType: AgentType,
  taskId: string
): Promise<void> {
  return getTransport().call("acp_uninstall_agent", { agentType, taskId })
}

export async function acpUpdateAgentPreferences(
  agentType: AgentType,
  params: {
    enabled: boolean
    env: Record<string, string>
    config_json?: string | null
    opencode_auth_json?: string | null
    codex_auth_json?: string | null
    codex_config_toml?: string | null
  }
): Promise<void> {
  return getTransport().call("acp_update_agent_preferences", {
    agentType,
    enabled: params.enabled,
    env: params.env,
    configJson: params.config_json ?? null,
    opencodeAuthJson: params.opencode_auth_json ?? null,
    codexAuthJson: params.codex_auth_json ?? null,
    codexConfigToml: params.codex_config_toml ?? null,
  })
}

export async function acpUpdateAgentEnv(
  agentType: AgentType,
  params: {
    enabled: boolean
    env: Record<string, string>
    modelProviderId?: number | null
  }
): Promise<void> {
  return getTransport().call("acp_update_agent_env", {
    agentType,
    enabled: params.enabled,
    env: params.env,
    modelProviderId: params.modelProviderId ?? null,
  })
}

export async function acpUpdateAgentConfig(
  agentType: AgentType,
  params: {
    config_json?: string | null
    opencode_auth_json?: string | null
    codex_auth_json?: string | null
    codex_config_toml?: string | null
  }
): Promise<void> {
  return getTransport().call("acp_update_agent_config", {
    agentType,
    configJson: params.config_json ?? null,
    opencodeAuthJson: params.opencode_auth_json ?? null,
    codexAuthJson: params.codex_auth_json ?? null,
    codexConfigToml: params.codex_config_toml ?? null,
  })
}

export async function acpReorderAgents(agentTypes: AgentType[]): Promise<void> {
  return getTransport().call("acp_reorder_agents", { agentTypes })
}

export async function codexRequestDeviceCode(): Promise<{
  userCode: string
  verificationUrl: string
  deviceAuthId: string
  interval: number
}> {
  return getTransport().call("codex_request_device_code", {})
}

export async function codexPollDeviceCode(params: {
  deviceAuthId: string
  userCode: string
}): Promise<{
  status: "pending" | "success" | "error"
  message?: string
  idToken?: string
  accessToken?: string
  refreshToken?: string
  accountId?: string
}> {
  return getTransport().call("codex_poll_device_code", {
    deviceAuthId: params.deviceAuthId,
    userCode: params.userCode,
  })
}

export async function acpPreflight(
  agentType: AgentType,
  forceRefresh?: boolean
): Promise<PreflightResult> {
  return getTransport().call("acp_preflight", {
    agentType,
    forceRefresh: forceRefresh ?? null,
  })
}

export async function opencodeListPlugins(): Promise<PluginCheckSummary> {
  return getTransport().call("opencode_list_plugins", {})
}

export async function opencodeInstallPlugins(
  taskId: string,
  names?: string[] | null
): Promise<void> {
  return getTransport().call("opencode_install_plugins", {
    names: names ?? null,
    taskId,
  })
}

export async function opencodeUninstallPlugin(
  name: string
): Promise<PluginCheckSummary> {
  return getTransport().call("opencode_uninstall_plugin", { name })
}

export async function acpListAgentSkills(params: {
  agentType: AgentType
  workspacePath?: string | null
}): Promise<AgentSkillsListResult> {
  return getTransport().call("acp_list_agent_skills", {
    agentType: params.agentType,
    workspacePath: params.workspacePath ?? null,
  })
}

export async function acpReadAgentSkill(params: {
  agentType: AgentType
  scope: AgentSkillScope
  skillId: string
  workspacePath?: string | null
}): Promise<AgentSkillContent> {
  return getTransport().call("acp_read_agent_skill", {
    agentType: params.agentType,
    scope: params.scope,
    skillId: params.skillId,
    workspacePath: params.workspacePath ?? null,
  })
}

export async function acpSaveAgentSkill(params: {
  agentType: AgentType
  scope: AgentSkillScope
  skillId: string
  content: string
  workspacePath?: string | null
  layout?: AgentSkillLayout | null
}): Promise<AgentSkillItem> {
  return getTransport().call("acp_save_agent_skill", {
    agentType: params.agentType,
    scope: params.scope,
    skillId: params.skillId,
    content: params.content,
    workspacePath: params.workspacePath ?? null,
    layout: params.layout ?? null,
  })
}

export async function acpDeleteAgentSkill(params: {
  agentType: AgentType
  scope: AgentSkillScope
  skillId: string
  workspacePath?: string | null
}): Promise<void> {
  return getTransport().call("acp_delete_agent_skill", {
    agentType: params.agentType,
    scope: params.scope,
    skillId: params.skillId,
    workspacePath: params.workspacePath ?? null,
  })
}

// ─── Experts (built-in expert skills) ───────────────────────────────────

export async function expertsList(): Promise<ExpertListItem[]> {
  return getTransport().call("experts_list")
}

export async function expertsListForAgent(
  agentType: AgentType
): Promise<ExpertListItem[]> {
  return getTransport().call("experts_list_for_agent", { agentType })
}

export async function expertsGetInstallStatus(
  expertId: string
): Promise<ExpertInstallStatus[]> {
  return getTransport().call("experts_get_install_status", { expertId })
}

export async function expertsLinkToAgent(params: {
  expertId: string
  agentType: AgentType
}): Promise<ExpertInstallStatus> {
  return getTransport().call("experts_link_to_agent", {
    expertId: params.expertId,
    agentType: params.agentType,
  })
}

export async function expertsUnlinkFromAgent(params: {
  expertId: string
  agentType: AgentType
}): Promise<void> {
  return getTransport().call("experts_unlink_from_agent", {
    expertId: params.expertId,
    agentType: params.agentType,
  })
}

export async function expertsReadContent(expertId: string): Promise<string> {
  return getTransport().call("experts_read_content", { expertId })
}

export async function expertsOpenCentralDir(): Promise<string> {
  return getTransport().call("experts_open_central_dir")
}

export async function getSystemProxySettings(): Promise<SystemProxySettings> {
  return getTransport().call("get_system_proxy_settings")
}

export async function updateSystemProxySettings(
  settings: SystemProxySettings
): Promise<SystemProxySettings> {
  return getTransport().call("update_system_proxy_settings", { settings })
}

export async function getSystemLanguageSettings(): Promise<SystemLanguageSettings> {
  return getTransport().call("get_system_language_settings")
}

export async function updateSystemLanguageSettings(
  settings: SystemLanguageSettings
): Promise<SystemLanguageSettings> {
  return getTransport().call("update_system_language_settings", { settings })
}

export async function getSystemTerminalSettings(): Promise<SystemTerminalSettings> {
  return getTransport().call("get_system_terminal_settings")
}

export async function updateSystemTerminalSettings(
  settings: SystemTerminalSettings
): Promise<SystemTerminalSettings> {
  return getTransport().call("update_system_terminal_settings", { settings })
}

export async function getAvailableTerminalShells(): Promise<AvailableTerminalShells> {
  return getTransport().call("get_available_terminal_shells")
}

export async function probeTerminalShellPath(path: string): Promise<boolean> {
  return getTransport().call("probe_terminal_shell_path", { path })
}

export async function getSystemRenderingSettings(): Promise<SystemRenderingSettings> {
  return getTransport().call("get_system_rendering_settings")
}

export async function updateSystemRenderingSettings(
  settings: SystemRenderingSettings
): Promise<SystemRenderingSettings> {
  return getTransport().call("update_system_rendering_settings", { settings })
}

// --- Version Control ---

export async function detectGit(): Promise<GitDetectResult> {
  return getTransport().call("detect_git")
}

export async function testGitPath(path: string): Promise<GitDetectResult> {
  return getTransport().call("test_git_path", { path })
}

export async function getGitSettings(): Promise<GitSettings> {
  return getTransport().call("get_git_settings")
}

export async function updateGitSettings(
  settings: GitSettings
): Promise<GitSettings> {
  return getTransport().call("update_git_settings", { settings })
}

export async function getGitHubAccounts(): Promise<GitHubAccountsSettings> {
  return getTransport().call("get_github_accounts")
}

export async function validateGitHubToken(
  serverUrl: string,
  token: string
): Promise<GitHubTokenValidation> {
  return getTransport().call("validate_github_token", { serverUrl, token })
}

export async function updateGitHubAccounts(
  settings: GitHubAccountsSettings
): Promise<GitHubAccountsSettings> {
  return getTransport().call("update_github_accounts", { settings })
}

export async function saveAccountToken(
  accountId: string,
  token: string
): Promise<void> {
  return getTransport().call("save_account_token", { accountId, token })
}

export async function getAccountToken(
  accountId: string
): Promise<string | null> {
  return getTransport().call("get_account_token", { accountId })
}

export async function deleteAccountToken(accountId: string): Promise<void> {
  return getTransport().call("delete_account_token", { accountId })
}

export async function mcpScanLocal(): Promise<LocalMcpServer[]> {
  return getTransport().call("mcp_scan_local")
}

export async function mcpListMarketplaces(): Promise<McpMarketplaceProvider[]> {
  return getTransport().call("mcp_list_marketplaces")
}

export async function mcpSearchMarketplace(params: {
  providerId: string
  query?: string | null
  limit?: number | null
}): Promise<McpMarketplaceItem[]> {
  return getTransport().call("mcp_search_marketplace", {
    providerId: params.providerId,
    query: params.query ?? null,
    limit: params.limit ?? null,
  })
}

export async function mcpGetMarketplaceServerDetail(params: {
  providerId: string
  serverId: string
}): Promise<McpMarketplaceServerDetail> {
  return getTransport().call("mcp_get_marketplace_server_detail", {
    providerId: params.providerId,
    serverId: params.serverId,
  })
}

export async function mcpInstallFromMarketplace(params: {
  providerId: string
  serverId: string
  apps: McpAppType[]
  specOverride?: Record<string, unknown> | null
  optionId?: string | null
  protocol?: string | null
  parameterValues?: Record<string, unknown> | null
}): Promise<LocalMcpServer> {
  return getTransport().call("mcp_install_from_marketplace", {
    providerId: params.providerId,
    serverId: params.serverId,
    apps: params.apps,
    specOverride: params.specOverride ?? null,
    optionId: params.optionId ?? null,
    protocol: params.protocol ?? null,
    parameterValues: params.parameterValues ?? null,
  })
}

export async function mcpUpsertLocalServer(params: {
  serverId: string
  spec: Record<string, unknown>
  apps: McpAppType[]
}): Promise<LocalMcpServer> {
  return getTransport().call("mcp_upsert_local_server", {
    serverId: params.serverId,
    spec: params.spec,
    apps: params.apps,
  })
}

export async function mcpSetServerApps(
  serverId: string,
  apps: McpAppType[]
): Promise<LocalMcpServer | null> {
  return getTransport().call("mcp_set_server_apps", { serverId, apps })
}

export async function mcpRemoveServer(
  serverId: string,
  apps?: McpAppType[] | null
): Promise<boolean> {
  return getTransport().call("mcp_remove_server", {
    serverId,
    apps: apps ?? null,
  })
}

// Folder history commands

export async function loadFolderHistory(): Promise<FolderHistoryEntry[]> {
  return getTransport().call("load_folder_history")
}

export async function getFolder(folderId: number): Promise<FolderDetail> {
  return getTransport().call("get_folder", { folderId })
}

export async function listAllConversations(params?: {
  folder_ids?: number[] | null
  agent_type?: AgentType | null
  search?: string | null
  sort_by?: string | null
  status?: string | null
  include_children?: boolean | null
}): Promise<DbConversationSummary[]> {
  return getTransport().call("list_all_conversations", {
    folderIds: params?.folder_ids ?? null,
    agentType: params?.agent_type ?? null,
    search: params?.search ?? null,
    sortBy: params?.sort_by ?? null,
    status: params?.status ?? null,
    includeChildren: params?.include_children ?? null,
  })
}

export async function listChildConversations(
  parentConversationId: number
): Promise<DbConversationSummary[]> {
  return getTransport().call("list_child_conversations", {
    parentConversationId,
  })
}

export async function listOpenedTabs(): Promise<OpenedTabsSnapshot> {
  return getTransport().call("list_opened_tabs")
}

export async function saveOpenedTabs(
  items: OpenedTab[],
  expectedVersion: number,
  origin: string
): Promise<SaveTabsOutcome> {
  return getTransport().call("save_opened_tabs", {
    items,
    expectedVersion,
    origin,
  })
}

export async function listOpenFolderDetails(): Promise<FolderDetail[]> {
  return getTransport().call("list_open_folder_details")
}

export async function listAllFolderDetails(): Promise<FolderDetail[]> {
  return getTransport().call("list_all_folder_details")
}

export async function openFolderById(folderId: number): Promise<FolderDetail> {
  return getTransport().call("open_folder_by_id", { folderId })
}

export async function removeFolderFromWorkspace(
  folderId: number
): Promise<void> {
  return getTransport().call("remove_folder_from_workspace", { folderId })
}

export async function reorderFolders(ids: number[]): Promise<void> {
  return getTransport().call("reorder_folders", { ids })
}

export async function updateFolderColor(
  folderId: number,
  color: FolderThemeColor
): Promise<FolderDetail> {
  return getTransport().call("update_folder_color", { folderId, color })
}

export async function updateFolderDefaultAgent(
  folderId: number,
  defaultAgentType: AgentType | null
): Promise<FolderDetail> {
  return getTransport().call("update_folder_default_agent", {
    folderId,
    defaultAgentType,
  })
}

export async function importLocalConversations(
  folderId: number
): Promise<ImportResult> {
  return getTransport().call("import_local_conversations", { folderId })
}

export async function getFolderConversation(
  conversationId: number
): Promise<DbConversationDetail> {
  return getTransport().call("get_folder_conversation", { conversationId })
}

export async function removeFolderFromHistory(path: string): Promise<void> {
  return getTransport().call("remove_folder_from_history", { path })
}

export async function createFolderDirectory(path: string): Promise<void> {
  return getTransport().call("create_folder_directory", { path })
}

export async function cloneRepository(
  url: string,
  targetDir: string,
  credentials?: GitCredentials | null
): Promise<void> {
  return getTransport().call("clone_repository", {
    url,
    targetDir,
    credentials: credentials ?? null,
  })
}

export async function getGitBranch(path: string): Promise<string | null> {
  return getTransport().call("get_git_branch", { path })
}

export async function gitInit(path: string): Promise<void> {
  return getTransport().call("git_init", { path })
}

export async function gitPull(
  path: string,
  credentials?: GitCredentials | null
): Promise<GitPullResult> {
  return getTransport().call("git_pull", {
    path,
    credentials: credentials ?? null,
  })
}

export async function gitStartPullMerge(
  path: string,
  upstreamCommit?: string | null
): Promise<void> {
  return getTransport().call("git_start_pull_merge", { path, upstreamCommit })
}

export async function gitHasMergeHead(path: string): Promise<boolean> {
  return getTransport().call("git_has_merge_head", { path })
}

export async function gitFetch(
  path: string,
  credentials?: GitCredentials | null
): Promise<string> {
  return getTransport().call("git_fetch", {
    path,
    credentials: credentials ?? null,
  })
}

export async function gitPushInfo(path: string): Promise<GitPushInfo> {
  return getTransport().call("git_push_info", { path })
}

export async function gitPush(
  path: string,
  remote?: string | null,
  credentials?: GitCredentials | null,
  folderId?: number | null
): Promise<GitPushResult> {
  return getTransport().call("git_push", {
    path,
    remote: remote ?? null,
    credentials: credentials ?? null,
    folderId: folderId ?? null,
  })
}

export async function gitNewBranch(
  path: string,
  branchName: string,
  startPoint?: string
): Promise<void> {
  return getTransport().call("git_new_branch", {
    path,
    branchName,
    startPoint: startPoint ?? null,
  })
}

export async function gitWorktreeAdd(
  path: string,
  branchName: string,
  worktreePath: string
): Promise<void> {
  return getTransport().call("git_worktree_add", {
    path,
    branchName,
    worktreePath,
  })
}

export async function gitCheckout(
  path: string,
  branchName: string
): Promise<void> {
  return getTransport().call("git_checkout", { path, branchName })
}

export async function gitListBranches(path: string): Promise<string[]> {
  return getTransport().call("git_list_branches", { path })
}

export async function gitListAllBranches(path: string): Promise<GitBranchList> {
  return getTransport().call("git_list_all_branches", { path })
}

export async function gitMerge(
  path: string,
  branchName: string
): Promise<GitMergeResult> {
  return getTransport().call("git_merge", { path, branchName })
}

export async function gitRebase(
  path: string,
  branchName: string
): Promise<GitRebaseResult> {
  return getTransport().call("git_rebase", { path, branchName })
}

export async function gitDeleteBranch(
  path: string,
  branchName: string,
  force: boolean = false
): Promise<string> {
  return getTransport().call("git_delete_branch", {
    path,
    branchName,
    force,
  })
}

export async function gitDeleteRemoteBranch(
  path: string,
  remote: string,
  branch: string,
  credentials?: GitCredentials | null
): Promise<void> {
  return getTransport().call("git_delete_remote_branch", {
    path,
    remote,
    branch,
    credentials: credentials ?? null,
  })
}

export async function gitListConflicts(path: string): Promise<string[]> {
  return getTransport().call("git_list_conflicts", { path })
}

export async function gitConflictFileVersions(
  path: string,
  file: string
): Promise<GitConflictFileVersions> {
  return getTransport().call("git_conflict_file_versions", { path, file })
}

export async function gitResolveConflict(
  path: string,
  file: string,
  content: string
): Promise<void> {
  return getTransport().call("git_resolve_conflict", { path, file, content })
}

export async function gitAbortOperation(
  path: string,
  operation: string
): Promise<void> {
  return getTransport().call("git_abort_operation", { path, operation })
}

export async function gitContinueOperation(
  path: string,
  operation: string
): Promise<void> {
  return getTransport().call("git_continue_operation", { path, operation })
}

export async function openMergeWindow(
  folderId: number,
  operation: string,
  upstreamCommit?: string | null
): Promise<void> {
  const locale = getCurrentEffectiveAppLocale()
  if (isDesktop()) {
    return getShellTransport().call("open_merge_window", {
      folderId,
      operation,
      upstreamCommit: upstreamCommit ?? null,
      locale,
      remoteConnectionId: getActiveRemoteConnectionId(),
    })
  }
  const result = await getTransport().call<{ path: string }>(
    "open_merge_window",
    {
      folderId,
      operation,
      upstreamCommit: upstreamCommit ?? null,
      locale,
    }
  )
  window.open(result.path, `merge-${folderId}`)
}

export async function openStashWindow(folderId: number): Promise<void> {
  const locale = getCurrentEffectiveAppLocale()
  if (isDesktop()) {
    return getShellTransport().call("open_stash_window", {
      folderId,
      locale,
      remoteConnectionId: getActiveRemoteConnectionId(),
    })
  }
  const result = await getTransport().call<{ path: string }>(
    "open_stash_window",
    { folderId, locale }
  )
  window.open(result.path, `stash-${folderId}`)
}

export async function openPushWindow(folderId: number): Promise<void> {
  const locale = getCurrentEffectiveAppLocale()
  if (isDesktop()) {
    return getShellTransport().call("open_push_window", {
      folderId,
      locale,
      remoteConnectionId: getActiveRemoteConnectionId(),
    })
  }
  const result = await getTransport().call<{ path: string }>(
    "open_push_window",
    { folderId, locale }
  )
  window.open(result.path, `push-${folderId}`)
}

export async function gitStashPush(
  path: string,
  message?: string,
  keepIndex?: boolean
): Promise<string> {
  return getTransport().call("git_stash_push", {
    path,
    message: message ?? null,
    keepIndex: keepIndex ?? false,
  })
}

export async function gitStashPop(
  path: string,
  stashRef?: string
): Promise<string> {
  return getTransport().call("git_stash_pop", {
    path,
    stashRef: stashRef ?? null,
  })
}

export async function gitStashList(path: string): Promise<GitStashEntry[]> {
  return getTransport().call("git_stash_list", { path })
}

export async function gitStashApply(
  path: string,
  stashRef: string
): Promise<string> {
  return getTransport().call("git_stash_apply", { path, stashRef })
}

export async function gitStashDrop(
  path: string,
  stashRef: string
): Promise<string> {
  return getTransport().call("git_stash_drop", { path, stashRef })
}

export async function gitStashClear(path: string): Promise<string> {
  return getTransport().call("git_stash_clear", { path })
}

export async function gitStashShow(
  path: string,
  stashRef: string
): Promise<GitStatusEntry[]> {
  return getTransport().call("git_stash_show", { path, stashRef })
}

export async function gitListRemotes(path: string): Promise<GitRemote[]> {
  return getTransport().call("git_list_remotes", { path })
}

export async function gitFetchRemote(
  path: string,
  name: string,
  credentials?: GitCredentials | null
): Promise<string> {
  return getTransport().call("git_fetch_remote", {
    path,
    name,
    credentials: credentials ?? null,
  })
}

export async function gitAddRemote(
  path: string,
  name: string,
  url: string
): Promise<void> {
  return getTransport().call("git_add_remote", { path, name, url })
}

export async function gitRemoveRemote(
  path: string,
  name: string
): Promise<void> {
  return getTransport().call("git_remove_remote", { path, name })
}

export async function gitSetRemoteUrl(
  path: string,
  name: string,
  url: string
): Promise<void> {
  return getTransport().call("git_set_remote_url", { path, name, url })
}

export async function gitStatus(
  path: string,
  showAllUntracked?: boolean
): Promise<GitStatusEntry[]> {
  return getTransport().call("git_status", {
    path,
    showAllUntracked: showAllUntracked ?? null,
  })
}

export async function gitDiff(path: string, file?: string): Promise<string> {
  return getTransport().call("git_diff", { path, file: file ?? null })
}

export async function gitDiffWithBranch(
  path: string,
  branch: string,
  file?: string
): Promise<string> {
  return getTransport().call("git_diff_with_branch", {
    path,
    branch,
    file: file ?? null,
  })
}

export async function gitShowDiff(
  path: string,
  commit: string,
  file?: string
): Promise<string> {
  return getTransport().call("git_show_diff", {
    path,
    commit,
    file: file ?? null,
  })
}

export async function gitShowFile(
  path: string,
  file: string,
  refName?: string
): Promise<string> {
  return getTransport().call("git_show_file", {
    path,
    file,
    refName: refName ?? null,
  })
}

export async function gitIsTracked(
  path: string,
  file: string
): Promise<boolean> {
  return getTransport().call("git_is_tracked", { path, file })
}

export async function gitCommit(
  path: string,
  message: string,
  files: string[],
  folderId?: number | null
): Promise<GitCommitResult> {
  return getTransport().call("git_commit", {
    path,
    message,
    files,
    folderId: folderId ?? null,
  })
}

export async function gitRollbackFile(
  path: string,
  file: string
): Promise<void> {
  return getTransport().call("git_rollback_file", { path, file })
}

export async function gitAddFiles(
  path: string,
  files: string[]
): Promise<void> {
  return getTransport().call("git_add_files", { path, files })
}

// Window management commands

export async function openFolder(path: string): Promise<FolderDetail> {
  return getTransport().call("open_folder", { path })
}

/**
 * Open a freshly created git worktree directory as a folder, recording the root
 * folder it descends from (`sourceFolderId` is the folder the worktree was
 * created from; the backend flattens to the root). Lets the worktree folder be
 * merged under its parent in the sidebar.
 */
export async function openWorktreeFolder(
  path: string,
  sourceFolderId: number
): Promise<FolderDetail> {
  return getTransport().call("open_worktree_folder", { path, sourceFolderId })
}

/**
 * Resolve where `branch` is checked out across the repo's worktrees. Returns the
 * canonical worktree path (or null if the branch isn't checked out anywhere) and
 * the registered folder id owning that path (or null for an external worktree).
 * Path matching is canonicalized on the host that runs git, so it is correct for
 * symlinked and remote-workspace paths the webview cannot resolve.
 */
export async function resolveWorktreeFolder(
  repoPath: string,
  branch: string
): Promise<WorktreeResolution> {
  return getTransport().call("resolve_worktree_folder", { repoPath, branch })
}

export async function openCommitWindow(folderId: number): Promise<void> {
  const locale = getCurrentEffectiveAppLocale()
  if (isDesktop()) {
    return getShellTransport().call("open_commit_window", {
      folderId,
      locale,
      remoteConnectionId: getActiveRemoteConnectionId(),
    })
  }
  const result = await getTransport().call<{ path: string }>(
    "open_commit_window",
    { folderId, locale }
  )
  window.open(result.path, `commit-${folderId}`)
}

export type SettingsSection =
  | "appearance"
  | "agents"
  | "mcp"
  | "skills"
  | "shortcuts"
  | "system"

interface OpenSettingsWindowOptions {
  agentType?: AgentType | null
}

export async function openSettingsWindow(
  section?: SettingsSection,
  options?: OpenSettingsWindowOptions
): Promise<void> {
  const locale = getCurrentEffectiveAppLocale()
  if (isDesktop()) {
    return getShellTransport().call("open_settings_window", {
      section: section ?? null,
      agentType: options?.agentType ?? null,
      locale,
      remoteConnectionId: getActiveRemoteConnectionId(),
    })
  }
  // Web mode: open in new window
  const result = await getTransport().call<{ path: string }>(
    "open_settings_window",
    {
      section: section ?? null,
      agentType: options?.agentType ?? null,
      locale,
    }
  )
  window.open(result.path, `settings-${section ?? "general"}`)
}

export async function openProjectBootWindow(source?: string): Promise<void> {
  if (isDesktop()) {
    return getShellTransport().call("open_project_boot_window", {
      source,
      locale: getCurrentEffectiveAppLocale(),
      remoteConnectionId: getActiveRemoteConnectionId(),
    })
  }
  if (typeof window !== "undefined") {
    window.open("/project-boot", "project-boot")
  }
}

// Cross-window handoff for the project launcher, which lives in its own
// window/tab and can't reach the workspace's React state directly. The
// backend upserts the folder and emits `folder://open-in-workspace` carrying
// the FolderDetail through the shared EventEmitter; the transport layer routes
// that to the right workspace window in every runtime (local Tauri bus, the
// server's WebSocket broadcaster for web, and the remote server's broadcaster
// for remote desktop), so only windows talking to this backend react. The
// workspace subscribes via WorkspaceOpenFolderListener.
export const FOLDER_OPEN_IN_WORKSPACE_EVENT = "folder://open-in-workspace"

export async function openFolderInWorkspace(
  path: string
): Promise<FolderDetail> {
  return getTransport().call("open_folder_in_workspace", { path })
}

export async function detectPackageManager(
  name: string
): Promise<PackageManagerInfo> {
  return getTransport().call("detect_package_manager", { name })
}

export async function createShadcnProject(params: {
  projectName: string
  template: string
  presetCode: string
  packageManager: string
  targetDir: string
}): Promise<string> {
  return getTransport().call("create_shadcn_project", {
    projectName: params.projectName,
    template: params.template,
    presetCode: params.presetCode,
    packageManager: params.packageManager,
    targetDir: params.targetDir,
  })
}

/**
 * Detect, per codeg-supported agent, whether the HyperFrames skills are already
 * installed globally. Cheap filesystem check, so no long timeout is needed.
 */
export async function detectHyperframesSkills(): Promise<
  HyperframesSkillAgent[]
> {
  return getTransport().call(
    "detect_hyperframes_skills",
    {},
    { timeoutMs: 30_000 }
  )
}

/**
 * Install the HyperFrames agent skills globally (symlinked) for the given
 * agents. Clones from GitHub, so allow a few minutes. Re-running is idempotent
 * (acts as an update for agents that already have the skills).
 */
export async function installHyperframesSkills(
  agents: string[]
): Promise<void> {
  await getTransport().call(
    "install_hyperframes_skills",
    { agents },
    { timeoutMs: 600_000 }
  )
}

export async function createHyperframesProject(params: {
  projectName: string
  example: string
  resolution: string
  packageManager: string
  targetDir: string
}): Promise<string> {
  return getTransport().call(
    "create_hyperframes_project",
    {
      projectName: params.projectName,
      example: params.example,
      resolution: params.resolution,
      packageManager: params.packageManager,
      targetDir: params.targetDir,
    },
    { timeoutMs: 600_000 }
  )
}

// Conversation CRUD commands

export async function createConversation(
  folderId: number,
  agentType: AgentType,
  title?: string
): Promise<number> {
  return getTransport().call("create_conversation", {
    folderId,
    agentType,
    title: title ?? null,
  })
}

export async function updateConversationStatus(
  conversationId: number,
  status: string
): Promise<void> {
  return getTransport().call("update_conversation_status", {
    conversationId,
    status,
  })
}

export async function updateConversationTitle(
  conversationId: number,
  title: string
): Promise<void> {
  return getTransport().call("update_conversation_title", {
    conversationId,
    title,
  })
}

export async function deleteConversation(
  conversationId: number
): Promise<void> {
  return getTransport().call("delete_conversation", { conversationId })
}

// Folder command management

export async function listFolderCommands(
  folderId: number
): Promise<FolderCommand[]> {
  return getTransport().call("list_folder_commands", { folderId })
}

export async function createFolderCommand(
  folderId: number,
  name: string,
  command: string
): Promise<FolderCommand> {
  return getTransport().call("create_folder_command", {
    folderId,
    name,
    command,
  })
}

export async function updateFolderCommand(
  id: number,
  name?: string,
  command?: string,
  sortOrder?: number
): Promise<FolderCommand> {
  return getTransport().call("update_folder_command", {
    id,
    name: name ?? null,
    command: command ?? null,
    sortOrder: sortOrder ?? null,
  })
}

export async function deleteFolderCommand(id: number): Promise<void> {
  return getTransport().call("delete_folder_command", { id })
}

export async function reorderFolderCommands(
  folderId: number,
  ids: number[]
): Promise<void> {
  return getTransport().call("reorder_folder_commands", { folderId, ids })
}

export async function bootstrapFolderCommandsFromPackageJson(
  folderId: number,
  folderPath: string
): Promise<FolderCommand[]> {
  return getTransport().call("bootstrap_folder_commands_from_package_json", {
    folderId,
    folderPath,
  })
}

// Quick message management

export async function quickMessagesList(): Promise<QuickMessage[]> {
  return getTransport().call("quick_messages_list")
}

export async function quickMessagesCreate(params: {
  title: string
  content: string
}): Promise<QuickMessage> {
  return getTransport().call("quick_messages_create", {
    title: params.title,
    content: params.content,
  })
}

export async function quickMessagesUpdate(params: {
  id: number
  title?: string
  content?: string
}): Promise<QuickMessage> {
  return getTransport().call("quick_messages_update", {
    id: params.id,
    title: params.title ?? null,
    content: params.content ?? null,
  })
}

export async function quickMessagesDelete(id: number): Promise<void> {
  return getTransport().call("quick_messages_delete", { id })
}

export async function quickMessagesReorder(ids: number[]): Promise<void> {
  return getTransport().call("quick_messages_reorder", { ids })
}

// Directory browser (for web/server mode)

export async function getHomeDirectory(): Promise<string> {
  return getTransport().call("get_home_directory")
}

export async function listDirectoryEntries(
  path: string
): Promise<DirectoryEntry[]> {
  return getTransport().call("list_directory_entries", { path })
}

export async function listDirectoryWithFiles(
  path: string
): Promise<DirectoryItem[]> {
  return getTransport().call("list_directory_with_files", { path })
}

// Hard ceiling for a single attachment, kept in lockstep with the server's
// `UPLOAD_MAX_BYTES`. Aligned with axum's default multipart body limit (and
// with the fact that anything larger won't fit a model context anyway).
export const UPLOAD_MAX_BYTES = 2 * 1024 * 1024

// `btoa` only accepts a binary string, and `String.fromCharCode(...bytes)`
// hits the call-stack limit somewhere around a few hundred KB. Chunk the
// buffer so a 2 MB upload encodes without blowing the stack.
function arrayBufferToBase64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf)
  let binary = ""
  const chunkSize = 0x8000
  for (let i = 0; i < bytes.length; i += chunkSize) {
    const slice = bytes.subarray(i, i + chunkSize) as unknown as number[]
    binary += String.fromCharCode.apply(null, slice)
  }
  return btoa(binary)
}

// i18n_key values the Rust upload layer stamps via `with_i18n` and that
// the frontend branches on. MUST stay in lockstep with the Rust
// constants `UPLOAD_I18N_KEY_TOO_LARGE` / `UPLOAD_I18N_KEY_NOT_A_FILE`
// in `src-tauri/src/app_error.rs`. If either side renames the literal,
// the Rust unit test
// `commands::remote_proxy::tests::upload_i18n_keys_have_expected_values`
// fails — that's the CI tripwire keeping the two languages aligned.
export const UPLOAD_I18N_KEY_TOO_LARGE = "errors.upload.tooLarge"
export const UPLOAD_I18N_KEY_NOT_A_FILE = "errors.upload.notAFile"
export const UPLOAD_I18N_KEY_QUOTA_EXCEEDED = "errors.upload.quotaExceeded"

// Structured error thrown by the upload functions when an attachment
// would be empty (0 bytes). Callers should recognize it and silently
// skip — attaching a zero-byte ResourceLink would be a no-op for the
// agent and a confusing chip in the UI. Modeled as a real `Error`
// subclass so it carries a proper stack trace through async pipelines
// (a bare object literal would lose that), and so existing `instanceof
// Error` catch-rendering in the UI doesn't see an undefined `message`.
//
// The `code` field is preserved for the legacy duck-type check path —
// any callers still inspecting `.code === UPLOAD_ERROR_EMPTY` continue
// to work, but new code should rely on `isEmptyAttachmentError` or
// `instanceof EmptyAttachmentError`.
export const UPLOAD_ERROR_EMPTY = "attachment_empty"

export class EmptyAttachmentError extends Error {
  readonly code = UPLOAD_ERROR_EMPTY
  readonly fileName: string

  constructor(fileName: string) {
    super(`Empty file skipped: ${fileName}`)
    this.name = "EmptyAttachmentError"
    this.fileName = fileName
  }
}

export function isEmptyAttachmentError(err: unknown): boolean {
  if (err instanceof EmptyAttachmentError) return true
  // Tolerate the older bare-object shape so anything thrown through an
  // IPC boundary (which strips the class identity) still gets caught.
  return (
    typeof err === "object" &&
    err !== null &&
    "code" in err &&
    (err as { code?: string }).code === UPLOAD_ERROR_EMPTY
  )
}

// Upload a single attachment to the server.
//
// Web mode: streams the file via multipart/form-data to the same origin the
// page was served from. Desktop + remote workspace: routes through the Rust
// `remote_upload_attachment` command, because the webview's `fetch` can't
// hit a plain `http://` remote (mixed-content rules block secure-context
// requests). Returns the server-side absolute path so the caller can attach
// it as a `file://` ResourceLink — identical shape on both transports.
export async function uploadAttachment(
  file: File,
  sessionId?: string | null
): Promise<UploadAttachmentResult> {
  if (file.size === 0) {
    // Skip empty files at the entry — both the web and remote-desktop
    // transports would otherwise dutifully POST a zero-byte multipart part
    // (the server records it under `~/.codeg/uploads/<bucket>/...`), and
    // we'd attach a ResourceLink to an empty file. Throw the sentinel and
    // let the pool's catch block log + continue.
    throw new EmptyAttachmentError(file.name)
  }
  const remoteId = getActiveRemoteConnectionId()
  if (isDesktop() && remoteId !== null) {
    const buf = await file.arrayBuffer()
    // `getShellTransport()` resolves to the local Tauri transport even when
    // a `RemoteDesktopTransport` is configured — we deliberately want the
    // local IPC here, not the proxy, because `remote_upload_attachment`
    // lives on this desktop binary.
    return getShellTransport().call<UploadAttachmentResult>(
      "remote_upload_attachment",
      {
        connectionId: remoteId,
        fileName: file.name,
        mimeType: file.type || null,
        sessionId: sessionId ?? null,
        dataBase64: arrayBufferToBase64(buf),
      }
    )
  }

  const token = getCodegToken()
  const form = new FormData()
  form.append("file", file, file.name)
  if (sessionId) form.append("session_id", sessionId)

  const res = await fetch(`${window.location.origin}/api/upload_attachment`, {
    method: "POST",
    headers: { Authorization: `Bearer ${token}` },
    body: form,
  })
  if (res.status === 401) {
    notifyWebUnauthorized()
    throw new Error("Unauthorized")
  }
  if (!res.ok) {
    const err = await res.json().catch(() => ({
      code: "network_error",
      message: `HTTP ${res.status}`,
    }))
    throw err
  }
  return res.json()
}

// Upload a file picked from the desktop machine's filesystem to the remote
// codeg-server bound to the current window. The Tauri-native drag-drop event
// hands us OS paths (not `File` objects), so we read the bytes via Rust,
// then reuse the same `remote_upload_attachment` channel. Only callable from
// a window that has a remote workspace attached; non-remote callers should
// continue to use `appendResourceAttachments` with the local path directly.
export async function uploadLocalPathToRemote(
  path: string,
  sessionId?: string | null
): Promise<UploadAttachmentResult> {
  const remoteId = getActiveRemoteConnectionId()
  if (remoteId === null) {
    throw new Error(
      "uploadLocalPathToRemote requires an active remote workspace"
    )
  }
  const shell = getShellTransport()
  const file = await shell.call<{
    fileName: string
    mimeType: string | null
    size: number
    dataBase64: string
  }>("read_local_file_for_upload", { path })
  if (file.size === 0) {
    // Mirror the `uploadAttachment` empty-file guard. The Rust side
    // already read the bytes, so we've paid the cost — drop on the
    // floor here rather than send a zero-byte multipart upstream.
    throw new EmptyAttachmentError(file.fileName)
  }
  return shell.call<UploadAttachmentResult>("remote_upload_attachment", {
    connectionId: remoteId,
    fileName: file.fileName,
    mimeType: file.mimeType,
    sessionId: sessionId ?? null,
    dataBase64: file.dataBase64,
  })
}

// ─── Workspace file upload / download ───
//
// Issue #179: in server mode the user has no native file dialog, so the
// file-tree context menu offers explicit upload + download actions
// against these endpoints. The local desktop build (no remote) uses OS
// dialogs instead, so these helpers throw there. A remote-desktop
// window is a Tauri runtime but its file ops must target the remote
// host — it goes through the `remote_*_workspace_*` proxy commands.

export interface UploadWorkspaceFileResult {
  path: string
  name: string
  size: number
}

/**
 * Returns true when the current window can drive these helpers. Both
 * pure-web mode and remote-desktop mode qualify; only a local-desktop
 * Tauri window (no remote binding) is rejected, because it has its own
 * native file dialogs and these helpers would just be the wrong tool.
 */
function isWorkspaceFileApiAvailable(): boolean {
  return !isDesktop() || isRemoteDesktopMode()
}

function assertWorkspaceFileApiAvailable(action: string): void {
  if (!isWorkspaceFileApiAvailable()) {
    throw new Error(
      `${action} is not available in local desktop mode; use the OS file dialogs instead.`
    )
  }
}

async function workspaceFileFetch(
  endpoint: string,
  body: BodyInit,
  isMultipart: boolean
): Promise<Response> {
  const token = getCodegToken()
  const headers: Record<string, string> = {
    Authorization: `Bearer ${token}`,
  }
  if (!isMultipart) {
    headers["Content-Type"] = "application/json"
  }
  const res = await fetch(`${window.location.origin}/api/${endpoint}`, {
    method: "POST",
    headers,
    body,
  })
  if (res.status === 401) {
    notifyWebUnauthorized()
    throw new Error("Unauthorized")
  }
  if (!res.ok) {
    const err = await res.json().catch(() => ({
      code: "network_error",
      message: `HTTP ${res.status}`,
    }))
    throw err
  }
  return res
}

export interface UploadWorkspaceFileArgs {
  rootPath: string
  targetPath: string
  file: File
  relativePath?: string | null
  signal?: AbortSignal
  /**
   * Byte-level progress callback fired on the request body as the
   * browser uploads it. `total` may equal 0 on streams where the size
   * is not pre-computable (rare for `File` objects but possible for
   * `Blob` slices) — callers should treat 0 as "unknown" rather than
   * "complete".
   */
  onProgress?: (loaded: number, total: number) => void
}

/**
 * Upload one workspace file. Two transports:
 *
 *   - **Web** — `XMLHttpRequest` direct to `/api/upload_workspace_file`,
 *     so we get byte-level upload progress and `AbortSignal` honoring.
 *   - **Remote desktop** — uses `uploadWorkspaceLocalPathsToRemote` with
 *     native file paths. Browser `File` objects are intentionally rejected
 *     there because Tauri IPC is not a streaming binary transport.
 *
 * Empty files are allowed: a workspace legitimately contains zero-byte
 * placeholders (`.gitkeep`, `__init__.py`). The chat-attachment uploader
 * still rejects them because feeding nothing to an LLM is meaningless,
 * but here we forward whatever the user picked.
 */
export async function uploadWorkspaceFile(
  args: UploadWorkspaceFileArgs
): Promise<UploadWorkspaceFileResult> {
  assertWorkspaceFileApiAvailable("uploadWorkspaceFile")

  if (isRemoteDesktopMode()) {
    throw new Error(
      "uploadWorkspaceFile requires browser File input; use uploadWorkspaceLocalPathsToRemote in remote desktop mode"
    )
  }

  return new Promise<UploadWorkspaceFileResult>((resolve, reject) => {
    const token = getCodegToken()
    const xhr = new XMLHttpRequest()
    xhr.open("POST", `${window.location.origin}/api/upload_workspace_file`)
    xhr.setRequestHeader("Authorization", `Bearer ${token}`)

    if (args.onProgress) {
      const onProgress = args.onProgress
      xhr.upload.onprogress = (event) => {
        if (event.lengthComputable) {
          onProgress(event.loaded, event.total)
        }
      }
    }

    xhr.onload = () => {
      if (xhr.status === 401) {
        notifyWebUnauthorized()
        reject(new Error("Unauthorized"))
        return
      }
      if (xhr.status < 200 || xhr.status >= 300) {
        let err: unknown
        try {
          err = JSON.parse(xhr.responseText) as unknown
        } catch {
          err = {
            code: "network_error",
            message: `HTTP ${xhr.status}`,
          }
        }
        reject(err)
        return
      }
      try {
        resolve(JSON.parse(xhr.responseText) as UploadWorkspaceFileResult)
      } catch (e) {
        reject(e instanceof Error ? e : new Error(String(e)))
      }
    }
    xhr.onerror = () => reject(new Error("Network error during upload"))
    xhr.onabort = () => reject(new DOMException("Upload aborted", "AbortError"))

    // Wire the AbortSignal. The listener is removed on `loadend` so a
    // long-lived controller shared across many sequential uploads
    // doesn't leak listeners (the parent XHR will have been GC'd
    // anyway, but the listener kept a strong reference until then).
    if (args.signal) {
      if (args.signal.aborted) {
        xhr.abort()
        return
      }
      const signal = args.signal
      const onAbort = () => xhr.abort()
      signal.addEventListener("abort", onAbort, { once: true })
      xhr.addEventListener("loadend", () => {
        signal.removeEventListener("abort", onAbort)
      })
    }

    // Order matters: the backend reads text fields before the `file`
    // stream so it can resolve the destination before any bytes land.
    const form = new FormData()
    form.append("root_path", args.rootPath)
    form.append("target_path", args.targetPath)
    if (args.relativePath) {
      form.append("relative_path", args.relativePath)
    }
    form.append("file", args.file, args.file.name)
    xhr.send(form)
  })
}

export interface RemoteWorkspaceUploadPathEntry {
  localPath: string
  relativePath?: string | null
}

export interface RemoteWorkspaceUploadPathsResult {
  transferId: string
  files: UploadWorkspaceFileResult[]
  bytes: number
}

export async function uploadWorkspaceLocalPathsToRemote(args: {
  rootPath: string
  targetPath: string
  entries: RemoteWorkspaceUploadPathEntry[]
}): Promise<RemoteWorkspaceUploadPathsResult> {
  const connectionId = getActiveRemoteConnectionId()
  if (connectionId === null) {
    throw new Error(
      "uploadWorkspaceLocalPathsToRemote: no active remote connection"
    )
  }
  try {
    return await getShellTransport().call<RemoteWorkspaceUploadPathsResult>(
      "remote_upload_workspace_paths",
      {
        connectionId,
        rootPath: args.rootPath,
        targetPath: args.targetPath,
        entries: args.entries,
      }
    )
  } catch (err) {
    if (isRemoteAuthenticationFailed(err)) {
      notifyRemoteDesktopUnauthorized()
    }
    throw err
  }
}

export interface WorkspaceTransferProgress {
  transferId: string
  direction: "upload" | "download"
  loaded: number
  total: number | null
  state: "running" | "done" | "cancelled" | "error"
  path?: string | null
  error?: string | null
}

export async function listenWorkspaceTransferProgress(
  handler: (event: WorkspaceTransferProgress) => void
): Promise<() => void> {
  if (!isDesktop()) return () => {}
  const { listen } = await import("@tauri-apps/api/event")
  return listen<WorkspaceTransferProgress>(
    "workspace://transfer-progress",
    (event) => handler(event.payload)
  )
}

export async function cancelWorkspaceTransfer(
  transferId: string
): Promise<boolean> {
  return getShellTransport().call<boolean>("remote_cancel_workspace_transfer", {
    transferId,
  })
}

function isRemoteAuthenticationFailed(err: unknown): boolean {
  return (
    typeof err === "object" &&
    err !== null &&
    "code" in err &&
    (err as { code?: unknown }).code === "authentication_failed"
  )
}

export function isUploadAbortError(err: unknown): boolean {
  return err instanceof DOMException && err.name === "AbortError"
}

interface WorkspaceDownloadTicket {
  ticket: string
  url: string
  filename: string
  expiresAt: number
}

type WorkspaceDownloadKind = "file" | "dir"

function openBrowserDownloadUrl(url: string, filename: string): void {
  const anchor = document.createElement("a")
  anchor.href = url
  anchor.download = filename
  anchor.rel = "noopener"
  document.body.appendChild(anchor)
  anchor.click()
  anchor.remove()
}

async function createWorkspaceDownloadTicket(args: {
  rootPath: string
  path: string
  kind: WorkspaceDownloadKind
}): Promise<WorkspaceDownloadTicket> {
  const res = await workspaceFileFetch(
    "workspace_download_ticket",
    JSON.stringify(args),
    false
  )
  return res.json()
}

/**
 * Sentinel return from a remote-desktop download path when the user
 * cancels the Tauri save dialog. Web mode never returns this — the
 * browser owns the download manager and there's no per-call cancel.
 */
export const WORKSPACE_DOWNLOAD_CANCELLED = "cancelled" as const

export type WorkspaceDownloadResult =
  | { status: "started" }
  | { status: "done"; savedPath?: string; bytes?: number; transferId?: string }
  | { status: typeof WORKSPACE_DOWNLOAD_CANCELLED }

export async function downloadWorkspaceFile(
  rootPath: string,
  path: string,
  fileName: string
): Promise<WorkspaceDownloadResult> {
  assertWorkspaceFileApiAvailable("downloadWorkspaceFile")

  if (isRemoteDesktopMode()) {
    return downloadWorkspaceViaRemoteProxy({
      endpoint: "remote_download_workspace_file",
      rootPath,
      path,
      suggestedName: fileName,
    })
  }

  const ticket = await createWorkspaceDownloadTicket({
    rootPath,
    path,
    kind: "file",
  })
  openBrowserDownloadUrl(ticket.url, ticket.filename || fileName)
  return { status: "started" }
}

export async function downloadWorkspaceDir(
  rootPath: string,
  path: string,
  dirName: string
): Promise<WorkspaceDownloadResult> {
  assertWorkspaceFileApiAvailable("downloadWorkspaceDir")

  if (isRemoteDesktopMode()) {
    return downloadWorkspaceViaRemoteProxy({
      endpoint: "remote_download_workspace_dir",
      rootPath,
      path,
      suggestedName: `${dirName}.zip`,
    })
  }

  const ticket = await createWorkspaceDownloadTicket({
    rootPath,
    path,
    kind: "dir",
  })
  openBrowserDownloadUrl(ticket.url, ticket.filename || `${dirName}.zip`)
  return { status: "started" }
}

async function downloadWorkspaceViaRemoteProxy(opts: {
  endpoint: "remote_download_workspace_file" | "remote_download_workspace_dir"
  rootPath: string
  path: string
  suggestedName: string
}): Promise<WorkspaceDownloadResult> {
  const connectionId = getActiveRemoteConnectionId()
  if (connectionId === null) {
    throw new Error(
      "downloadWorkspaceFile (remote): no active remote connection"
    )
  }
  const { save } = await import("@tauri-apps/plugin-dialog")
  const savePath = await save({ defaultPath: opts.suggestedName })
  if (!savePath) {
    return { status: WORKSPACE_DOWNLOAD_CANCELLED }
  }
  const { invoke } = await import("@tauri-apps/api/core")
  let result: { transferId: string; bytes: number }
  try {
    result = await invoke<{ transferId: string; bytes: number }>(
      opts.endpoint,
      {
        connectionId,
        rootPath: opts.rootPath,
        path: opts.path,
        savePath,
      }
    )
  } catch (err) {
    if (isRemoteAuthenticationFailed(err)) {
      notifyRemoteDesktopUnauthorized()
    }
    throw err
  }
  return {
    status: "done",
    savedPath: savePath,
    bytes: result.bytes,
    transferId: result.transferId,
  }
}

// File tree and git log commands

export async function getFileTree(
  path: string,
  maxDepth?: number
): Promise<FileTreeNode[]> {
  return getTransport().call("get_file_tree", {
    path,
    maxDepth: maxDepth ?? null,
  })
}

export async function startWorkspaceStateStream(
  rootPath: string
): Promise<WorkspaceSnapshotResponse> {
  return getTransport().call("start_workspace_state_stream", { rootPath })
}

export async function stopWorkspaceStateStream(
  rootPath: string
): Promise<void> {
  return getTransport().call("stop_workspace_state_stream", { rootPath })
}

export async function getWorkspaceSnapshot(
  rootPath: string,
  sinceSeq?: number
): Promise<WorkspaceSnapshotResponse> {
  return getTransport().call("get_workspace_snapshot", {
    rootPath,
    sinceSeq: sinceSeq ?? null,
  })
}

export async function readFileBase64(
  path: string,
  maxBytes?: number
): Promise<string> {
  return getTransport().call("read_file_base64", {
    path,
    maxBytes: maxBytes ?? null,
  })
}

// Workspace-confined base64 read: `path` is relative to `rootPath` and is
// canonicalized server-side (resolving symlinks), so it can never read outside
// the workspace. Used by the HTML preview to inline local sub-resources safely.
export async function readWorkspaceFileBase64(
  rootPath: string,
  path: string,
  maxBytes?: number
): Promise<string> {
  return getTransport().call("read_workspace_file_base64", {
    rootPath,
    path,
    maxBytes: maxBytes ?? null,
  })
}

export async function readFilePreview(
  rootPath: string,
  path: string
): Promise<FilePreviewContent> {
  return getTransport().call("read_file_preview", { rootPath, path })
}

export async function readFileForEdit(
  rootPath: string,
  path: string
): Promise<FileEditContent> {
  return getTransport().call("read_file_for_edit", { rootPath, path })
}

export async function saveFileContent(
  rootPath: string,
  path: string,
  content: string,
  expectedEtag?: string | null
): Promise<FileSaveResult> {
  return getTransport().call("save_file_content", {
    rootPath,
    path,
    content,
    expectedEtag: expectedEtag ?? null,
  })
}

export async function saveFileCopy(
  rootPath: string,
  path: string,
  content: string
): Promise<FileSaveResult> {
  return getTransport().call("save_file_copy", {
    rootPath,
    path,
    content,
  })
}

export async function renameFileTreeEntry(
  rootPath: string,
  path: string,
  newName: string
): Promise<string> {
  return getTransport().call("rename_file_tree_entry", {
    rootPath,
    path,
    newName,
  })
}

export async function deleteFileTreeEntry(
  rootPath: string,
  path: string
): Promise<void> {
  return getTransport().call("delete_file_tree_entry", { rootPath, path })
}

export async function createFileTreeEntry(
  rootPath: string,
  path: string,
  name: string,
  kind: "file" | "dir"
): Promise<string> {
  return getTransport().call("create_file_tree_entry", {
    rootPath,
    path,
    name,
    kind,
  })
}

export async function gitLog(
  path: string,
  limit?: number,
  branch?: string,
  remote?: string
): Promise<GitLogResult> {
  return getTransport().call("git_log", {
    path,
    limit: limit ?? null,
    branch: branch ?? null,
    remote: remote ?? null,
  })
}

export async function gitCommitBranches(
  path: string,
  commit: string
): Promise<string[]> {
  return getTransport().call("git_commit_branches", { path, commit })
}

export async function gitReset(
  path: string,
  commit: string,
  mode: GitResetMode
): Promise<void> {
  return getTransport().call("git_reset", { path, commit, mode })
}

// Terminal commands

export async function terminalSpawn(
  workingDir: string,
  shell?: string,
  initialCommand?: string,
  terminalId?: string
): Promise<string> {
  return getTransport().call("terminal_spawn", {
    workingDir,
    shell: shell ?? null,
    initialCommand: initialCommand ?? null,
    terminalId: terminalId ?? null,
  })
}

export async function terminalWrite(
  terminalId: string,
  data: string
): Promise<void> {
  return getTransport().call("terminal_write", { terminalId, data })
}

export async function terminalResize(
  terminalId: string,
  cols: number,
  rows: number
): Promise<void> {
  return getTransport().call("terminal_resize", { terminalId, cols, rows })
}

export async function terminalKill(terminalId: string): Promise<void> {
  return getTransport().call("terminal_kill", { terminalId })
}

export async function terminalList(): Promise<TerminalInfo[]> {
  return getTransport().call("terminal_list")
}

// ── Web Server Management ──

export interface WebServerInfo {
  port: number
  token: string
  addresses: string[]
}

export async function startWebServer(params?: {
  port?: number
  host?: string
  token?: string | null
}): Promise<WebServerInfo> {
  return getTransport().call("start_web_server", {
    port: params?.port ?? null,
    host: params?.host ?? null,
    token: params?.token ?? null,
  })
}

export async function stopWebServer(): Promise<void> {
  return getTransport().call("stop_web_server")
}

export async function getWebServerStatus(): Promise<WebServerInfo | null> {
  return getTransport().call("get_web_server_status")
}

export interface WebServiceConfig {
  token: string | null
  port: number | null
  autoStart: boolean
}

export async function getWebServiceConfig(): Promise<WebServiceConfig> {
  return getTransport().call("get_web_service_config")
}

export async function updateWebServiceConfig(
  config: WebServiceConfig
): Promise<WebServiceConfig> {
  return getTransport().call("update_web_service_config", { config })
}

export type WebServicePortState = "free" | "occupied" | "unknown"

export interface WebServicePortProbe {
  port: number
  state: WebServicePortState
}

export async function probeWebServicePort(
  port?: number
): Promise<WebServicePortProbe> {
  return getTransport().call("probe_web_service_port", {
    port: port ?? null,
  })
}

// ─── Chat Channels ───

export async function listChatChannels(): Promise<ChatChannelInfo[]> {
  return getTransport().call("list_chat_channels")
}

export async function createChatChannel(params: {
  name: string
  channelType: string
  configJson: string
  enabled: boolean
  dailyReportEnabled: boolean
  dailyReportTime?: string | null
}): Promise<ChatChannelInfo> {
  return getTransport().call("create_chat_channel", {
    name: params.name,
    channelType: params.channelType,
    configJson: params.configJson,
    enabled: params.enabled,
    dailyReportEnabled: params.dailyReportEnabled,
    dailyReportTime: params.dailyReportTime ?? null,
  })
}

export async function updateChatChannel(params: {
  id: number
  name?: string | null
  enabled?: boolean | null
  configJson?: string | null
  eventFilterJson?: string | null
  dailyReportEnabled?: boolean | null
  dailyReportTime?: string | null
}): Promise<ChatChannelInfo> {
  return getTransport().call("update_chat_channel", {
    id: params.id,
    name: params.name ?? null,
    enabled: params.enabled ?? null,
    configJson: params.configJson ?? null,
    eventFilterJson: params.eventFilterJson ?? null,
    dailyReportEnabled: params.dailyReportEnabled ?? null,
    dailyReportTime: params.dailyReportTime ?? null,
  })
}

export async function deleteChatChannel(id: number): Promise<void> {
  return getTransport().call("delete_chat_channel", { id })
}

export async function saveChatChannelToken(
  channelId: number,
  token: string
): Promise<void> {
  return getTransport().call("save_chat_channel_token", { channelId, token })
}

export async function getChatChannelHasToken(
  channelId: number
): Promise<boolean> {
  return getTransport().call("get_chat_channel_has_token", { channelId })
}

export async function deleteChatChannelToken(channelId: number): Promise<void> {
  return getTransport().call("delete_chat_channel_token", { channelId })
}

export async function connectChatChannel(id: number): Promise<void> {
  return getTransport().call("connect_chat_channel", { id })
}

export async function disconnectChatChannel(id: number): Promise<void> {
  return getTransport().call("disconnect_chat_channel", { id })
}

export async function testChatChannel(id: number): Promise<void> {
  return getTransport().call("test_chat_channel", { id })
}

export async function getChatChannelStatus(): Promise<ChannelStatusInfo[]> {
  return getTransport().call("get_chat_channel_status")
}

export async function listChatChannelMessages(params: {
  channelId: number
  limit?: number
  offset?: number
}): Promise<ChatChannelMessageLog[]> {
  return getTransport().call("list_chat_channel_messages", {
    channelId: params.channelId,
    limit: params.limit ?? null,
    offset: params.offset ?? null,
  })
}

export async function getChatCommandPrefix(): Promise<string> {
  return getTransport().call("get_chat_command_prefix")
}

export async function setChatCommandPrefix(prefix: string): Promise<void> {
  return getTransport().call("set_chat_command_prefix", { prefix })
}

export async function getChatEventFilter(): Promise<string[] | null> {
  return getTransport().call("get_chat_event_filter")
}

export async function setChatEventFilter(
  filter: string[] | null
): Promise<void> {
  return getTransport().call("set_chat_event_filter", { filter })
}

export async function getChatEventWebhooks(): Promise<WebhookConfig[]> {
  return getTransport().call("get_chat_event_webhooks")
}

export async function setChatEventWebhooks(
  webhooks: WebhookConfig[]
): Promise<void> {
  return getTransport().call("set_chat_event_webhooks", { webhooks })
}

export async function getChatMessageLanguage(): Promise<string> {
  return getTransport().call("get_chat_message_language")
}

export async function setChatMessageLanguage(language: string): Promise<void> {
  return getTransport().call("set_chat_message_language", { language })
}

// ─── WeChat QR Code Auth ───

export async function weixinGetQrcode(): Promise<{
  qrcode_id: string
  qrcode_img_content: string
}> {
  return getTransport().call("weixin_get_qrcode")
}

export async function weixinCheckQrcode(
  channelId: number,
  qrcode: string
): Promise<{
  status: string
}> {
  return getTransport().call("weixin_check_qrcode", { channelId, qrcode })
}

// ---------------------------------------------------------------------------
// Model Providers
// ---------------------------------------------------------------------------

export async function listModelProviders(): Promise<ModelProviderInfo[]> {
  return getTransport().call("list_model_providers")
}

export async function createModelProvider(params: {
  name: string
  apiUrl: string
  apiKey: string
  agentType: string
  model?: string | null
}): Promise<ModelProviderInfo> {
  return getTransport().call("create_model_provider", {
    name: params.name,
    apiUrl: params.apiUrl,
    apiKey: params.apiKey,
    agentType: params.agentType,
    model: params.model ?? null,
  })
}

export async function updateModelProvider(params: {
  id: number
  name?: string | null
  apiUrl?: string | null
  apiKey?: string | null
  agentType?: string | null
  model?: string | null
}): Promise<ModelProviderInfo> {
  return getTransport().call("update_model_provider", {
    id: params.id,
    name: params.name ?? null,
    apiUrl: params.apiUrl ?? null,
    apiKey: params.apiKey ?? null,
    agentType: params.agentType ?? null,
    model: params.model ?? null,
  })
}

export async function deleteModelProvider(id: number): Promise<void> {
  return getTransport().call("delete_model_provider", { id })
}

// ─── Delegation settings ───────────────────────────────────────────────

export interface DelegationSettings {
  enabled: boolean
  depth_limit: number
  /** Per-parent byte budget (in MB) for the broker's in-memory cache of
   * completed sub-agent result text. `0` = unlimited. */
  completed_cache_max_mb: number
  /** Optional per-agent overrides applied when codeg-mcp spawns a subagent.
   * Keyed by `agent_type`. Missing entries mean "use agent defaults." */
  agent_defaults?: Partial<Record<AgentType, AgentDelegationDefaults>>
}

export async function getDelegationSettings(): Promise<DelegationSettings> {
  return getTransport().call("get_delegation_settings")
}

export async function setDelegationSettings(
  settings: DelegationSettings
): Promise<DelegationSettings> {
  return getTransport().call("set_delegation_settings", { settings })
}

// ─── Live feedback settings + submit ───────────────────────────────────

/** Mirror of Rust `FeedbackSettings`. */
export interface FeedbackSettings {
  enabled: boolean
}

export async function getFeedbackSettings(): Promise<FeedbackSettings> {
  return getTransport().call("get_feedback_settings")
}

export async function setFeedbackSettings(
  settings: FeedbackSettings
): Promise<FeedbackSettings> {
  return getTransport().call("set_feedback_settings", { settings })
}

/**
 * Submit a live-feedback note to a running connection (the `check_user_feedback`
 * steering path). Returns the stored note (it also arrives via the
 * `feedback_submitted` event). Rejects when no turn is in flight — callers
 * detect that with `isNoActiveTurnRejection` and fall back to a normal prompt.
 */
export async function submitSessionFeedback(
  connectionId: string,
  text: string
): Promise<FeedbackItem> {
  return getTransport().call("submit_session_feedback", {
    connectionId,
    text,
  })
}

/** Live probe — opens a transient ACP connection to `agent_type`, reads what
 * it advertises (modes / config_options), and tears down. Used by the
 * delegation-settings UI so the option set on screen matches exactly what
 * codeg-mcp will receive when a subagent is spawned for delegation.
 *
 * Does NOT touch chat-side `selectorsCache` or `localStorage` preferences. */
export async function describeAgentOptions(
  agentType: AgentType,
  workingDir?: string | null
): Promise<AgentOptionsSnapshot> {
  // The backend probe has its own 60s timeout (`ConnectionManager::
  // probe_agent_options`) plus 500ms grace + poll/serialization
  // overhead. The default transport timeout of 60s would race with
  // that and surface "Request timed out" before the backend can
  // return `ProbeTimedOut`. 70s gives the backend a clean margin to
  // produce its structured error.
  return getTransport().call(
    "acp_describe_agent_options",
    {
      agentType,
      workingDir: workingDir ?? null,
    },
    { timeoutMs: 70_000 }
  )
}

// ───────────────────────────────────────────────────────────────────────────
// Backup & restore
// ───────────────────────────────────────────────────────────────────────────

export interface BackupManifestEntry {
  path: string
  size: number
  sha256: string
}

export interface BackupManifest {
  formatVersion: number
  kind: string
  createdAt: string
  appVersion: string
  latestMigration: string
  runtime: string
  includesExternalTranscripts: boolean
  includesSecrets: boolean
  entries: BackupManifestEntry[]
}

export type BackupPhase =
  | "snapshotting"
  | "archiving"
  | "encrypting"
  | "decrypting"
  | "extracting"
  | "verifying"
  | "swapping"
  | "done"
  | "cancelled"
  | "error"

export interface BackupProgress {
  opId: string
  phase: BackupPhase
  processedBytes: number
  totalBytes: number | null
  currentPath?: string | null
  error?: string | null
}

export interface BackupPreview {
  encrypted: boolean
  needsPassphrase: boolean
  manifest?: BackupManifest | null
  compatible: boolean
  rejectReason?: string | null
}

export interface StagedRestore {
  stagingDir: string
  manifest: BackupManifest
  restoredExternalPath?: string | null
  skippedConflicts: string[]
}

/** Where (if anywhere) external agent transcripts are restored. */
export type ExternalRestoreMode =
  | { mode: "skip" }
  | { mode: "side_location" }
  | { mode: "original_locations"; on_conflict: "overwrite" | "skip_existing" }

export interface BackupExportOptions {
  includeExternalTranscripts: boolean
  passphrase?: string | null
}

/**
 * Subscribe to backup/restore progress. Works in both runtimes: the backend
 * emits through the unified event bridge (Tauri webview + WS broadcaster).
 */
export async function listenBackupProgress(
  handler: (event: BackupProgress) => void
): Promise<() => void> {
  return getTransport().subscribe<BackupProgress>("backup://progress", handler)
}

export async function cancelBackup(opId: string): Promise<boolean> {
  return getTransport().call<boolean>("backup_cancel", { opId })
}

/** Desktop export: native save dialog → write the archive to the chosen path. */
export async function exportBackupDesktop(
  opts: BackupExportOptions
): Promise<BackupManifest | null> {
  const { save } = await import("@tauri-apps/plugin-dialog")
  const encrypted = !!opts.passphrase
  const ext = encrypted ? "codegbak" : "codeg.zip"
  const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-")
  const destPath = await save({
    defaultPath: `codeg-backup-${stamp}.${ext}`,
    filters: [{ name: "Codeg backup", extensions: [ext] }],
  })
  if (!destPath) return null
  return getTransport().call<BackupManifest>("backup_create", {
    options: {
      includeExternalTranscripts: opts.includeExternalTranscripts,
      passphrase: opts.passphrase ?? null,
    },
    destPath,
  })
}

// Backup create/inspect/stage can legitimately run far longer than the
// default 60s web transport timeout (large archives, encrypted decrypt,
// extract+verify). Use a very generous client bound so the UI doesn't abort
// while the backend is still working (and possibly committing a restore).
const BACKUP_LONG_CALL_TIMEOUT_MS = 60 * 60_000

/** Web export: build server-side, then trigger a browser download via ticket. */
export async function exportBackupWeb(
  opts: BackupExportOptions
): Promise<void> {
  const ticket = await getTransport().call<{ url: string; filename: string }>(
    "backup_create_ticket",
    {
      includeExternalTranscripts: opts.includeExternalTranscripts,
      passphrase: opts.passphrase ?? null,
    },
    { timeoutMs: BACKUP_LONG_CALL_TIMEOUT_MS }
  )
  const a = document.createElement("a")
  a.href = `${window.location.origin}${ticket.url}`
  a.download = ticket.filename
  document.body.appendChild(a)
  a.click()
  a.remove()
}

/** Web restore step 1: upload the archive once; returns an opaque upload id. */
export async function uploadBackupWeb(
  file: File,
  onProgress?: (loaded: number, total: number) => void
): Promise<string> {
  return new Promise<string>((resolve, reject) => {
    const token = getCodegToken()
    const xhr = new XMLHttpRequest()
    xhr.open("POST", `${window.location.origin}/api/backup_upload`)
    xhr.setRequestHeader("Authorization", `Bearer ${token}`)
    if (onProgress) {
      xhr.upload.onprogress = (event) => {
        if (event.lengthComputable) onProgress(event.loaded, event.total)
      }
    }
    xhr.onload = () => {
      if (xhr.status === 401) {
        notifyWebUnauthorized()
        reject(new Error("Unauthorized"))
        return
      }
      if (xhr.status < 200 || xhr.status >= 300) {
        let err: unknown
        try {
          err = JSON.parse(xhr.responseText) as unknown
        } catch {
          err = { code: "network_error", message: `HTTP ${xhr.status}` }
        }
        reject(err)
        return
      }
      try {
        const res = JSON.parse(xhr.responseText) as { uploadId: string }
        resolve(res.uploadId)
      } catch (e) {
        reject(e instanceof Error ? e : new Error(String(e)))
      }
    }
    xhr.onerror = () => reject(new Error("Network error during upload"))
    const form = new FormData()
    form.append("file", file, file.name)
    xhr.send(form)
  })
}

/** Validate a backup (desktop: by path). */
export async function inspectBackupDesktop(
  srcPath: string,
  passphrase?: string | null
): Promise<BackupPreview> {
  return getTransport().call<BackupPreview>("backup_inspect", {
    srcPath,
    passphrase: passphrase ?? null,
  })
}

/** Validate a backup (web: by upload id). */
export async function inspectBackupWeb(
  uploadId: string,
  passphrase?: string | null
): Promise<BackupPreview> {
  return getTransport().call<BackupPreview>(
    "backup_inspect",
    {
      uploadId,
      passphrase: passphrase ?? null,
    },
    { timeoutMs: BACKUP_LONG_CALL_TIMEOUT_MS }
  )
}

/** Stage a restore (desktop: by path). Applied on next app start. */
export async function stageRestoreDesktop(args: {
  srcPath: string
  passphrase?: string | null
  externalMode?: ExternalRestoreMode | null
}): Promise<StagedRestore> {
  return getTransport().call<StagedRestore>("backup_restore_stage", {
    srcPath: args.srcPath,
    passphrase: args.passphrase ?? null,
    externalMode: args.externalMode ?? null,
  })
}

export interface StageRestoreWebResult {
  needsRestart: boolean
  restartDelayMs: number
  staged: StagedRestore
}

/** Stage a restore (web: by upload id). Applied on next server start. */
export async function stageRestoreWeb(args: {
  uploadId: string
  passphrase?: string | null
  externalMode?: ExternalRestoreMode | null
}): Promise<StageRestoreWebResult> {
  return getTransport().call<StageRestoreWebResult>(
    "backup_restore_stage",
    {
      uploadId: args.uploadId,
      passphrase: args.passphrase ?? null,
      externalMode: args.externalMode ?? null,
    },
    { timeoutMs: BACKUP_LONG_CALL_TIMEOUT_MS }
  )
}

export interface ExternalConflict {
  agent: string
  archivePath: string
  targetPath: string
  targetSize?: number | null
}

/** Scan a backup for external transcripts whose live target already exists. */
export async function scanExternalConflictsDesktop(
  srcPath: string,
  passphrase?: string | null
): Promise<ExternalConflict[]> {
  return getTransport().call<ExternalConflict[]>(
    "backup_scan_external_conflicts",
    { srcPath, passphrase: passphrase ?? null }
  )
}

export async function scanExternalConflictsWeb(
  uploadId: string,
  passphrase?: string | null
): Promise<ExternalConflict[]> {
  return getTransport().call<ExternalConflict[]>(
    "backup_scan_external_conflicts",
    { uploadId, passphrase: passphrase ?? null },
    { timeoutMs: BACKUP_LONG_CALL_TIMEOUT_MS }
  )
}
