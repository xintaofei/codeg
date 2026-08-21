//! Forge workbench commands: list a folder's issues/PRs, trigger a work task
//! from one, reverse-lookup task chips for visible rows, and map a folder to
//! its forge repository. `_core` fns are mode-agnostic (Tauri + Axum).
//!
//! Trust boundary (the load-bearing part): the client only supplies
//! COORDINATES (host, repo, number) and a display snapshot. Everything a task
//! will trust — canonical URL, api base, account identity, the source key,
//! the prompt with its untrusted-data envelope — is derived server-side, and
//! the folder's actual `origin` remote must match the claimed repository
//! before a card is minted. UI gating is presentation; the gates live here.

use serde::{Deserialize, Serialize};

use crate::app_error::AppCommandError;
use crate::commands::folders::get_folder_core;
use crate::db::service::work_task_service::{self, ForgeCreateOutcome};
use crate::db::AppDatabase;
use crate::forge::envelope::{forge_untrusted_envelope, ForgeSnapshot};
use crate::forge::{
    self, CountFilters, ForgeError, ForgeIssueList, ForgeItemKind, ForgeProvider, ForgeSourceMeta,
    ForgeTab, ListFilters, ListIssuesRequest,
};
use crate::models::{WorkTaskConfig, WorkTaskDraft, WorkTaskInfo, WorkTaskSource};
use crate::web::event_bridge::{emit_event, EventEmitter, WorkTaskChange, WORK_TASK_CHANGED_EVENT};

/// Hard cap for one reverse-lookup batch (a screen shows ~30 rows).
const LOOKUP_KEYS_CAP: usize = 100;
/// Task card titles inherit the automation convention: 80 chars.
const TITLE_CAP: usize = 80;

#[derive(Debug, Clone, Serialize)]
pub struct ForgeRemote {
    pub server_host: String,
    pub owner_repo: String,
    pub remote_url: String,
    /// Which forge this host is, decided HERE (see `forge::provider_for_host`)
    /// and handed to the client for display and for building the reverse-lookup
    /// keys. The client never picks it: that choice selects a credential.
    pub provider: ForgeProvider,
}

/// Client-supplied coordinates of the work item being triggered.
#[derive(Debug, Clone, Deserialize)]
pub struct ForgeTaskSourceInput {
    /// "issue" | "pr".
    pub kind: String,
    /// What the client believes this host is. Checked against the server's own
    /// derivation and otherwise unused — a disagreement means the workbench is
    /// looking at stale account settings, which is worth saying out loud
    /// rather than silently minting provenance the client cannot match.
    pub provider: String,
    pub server_host: String,
    #[serde(default)]
    pub account_id: Option<String>,
    pub owner_repo: String,
    pub number: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ForgeTaskDraft {
    pub folder_id: i32,
    pub source: ForgeTaskSourceInput,
    pub snapshot: ForgeSnapshot,
    /// Extra instruction the user typed in the trigger dialog.
    #[serde(default)]
    pub instruction: Option<String>,
    /// Per-task agent override; `None` inherits folder settings.
    #[serde(default)]
    pub agent_type: Option<String>,
    /// Deliberately create a second live task for the same work item.
    #[serde(default)]
    pub force: bool,
}

/// Discriminated result: a dedup hit and a folder/repo mismatch are answers
/// the dialog acts on, not errors.
#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ForgeCreateResult {
    Created { task: WorkTaskInfo },
    Duplicate { existing: WorkTaskInfo },
    FolderMismatch { folder_remote: Option<ForgeRemote> },
}

