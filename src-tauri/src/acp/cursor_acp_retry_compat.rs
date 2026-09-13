//! Version-gated compatibility patch for Cursor ACP sessions that omit
//! `enableAgentRetries` from `agentClient.run` options (see `agent-session.ts`).
//!
//! Mirrors Cursor's existing `w5(action.case)` retry policy from TUI/headless:
//! user/resume actions enable transport retry; shell/background/goal actions do not.

use std::path::{Path, PathBuf};

use crate::acp::error::AcpError;

const CURSOR_AGENT_ID: &str = "cursor";

/// Cursor agent-cli versions known to ship the vulnerable ACP runOptions omission.
const AFFECTED_VERSIONS: &[&str] = &["2026.09.02-c22c1a3"];

const AGENT_SESSION_MODULE: &str = "\"./src/acp/agent-session.ts\"";

/// Vulnerable minified runOptions tail in affected bundles (no `enableAgentRetries`).
const VULNERABLE_RUN_OPTIONS: &str =
    ")),{onConnectionStateChange:e=>{\"reconnecting\"===e.state?(0,S.debugLog)(\"Connection state: reconnecting\"):\"connected\"===e.state&&(0,S.debugLog)(\"Connection state: connected\")},onErrorNotRetried:e=>{(0,P.Z)({configProvider:this.sharedServices.configProvider,info:e})}})";

/// Patched runOptions: inline equivalent of `w5(I.action.case)`.
const PATCHED_RUN_OPTIONS: &str =
    ")),{enableAgentRetries:\"shellCommandAction\"!==I.action.case&&\"backgroundTaskCompletionAction\"!==I.action.case&&\"goalContinuationAction\"!==I.action.case,onConnectionStateChange:e=>{\"reconnecting\"===e.state?(0,S.debugLog)(\"Connection state: reconnecting\"):\"connected\"===e.state&&(0,S.debugLog)(\"Connection state: connected\")},onErrorNotRetried:e=>{(0,P.Z)({configProvider:this.sharedServices.configProvider,info:e})}})";

/// Marker written by this patch or upstream fixes.
const ENABLE_AGENT_RETRIES_MARKER: &str = "enableAgentRetries:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatPatchStatus {
    NotApplicable,
    AlreadyFixed,
    Applied,
    PatternMismatch,
}

impl CompatPatchStatus {
    pub fn log_label(self) -> &'static str {
        match self {
            Self::NotApplicable => "NOT_APPLICABLE",
            Self::AlreadyFixed => "ALREADY_FIXED",
            Self::Applied => "APPLIED",
            Self::PatternMismatch => "PATTERN_MISMATCH",
        }
    }
}

/// Apply the compatibility patch when `platform_dir` holds a managed Cursor
/// install for a known-affected version. Idempotent and fail-closed.
pub fn maybe_apply(platform_dir: &Path, version: &str) -> CompatPatchStatus {
    let normalized = normalize_version_label(version);
    if !AFFECTED_VERSIONS.iter().any(|v| *v == normalized) {
        return CompatPatchStatus::NotApplicable;
    }

    let dist_package = platform_dir.join("dist-package");
    if !dist_package.is_dir() {
        log_status(CompatPatchStatus::PatternMismatch, &normalized, None);
        return CompatPatchStatus::PatternMismatch;
    }

    let bundle_path = match find_agent_session_bundle(&dist_package) {
        Some(path) => path,
        None => {
            log_status(CompatPatchStatus::PatternMismatch, &normalized, None);
            return CompatPatchStatus::PatternMismatch;
        }
    };

    let content = match std::fs::read_to_string(&bundle_path) {
        Ok(content) => content,
        Err(err) => {
            tracing::warn!(
                "Cursor ACP retry compatibility patch: read failed (version={}, bundle={}, err={})",
                normalized,
                bundle_path.display(),
                err
            );
            log_status(
                CompatPatchStatus::PatternMismatch,
                &normalized,
                Some(&bundle_path),
            );
            return CompatPatchStatus::PatternMismatch;
        }
    };

    if !content.contains(AGENT_SESSION_MODULE) {
        log_status(
            CompatPatchStatus::PatternMismatch,
            &normalized,
            Some(&bundle_path),
        );
        return CompatPatchStatus::PatternMismatch;
    }

    if content.contains(ENABLE_AGENT_RETRIES_MARKER) {
        log_status(
            CompatPatchStatus::AlreadyFixed,
            &normalized,
            Some(&bundle_path),
        );
        return CompatPatchStatus::AlreadyFixed;
    }

    if !content.contains(VULNERABLE_RUN_OPTIONS) {
        log_status(
            CompatPatchStatus::PatternMismatch,
            &normalized,
            Some(&bundle_path),
        );
        return CompatPatchStatus::PatternMismatch;
    }

    let patched = content.replace(VULNERABLE_RUN_OPTIONS, PATCHED_RUN_OPTIONS);
    if patched == content {
        log_status(
            CompatPatchStatus::PatternMismatch,
            &normalized,
            Some(&bundle_path),
        );
        return CompatPatchStatus::PatternMismatch;
    }

    if let Err(err) = write_atomically(&bundle_path, &patched) {
        tracing::warn!(
            "Cursor ACP retry compatibility patch: write failed (version={}, bundle={}, err={})",
            normalized,
            bundle_path.display(),
            err
        );
        log_status(
            CompatPatchStatus::PatternMismatch,
            &normalized,
            Some(&bundle_path),
        );
        return CompatPatchStatus::PatternMismatch;
    }

    log_status(CompatPatchStatus::Applied, &normalized, Some(&bundle_path));
    CompatPatchStatus::Applied
}

