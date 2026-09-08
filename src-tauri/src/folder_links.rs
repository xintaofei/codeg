//! Process-global registry of user-authorized workspace links.
//!
//! A workspace folder can have other directories symlinked in as subdirectories
//! (see `commands::folder_links`). Every file operation in the workspace is
//! confined by canonicalizing the target and requiring it to stay under the
//! canonical root — deliberately, so a cloned repo shipping
//! `secrets -> ~/.ssh` can't be read through the HTML preview's sub-resource
//! inlining. Following the *user's own* links therefore needs an explicit
//! allowlist, and that allowlist is this registry.
//!
//! Hydrated from `folder_link` rows at startup (mirroring
//! `custom_agent_service::hydrate_registry`) and kept in sync as links are
//! created / renamed / removed. Empty by default, so an install that never
//! links a folder behaves exactly as before.
//!
//! Paths are stored verbatim and canonicalized at check time rather than at
//! registration time: a target on a slow/absent mount that fails to resolve
//! during startup still works once it's back, and a target replaced on disk
//! never keeps its old authorization.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

#[derive(Debug, Clone, PartialEq, Eq)]
struct LinkEntry {
    /// Workspace folder path, as stored in `folder.path`.
    root: PathBuf,
    /// Linked directory, as the user picked it.
    target: PathBuf,
}

static LINKS: LazyLock<RwLock<Vec<LinkEntry>>> = LazyLock::new(|| RwLock::new(Vec::new()));

fn snapshot() -> Vec<LinkEntry> {
    LINKS
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

fn with_links<R>(f: impl FnOnce(&mut Vec<LinkEntry>) -> R) -> R {
    match LINKS.write() {
        Ok(mut guard) => f(&mut guard),
        Err(poisoned) => f(&mut poisoned.into_inner()),
    }
}

/// Load every persisted link into the registry, replacing whatever was there.
/// Called once per process right after migrations, on the single chokepoint
/// both runtimes go through.
pub async fn hydrate(
    conn: &sea_orm::DatabaseConnection,
) -> Result<usize, crate::db::error::DbError> {
    let rows = crate::db::service::folder_link_service::list_all_with_root(conn).await?;
    let entries: Vec<LinkEntry> = rows
        .into_iter()
        .map(|(root, target)| LinkEntry {
            root: PathBuf::from(root),
            target: PathBuf::from(target),
        })
        .collect();
    let count = entries.len();
    with_links(|links| *links = entries);
    Ok(count)
}

/// Authorize `target` as a link of `root`. Idempotent.
pub fn register(root: &Path, target: &Path) {
    let entry = LinkEntry {
        root: root.to_path_buf(),
        target: target.to_path_buf(),
    };
    with_links(|links| {
        if !links.contains(&entry) {
            links.push(entry);
        }
    });
}

/// Revoke a single link. A no-op when it was never registered.
pub fn unregister(root: &Path, target: &Path) {
    with_links(|links| {
        links.retain(|entry| !(entry.root == root && entry.target == target));
    });
}

/// Whether `canonical_target` lies inside a directory the user linked into
/// `canonical_root`.
///
/// Ordered target-first: this only runs after the plain "inside the root" check
/// already failed, which for a workspace with no links means the registry is
/// empty and the call costs nothing.
pub fn is_allowed(canonical_root: &Path, canonical_target: &Path) -> bool {
    let entries = snapshot();
    if entries.is_empty() {
        return false;
    }
    for entry in entries {
        let Ok(link_target) = std::fs::canonicalize(&entry.target) else {
            continue;
        };
        if !canonical_target.starts_with(&link_target) {
            continue;
        }
        let Ok(link_root) = std::fs::canonicalize(&entry.root) else {
            continue;
        };
        if link_root == canonical_root {
            return true;
        }
    }
    false
}

/// Canonicalized targets linked into `root`. Used by the file-tree and
/// workspace-search walkers to decide which symlinks to descend into; a target
/// that no longer resolves is simply absent.
pub fn canonical_targets_for(canonical_root: &Path) -> Vec<PathBuf> {
    let entries = snapshot();
    let mut out = Vec::new();
    for entry in entries {
        let Ok(link_root) = std::fs::canonicalize(&entry.root) else {
            continue;
        };
        if link_root != canonical_root {
            continue;
        }
        if let Ok(link_target) = std::fs::canonicalize(&entry.target) {
            if !out.contains(&link_target) {
                out.push(link_target);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real symlink behaviour is exercised in `commands::folder_links`; these
    // cover the registry bookkeeping itself, which is platform-independent.
    // The registry is process-global and tests share a process, so every case
    // uses paths unique to itself and never asserts on the global size.

    fn count(root: &Path, target: &Path) -> usize {
        snapshot()
            .iter()
            .filter(|e| e.root == root && e.target == target)
            .count()
    }

    #[test]
    fn register_is_idempotent_and_unregister_removes() {
        let root = Path::new("/tmp/codeg-registry-a/root");
        let target = Path::new("/tmp/codeg-registry-a/target");
        register(root, target);
        register(root, target);
        assert_eq!(count(root, target), 1);
        unregister(root, target);
        assert_eq!(count(root, target), 0);
    }

    #[test]
    fn unknown_paths_are_denied() {
        // Neither path exists, so canonicalization fails and no entry can match.
        assert!(!is_allowed(
            Path::new("/tmp/codeg-registry-b/root"),
            Path::new("/tmp/codeg-registry-b/elsewhere/file.txt")
        ));
    }
}
