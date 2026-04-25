use crate::acp::types::PromptInputBlock;
use crate::models::squad::{SquadRoleKind, SquadRoleProfileInfo, SquadTaskInfo};

pub fn build_role_prompt(
    profile: &SquadRoleProfileInfo,
    goal_summary: &str,
    task: Option<&SquadTaskInfo>,
    workspace_path: Option<&str>,
) -> Vec<PromptInputBlock> {
    let role_name = match profile.role_kind {
        SquadRoleKind::Conductor => "conductor",
        SquadRoleKind::Frontend => "frontend",
        SquadRoleKind::Backend => "backend",
        SquadRoleKind::Worker => "worker",
    };
    let task_text = task
        .map(|task| {
            format!(
                "\nTask:\nTitle: {}\nDescription: {}\n",
                task.title, task.description
            )
        })
        .unwrap_or_default();
    let workspace_text = workspace_path
        .map(|path| format!("\nWorkspace boundary: work only inside `{path}` unless explicitly instructed.\n"))
        .unwrap_or_else(|| "\nWorkspace boundary: treat this role as read-only unless a task explicitly authorizes edits.\n".to_string());

    let text = format!(
        "You are the Codeg role-squad {role_name} role.\n\nRole policy:\n{}\n\nUser goal:\n{}{}{}\nReporting contract:\n- Keep updates concise.\n- Report changed files, blockers, risks, and validation results.\n- Do not perform destructive git operations without explicit instruction.\n",
        profile.system_prompt, goal_summary, task_text, workspace_text
    );

    vec![PromptInputBlock::Text { text }]
}
