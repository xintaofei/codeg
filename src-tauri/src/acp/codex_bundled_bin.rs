//! Locate the Codex executable that belongs to a `codex-acp` install, together
//! with the code-mode host that ships beside it.
//!
//! Codex decodes host IPC with `deny_unknown_fields`. A client from before
//! `code_mode_host_duration_ns` (Codex ≤ 0.151) rejects that field when the
//! host is newer, which is the `tools.shell_command` / `functions.exec` failure
//! in codeg #656. The two binaries are one package: the host Codex actually
//! spawns has to be the sibling of the executable `codex-acp` runs, not a
//! newer `codex-code-mode-host` found later on `PATH` or via `CODEX_PATH`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Native Codex binary and the directory that also holds its code-mode host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexBundledPair {
    pub codex_exe: PathBuf,
    pub host_dir: PathBuf,
}

/// Pin `CODEX_PATH` at `pair` and put that directory first on `PATH`.
///
/// A non-empty `CODEX_PATH` already in `merged` is an explicit runtime-env
/// choice and is left alone. With no pair, `CODEX_PATH` is set empty so the
/// spawn layer strips an inherited value instead of launching some other
/// Codex install's client against a newer host.
pub(crate) fn apply_codex_bundle_isolation(
    merged: &mut Vec<(String, String)>,
    pair: Option<&CodexBundledPair>,
    fallback_path: &str,
    windows: bool,
) {
    if merged
        .iter()
        .any(|(key, value)| key == "CODEX_PATH" && !value.trim().is_empty())
    {
        return;
    }
    merged.retain(|(key, _)| key != "CODEX_PATH");
    let Some(pair) = pair else {
        merged.push(("CODEX_PATH".to_string(), String::new()));
        return;
    };
    merged.push((
        "CODEX_PATH".to_string(),
        pair.codex_exe.to_string_lossy().into_owned(),
    ));
    let mut map: BTreeMap<String, String> = std::mem::take(merged).into_iter().collect();
    prepend_host_dir(
        &mut map,
        &pair.host_dir.to_string_lossy(),
        fallback_path,
        windows,
    );
    *merged = map.into_iter().collect();
}

fn prepend_host_dir(
    env: &mut BTreeMap<String, String>,
    dir: &str,
    fallback_path: &str,
    windows: bool,
) {
    let sep = if windows { ';' } else { ':' };
    let matching: Vec<String> = env
        .keys()
        .filter(|key| {
            if windows {
                key.eq_ignore_ascii_case("PATH")
            } else {
                key.as_str() == "PATH"
            }
        })
        .cloned()
        .collect();
    let mut existing = None;
    for key in &matching {
        existing = env.remove(key);
    }
    let existing = existing.unwrap_or_else(|| fallback_path.to_string());
    let new_path = if existing.is_empty() {
        dir.to_string()
    } else {
        format!("{dir}{sep}{existing}")
    };
    let key = matching
        .into_iter()
        .next_back()
        .unwrap_or_else(|| if windows { "Path" } else { "PATH" }.to_string());
    env.insert(key, new_path);
}

/// Target triple `codex.js` uses for this process, when one exists.
pub(crate) fn host_target_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => Some("aarch64-pc-windows-msvc"),
        _ => None,
    }
}

/// `codex-acp` package directory for an installed launcher (`codex-acp`,
/// `codex-acp.cmd`, or a symlink into `dist/index.js`).
pub(crate) fn codex_acp_package_dir(command: &Path) -> Option<PathBuf> {
    let mut starts = Vec::new();
    if let Ok(canon) = command.canonicalize() {
        starts.push(canon);
    }
    starts.push(command.to_path_buf());
    for start in starts {
        if let Some(dir) = package_dir_from_start(&start) {
            return Some(dir);
        }
    }
    None
}

/// Codex + host pair for the launcher `command`, using this machine's triple.
pub(crate) fn bundled_pair_for_command(command: &Path) -> Option<CodexBundledPair> {
    let triple = host_target_triple()?;
    let acp_dir = codex_acp_package_dir(command)?;
    resolve_bundled_pair(&acp_dir, triple)
}

