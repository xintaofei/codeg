// Thin HTTP client for the daemon's loopback REST surface (capabilities,
// health, conversations). The remote daemon runs on `127.0.0.1:<port>` of
// the desktop host because we forward the port over SSH; the bearer token
// comes from the bootstrap handshake.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::acp::types::{ForkResultInfo, PromptInputBlock};
use crate::acp::LiveSessionSnapshot;
use crate::commands::folders::{
    FileEditContent, FilePreviewContent, FileSaveResult, GitBranchList, GitCommitResult,
    GitLogResult, GitPullResult, GitPushInfo, GitPushResult, GitRemote, GitStashEntry,
    GitStatusEntry,
};
use crate::models::{AgentType, ConversationDetail, ConversationSummary, GitCredentials};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CapabilitiesResponse {
    pub version: String,
    #[serde(default)]
    pub schema_version: String,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub features: CapabilityFlags,
    #[serde(default)]
    pub server_time: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CapabilityFlags {
    #[serde(default)]
    pub topic_subscribe: bool,
    #[serde(default)]
    pub remote_terminal: bool,
    #[serde(default)]
    pub workspace_watch: bool,
    #[serde(default)]
    pub git_operations: bool,
    #[serde(default)]
    pub file_editing: bool,
}

pub struct DaemonClient {
    base_url: String,
    bearer: String,
    http: reqwest::Client,
}

impl DaemonClient {
    pub fn new(local_port: u16, token: String) -> Self {
        Self {
            base_url: format!("http://127.0.0.1:{}", local_port),
            bearer: token,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("build http client"),
        }
    }

    pub async fn capabilities(&self) -> Result<CapabilitiesResponse, ClientError> {
        // Daemon does not yet implement /capabilities (M0); fall back to /health
        // as a liveness signal and return a minimal CapabilitiesResponse with
        // the desktop's compile-time version. M1 will swap this to the real
        // capabilities endpoint.
        match self.try_capabilities().await {
            Ok(c) => Ok(c),
            Err(ClientError::HttpStatus(404)) => {
                self.health().await?;
                Ok(CapabilitiesResponse {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    schema_version: "v3".to_string(),
                    agents: vec![],
                    features: CapabilityFlags::default(),
                    server_time: String::new(),
                })
            }
            Err(e) => Err(e),
        }
    }

    async fn try_capabilities(&self) -> Result<CapabilitiesResponse, ClientError> {
        let url = format!("{}/api/capabilities", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::HttpStatus(status.as_u16()));
        }
        resp.json::<CapabilitiesResponse>()
            .await
            .map_err(|e| ClientError::Parse(e.to_string()))
    }

    pub async fn health(&self) -> Result<(), ClientError> {
        let url = format!("{}/api/health", self.base_url);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ClientError::HttpStatus(resp.status().as_u16()));
        }
        Ok(())
    }

    pub async fn list_conversations(
        &self,
        agent_type: Option<AgentType>,
        folder_path: Option<String>,
    ) -> Result<Vec<ConversationSummary>, ClientError> {
        let url = format!("{}/api/list_conversations", self.base_url);
        let body = ListConversationsBody {
            agent_type,
            search: None,
            sort_by: None,
            folder_path,
        };
        self.post_json(&url, &body).await
    }

    pub async fn get_conversation(
        &self,
        agent_type: AgentType,
        conversation_id: String,
    ) -> Result<ConversationDetail, ClientError> {
        let url = format!("{}/api/get_conversation", self.base_url);
        let body = GetConversationBody {
            agent_type,
            conversation_id,
        };
        self.post_json(&url, &body).await
    }

    pub async fn acp_connect(
        &self,
        agent_type: AgentType,
        working_dir: Option<String>,
        session_id: Option<String>,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/acp_connect", self.base_url);
        let body = AcpConnectBody {
            agent_type,
            working_dir,
            session_id,
        };
        self.post_json(&url, &body).await
    }

    pub async fn acp_prompt(
        &self,
        connection_id: String,
        blocks: Vec<PromptInputBlock>,
        folder_id: Option<i32>,
        conversation_id: Option<i32>,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/acp_prompt", self.base_url);
        let body = AcpPromptBody {
            connection_id,
            blocks,
            folder_id,
            conversation_id,
        };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn acp_cancel(&self, connection_id: String) -> Result<(), ClientError> {
        let url = format!("{}/api/acp_cancel", self.base_url);
        let body = AcpConnectionIdBody { connection_id };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn acp_respond_permission(
        &self,
        connection_id: String,
        request_id: String,
        option_id: String,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/acp_respond_permission", self.base_url);
        let body = AcpRespondPermissionBody {
            connection_id,
            request_id,
            option_id,
        };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn acp_set_mode(
        &self,
        connection_id: String,
        mode_id: String,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/acp_set_mode", self.base_url);
        let body = AcpSetModeBody {
            connection_id,
            mode_id,
        };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn acp_set_config_option(
        &self,
        connection_id: String,
        config_id: String,
        value_id: String,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/acp_set_config_option", self.base_url);
        let body = AcpSetConfigOptionBody {
            connection_id,
            config_id,
            value_id,
        };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn acp_fork(&self, connection_id: String) -> Result<ForkResultInfo, ClientError> {
        let url = format!("{}/api/acp_fork", self.base_url);
        let body = AcpConnectionIdBody { connection_id };
        self.post_json(&url, &body).await
    }

    pub async fn acp_disconnect(&self, connection_id: String) -> Result<(), ClientError> {
        let url = format!("{}/api/acp_disconnect", self.base_url);
        let body = AcpConnectionIdBody { connection_id };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn acp_touch_connection(
        &self,
        connection_id: String,
    ) -> Result<bool, ClientError> {
        let url = format!("{}/api/acp_touch_connection", self.base_url);
        let body = AcpConnectionIdBody { connection_id };
        self.post_json(&url, &body).await
    }

    pub async fn acp_get_session_snapshot(
        &self,
        connection_id: String,
    ) -> Result<Option<LiveSessionSnapshot>, ClientError> {
        let url = format!("{}/api/acp_get_session_snapshot", self.base_url);
        let body = AcpConnectionIdBody { connection_id };
        self.post_json(&url, &body).await
    }

    pub async fn read_file_preview(
        &self,
        root_path: String,
        path: String,
    ) -> Result<FilePreviewContent, ClientError> {
        let url = format!("{}/api/read_file_preview", self.base_url);
        let body = ReadFileBody { root_path, path };
        self.post_json(&url, &body).await
    }

    pub async fn read_file_for_edit(
        &self,
        root_path: String,
        path: String,
    ) -> Result<FileEditContent, ClientError> {
        let url = format!("{}/api/read_file_for_edit", self.base_url);
        let body = ReadFileBody { root_path, path };
        self.post_json(&url, &body).await
    }

    pub async fn save_file_content(
        &self,
        root_path: String,
        path: String,
        content: String,
        expected_etag: Option<String>,
    ) -> Result<FileSaveResult, ClientError> {
        let url = format!("{}/api/save_file_content", self.base_url);
        let body = SaveFileContentBody {
            root_path,
            path,
            content,
            expected_etag,
        };
        self.post_json(&url, &body).await
    }

    pub async fn save_file_copy(
        &self,
        root_path: String,
        path: String,
        content: String,
    ) -> Result<FileSaveResult, ClientError> {
        let url = format!("{}/api/save_file_copy", self.base_url);
        let body = SaveFileCopyBody {
            root_path,
            path,
            content,
        };
        self.post_json(&url, &body).await
    }

    pub async fn rename_file_tree_entry(
        &self,
        root_path: String,
        path: String,
        new_name: String,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/rename_file_tree_entry", self.base_url);
        let body = RenameFileBody {
            root_path,
            path,
            new_name,
        };
        self.post_json(&url, &body).await
    }

    pub async fn delete_file_tree_entry(
        &self,
        root_path: String,
        path: String,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/delete_file_tree_entry", self.base_url);
        let body = ReadFileBody { root_path, path };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn create_file_tree_entry(
        &self,
        root_path: String,
        path: String,
        name: String,
        kind: String,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/create_file_tree_entry", self.base_url);
        let body = CreateFileBody {
            root_path,
            path,
            name,
            kind,
        };
        self.post_json(&url, &body).await
    }

    pub async fn git_status(
        &self,
        path: String,
        show_all_untracked: Option<bool>,
    ) -> Result<Vec<GitStatusEntry>, ClientError> {
        let url = format!("{}/api/git_status", self.base_url);
        let body = GitStatusBody {
            path,
            show_all_untracked,
        };
        self.post_json(&url, &body).await
    }

    pub async fn git_diff(
        &self,
        path: String,
        file: Option<String>,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/git_diff", self.base_url);
        let body = GitDiffBody { path, file };
        self.post_json(&url, &body).await
    }

    pub async fn git_log(
        &self,
        path: String,
        limit: Option<u32>,
        branch: Option<String>,
        remote: Option<String>,
    ) -> Result<GitLogResult, ClientError> {
        let url = format!("{}/api/git_log", self.base_url);
        let body = GitLogBody {
            path,
            limit,
            branch,
            remote,
        };
        self.post_json(&url, &body).await
    }

    pub async fn git_show_file(
        &self,
        path: String,
        file: String,
        ref_name: Option<String>,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/git_show_file", self.base_url);
        let body = GitShowFileBody {
            path,
            file,
            ref_name,
        };
        self.post_json(&url, &body).await
    }

    pub async fn git_list_all_branches(
        &self,
        path: String,
    ) -> Result<GitBranchList, ClientError> {
        let url = format!("{}/api/git_list_all_branches", self.base_url);
        let body = GitPathBody { path };
        self.post_json(&url, &body).await
    }

    pub async fn git_list_branches(
        &self,
        path: String,
    ) -> Result<Vec<String>, ClientError> {
        let url = format!("{}/api/git_list_branches", self.base_url);
        let body = GitPathBody { path };
        self.post_json(&url, &body).await
    }

    pub async fn git_list_remotes(
        &self,
        path: String,
    ) -> Result<Vec<GitRemote>, ClientError> {
        let url = format!("{}/api/git_list_remotes", self.base_url);
        let body = GitPathBody { path };
        self.post_json(&url, &body).await
    }

    pub async fn git_is_tracked(
        &self,
        path: String,
        file: String,
    ) -> Result<bool, ClientError> {
        let url = format!("{}/api/git_is_tracked", self.base_url);
        let body = GitPathFileBody { path, file };
        self.post_json(&url, &body).await
    }

    pub async fn git_diff_with_branch(
        &self,
        path: String,
        branch: String,
        file: Option<String>,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/git_diff_with_branch", self.base_url);
        let body = GitDiffWithBranchBody { path, branch, file };
        self.post_json(&url, &body).await
    }

    pub async fn git_show_diff(
        &self,
        path: String,
        commit: String,
        file: Option<String>,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/git_show_diff", self.base_url);
        let body = GitShowDiffBody { path, commit, file };
        self.post_json(&url, &body).await
    }

    pub async fn git_init(&self, path: String) -> Result<(), ClientError> {
        let url = format!("{}/api/git_init", self.base_url);
        let body = GitPathBody { path };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn git_checkout(
        &self,
        path: String,
        branch_name: String,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/git_checkout", self.base_url);
        let body = GitCheckoutBody { path, branch_name };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn git_reset(
        &self,
        path: String,
        commit: String,
        mode: String,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/git_reset", self.base_url);
        let body = GitResetBody { path, commit, mode };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn git_add_files(
        &self,
        path: String,
        files: Vec<String>,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/git_add_files", self.base_url);
        let body = GitAddFilesBody { path, files };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn git_rollback_file(
        &self,
        path: String,
        file: String,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/git_rollback_file", self.base_url);
        let body = GitPathFileBody { path, file };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn git_commit(
        &self,
        path: String,
        message: String,
        files: Vec<String>,
        folder_id: Option<i32>,
    ) -> Result<GitCommitResult, ClientError> {
        let url = format!("{}/api/git_commit", self.base_url);
        let body = GitCommitBody {
            folder_id,
            path,
            message,
            files,
        };
        self.post_json(&url, &body).await
    }

    pub async fn git_pull(
        &self,
        path: String,
        credentials: Option<GitCredentials>,
    ) -> Result<GitPullResult, ClientError> {
        let url = format!("{}/api/git_pull", self.base_url);
        let body = GitPullBody { path, credentials };
        self.post_json(&url, &body).await
    }

    pub async fn git_fetch(
        &self,
        path: String,
        credentials: Option<GitCredentials>,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/git_fetch", self.base_url);
        let body = GitFetchBody { path, credentials };
        self.post_json(&url, &body).await
    }

    pub async fn git_fetch_remote(
        &self,
        path: String,
        name: String,
        credentials: Option<GitCredentials>,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/git_fetch_remote", self.base_url);
        let body = GitFetchRemoteBody {
            path,
            name,
            credentials,
        };
        self.post_json(&url, &body).await
    }

    pub async fn git_push(
        &self,
        path: String,
        remote: Option<String>,
        credentials: Option<GitCredentials>,
        folder_id: Option<i32>,
    ) -> Result<GitPushResult, ClientError> {
        let url = format!("{}/api/git_push", self.base_url);
        let body = GitPushBody {
            folder_id,
            path,
            remote,
            credentials,
        };
        self.post_json(&url, &body).await
    }

    pub async fn git_push_info(&self, path: String) -> Result<GitPushInfo, ClientError> {
        let url = format!("{}/api/git_push_info", self.base_url);
        let body = GitPathBody { path };
        self.post_json(&url, &body).await
    }

    pub async fn git_start_pull_merge(
        &self,
        path: String,
        upstream_commit: Option<String>,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/git_start_pull_merge", self.base_url);
        let body = GitStartPullMergeBody {
            path,
            upstream_commit,
        };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn git_has_merge_head(&self, path: String) -> Result<bool, ClientError> {
        let url = format!("{}/api/git_has_merge_head", self.base_url);
        let body = GitPathBody { path };
        self.post_json(&url, &body).await
    }

    pub async fn git_add_remote(
        &self,
        path: String,
        name: String,
        url_value: String,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/git_add_remote", self.base_url);
        let body = GitPathNameUrlBody {
            path,
            name,
            url: url_value,
        };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn git_remove_remote(
        &self,
        path: String,
        name: String,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/git_remove_remote", self.base_url);
        let body = GitPathNameBody { path, name };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn git_set_remote_url(
        &self,
        path: String,
        name: String,
        url_value: String,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/git_set_remote_url", self.base_url);
        let body = GitPathNameUrlBody {
            path,
            name,
            url: url_value,
        };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn git_new_branch(
        &self,
        path: String,
        branch_name: String,
        start_point: Option<String>,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/git_new_branch", self.base_url);
        let body = GitNewBranchBody {
            path,
            branch_name,
            start_point,
        };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn git_worktree_add(
        &self,
        path: String,
        branch_name: String,
        worktree_path: String,
    ) -> Result<(), ClientError> {
        let url = format!("{}/api/git_worktree_add", self.base_url);
        let body = GitWorktreeAddBody {
            path,
            branch_name,
            worktree_path,
        };
        let _: serde_json::Value = self.post_json(&url, &body).await?;
        Ok(())
    }

    pub async fn git_delete_branch(
        &self,
        path: String,
        branch_name: String,
        force: bool,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/git_delete_branch", self.base_url);
        let body = GitDeleteBranchBody {
            path,
            branch_name,
            force,
        };
        self.post_json(&url, &body).await
    }

    pub async fn git_delete_remote_branch(
        &self,
        path: String,
        remote: String,
        branch: String,
        credentials: Option<GitCredentials>,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/git_delete_remote_branch", self.base_url);
        let body = GitDeleteRemoteBranchBody {
            path,
            remote,
            branch,
            credentials,
        };
        self.post_json(&url, &body).await
    }

    pub async fn git_stash_push(
        &self,
        path: String,
        message: Option<String>,
        keep_index: bool,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/git_stash_push", self.base_url);
        let body = GitStashPushBody {
            path,
            message,
            keep_index,
        };
        self.post_json(&url, &body).await
    }

    pub async fn git_stash_pop(
        &self,
        path: String,
        stash_ref: Option<String>,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/git_stash_pop", self.base_url);
        let body = GitStashPopBody { path, stash_ref };
        self.post_json(&url, &body).await
    }

    pub async fn git_stash_list(&self, path: String) -> Result<Vec<GitStashEntry>, ClientError> {
        let url = format!("{}/api/git_stash_list", self.base_url);
        let body = GitPathBody { path };
        self.post_json(&url, &body).await
    }

    pub async fn git_stash_apply(
        &self,
        path: String,
        stash_ref: String,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/git_stash_apply", self.base_url);
        let body = GitStashRefBody { path, stash_ref };
        self.post_json(&url, &body).await
    }

    pub async fn git_stash_drop(
        &self,
        path: String,
        stash_ref: String,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/git_stash_drop", self.base_url);
        let body = GitStashRefBody { path, stash_ref };
        self.post_json(&url, &body).await
    }

    pub async fn git_stash_clear(&self, path: String) -> Result<String, ClientError> {
        let url = format!("{}/api/git_stash_clear", self.base_url);
        let body = GitPathBody { path };
        self.post_json(&url, &body).await
    }

    pub async fn git_stash_show(
        &self,
        path: String,
        stash_ref: String,
    ) -> Result<Vec<GitStatusEntry>, ClientError> {
        let url = format!("{}/api/git_stash_show", self.base_url);
        let body = GitStashRefBody { path, stash_ref };
        self.post_json(&url, &body).await
    }

    pub async fn git_commit_branches(
        &self,
        path: String,
        commit: String,
    ) -> Result<Vec<String>, ClientError> {
        let url = format!("{}/api/git_commit_branches", self.base_url);
        let body = GitCommitBranchesBody { path, commit };
        self.post_json(&url, &body).await
    }

    async fn post_json<B: serde::Serialize, R: for<'de> serde::Deserialize<'de>>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<R, ClientError> {
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.bearer)
            .json(body)
            .send()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let snippet = resp.text().await.unwrap_or_default();
            return Err(ClientError::HttpStatusWithBody {
                status: status.as_u16(),
                body: snippet,
            });
        }
        resp.json::<R>()
            .await
            .map_err(|e| ClientError::Parse(e.to_string()))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListConversationsBody {
    agent_type: Option<AgentType>,
    search: Option<String>,
    sort_by: Option<String>,
    folder_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetConversationBody {
    agent_type: AgentType,
    conversation_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpConnectBody {
    agent_type: AgentType,
    working_dir: Option<String>,
    session_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpPromptBody {
    connection_id: String,
    blocks: Vec<PromptInputBlock>,
    folder_id: Option<i32>,
    conversation_id: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpConnectionIdBody {
    connection_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpRespondPermissionBody {
    connection_id: String,
    request_id: String,
    option_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpSetModeBody {
    connection_id: String,
    mode_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpSetConfigOptionBody {
    connection_id: String,
    config_id: String,
    value_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadFileBody {
    root_path: String,
    path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveFileContentBody {
    root_path: String,
    path: String,
    content: String,
    expected_etag: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveFileCopyBody {
    root_path: String,
    path: String,
    content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RenameFileBody {
    root_path: String,
    path: String,
    new_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateFileBody {
    root_path: String,
    path: String,
    name: String,
    kind: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitPathBody {
    path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitStatusBody {
    path: String,
    show_all_untracked: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitDiffBody {
    path: String,
    file: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitLogBody {
    path: String,
    limit: Option<u32>,
    branch: Option<String>,
    remote: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitShowFileBody {
    path: String,
    file: String,
    ref_name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitPathFileBody {
    path: String,
    file: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitDiffWithBranchBody {
    path: String,
    branch: String,
    file: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitShowDiffBody {
    path: String,
    commit: String,
    file: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitCheckoutBody {
    path: String,
    branch_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitResetBody {
    path: String,
    commit: String,
    mode: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitAddFilesBody {
    path: String,
    files: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitCommitBody {
    folder_id: Option<i32>,
    path: String,
    message: String,
    files: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitPullBody {
    path: String,
    credentials: Option<GitCredentials>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitFetchBody {
    path: String,
    credentials: Option<GitCredentials>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitFetchRemoteBody {
    path: String,
    name: String,
    credentials: Option<GitCredentials>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitPushBody {
    folder_id: Option<i32>,
    path: String,
    remote: Option<String>,
    credentials: Option<GitCredentials>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitStartPullMergeBody {
    path: String,
    upstream_commit: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitPathNameBody {
    path: String,
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitPathNameUrlBody {
    path: String,
    name: String,
    url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitNewBranchBody {
    path: String,
    branch_name: String,
    start_point: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitWorktreeAddBody {
    path: String,
    branch_name: String,
    worktree_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitDeleteBranchBody {
    path: String,
    branch_name: String,
    force: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitDeleteRemoteBranchBody {
    path: String,
    remote: String,
    branch: String,
    credentials: Option<GitCredentials>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitStashPushBody {
    path: String,
    message: Option<String>,
    keep_index: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitStashPopBody {
    path: String,
    stash_ref: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitStashRefBody {
    path: String,
    stash_ref: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitCommitBranchesBody {
    path: String,
    commit: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("network: {0}")]
    Network(String),
    #[error("http status: {0}")]
    HttpStatus(u16),
    #[error("http {status}: {body}")]
    HttpStatusWithBody { status: u16, body: String },
    #[error("parse: {0}")]
    Parse(String),
}