/// Convenience wrapper for managed-cache installs keyed by agent id.
pub fn maybe_apply_for_agent(
    agent_id: &str,
    platform_dir: &Path,
    version: &str,
) -> CompatPatchStatus {
    if agent_id != CURSOR_AGENT_ID {
        return CompatPatchStatus::NotApplicable;
    }
    maybe_apply(platform_dir, version)
}

fn find_agent_session_bundle(dist_package: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dist_package).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str())?;
        if !name.ends_with(".index.js") {
            continue;
        }
        let content = std::fs::read_to_string(&path).ok()?;
        if content.contains(AGENT_SESSION_MODULE) {
            return Some(path);
        }
    }
    None
}

fn write_atomically(path: &Path, content: &str) -> Result<(), AcpError> {
    let parent = path
        .parent()
        .ok_or_else(|| AcpError::DownloadFailed("bundle path has no parent".into()))?;
    let tmp = parent.join(format!(".codeg-acp-retry-patch-{}.tmp", std::process::id()));
    std::fs::write(&tmp, content.as_bytes())
        .map_err(|e| AcpError::DownloadFailed(format!("write temp bundle patch: {e}")))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| AcpError::DownloadFailed(format!("commit bundle patch: {e}")))?;
    Ok(())
}

fn normalize_version_label(version: &str) -> String {
    let trimmed = version.trim();
    if let Some(stripped) = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
    {
        stripped.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn log_status(status: CompatPatchStatus, version: &str, bundle: Option<&Path>) {
    match bundle {
        Some(path) => tracing::info!(
            "Cursor ACP retry compatibility patch: {} (version={}, bundle={})",
            status.log_label(),
            version,
            path.display()
        ),
        None => tracing::info!(
            "Cursor ACP retry compatibility patch: {} (version={})",
            status.log_label(),
            version
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vulnerable_fixture() -> String {
        format!(
            "exports.modules={{{}(e,t,o){{placeholder {} end}}}}",
            AGENT_SESSION_MODULE, VULNERABLE_RUN_OPTIONS
        )
    }

    fn write_bundle(dir: &Path, content: &str) -> PathBuf {
        let dist = dir.join("dist-package");
        std::fs::create_dir_all(&dist).unwrap();
        let path = dist.join("2471.index.js");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn unknown_version_is_not_applicable() {
        let tmp = tempfile::tempdir().unwrap();
        let status = maybe_apply(tmp.path(), "2026.08.11-e8db854");
        assert_eq!(status, CompatPatchStatus::NotApplicable);
    }

    #[test]
    fn affected_version_without_bundle_is_pattern_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let status = maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]);
        assert_eq!(status, CompatPatchStatus::PatternMismatch);
    }

    #[test]
    fn affected_vulnerable_bundle_is_patched_once() {
        let tmp = tempfile::tempdir().unwrap();
        write_bundle(tmp.path(), &vulnerable_fixture());
        let status = maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]);
        assert_eq!(status, CompatPatchStatus::Applied);
        let bundle = find_agent_session_bundle(&tmp.path().join("dist-package")).unwrap();
        let content = std::fs::read_to_string(bundle).unwrap();
        assert!(content.contains(PATCHED_RUN_OPTIONS));
        assert!(content.contains("enableAgentRetries:\"shellCommandAction\"!==I.action.case"));
    }

    #[test]
    fn second_application_is_already_fixed() {
        let tmp = tempfile::tempdir().unwrap();
        write_bundle(tmp.path(), &vulnerable_fixture());
        assert_eq!(
            maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::Applied
        );
        assert_eq!(
            maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::AlreadyFixed
        );
    }

    #[test]
    fn upstream_fixed_bundle_is_already_fixed() {
        let tmp = tempfile::tempdir().unwrap();
        let content = vulnerable_fixture().replace(
            VULNERABLE_RUN_OPTIONS,
            ")),{enableAgentRetries:(0,w5.w5)(I.action.case),onConnectionStateChange:e=>{}",
        );
        write_bundle(tmp.path(), &content);
        assert_eq!(
            maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::AlreadyFixed
        );
    }

    #[test]
    fn unexpected_bundle_structure_is_pattern_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        write_bundle(
            tmp.path(),
            "exports.modules={{\"./src/acp/agent-session.ts\"(){broken",
        );
        assert_eq!(
            maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::PatternMismatch
        );
    }

    #[test]
    fn patch_preserves_w5_action_policy() {
        assert!(PATCHED_RUN_OPTIONS.contains("\"shellCommandAction\"!==I.action.case"));
        assert!(PATCHED_RUN_OPTIONS.contains("\"backgroundTaskCompletionAction\"!==I.action.case"));
        assert!(PATCHED_RUN_OPTIONS.contains("\"goalContinuationAction\"!==I.action.case"));
    }

    #[test]
    fn non_cursor_agent_id_is_not_applicable() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            maybe_apply_for_agent("opencode", tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::NotApplicable
        );
    }
}
