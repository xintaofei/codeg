use serde::Serialize;
use std::path::{Component, Path, PathBuf};

use crate::acp::types::AgentSkillScope;
use crate::app_error::AppCommandError;
use crate::commands::acp::{list_skills_from_dir, scoped_skill_dirs, skill_storage_spec};
use crate::models::agent::AgentType;

// ---------------------------------------------------------------------------
// Package manager detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PackageManagerInfo {
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
}

async fn detect_one(name: &str) -> PackageManagerInfo {
    let program = match name {
        "bun" => "bun",
        "pnpm" => "pnpm",
        "yarn" => "yarn",
        _ => "npm",
    };

    let result = crate::process::tokio_command(program)
        .arg("--version")
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            PackageManagerInfo {
                name: name.to_string(),
                installed: true,
                version: Some(version),
            }
        }
        _ => PackageManagerInfo {
            name: name.to_string(),
            installed: false,
            version: None,
        },
    }
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn detect_package_manager(name: String) -> PackageManagerInfo {
    detect_one(&name).await
}

// ---------------------------------------------------------------------------
// Project creation
// ---------------------------------------------------------------------------

/// Validate a user-supplied project name. It becomes a directory name handed
/// straight to the scaffolding CLI (`shadcn -n <name>` / `hyperframes init
/// <name>`), which runs with `cwd = target_dir`. So it MUST be a single, safe
/// path segment: a `..`, a nested `a/b`, or an absolute path would make the CLI
/// write OUTSIDE the chosen save directory and bypass the `target_dir` boundary
/// (also reachable via the server HTTP API, not just the desktop picker).
fn validate_project_name(name: &str) -> Result<(), AppCommandError> {
    if name.is_empty() {
        return Err(AppCommandError::invalid_input("Project name is required"));
    }
    // A leading '-' makes the name look like a CLI option to the scaffolding CLI:
    // `hyperframes init <name>` parses a positional starting with '-' as a flag,
    // and `shadcn -n <name>` likewise. So "-x"/"--help" would make the CLI ignore
    // the name (scaffold its default elsewhere, or just print help) and exit 0,
    // while we'd still return `target_dir/<name>`. Reject it — no real folder
    // needs a leading dash, and `--` isn't reliably honored across every runner
    // (npx/dlx/bunx) + CLI to safely end option parsing.
    if name.starts_with('-') {
        return Err(AppCommandError::invalid_input(
            "Project name must not start with '-'",
        ));
    }
    let mut components = Path::new(name).components();
    let single_segment = matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    );
    // On Unix '\\' is a legal filename char, so the component scan won't flag
    // "..\\x" as traversal — reject separators explicitly since codeg ships on
    // Windows too.
    if !single_segment || name.contains('/') || name.contains('\\') {
        return Err(AppCommandError::invalid_input(
            "Project name must be a single folder name (no path separators, '..', or absolute paths)",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_project_name;

    #[test]
    fn accepts_plain_folder_names() {
        for ok in ["my-video", "my_app", "proj.2", "v1"] {
            assert!(validate_project_name(ok).is_ok(), "should accept {ok:?}");
        }
    }

    #[test]
    fn rejects_traversal_separators_and_absolute() {
        for bad in [
            "",
            ".",
            "..",
            "../escape",
            "a/b",
            "/abs",
            "sub/../x",
            "foo\\bar",
            "..\\win",
        ] {
            assert!(validate_project_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn rejects_leading_dash_option_injection() {
        // A leading '-' would be parsed as a flag by the scaffolding CLI.
        for bad in ["-x", "--help", "-", "--example", "-rf"] {
            assert!(validate_project_name(bad).is_err(), "should reject {bad:?}");
        }
    }
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_shadcn_project(
    project_name: String,
    template: String,
    preset_code: String,
    package_manager: String,
    target_dir: String,
) -> Result<String, AppCommandError> {
    let project_name = project_name.trim().to_string();
    let template = template.trim().to_string();
    let preset_code = preset_code.trim().to_string();
    let package_manager = package_manager.trim().to_string();
    let target_dir = target_dir.trim().to_string();

    validate_project_name(&project_name)?;
    if template.is_empty() {
        return Err(AppCommandError::invalid_input("Template is required"));
    }
    if target_dir.is_empty() {
        return Err(AppCommandError::invalid_input(
            "Target directory is required",
        ));
    }

    let full_path = PathBuf::from(&target_dir).join(&project_name);
    let full_path_str = full_path.to_string_lossy().to_string();

    // Check if directory already exists and is non-empty
    if full_path.exists() {
        let is_empty = full_path
            .read_dir()
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if !is_empty {
            return Err(AppCommandError::already_exists(
                "Target directory already exists and is not empty",
            ));
        }
    }

    // Determine the command based on package manager
    let (program, prefix_args): (&str, Vec<&str>) = match package_manager.as_str() {
        "pnpm" => ("pnpm", vec!["dlx"]),
        "yarn" => ("yarn", vec!["dlx"]),
        "bun" => ("bunx", vec![]),
        _ => ("npx", vec![]),
    };

    let mut cmd = crate::process::tokio_command(program);
    cmd.args(&prefix_args);
    cmd.args([
        "shadcn@latest",
        "init",
        "-n",
        &project_name,
        "-t",
        &template,
        "-p",
        &preset_code,
        "-y",
    ]);
    cmd.current_dir(&target_dir);

    // Log the full command for debugging
    let cmd_display = format!(
        "{} {} shadcn@latest init -n {} -t {} -p {} -y (cwd={})",
        program,
        prefix_args.join(" "),
        project_name,
        template,
        preset_code,
        target_dir
    );
    eprintln!("[ProjectBoot] executing: {cmd_display}");

    let output = cmd.output().await.map_err(|e| {
        eprintln!("[ProjectBoot] spawn error: {e}");
        if e.kind() == std::io::ErrorKind::NotFound {
            AppCommandError::dependency_missing(format!(
                "{program} is not installed. Please install Node.js first."
            ))
        } else {
            AppCommandError::external_command(
                "Failed to execute project creation command",
                e.to_string(),
            )
        }
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    eprintln!(
        "[ProjectBoot] exit={} stdout_len={} stderr_len={}",
        output.status,
        stdout.len(),
        stderr.len()
    );
    if !stdout.is_empty() {
        eprintln!("[ProjectBoot] stdout: {stdout}");
    }
    if !stderr.is_empty() {
        eprintln!("[ProjectBoot] stderr: {stderr}");
    }

    if !output.status.success() {
        let mut detail = String::new();
        if !stderr.is_empty() {
            detail.push_str(&stderr);
        }
        if !stdout.is_empty() {
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str(&stdout);
        }
        if detail.is_empty() {
            detail = format!("Command exited with status: {}", output.status);
        }
        return Err(AppCommandError::external_command(
            "Project creation command failed",
            detail,
        ));
    }

    Ok(full_path_str)
}

// ---------------------------------------------------------------------------
// HyperFrames (HTML-to-video) launcher
// ---------------------------------------------------------------------------

/// codeg's six supported agents, mapped to the `skills` CLI's `--agent` ids.
/// The launcher only ever targets these agents (never the CLI's full 55+ agent
/// universe via `--all`/`--agent '*'`), so installs stay scoped to tools codeg
/// can actually orchestrate. Also the allowlist that validates incoming ids.
const HYPERFRAMES_SKILL_AGENTS: [&str; 6] = [
    "claude-code",
    "codex",
    "opencode",
    "gemini-cli",
    "openclaw",
    "cline",
];

/// Per-agent install status of the HyperFrames skills.
#[derive(Debug, Clone, Serialize)]
pub struct HyperframesSkillAgent {
    pub agent: String,
    pub installed: bool,
}

/// Marker prefix for the HyperFrames skill family. The launcher installs the
/// whole repo (`--skill '*'`), whose skill set changes over time — observed ids
/// include `hyperframes`, `hyperframes-cli`, `hyperframes-media` and
/// `hyperframes-registry`. The `skills` CLI also installs only a *variable
/// subset* per run: it skips skills already present in the shared
/// `~/.agents/skills` store and won't re-symlink them into an agent, so no
/// single id (not even `hyperframes`) is guaranteed to land in a given agent's
/// dir. We therefore treat the family as installed when ANY skill whose id
/// starts with this prefix is present in the agent's global skill dirs.
const HYPERFRAMES_SKILL_PREFIX: &str = "hyperframes";

/// Map a `skills` CLI `--agent` id (what the install command and the frontend
/// speak) to codeg's internal `AgentType`, which drives skill-dir resolution.
fn agent_type_for_skill_id(skill_agent: &str) -> Option<AgentType> {
    Some(match skill_agent {
        "claude-code" => AgentType::ClaudeCode,
        "codex" => AgentType::Codex,
        "opencode" => AgentType::OpenCode,
        "gemini-cli" => AgentType::Gemini,
        "openclaw" => AgentType::OpenClaw,
        "cline" => AgentType::Cline,
        _ => return None,
    })
}

/// Whether any HyperFrames-family skill is present in the agent's GLOBAL skill
/// directories. Reuses codeg's own skill-dir resolution + enumeration
/// (`acp.rs`), so it matches what Settings → Skills lists by construction —
/// including the shared `~/.agents/skills` store that the `skills` CLI's
/// "universal" agents (codex, opencode, cline, …) read from. See
/// [`HYPERFRAMES_SKILL_PREFIX`] for why a prefix, not a single id, is the
/// reliable marker.
fn hyperframes_skill_installed(skill_agent: &str) -> bool {
    let Some(agent_type) = agent_type_for_skill_id(skill_agent) else {
        return false;
    };
    let Some(spec) = skill_storage_spec(agent_type) else {
        return false;
    };
    let Ok(dirs) = scoped_skill_dirs(agent_type, AgentSkillScope::Global, None) else {
        return false;
    };
    dirs.iter().any(|dir| {
        list_skills_from_dir(AgentSkillScope::Global, dir, spec.kind)
            .map(|skills| {
                skills
                    .iter()
                    .any(|skill| skill.id.starts_with(HYPERFRAMES_SKILL_PREFIX))
            })
            .unwrap_or(false)
    })
}

/// Detect, per agent, whether the HyperFrames skill is already installed
/// globally. Backs the launcher's "Installed" badges; consistent with the
/// Settings → Skills view by construction (same resolution path).
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn detect_hyperframes_skills() -> Vec<HyperframesSkillAgent> {
    HYPERFRAMES_SKILL_AGENTS
        .iter()
        .map(|&agent| HyperframesSkillAgent {
            agent: agent.to_string(),
            installed: hyperframes_skill_installed(agent),
        })
        .collect()
}

/// Install the HyperFrames agent skills globally (symlinked) for the given
/// agents via `npx skills add`, ONE AGENT PER INVOCATION.
///
/// Why per-agent and not a single `--agent a b c …` call: the `skills` CLI
/// filters `--skill '*'` to the INTERSECTION of skills compatible with every
/// `--agent` passed in one invocation. Batching all agents therefore installs
/// only the small common subset and leaves each agent under-provisioned
/// (observed: 4 skills instead of the 7–15 a solo install gives). Looping gives
/// each agent its full compatible set, which also guarantees `hyperframes*`
/// core skills land — keeping the prefix-based detection reliable.
///
/// Flags: `--skill '*'` = all (compatible) skills; symlink is the CLI default
/// (we omit `--copy`); `-g` = user-global; the trailing `-y` skips the CLI's
/// own prompts. Re-running is idempotent, so this doubles as an "update".
///
/// After the per-agent commands run, each one that exited 0 is re-checked with
/// [`hyperframes_skill_installed`]: the CLI can return success yet provision no
/// HyperFrames skill (a flaky/partial clone was observed installing only an
/// unrelated skill), and that is reported as a failure rather than left as a
/// silent "installed-but-empty" badge.
///
/// `npx --yes skills@latest` pins the package spec: a bare `npx skills` would
/// instead execute a `node_modules/.bin/skills` shim from the process CWD if
/// one exists. `--yes` also keeps npx non-interactive when it isn't cached.
///
/// `agents` is validated against the fixed allowlist above.
/// `GIT_CLONE_PROTECTION_ACTIVE=0` mirrors the upstream `hyperframes skills`
/// wrapper: the `skills` CLI shells out to `git clone`, and Git's clone-hook
/// protection can otherwise abort it when a global `git lfs` post-checkout hook
/// is registered. The source repo is hardcoded, so opting out is safe here.
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn install_hyperframes_skills(agents: Vec<String>) -> Result<(), AppCommandError> {
    let selected: Vec<&str> = agents
        .iter()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .collect();
    if selected.is_empty() {
        return Err(AppCommandError::invalid_input("No agents selected"));
    }
    for a in &selected {
        if !HYPERFRAMES_SKILL_AGENTS.contains(a) {
            return Err(AppCommandError::invalid_input(format!(
                "Unsupported agent: {a}"
            )));
        }
    }

    // Install agents one at a time (see doc comment for the intersection-filter
    // rationale). Collect per-agent failures so one bad agent doesn't mask the
    // others; agents that succeed stay installed and re-detection reflects that.
    let mut failures: Vec<String> = Vec::new();
    let mut ran_ok: Vec<&str> = Vec::new();
    for &agent in &selected {
        let mut cmd = crate::process::tokio_command("npx");
        cmd.args([
            "--yes",
            "skills@latest",
            "add",
            "heygen-com/hyperframes",
            "--skill",
            "*",
            "-g",
            "-y",
            "--agent",
            agent,
        ]);
        cmd.env("GIT_CLONE_PROTECTION_ACTIVE", "0");

        eprintln!(
            "[ProjectBoot] executing: npx --yes skills@latest add heygen-com/hyperframes --skill * -g -y --agent {agent}"
        );

        let output = cmd.output().await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppCommandError::dependency_missing(
                    "npx is not installed. Please install Node.js first.",
                )
            } else {
                AppCommandError::external_command("Failed to run skills install", e.to_string())
            }
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                format!("exited with status: {}", output.status)
            };
            eprintln!("[ProjectBoot] skills install failed for {agent}: {detail}");
            failures.push(format!("{agent}: {detail}"));
        } else {
            ran_ok.push(agent);
        }
    }

    // Post-install verification. `skills add` can exit 0 yet leave an agent with
    // no HyperFrames skill (a flaky/partial clone was observed installing only an
    // unrelated skill while still returning success). Without this the launcher
    // would show that agent's badge silently "not installed" after a "successful"
    // install. Reuse the same detection the badges use — drift-proof: it checks
    // for ANY `hyperframes*` skill, not a hardcoded id. Universal agents share
    // `~/.agents/skills`, so one satisfied install covers all that read it.
    for &agent in &ran_ok {
        if !hyperframes_skill_installed(agent) {
            eprintln!("[ProjectBoot] {agent}: install exited 0 but no HyperFrames skill detected");
            failures.push(format!(
                "{agent}: install reported success but no HyperFrames skill was detected"
            ));
        }
    }

    if !failures.is_empty() {
        return Err(AppCommandError::external_command(
            "Failed to install HyperFrames skills",
            failures.join("\n"),
        ));
    }

    Ok(())
}

/// Scaffold a new HyperFrames composition project via the `hyperframes` CLI.
/// Mirrors `create_shadcn_project`: shells out to the chosen package runner,
/// runs in `target_dir`, and returns the created project directory.
///
/// Note on skills: in `--non-interactive` mode the CLI does NOT install the
/// agent coding skills — it only prints a `npx skills add heygen-com/hyperframes`
/// hint and returns. So `--skip-skills` would be a no-op here and is left off;
/// authoring skills, if wanted, must be installed as a separate step.
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_hyperframes_project(
    project_name: String,
    example: String,
    resolution: String,
    package_manager: String,
    target_dir: String,
) -> Result<String, AppCommandError> {
    let project_name = project_name.trim().to_string();
    let example = example.trim().to_string();
    let resolution = resolution.trim().to_string();
    let package_manager = package_manager.trim().to_string();
    let target_dir = target_dir.trim().to_string();

    validate_project_name(&project_name)?;
    if target_dir.is_empty() {
        return Err(AppCommandError::invalid_input(
            "Target directory is required",
        ));
    }

    let full_path = PathBuf::from(&target_dir).join(&project_name);
    let full_path_str = full_path.to_string_lossy().to_string();

    // Check if directory already exists and is non-empty
    if full_path.exists() {
        let is_empty = full_path
            .read_dir()
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if !is_empty {
            return Err(AppCommandError::already_exists(
                "Target directory already exists and is not empty",
            ));
        }
    }

    // Determine the runner based on package manager (same mapping as shadcn).
    // npx gets `--yes` so it stays non-interactive when `hyperframes` isn't
    // cached; `dlx`/`bunx` already fetch-and-run by spec.
    let (program, prefix_args): (&str, Vec<&str>) = match package_manager.as_str() {
        "pnpm" => ("pnpm", vec!["dlx"]),
        "yarn" => ("yarn", vec!["dlx"]),
        "bun" => ("bunx", vec![]),
        _ => ("npx", vec!["--yes"]),
    };

    // `--example` defaults to "blank" (works offline). `--non-interactive`
    // forces the flag-driven path (a piped, non-TTY stdout already disables
    // prompts, so this is belt-and-suspenders). Skills are NOT auto-installed
    // in this mode, so there is nothing for `--skip-skills` to suppress.
    let example_arg = if example.is_empty() {
        "blank"
    } else {
        example.as_str()
    };
    // Pin the package spec (`hyperframes@latest`): a bare `hyperframes` lets npx
    // (and dlx/bunx) run a `node_modules/.bin/hyperframes` shim from the target
    // dir if one is present. Mirrors the sibling `shadcn@latest`.
    let mut args: Vec<&str> = vec![
        "hyperframes@latest",
        "init",
        project_name.as_str(),
        "--example",
        example_arg,
    ];
    if !resolution.is_empty() {
        args.push("--resolution");
        args.push(resolution.as_str());
    }
    args.push("--non-interactive");

    let mut cmd = crate::process::tokio_command(program);
    cmd.args(&prefix_args);
    cmd.args(&args);
    cmd.current_dir(&target_dir);

    let cmd_display = format!(
        "{} {} {} (cwd={})",
        program,
        prefix_args.join(" "),
        args.join(" "),
        target_dir
    );
    eprintln!("[ProjectBoot] executing: {cmd_display}");

    let output = cmd.output().await.map_err(|e| {
        eprintln!("[ProjectBoot] spawn error: {e}");
        if e.kind() == std::io::ErrorKind::NotFound {
            AppCommandError::dependency_missing(format!(
                "{program} is not installed. Please install Node.js first."
            ))
        } else {
            AppCommandError::external_command(
                "Failed to execute project creation command",
                e.to_string(),
            )
        }
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    eprintln!(
        "[ProjectBoot] exit={} stdout_len={} stderr_len={}",
        output.status,
        stdout.len(),
        stderr.len()
    );
    if !stdout.is_empty() {
        eprintln!("[ProjectBoot] stdout: {stdout}");
    }
    if !stderr.is_empty() {
        eprintln!("[ProjectBoot] stderr: {stderr}");
    }

    if !output.status.success() {
        let mut detail = String::new();
        if !stderr.is_empty() {
            detail.push_str(&stderr);
        }
        if !stdout.is_empty() {
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str(&stdout);
        }
        if detail.is_empty() {
            detail = format!("Command exited with status: {}", output.status);
        }
        return Err(AppCommandError::external_command(
            "Project creation command failed",
            detail,
        ));
    }

    Ok(full_path_str)
}