/// One reverse-lookup row: the latest task (any state) for a source key.
#[derive(Debug, Serialize)]
pub struct ForgeTaskLink {
    pub source_key: String,
    pub task_id: i32,
    pub status: crate::models::WorkTaskStatus,
    pub verdict: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

// ── shared business logic (both modes) ──────────────────────────────────────

/// The folder's `origin` remote, parsed into forge coordinates. `None` when
/// there is no origin or its URL is not a recognizable forge repo.
pub async fn folder_forge_remote_core(
    db: &AppDatabase,
    folder_id: i32,
) -> Result<Option<ForgeRemote>, AppCommandError> {
    let folder = get_folder_core(db, folder_id).await?;
    let output = crate::process::tokio_command("git")
        .args(["-C", &folder.path, "remote", "get-url", "origin"])
        .output()
        .await
        .map_err(|e| AppCommandError::io_error("failed to run git").with_detail(e.to_string()))?;
    if !output.status.success() {
        return Ok(None);
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let Some((server_host, owner_repo)) = forge::parse_remote_url(&url) else {
        return Ok(None);
    };
    let profile = forge::host_profile(&db.conn, &server_host).await;
    Ok(Some(ForgeRemote {
        server_host,
        // A GitLab mounted under a relative URL root puts that mount path in
        // front of every repository path a git remote carries, while no API
        // path has it. Dropping it here is what keeps ONE spelling of the
        // repository in the source key, the API calls, the links and the push.
        owner_repo: forge::strip_base_path(&owner_repo, &profile.base_path),
        // Redacted, not raw: a remote URL can carry `user:token@` from
        // whatever configured it, and this value crosses to the client.
        remote_url: redact_userinfo(&url),
        provider: profile.provider,
    }))
}

/// Strip any `user:password@` from a URL before it leaves the backend. Git
/// remotes configured elsewhere (or by an older codeg) can embed a token, and
/// this string is shown and logged.
fn redact_userinfo(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    match rest.split_once('@') {
        // Only the authority may contain userinfo; an `@` after the first `/`
        // is part of the path and stays.
        Some((userinfo, after)) if !userinfo.contains('/') => {
            format!("{scheme}://{after}")
        }
        _ => url.to_string(),
    }
}

/// Resolve the folder's repository AND the credential to read it with — the
/// two things every workbench read needs and neither of which the client may
/// supply.
async fn resolve_folder_repo(
    db: &AppDatabase,
    folder_id: i32,
    account_id: Option<&str>,
) -> Result<(ForgeRemote, forge::ResolvedAuth), AppCommandError> {
    let remote = folder_forge_remote_core(db, folder_id)
        .await?
        .ok_or_else(|| {
            AppCommandError::configuration_missing(
                "this folder has no recognizable forge remote (origin)",
            )
        })?;
    let auth =
        forge::resolve_forge_auth(&db.conn, remote.provider, &remote.server_host, account_id)
            .await?;
    Ok((remote, auth))
}

pub async fn forge_list_issues_core(
    db: &AppDatabase,
    folder_id: i32,
    filters: ListFilters,
) -> Result<ForgeIssueList, AppCommandError> {
    let (remote, auth) =
        resolve_folder_repo(db, folder_id, filters.account_id.as_deref()).await?;
    // The repository is an ARGUMENT here, never a field of `filters`: it is
    // derived from the folder's own remote, so there is nothing for a client to
    // claim. Paging is clamped inside each client (`ListIssuesRequest::clamped`)
    // and the text/label filters are normalized by `new`.
    let request = ListIssuesRequest::new(remote.owner_repo, filters);
    Ok(match remote.provider {
        ForgeProvider::GitHub => forge::github::list_issues(&auth, &request).await?,
        ForgeProvider::GitLab => forge::gitlab::list_issues(&auth, &request).await?,
    })
}

/// One tab's item count under a set of filters — a badge on the workbench's
/// Issues/PR switcher.
///
/// **One tab, and specifically the tab the user is NOT looking at.** The active
/// tab's count arrives inside the list response the page already paid for, so
/// asking for it again would make every filter change cost three search calls
/// against a quota of thirty a MINUTE — decoration competing for quota with the
/// content it decorates. Paired with a frontend that remembers both numbers per
/// filter set, this makes switching tabs and turning pages cost nothing at all.
///
/// Its own command rather than a probe fired from the frontend: every list call
/// re-derives the folder's remote (which spawns `git`) and re-resolves the
/// account, so the sequencing has to happen behind one resolution.
///
/// `None` — never an error — for a forge that declines to count, a probe that
/// failed, or a search GitHub itself calls incomplete. A badge is decoration:
/// losing one costs that badge and nothing else, not the other badge and
/// certainly not the list, which reports its own failures with its own error.
pub async fn forge_tab_count_core(
    db: &AppDatabase,
    folder_id: i32,
    tab: ForgeTab,
    filters: CountFilters,
) -> Result<Option<i64>, AppCommandError> {
    let (remote, auth) = resolve_folder_repo(db, folder_id, filters.account_id.as_deref()).await?;
    let request = ListIssuesRequest::new(remote.owner_repo.clone(), filters.probe(tab));
    let listed = match remote.provider {
        ForgeProvider::GitHub => forge::github::list_issues(&auth, &request).await,
        ForgeProvider::GitLab => forge::gitlab::list_issues(&auth, &request).await,
    };
    Ok(listed.ok().and_then(|page| page.trustworthy_count()))
}

/// The repository's label vocabulary, for the workbench's label filter. Its
/// own command rather than a field on the list response: the labels change far
/// more slowly than the list, so the frontend fetches them once per repository
/// instead of on every page turn.
pub async fn forge_list_labels_core(
    db: &AppDatabase,
    folder_id: i32,
    account_id: Option<String>,
) -> Result<forge::ForgeLabelList, AppCommandError> {
    let (remote, auth) = resolve_folder_repo(db, folder_id, account_id.as_deref()).await?;
    Ok(match remote.provider {
        ForgeProvider::GitHub => forge::github::list_labels(&auth, &remote.owner_repo).await?,
        ForgeProvider::GitLab => forge::gitlab::list_labels(&auth, &remote.owner_repo).await?,
    })
}

pub async fn work_task_create_from_forge_core(
    emitter: &EventEmitter,
    db: &AppDatabase,
    draft: ForgeTaskDraft,
) -> Result<ForgeCreateResult, AppCommandError> {
    let source = &draft.source;
    let item_kind = match source.kind.trim() {
        "issue" => ForgeItemKind::Issue,
        "pr" => ForgeItemKind::Change,
        other => {
            return Err(AppCommandError::invalid_input(format!(
                "unknown work item kind: {other}"
            )))
        }
    };
    let is_pr = item_kind == ForgeItemKind::Change;
    let claimed_provider =
        ForgeProvider::parse(&source.provider).map_err(AppCommandError::from)?;

    let server_host = source.server_host.trim().to_ascii_lowercase();
    let owner_repo = forge::normalize_repo(&source.owner_repo).ok_or_else(|| {
        AppCommandError::invalid_input(format!("bad repository path: {}", source.owner_repo))
    })?;

    // The folder's actual origin must BE the claimed repository — a task's
    // whole execution (worktree, diff, later delivery) happens against this
    // folder, so a mismatch here would run the issue against a stranger repo.
    let remote = match folder_forge_remote_core(db, draft.folder_id).await? {
        Some(remote)
            if remote.server_host == server_host
                && forge::same_repo(&remote.owner_repo, &owner_repo) =>
        {
            remote
        }
        folder_remote => return Ok(ForgeCreateResult::FolderMismatch { folder_remote }),
    };
    // Derived, not taken: which forge this is decides which token gets spent
    // and which endpoints a whole task's worth of writes will go to. The
    // client's claim only has to agree.
    let provider = remote.provider;
    if claimed_provider != provider {
        return Err(AppCommandError::invalid_input(format!(
            "this folder's remote is a {} host, not {} — refresh the workbench and try again",
            provider.as_str(),
            claimed_provider.as_str()
        )));
    }

    // Account resolution pins the identity the task will keep using (writeback
    // and delivery read it from source_meta — never "the current default").
    let auth =
        forge::resolve_forge_auth(&db.conn, provider, &server_host, source.account_id.as_deref())
            .await?;

    // A proposed change has to be hydrated before a card can exist: the list
    // rows carry no refs at all, and without head/base there is nothing to
    // check out and nothing to push back to.
    let pull = if is_pr {
        let pull = match provider {
            ForgeProvider::GitHub => {
                forge::deliver::get_pull(&auth, &owner_repo, source.number).await?
            }
            ForgeProvider::GitLab => {
                forge::gitlab::get_merge_request(&auth, &owner_repo, source.number).await?
            }
        };
        // Refused here, at the only moment the user can still choose something
        // else, rather than at the end of a task whose work has nowhere to go.
        forge::deliver::pull_is_workable(provider, &pull, &owner_repo)
            .map_err(AppCommandError::invalid_input)?;
        Some(pull)
    } else {
        None
    };

    let key = forge::source_key(
        provider.as_str(),
        &server_host,
        &owner_repo,
        item_kind.key_segment(),
        source.number,
    )
    .map_err(AppCommandError::from)?;
    // The link is stored on the card, shown to the user and put in the agent's
    // prompt. It comes from the resolved API base, not from the bare host: a
    // self-hosted instance on `http://` or a non-default port would otherwise
    // get a link that does not open.
    let url = provider.item_url(
        &forge::web_origin(&auth),
        &owner_repo,
        item_kind,
        source.number,
    );
    let meta = ForgeSourceMeta {
        provider,
        server_host: server_host.clone(),
        api_base: auth.api_base.clone(),
        account_id: auth.account_id.clone(),
        owner_repo: owner_repo.clone(),
        number: source.number,
        url: url.clone(),
        title: truncate_chars(snapshot_title(&draft.snapshot), 400),
        base_ref: pull.as_ref().map(|p| p.base_ref.clone()),
        head_ref: pull.as_ref().map(|p| p.head_ref.clone()),
        // The OID the user was looking at. The checkout pins to it rather than
        // to whatever the branch points at by then, so a push that lands while
        // the task is queued cannot silently change what gets worked on.
        head_sha: pull.as_ref().map(|p| p.head_sha.clone()),
        head_repo: pull.as_ref().map(|p| p.head_repo.clone()),
        result_pr: None,
    };

    // Prompt composed HERE, server-side: instruction block + envelope block.
    // The client never hands us prompt text (the dialog's extra instruction is
    // plain user input in its own paragraph, same trust level as any chat).
    let mut instruction = if is_pr {
        let noun = provider.change_noun();
        format!(
            "Review {noun} #{} of {url}.\nThis worktree is already checked out at the {noun}'s \
             head commit, so its changes are here and your commits go on top of them — they are \
             pushed back to the same {noun} branch when the task is accepted. Read the fenced \
             external content below to understand what the {noun} claims to do, then review it \
             and fix what needs fixing. Report milestones with task_progress and call \
             task_complete once right before you finish.",
            source.number
        )
    } else {
        format!(
            "Handle issue #{} of {url}.\nRead the fenced external content below to understand \
             what is being asked, then implement/fix it inside this worktree. Report milestones \
             with task_progress and call task_complete once right before you finish.",
            source.number
        )
    };
    if let Some(extra) = draft.instruction.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        instruction.push_str("\n\nAdditional instruction from the user:\n");
        instruction.push_str(extra);
    }
    let envelope = forge_untrusted_envelope(provider.as_str(), &draft.snapshot);
    let blocks = vec![
        serde_json::to_value(crate::acp::types::PromptInputBlock::Text {
            text: instruction.clone(),
        })
        .map_err(|e| AppCommandError::invalid_input(e.to_string()))?,
        serde_json::to_value(crate::acp::types::PromptInputBlock::Text { text: envelope })
            .map_err(|e| AppCommandError::invalid_input(e.to_string()))?,
    ];
    let config = WorkTaskConfig {
        prompt_blocks: blocks,
        display_text: instruction,
        agent_type: draft
            .agent_type
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        mode_id: None,
        config_values: Default::default(),
        label_snapshot: None,
    };
    let task_draft = WorkTaskDraft {
        folder_id: draft.folder_id,
        title: truncate_chars(
            &format!("#{} · {}", source.number, snapshot_title(&draft.snapshot)),
            TITLE_CAP,
        ),
        config: serde_json::to_value(&config)
            .map_err(|e| AppCommandError::invalid_input(e.to_string()))?,
    };
    let source_row = WorkTaskSource {
        kind: if is_pr {
            forge::SOURCE_KIND_PR
        } else {
            forge::SOURCE_KIND_ISSUE
        }
        .to_string(),
        key,
        meta: serde_json::to_value(&meta)
            .map_err(|e| AppCommandError::invalid_input(e.to_string()))?,
    };

    match work_task_service::create_from_forge(&db.conn, task_draft, source_row, draft.force)
        .await?
    {
        ForgeCreateOutcome::Created(task) => {
            emit_event(
                emitter,
                WORK_TASK_CHANGED_EVENT,
                WorkTaskChange::Upsert { id: task.id },
            );
            crate::commands::work_task::nudge_pump(task.folder_id);
            Ok(ForgeCreateResult::Created { task })
        }
        ForgeCreateOutcome::Duplicate(existing) => {
            Ok(ForgeCreateResult::Duplicate { existing })
        }
    }
}

pub async fn work_task_lookup_by_source_core(
    db: &AppDatabase,
    mut source_keys: Vec<String>,
) -> Result<Vec<ForgeTaskLink>, AppCommandError> {
    source_keys.truncate(LOOKUP_KEYS_CAP);
    let rows = work_task_service::lookup_latest_by_source_keys(&db.conn, &source_keys).await?;
    Ok(rows
        .into_iter()
        .map(|(source_key, m)| ForgeTaskLink {
            source_key,
            task_id: m.id,
            status: m.status,
            verdict: m.verdict,
            updated_at: m.updated_at,
        })
        .collect())
}

fn snapshot_title(snapshot: &ForgeSnapshot) -> &str {
    let trimmed = snapshot.title.trim();
    if trimmed.is_empty() {
        "(untitled)"
    } else {
        trimmed
    }
}

fn truncate_chars(input: &str, cap: usize) -> String {
    if input.chars().count() <= cap {
        return input.to_string();
    }
    input.chars().take(cap).collect()
}

// ── Tauri wrappers (desktop mode) ───────────────────────────────────────────

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn folder_forge_remote(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
) -> Result<Option<ForgeRemote>, AppCommandError> {
    folder_forge_remote_core(&db, folder_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn forge_list_issues(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    query: ListFilters,
) -> Result<ForgeIssueList, AppCommandError> {
    forge_list_issues_core(&db, folder_id, query).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn forge_tab_count(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    tab: ForgeTab,
    filters: CountFilters,
) -> Result<Option<i64>, AppCommandError> {
    forge_tab_count_core(&db, folder_id, tab, filters).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn forge_list_labels(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    account_id: Option<String>,
) -> Result<forge::ForgeLabelList, AppCommandError> {
    forge_list_labels_core(&db, folder_id, account_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn work_task_create_from_forge(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    draft: ForgeTaskDraft,
) -> Result<ForgeCreateResult, AppCommandError> {
    work_task_create_from_forge_core(&EventEmitter::Tauri(app), &db, draft).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn work_task_lookup_by_source(
    db: tauri::State<'_, AppDatabase>,
    source_keys: Vec<String>,
) -> Result<Vec<ForgeTaskLink>, AppCommandError> {
    work_task_lookup_by_source_core(&db, source_keys).await
}

// AppCommandError ← ForgeError conversion lives in `forge::mod` (used above
// via `?` and the explicit map for `source_key`).
#[allow(unused)]
fn _assert_forge_error_converts(err: ForgeError) -> AppCommandError {
    err.into()
}