/// First Codex install under `acp_dir` whose platform package contains both
/// `codex` and `codex-code-mode-host` in the same `vendor/<triple>/bin`.
///
/// Nearest `node_modules` wins, matching Node's resolver, but a directory that
/// has only the client (or only the host) is skipped. Launching that client
/// is what lets a newer host from another install answer the IPC frame.
pub(crate) fn resolve_bundled_pair(acp_dir: &Path, triple: &str) -> Option<CodexBundledPair> {
    let platform = platform_package_name(triple)?;
    let (exe_name, host_name) = binary_names(triple);
    // Stop at the npm prefix (the directory that contains this package's
    // `node_modules`). Walking further would treat an unrelated checkout's
    // Codex as the one this adapter installed.
    let stop = npm_prefix(acp_dir);
    let mut dir = acp_dir.to_path_buf();
    loop {
        let codex_root = dir.join("node_modules").join("@openai").join("codex");
        if codex_root.join("package.json").is_file() {
            if let Some(pair) =
                pair_from_codex_root(&codex_root, &stop, triple, platform, exe_name, host_name)
            {
                return Some(pair);
            }
        }
        if dir == stop || !dir.pop() {
            break;
        }
    }
    None
}

fn npm_prefix(acp_dir: &Path) -> PathBuf {
    let mut dir = acp_dir.to_path_buf();
    loop {
        if dir.file_name().and_then(|name| name.to_str()) == Some("node_modules") {
            return dir.parent().unwrap_or(acp_dir).to_path_buf();
        }
        if !dir.pop() {
            return acp_dir.to_path_buf();
        }
    }
}

fn pair_from_codex_root(
    codex_root: &Path,
    stop: &Path,
    triple: &str,
    platform: &str,
    exe_name: &str,
    host_name: &str,
) -> Option<CodexBundledPair> {
    let mut dir = codex_root.to_path_buf();
    loop {
        let platform_dir = dir.join("node_modules").join("@openai").join(platform);
        if let Some(pair) = complete_pair(&platform_dir, triple, exe_name, host_name) {
            return Some(pair);
        }
        if dir == stop || !dir.pop() {
            break;
        }
    }
    // `codex.js` falls back to `<package>/vendor` when the optional platform
    // package does not resolve.
    complete_pair(codex_root, triple, exe_name, host_name)
}

fn complete_pair(
    package_dir: &Path,
    triple: &str,
    exe_name: &str,
    host_name: &str,
) -> Option<CodexBundledPair> {
    let host_dir = package_dir.join("vendor").join(triple).join("bin");
    let codex_exe = host_dir.join(exe_name);
    let host = host_dir.join(host_name);
    if codex_exe.is_file() && host.is_file() {
        Some(CodexBundledPair {
            codex_exe,
            host_dir,
        })
    } else {
        None
    }
}

