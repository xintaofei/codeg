import { getTransport } from "./transport"
import type {
  AgentType,
  ConversationSummary,
  ConversationDetail,
  DbConversationDetail,
  FolderInfo,
  AgentStats,
  SidebarData,
  ConnectionInfo,
  AcpAgentInfo,
  AcpAgentStatus,
  AgentSkillScope,
  AgentSkillLayout,
  AgentSkillItem,
  AgentSkillsListResult,
  AgentSkillContent,
  FolderHistoryEntry,
  FolderDetail,
  DbConversationSummary,
  ImportResult,
  OpenedConversation,
  GitStatusEntry,
  GitBranchList,
  GitPullResult,
  GitPushResult,
  GitPushInfo,
  GitMergeResult,
  GitRebaseResult,
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
  FilePreviewContent,
  FileEditContent,
  FileSaveResult,
  GitLogResult,
  SystemLanguageSettings,
  SystemProxySettings,
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
  blocks: PromptInputBlock[]
): Promise<void> {
  return getTransport().call("acp_prompt", { connectionId, blocks })
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

export async function acpListConnections(): Promise<ConnectionInfo[]> {
  return getTransport().call("acp_list_connections")
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
  agentType: AgentType
): Promise<void> {
  return getTransport().call("acp_download_agent_binary", { agentType })
}

export async function acpDetectAgentLocalVersion(
  agentType: AgentType
): Promise<string | null> {
  return getTransport().call("acp_detect_agent_local_version", { agentType })
}

export async function acpPrepareNpxAgent(
  agentType: AgentType,
  registryVersion?: string | null
): Promise<string> {
  return getTransport().call("acp_prepare_npx_agent", {
    agentType,
    registryVersion: registryVersion ?? null,
  })
}

export async function acpUninstallAgent(agentType: AgentType): Promise<void> {
  return getTransport().call("acp_uninstall_agent", { agentType })
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

export async function acpReorderAgents(agentTypes: AgentType[]): Promise<void> {
  return getTransport().call("acp_reorder_agents", { agentTypes })
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

export async function listFolderConversations(params: {
  folder_id: number
  agent_type?: AgentType | null
  search?: string | null
  sort_by?: string | null
  status?: string | null
}): Promise<DbConversationSummary[]> {
  return getTransport().call("list_folder_conversations", {
    folderId: params.folder_id,
    agentType: params.agent_type ?? null,
    search: params.search ?? null,
    sortBy: params.sort_by ?? null,
    status: params.status ?? null,
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

export async function saveFolderOpenedConversations(
  folderId: number,
  items: OpenedConversation[]
): Promise<void> {
  return getTransport().call("save_folder_opened_conversations", {
    folderId,
    items,
  })
}

export async function setFolderParentBranch(
  path: string,
  parentBranch: string | null
): Promise<void> {
  return getTransport().call("set_folder_parent_branch", {
    path,
    parentBranch,
  })
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
  credentials?: GitCredentials | null
): Promise<GitPushResult> {
  return getTransport().call("git_push", {
    path,
    remote: remote ?? null,
    credentials: credentials ?? null,
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
  force = false
): Promise<string> {
  return getTransport().call("git_delete_branch", { path, branchName, force })
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
  if (getTransport().isDesktop()) {
    return getTransport().call("open_merge_window", {
      folderId,
      operation,
      upstreamCommit: upstreamCommit ?? null,
    })
  }
  const result = await getTransport().call<{ path: string }>(
    "open_merge_window",
    {
      folderId,
      operation,
      upstreamCommit: upstreamCommit ?? null,
    }
  )
  window.open(result.path, `merge-${folderId}`)
}

export async function openStashWindow(folderId: number): Promise<void> {
  if (getTransport().isDesktop()) {
    return getTransport().call("open_stash_window", { folderId })
  }
  const result = await getTransport().call<{ path: string }>(
    "open_stash_window",
    { folderId }
  )
  window.open(result.path, `stash-${folderId}`)
}

export async function openPushWindow(folderId: number): Promise<void> {
  if (getTransport().isDesktop()) {
    return getTransport().call("open_push_window", { folderId })
  }
  const result = await getTransport().call<{ path: string }>(
    "open_push_window",
    { folderId }
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
  files: string[]
): Promise<GitCommitResult> {
  return getTransport().call("git_commit", { path, message, files })
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

export async function openFolderWindow(
  path: string,
  options?: { newWindow?: boolean }
): Promise<void> {
  if (getTransport().isDesktop()) {
    return getTransport().call("open_folder_window", { path })
  }
  const entry = await getTransport().call<{ id: number }>(
    "open_folder_window",
    { path }
  )
  const url = `/folder?id=${entry.id}`
  if (options?.newWindow) {
    window.open(url, `folder-${entry.id}`)
  } else {
    window.location.href = url
  }
}

export async function openCommitWindow(folderId: number): Promise<void> {
  if (getTransport().isDesktop()) {
    return getTransport().call("open_commit_window", { folderId })
  }
  const result = await getTransport().call<{ path: string }>(
    "open_commit_window",
    { folderId }
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
  if (getTransport().isDesktop()) {
    return getTransport().call("open_settings_window", {
      section: section ?? null,
      agentType: options?.agentType ?? null,
    })
  }
  // Web mode: open in new window
  const result = await getTransport().call<{ path: string }>(
    "open_settings_window",
    {
      section: section ?? null,
      agentType: options?.agentType ?? null,
    }
  )
  window.open(result.path, `settings-${section ?? "general"}`)
}

export async function openProjectBootWindow(source?: string): Promise<void> {
  if (getTransport().isDesktop()) {
    return getTransport().call("open_project_boot_window", { source })
  }
  window.open("/project-boot", "project-boot")
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

export async function listOpenFolders(): Promise<FolderHistoryEntry[]> {
  return getTransport().call("list_open_folders")
}

export async function focusFolderWindow(folderId: number): Promise<void> {
  if (getTransport().isDesktop()) {
    return getTransport().call("focus_folder_window", { folderId })
  }
  // Web mode: open empty string to focus existing named window without reload.
  // If the window doesn't exist (was closed), open the folder page.
  const win = window.open("", `folder-${folderId}`)
  if (
    !win ||
    win.closed ||
    !win.location.href ||
    win.location.href === "about:blank"
  ) {
    window.open(`/folder?id=${folderId}`, `folder-${folderId}`)
  }
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

export async function updateConversationExternalId(
  conversationId: number,
  externalId: string
): Promise<void> {
  return getTransport().call("update_conversation_external_id", {
    conversationId,
    externalId,
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

// Directory browser (for web/server mode)

export async function getHomeDirectory(): Promise<string> {
  return getTransport().call("get_home_directory")
}

export async function listDirectoryEntries(
  path: string
): Promise<DirectoryEntry[]> {
  return getTransport().call("list_directory_entries", { path })
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

export async function startFileTreeWatch(rootPath: string): Promise<void> {
  return getTransport().call("start_file_tree_watch", { rootPath })
}

export async function stopFileTreeWatch(rootPath: string): Promise<void> {
  return getTransport().call("stop_file_tree_watch", { rootPath })
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

// Terminal commands

export async function terminalSpawn(
  workingDir: string,
  initialCommand?: string
): Promise<string> {
  return getTransport().call("terminal_spawn", {
    workingDir,
    initialCommand: initialCommand ?? null,
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
}): Promise<WebServerInfo> {
  return getTransport().call("start_web_server", {
    port: params?.port ?? null,
    host: params?.host ?? null,
  })
}

export async function stopWebServer(): Promise<void> {
  return getTransport().call("stop_web_server")
}

export async function getWebServerStatus(): Promise<WebServerInfo | null> {
  return getTransport().call("get_web_server_status")
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
  token: string,
): Promise<void> {
  return getTransport().call("save_chat_channel_token", { channelId, token })
}

export async function getChatChannelHasToken(
  channelId: number,
): Promise<boolean> {
  return getTransport().call("get_chat_channel_has_token", { channelId })
}

export async function deleteChatChannelToken(
  channelId: number,
): Promise<void> {
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
  filter: string[] | null,
): Promise<void> {
  return getTransport().call("set_chat_event_filter", { filter })
}
