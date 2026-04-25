use std::path::{Path, PathBuf};

use crate::models::squad::{SquadRoleKind, SquadWorkspacePolicy};

pub fn role_workspace_path(
    base_working_dir: Option<&str>,
    run_id: i32,
    role_kind: SquadRoleKind,
    policy: SquadWorkspacePolicy,
) -> Option<String> {
    let base = base_working_dir?.trim();
    if base.is_empty() {
        return None;
    }
    match policy {
        SquadWorkspacePolicy::ReadOnly | SquadWorkspacePolicy::WriteShared => {
            Some(base.to_string())
        }
        SquadWorkspacePolicy::WriteIsolated => Some(
            planned_worktree_path(Path::new(base), run_id, role_kind)
                .display()
                .to_string(),
        ),
    }
}

fn planned_worktree_path(base: &Path, run_id: i32, role_kind: SquadRoleKind) -> PathBuf {
    let repo_name = base
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("workspace");
    let role = serde_json::to_string(&role_kind)
        .unwrap_or_else(|_| "worker".to_string())
        .trim_matches('"')
        .to_string();
    let parent = base.parent().unwrap_or_else(|| Path::new("."));
    parent
        .join(".codeg-worktrees")
        .join(repo_name)
        .join(run_id.to_string())
        .join(role)
}
