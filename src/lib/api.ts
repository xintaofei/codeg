import {
  getActiveRemoteConnectionId,
  getShellTransport,
  getTransport,
  isDesktop,
} from "./transport"
import { getCodegToken, redirectToCodegLogin } from "./transport/web-auth"
import { getCurrentEffectiveAppLocale } from "./i18n"
import type { FolderThemeColor } from "./theme-presets"
import type {
  AgentType,
  ConversationSummary,
  ConversationDetail,
  DbConversationDetail,
  FolderInfo,
  AgentStats,
  SidebarData,
  ConnectionInfo,
  LiveSessionSnapshot,
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
  DbConversationSummary,
  ImportResult,
  OpenedTab,
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
  sessionId?: string
): Promise<string> {
  return getTransport().call("acp_connect", {
    agentType,
    workingDir: workingDir ?? null,
    sessionId: sessionId ?? null,
  })
}

export async function acpPrompt(
  connectionId: string,
  blocks: PromptInputBlock[],
  folderId: number | null = null,
  conversationId: number | null = null
): Promise<void> {
  return getTransport().call("acp_prompt", {
    connectionId,
    blocks,
    folderId,
    conversationId,
  })
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
  return getTransport().call("acp_fork", { connectionId })
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
  taskId: string
): Promise<void> {
  return getTransport().call("acp_download_agent_binary", { agentType, taskId })
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
  cleanFirst: boolean = false
): Promise<string> {
  return getTransport().call("acp_prepare_npx_agent", {
    agentType,
    registryVersion: registryVersion ?? null,
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
}): Promise<DbConversationSummary[]> {
  return getTransport().call("list_all_conversations", {
    folderIds: params?.folder_ids ?? null,
    agentType: params?.agent_type ?? null,
    search: params?.search ?? null,
    sortBy: params?.sort_by ?? null,
    status: params?.status ?? null,
  })
}

export async function listOpenedTabs(): Promise<OpenedTab[]> {
  return getTransport().call("list_opened_tabs")
}

export async function saveOpenedTabs(items: OpenedTab[]): Promise<void> {
  return getTransport().call("save_opened_tabs", { items })
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
    redirectToCodegLogin()
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
  agentTypes: string[]
}): Promise<ModelProviderInfo> {
  return getTransport().call("create_model_provider", params)
}

export async function updateModelProvider(params: {
  id: number
  name?: string | null
  apiUrl?: string | null
  apiKey?: string | null
  agentTypes?: string[] | null
}): Promise<ModelProviderInfo> {
  return getTransport().call("update_model_provider", {
    id: params.id,
    name: params.name ?? null,
    apiUrl: params.apiUrl ?? null,
    apiKey: params.apiKey ?? null,
    agentTypes: params.agentTypes ?? null,
  })
}

export async function deleteModelProvider(id: number): Promise<void> {
  return getTransport().call("delete_model_provider", { id })
}