fn package_dir_from_start(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    for _ in 0..10 {
        if is_codex_acp_dir(&dir) {
            return Some(dir);
        }
        let nested = dir
            .join("node_modules")
            .join("@agentclientprotocol")
            .join("codex-acp");
        if is_codex_acp_dir(&nested) {
            return Some(nested);
        }
        if dir.file_name().and_then(|name| name.to_str()) == Some("bin") {
            if let Some(parent) = dir.parent() {
                let via_lib = parent
                    .join("lib")
                    .join("node_modules")
                    .join("@agentclientprotocol")
                    .join("codex-acp");
                if is_codex_acp_dir(&via_lib) {
                    return Some(via_lib);
                }
            }
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

fn is_codex_acp_dir(dir: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value.get("name").and_then(|name| name.as_str()) == Some("@agentclientprotocol/codex-acp")
}

fn platform_package_name(triple: &str) -> Option<&'static str> {
    Some(match triple {
        "x86_64-unknown-linux-musl" => "codex-linux-x64",
        "aarch64-unknown-linux-musl" => "codex-linux-arm64",
        "x86_64-apple-darwin" => "codex-darwin-x64",
        "aarch64-apple-darwin" => "codex-darwin-arm64",
        "x86_64-pc-windows-msvc" => "codex-win32-x64",
        "aarch64-pc-windows-msvc" => "codex-win32-arm64",
        _ => return None,
    })
}

fn binary_names(triple: &str) -> (&'static str, &'static str) {
    if triple.contains("windows") {
        ("codex.exe", "codex-code-mode-host.exe")
    } else {
        ("codex", "codex-code-mode-host")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIPLE: &str = "x86_64-pc-windows-msvc";

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"x").unwrap();
    }

    fn write_acp_package(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"@agentclientprotocol/codex-acp","version":"1.12.0"}"#,
        )
        .unwrap();
    }

    fn write_codex_package(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"@openai/codex","version":"0.154.0"}"#,
        )
        .unwrap();
    }

    fn vendor_bin(platform_dir: &Path) -> PathBuf {
        platform_dir.join("vendor").join(TRIPLE).join("bin")
    }

    #[test]
    fn prefers_the_nested_pair_over_a_hoisted_host_only_install() {
        let root = tempfile::tempdir().unwrap();
        let acp = root
            .path()
            .join("node_modules")
            .join("@agentclientprotocol")
            .join("codex-acp");
        write_acp_package(&acp);
        let nested_codex = acp.join("node_modules").join("@openai").join("codex");
        write_codex_package(&nested_codex);
        let nested_bin = vendor_bin(
            &nested_codex
                .join("node_modules")
                .join("@openai")
                .join("codex-win32-x64"),
        );
        touch(&nested_bin.join("codex.exe"));
        touch(&nested_bin.join("codex-code-mode-host.exe"));

        // A hoisted install that has only the newer host. Selecting it would
        // be the unknown-field failure: an older client talking to this host.
        let hoisted_bin = vendor_bin(
            &root
                .path()
                .join("node_modules")
                .join("@openai")
                .join("codex-win32-x64"),
        );
        touch(&hoisted_bin.join("codex-code-mode-host.exe"));

        let pair = resolve_bundled_pair(&acp, TRIPLE).unwrap();
        assert_eq!(pair.codex_exe, nested_bin.join("codex.exe"));
        assert_eq!(pair.host_dir, nested_bin);
    }

    #[test]
    fn skips_a_client_whose_directory_has_no_host() {
        let root = tempfile::tempdir().unwrap();
        let acp = root
            .path()
            .join("node_modules")
            .join("@agentclientprotocol")
            .join("codex-acp");
        write_acp_package(&acp);
        let nested_codex = acp.join("node_modules").join("@openai").join("codex");
        write_codex_package(&nested_codex);
        let incomplete = vendor_bin(
            &nested_codex
                .join("node_modules")
                .join("@openai")
                .join("codex-win32-x64"),
        );
        touch(&incomplete.join("codex.exe"));

        // Platform package hoisted beside `@openai/codex`, which is where Node
        // looks after the nested optional dependency has no host.
        let hoisted_platform = vendor_bin(
            &root
                .path()
                .join("node_modules")
                .join("@openai")
                .join("codex-win32-x64"),
        );
        touch(&hoisted_platform.join("codex.exe"));
        touch(&hoisted_platform.join("codex-code-mode-host.exe"));

        let pair = resolve_bundled_pair(&acp, TRIPLE).unwrap();
        assert_eq!(pair.codex_exe, hoisted_platform.join("codex.exe"));
        assert_eq!(pair.host_dir, hoisted_platform);
    }

    #[test]
    fn platform_package_names_match_codex_js() {
        assert_eq!(
            platform_package_name("x86_64-unknown-linux-musl"),
            Some("codex-linux-x64")
        );
        assert_eq!(
            platform_package_name("aarch64-unknown-linux-musl"),
            Some("codex-linux-arm64")
        );
        assert_eq!(
            platform_package_name("x86_64-apple-darwin"),
            Some("codex-darwin-x64")
        );
        assert_eq!(
            platform_package_name("aarch64-apple-darwin"),
            Some("codex-darwin-arm64")
        );
        assert_eq!(
            platform_package_name("x86_64-pc-windows-msvc"),
            Some("codex-win32-x64")
        );
        assert_eq!(
            platform_package_name("aarch64-pc-windows-msvc"),
            Some("codex-win32-arm64")
        );
        assert_eq!(platform_package_name("wasm32-unknown-unknown"), None);
    }

    #[test]
    fn bundled_pair_for_command_uses_this_hosts_triple() {
        let Some(triple) = host_target_triple() else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let acp = root
            .path()
            .join("node_modules")
            .join("@agentclientprotocol")
            .join("codex-acp");
        write_acp_package(&acp);
        let codex = acp.join("node_modules").join("@openai").join("codex");
        write_codex_package(&codex);
        let (exe_name, host_name) = binary_names(triple);
        let bin = codex.join("vendor").join(triple).join("bin");
        touch(&bin.join(exe_name));
        touch(&bin.join(host_name));
        // A Windows pair must not be selected when this process is not Windows.
        if !triple.contains("windows") {
            let windows = codex
                .join("vendor")
                .join("x86_64-pc-windows-msvc")
                .join("bin");
            touch(&windows.join("codex.exe"));
            touch(&windows.join("codex-code-mode-host.exe"));
        }

        let cmd = root.path().join("codex-acp.cmd");
        touch(&cmd);
        let pair = bundled_pair_for_command(&cmd).unwrap();
        assert_eq!(pair.codex_exe, bin.join(exe_name));
        assert_eq!(pair.host_dir, bin);
    }

    #[test]
    fn command_on_an_npm_prefix_finds_the_package_and_the_pair() {
        let root = tempfile::tempdir().unwrap();
        let acp = root
            .path()
            .join("node_modules")
            .join("@agentclientprotocol")
            .join("codex-acp");
        write_acp_package(&acp);
        let codex = acp.join("node_modules").join("@openai").join("codex");
        write_codex_package(&codex);
        let bin = vendor_bin(&codex);
        touch(&bin.join("codex.exe"));
        touch(&bin.join("codex-code-mode-host.exe"));

        let cmd = root.path().join("codex-acp.cmd");
        touch(&cmd);
        let pair = resolve_bundled_pair(&codex_acp_package_dir(&cmd).unwrap(), TRIPLE).unwrap();
        assert_eq!(pair.host_dir, bin);
    }

    #[test]
    fn unix_prefix_bin_finds_lib_node_modules() {
        let root = tempfile::tempdir().unwrap();
        let acp = root
            .path()
            .join("lib")
            .join("node_modules")
            .join("@agentclientprotocol")
            .join("codex-acp");
        write_acp_package(&acp);
        let cmd_dir = root.path().join("bin");
        std::fs::create_dir_all(&cmd_dir).unwrap();
        let cmd = cmd_dir.join("codex-acp");
        touch(&cmd);

        assert_eq!(codex_acp_package_dir(&cmd).unwrap(), acp);
    }

    #[test]
    fn host_without_a_client_is_not_a_pair() {
        let root = tempfile::tempdir().unwrap();
        let acp = root.path().join("codex-acp");
        write_acp_package(&acp);
        let codex = acp.join("node_modules").join("@openai").join("codex");
        write_codex_package(&codex);
        let bin = vendor_bin(&codex);
        touch(&bin.join("codex-code-mode-host.exe"));
        assert!(resolve_bundled_pair(&acp, TRIPLE).is_none());
    }

    #[test]
    fn isolation_pins_codex_path_and_leads_path_with_the_host_dir() {
        let pair = CodexBundledPair {
            codex_exe: PathBuf::from("/opt/codex/vendor/bin/codex.exe"),
            host_dir: PathBuf::from("/opt/codex/vendor/bin"),
        };
        let mut env = vec![("PATH".to_string(), r"C:\Windows".to_string())];
        apply_codex_bundle_isolation(&mut env, Some(&pair), r"C:\fallback", true);
        assert!(env.iter().any(|(key, value)| {
            key == "CODEX_PATH" && value == "/opt/codex/vendor/bin/codex.exe"
        }));
        let path = env
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
            .unwrap();
        assert!(
            path.1.starts_with(r"/opt/codex/vendor/bin;C:\Windows"),
            "{}",
            path.1
        );
    }

    #[test]
    fn isolation_keeps_an_explicit_codex_path() {
        let pair = CodexBundledPair {
            codex_exe: PathBuf::from("/bundled/codex"),
            host_dir: PathBuf::from("/bundled"),
        };
        let mut env = vec![
            ("CODEX_PATH".to_string(), "/custom/codex".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
        ];
        apply_codex_bundle_isolation(&mut env, Some(&pair), "/fallback", false);
        assert!(env
            .iter()
            .any(|(key, value)| key == "CODEX_PATH" && value == "/custom/codex"));
        assert!(env
            .iter()
            .any(|(key, value)| key == "PATH" && value == "/usr/bin"));
    }

    #[test]
    fn isolation_clears_codex_path_when_no_sibling_pair_exists() {
        let mut env = vec![("PATH".to_string(), "/usr/bin".to_string())];
        apply_codex_bundle_isolation(&mut env, None, "/usr/bin", false);
        assert!(env
            .iter()
            .any(|(key, value)| key == "CODEX_PATH" && value.is_empty()));
    }
}
