//! Runtime registry for **custom (user-registered) ACP agents**.
//!
//! Built-in agents are compile-time constants in [`crate::acp::registry`].
//! Custom agents are rows in the `custom_agent` table, so their launch
//! metadata only exists at runtime — but every consumer of
//! [`crate::acp::registry::get_agent_meta`] expects a `&'static str`-shaped
//! [`AcpAgentMeta`] from a synchronous, infallible call.
//!
//! This module bridges the two: [`hydrate`] converts database definitions into
//! `&'static AcpAgentMeta` values (via [`crate::intern`]) and publishes them in
//! a process-global map that `get_agent_meta` consults. Consumers are
//! unchanged — `AgentDistribution` keeps its `&'static str` fields, and the
//! 57 destructuring sites across `commands/acp.rs`, `binary_cache.rs`,
//! `preflight.rs` and `connection.rs` need no `Cow` conversion.
//!
//! ## Leak discipline
//!
//! Promoting to `&'static` means leaking. Each definition is fingerprinted
//! (`serde_json` of the stored spec), and [`hydrate`] reuses the previously
//! leaked meta whenever the fingerprint is unchanged — so re-hydrating an
//! untouched registry (which happens on every startup and after every unrelated
//! CRUD call) allocates nothing. Only an actual edit to a definition leaks a
//! fresh meta, bounding the leak by the number of edits in one app run.
//!
//! ## Spec format
//!
//! [`CustomAgentSpec`] is intentionally **paste-compatible with the ACP
//! registry's `distribution` object**
//! (`https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`), so
//! a user can copy an entry verbatim. The few things the registry does not
//! carry — the npx/uvx console-script name, and runtime version floors — are
//! optional extra keys with derived defaults.

use std::collections::{BTreeMap, HashMap};
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

use crate::acp::registry::{AcpAgentMeta, AgentDistribution, BinaryDirEntry, PlatformBinary};
use crate::intern::{intern, leak, leak_slice};
use crate::models::agent::{is_valid_custom_agent_id, AgentType};

/// Which distribution channel a custom agent launches through. Stored as a
/// column so an agent that publishes BOTH a binary and an npx package (the
/// ACP registry has two such entries) has a stable, user-chosen answer rather
/// than an implicit preference that could flip between releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomDistributionKind {
    Npx,
    Uvx,
    Binary,
}

impl CustomDistributionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CustomDistributionKind::Npx => "npx",
            CustomDistributionKind::Uvx => "uvx",
            CustomDistributionKind::Binary => "binary",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "npx" => Some(CustomDistributionKind::Npx),
            "uvx" => Some(CustomDistributionKind::Uvx),
            "binary" => Some(CustomDistributionKind::Binary),
            _ => None,
        }
    }
}

/// npx channel — mirrors the registry's `distribution.npx`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NpxSpec {
    /// Full npm spec including the version, e.g. `@qwen-code/qwen-code@0.21.0`.
    pub package: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Console-script name the package installs (e.g. `qwen` for
    /// `@qwen-code/qwen-code`). The ACP registry does NOT carry this, so it is
    /// resolved from npm metadata when adding through the picker and falls
    /// back to [`derive_command_name`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    /// Minimum Node.js version, surfaced by preflight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_required: Option<String>,
}

/// uvx channel — mirrors the registry's `distribution.uvx`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UvxSpec {
    /// `uvx --from` spec, e.g. `fast-agent-acp==0.9.24`.
    pub package: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Console-script entry point. See [`NpxSpec::cmd`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uv_required: Option<String>,
    /// Interpreter to pin via `uvx --python <ver>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
}

/// One platform's binary release — mirrors the registry's
/// `distribution.binary.<platform>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BinaryPlatformSpec {
    /// Archive download URL.
    #[serde(alias = "url")]
    pub archive: String,
    /// Launch path INSIDE the archive, registry-style (`./goose`,
    /// `dist-package/cursor-agent`, `amp-acp.exe`).
    #[serde(default)]
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Hex SHA-256 of the archive. Verified before use when present — custom
    /// agents download from user-supplied URLs, unlike built-ins whose URLs are
    /// repository constants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// The stored `spec_json`: the ACP registry `distribution` object, verbatim.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomAgentSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npx: Option<NpxSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uvx: Option<UvxSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub binary: BTreeMap<String, BinaryPlatformSpec>,
}

/// A full custom-agent definition as stored in the `custom_agent` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomAgentDef {
    pub registry_id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Display/pin version. Defaults to [`FALLBACK_VERSION`] when neither the
    /// registry nor the user supplied one.
    pub version: String,
    pub distribution_kind: CustomDistributionKind,
    pub spec: CustomAgentSpec,
    /// The agent's icon, for display only — never used to launch anything.
    ///
    /// Normally a `data:image/…;base64,…` URL: the add path downloads the
    /// registry's SVG once (see `commands::custom_agents::inline_icon`) and the
    /// manual form reads an uploaded file, so rendering needs no network and
    /// works offline and in the server build alike. A plain `https://` URL is
    /// still accepted (definitions stored before inlining existed, or a
    /// hand-written one) and is passed through to the webview as-is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    /// User declaration that this agent reads the shared `.agents/skills`
    /// store (`~/.agents/skills` plus project-local `.agents/skills`) — the
    /// cross-agent convention OpenCode, Gemini, Cline, Codex, pi, and Cursor
    /// already follow. codeg cannot detect where an arbitrary ACP agent loads
    /// skills from, so this stays off until the user turns it on; it is the
    /// single gate that puts the agent into every skills matrix (see
    /// `skill_storage_spec`).
    #[serde(default)]
    pub skills_shared_store: bool,
}

/// Version stamped on a definition that carries none. Used as a cache key and
/// as the displayed "registry version", never as an install pin (npx/uvx pin
/// through the `package` spec, which embeds its own version).
pub const FALLBACK_VERSION: &str = "custom";

/// Derive a plausible console-script name from a package spec when the
/// definition does not state one.
///
/// `@qwen-code/qwen-code@0.21.0` → `qwen-code`; `fast-agent-acp==0.9.24` →
/// `fast-agent-acp`; `hermes-agent[acp,mcp]==0.19.0` → `hermes-agent`. This is
/// a guess — many packages install a bin whose name differs from the package
/// (e.g. `@github/copilot` installs `copilot`) — so the add path prefers the
/// authoritative value from npm metadata and only falls back here.
pub fn derive_command_name(package: &str) -> String {
    let mut s = package;
    // Strip a PEP 508 version specifier / extras (`pkg[a,b]==1.2.3`).
    for sep in ["==", ">=", "<=", "~=", "!=", ">", "<"] {
        if let Some(idx) = s.find(sep) {
            s = &s[..idx];
        }
    }
    if let Some(idx) = s.find('[') {
        s = &s[..idx];
    }
    // Strip an npm version suffix, being careful not to eat a leading scope
    // `@` (`@scope/name@1.2.3` — the version `@` is the one after the slash).
    let search_from = if s.starts_with('@') {
        s.find('/').map(|i| i + 1).unwrap_or(0)
    } else {
        0
    };
    if let Some(idx) = s[search_from..].find('@') {
        s = &s[..search_from + idx];
    }
    // Drop the npm scope.
    if let Some(idx) = s.rfind('/') {
        s = &s[idx + 1..];
    }
    s.trim().to_string()
}

/// Reason a definition cannot be turned into launch metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomAgentError {
    InvalidId(String),
    EmptyName,
    /// The chosen channel has no entry in the spec.
    MissingChannel(&'static str),
    MissingPackage(&'static str),
    /// The binary channel has no release for the machine's platform.
    UnsupportedPlatform(String),
    MissingArchiveUrl(String),
    /// The version label would not be usable as a single directory name.
    UnsafeVersion(String),
    /// A platform's in-archive launch path escapes the extracted tree.
    UnsafeArchiveEntry(String),
}

impl std::fmt::Display for CustomAgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CustomAgentError::InvalidId(id) => write!(
                f,
                "invalid custom agent id {id:?}: use letters, digits, '-', '_' or '.' (max 64), and not a built-in agent name"
            ),
            CustomAgentError::EmptyName => write!(f, "custom agent name must not be empty"),
            CustomAgentError::MissingChannel(kind) => {
                write!(f, "spec has no `{kind}` distribution entry")
            }
            CustomAgentError::MissingPackage(kind) => {
                write!(f, "`{kind}` distribution has an empty package")
            }
            CustomAgentError::UnsupportedPlatform(p) => {
                write!(f, "no binary release for this platform ({p})")
            }
            CustomAgentError::MissingArchiveUrl(p) => {
                write!(f, "binary release for {p} has no archive URL")
            }
            CustomAgentError::UnsafeVersion(v) => write!(
                f,
                "version {v:?} is not usable as a directory name: use letters, digits, '-', '_' or '.' (max 128, no leading dot)"
            ),
            CustomAgentError::UnsafeArchiveEntry(p) => write!(
                f,
                "binary release for {p} has a launch path that escapes the archive: it must be relative, with no '..' segment"
            ),
        }
    }
}

impl std::error::Error for CustomAgentError {}

/// Whether `s` is usable as ONE directory name.
///
/// The version label becomes a path segment of the binary cache
/// (`binary_cache::binary_dir`), which codeg also *deletes* when reinstalling —
/// so a definition must not be able to steer it out of the cache. Mirrors
/// `acp_transcript::safe_component`, and deliberately rejects a Windows drive
/// or alternate-data-stream prefix (`C:`, `f:s`) too, which `Path::is_absolute`
/// would not catch on a unix build.
fn is_safe_path_component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        // Also rules out "." and "..", which the byte check alone allows.
        && !s.starts_with('.')
}

/// Whether an in-archive launch path stays inside the extracted tree.
///
/// `binary_cache` joins this onto the install directory and then makes the
/// result executable, so an absolute or `..`-bearing entry would chmod and
/// launch a file of the definition's choosing anywhere on disk. Checked against
/// the NORMALIZED form (see [`split_archive_entry`]), which has already folded
/// `\` into `/`, so a Windows-style `..\..\x` is caught on a unix build too.
fn is_safe_archive_entry(raw: &str) -> bool {
    let (path, _) = split_archive_entry(raw);
    if path.is_empty() {
        // No entry at all — the command name falls back to the archive stem.
        return true;
    }
    // A leading `/` is the unix absolute form and also what a UNC `\\host\share`
    // normalizes to; `:` is a drive or stream prefix.
    if path.starts_with('/') || path.contains(':') {
        return false;
    }
    path.split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// The version label [`build_meta`] would stamp on a definition: its own, or
/// [`FALLBACK_VERSION`] when it carries none.
fn effective_version(def: &CustomAgentDef) -> &str {
    let trimmed = def.version.trim();
    if trimmed.is_empty() {
        FALLBACK_VERSION
    } else {
        trimmed
    }
}

/// Check a definition **without building (and therefore leaking) anything**.
///
/// [`build_meta`] promotes strings to `&'static` by leaking them, so it must
/// only run where the result is kept — hydration, which is fingerprint-gated.
/// Callers that merely want to know whether a definition is usable (the
/// settings list renders a "why not" for each row, on every refresh) call this
/// instead. `build_meta` runs the same checks by calling it first, so the two
/// can never disagree about what is valid.
pub fn validate(def: &CustomAgentDef) -> Result<(), CustomAgentError> {
    if !is_valid_custom_agent_id(&def.registry_id) {
        return Err(CustomAgentError::InvalidId(def.registry_id.clone()));
    }
    if def.name.trim().is_empty() {
        return Err(CustomAgentError::EmptyName);
    }
    let version = effective_version(def);
    if !is_safe_path_component(version) {
        return Err(CustomAgentError::UnsafeVersion(version.to_string()));
    }
    match def.distribution_kind {
        CustomDistributionKind::Npx => {
            let npx = def
                .spec
                .npx
                .as_ref()
                .ok_or(CustomAgentError::MissingChannel("npx"))?;
            if npx.package.trim().is_empty() {
                return Err(CustomAgentError::MissingPackage("npx"));
            }
        }
        CustomDistributionKind::Uvx => {
            let uvx = def
                .spec
                .uvx
                .as_ref()
                .ok_or(CustomAgentError::MissingChannel("uvx"))?;
            if uvx.package.trim().is_empty() {
                return Err(CustomAgentError::MissingPackage("uvx"));
            }
        }
        CustomDistributionKind::Binary => {
            if def.spec.binary.is_empty() {
                return Err(CustomAgentError::MissingChannel("binary"));
            }
            let current = crate::acp::registry::current_platform();
            let here = def
                .spec
                .binary
                .get(current)
                .ok_or_else(|| CustomAgentError::UnsupportedPlatform(current.to_string()))?;
            if here.archive.trim().is_empty() {
                return Err(CustomAgentError::MissingArchiveUrl(current.to_string()));
            }
            // EVERY platform's entry, not just this machine's: a nested layout
            // makes `build_binary_distribution` borrow another family's path
            // for the `dir_entry` it publishes.
            for (platform, binary) in &def.spec.binary {
                if !is_safe_archive_entry(&binary.cmd) {
                    return Err(CustomAgentError::UnsafeArchiveEntry(platform.clone()));
                }
            }
        }
    }
    Ok(())
}

/// Validate a definition and convert it into leaked launch metadata.
///
/// Every string that ends up in [`AcpAgentMeta`] is interned, so calling this
/// twice with equal input leaks only the slices (which [`hydrate`] avoids via
/// fingerprinting). **Call [`validate`] instead when the metadata is not kept**
/// — those slice leaks are not deduplicated, so building a throwaway meta on a
/// repeatable request would grow the process without bound.
pub fn build_meta(def: &CustomAgentDef) -> Result<AcpAgentMeta, CustomAgentError> {
    validate(def)?;
    let agent_type = AgentType::Custom(intern(&def.registry_id));
    let version = intern(effective_version(def));

    let distribution = match def.distribution_kind {
        CustomDistributionKind::Npx => {
            let npx = def
                .spec
                .npx
                .as_ref()
                .ok_or(CustomAgentError::MissingChannel("npx"))?;
            if npx.package.trim().is_empty() {
                return Err(CustomAgentError::MissingPackage("npx"));
            }
            AgentDistribution::Npx {
                version,
                package: intern(npx.package.trim()),
                cmd: intern(&resolved_cmd(npx.cmd.as_deref(), &npx.package)),
                args: intern_args(&npx.args),
                env: intern_env(&npx.env),
                node_required: npx.node_required.as_deref().map(intern),
            }
        }
        CustomDistributionKind::Uvx => {
            let uvx = def
                .spec
                .uvx
                .as_ref()
                .ok_or(CustomAgentError::MissingChannel("uvx"))?;
            if uvx.package.trim().is_empty() {
                return Err(CustomAgentError::MissingPackage("uvx"));
            }
            AgentDistribution::Uvx {
                version,
                package: intern(uvx.package.trim()),
                cmd: intern(&resolved_cmd(uvx.cmd.as_deref(), &uvx.package)),
                args: intern_args(&uvx.args),
                env: intern_env(&uvx.env),
                uv_required: uvx.uv_required.as_deref().map(intern),
                python: uvx.python.as_deref().map(intern),
                // Custom agents have no vetted "installed via the official
                // installer" fallback command; PATH resolution still happens
                // inside the uvx path itself.
                system_cmd: None,
            }
        }
        CustomDistributionKind::Binary => build_binary_distribution(&def.spec, version)?,
    };

    Ok(AcpAgentMeta {
        agent_type,
        // MCP is forwarded over the ACP wire (`session/new`'s `mcpServers`),
        // which is protocol behaviour rather than per-agent configuration —
        // custom agents get it, including the codeg-mcp companion. OpenClaw
        // remains the only opt-out (see `only_openclaw_opts_out_of_mcp`).
        supports_mcp: true,
        name: intern(def.name.trim()),
        description: intern(def.description.trim()),
        distribution,
    })
}

fn resolved_cmd(explicit: Option<&str>, package: &str) -> String {
    explicit
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| derive_command_name(package))
}

fn intern_args(args: &[String]) -> &'static [&'static str] {
    leak_slice(args.iter().map(|a| intern(a)).collect::<Vec<_>>())
}

fn intern_env(env: &BTreeMap<String, String>) -> &'static [(&'static str, &'static str)] {
    leak_slice(
        env.iter()
            .map(|(k, v)| (intern(k), intern(v)))
            .collect::<Vec<_>>(),
    )
}

/// Registry-style in-archive launch paths are `./name`, `name.exe`, or a
/// nested `dir/name`. Strip the `./` and report whether the remainder is
/// nested (which forces whole-tree extraction instead of copy-out).
fn split_archive_entry(raw: &str) -> (String, bool) {
    let trimmed = raw.trim().trim_start_matches("./").replace('\\', "/");
    let nested = trimmed.contains('/');
    (trimmed, nested)
}

fn build_binary_distribution(
    spec: &CustomAgentSpec,
    version: &'static str,
) -> Result<AgentDistribution, CustomAgentError> {
    if spec.binary.is_empty() {
        return Err(CustomAgentError::MissingChannel("binary"));
    }
    let current = crate::acp::registry::current_platform();
    let here = spec
        .binary
        .get(current)
        .ok_or_else(|| CustomAgentError::UnsupportedPlatform(current.to_string()))?;
    if here.archive.trim().is_empty() {
        return Err(CustomAgentError::MissingArchiveUrl(current.to_string()));
    }

    let (entry_path, nested) = split_archive_entry(&here.cmd);
    // `cmd` is both the PATH probe name and the file copied out of a
    // single-binary archive, so it must be the bare file name.
    let file_name = entry_path.rsplit('/').next().unwrap_or(&entry_path);
    let cmd = intern(if file_name.is_empty() {
        // A spec with no `cmd` at all: fall back to the archive's file stem.
        archive_stem(&here.archive)
    } else {
        file_name
    });

    // A nested entry means the tree must stay intact. Take each family's own
    // path when it has one so a Windows `.cmd` shim is honoured, and reuse the
    // other when only one side declares a nested layout.
    let dir_entry = if nested {
        let unix = spec
            .binary
            .iter()
            .find(|(p, _)| !p.starts_with("windows"))
            .map(|(_, b)| split_archive_entry(&b.cmd).0)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| entry_path.clone());
        let windows = spec
            .binary
            .iter()
            .find(|(p, _)| p.starts_with("windows"))
            .map(|(_, b)| split_archive_entry(&b.cmd).0)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| entry_path.clone());
        Some(BinaryDirEntry {
            unix: intern(&unix),
            windows: intern(&windows),
        })
    } else {
        None
    };

    let mut platforms: Vec<PlatformBinary> = spec
        .binary
        .iter()
        .filter(|(_, b)| !b.archive.trim().is_empty())
        .map(|(platform, b)| PlatformBinary {
            platform: intern(platform),
            url: intern(b.archive.trim()),
            sha256: b
                .sha256
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(intern),
        })
        .collect();
    platforms.sort_by(|a, b| a.platform.cmp(b.platform));

    Ok(AgentDistribution::Binary {
        version,
        cmd,
        args: intern_args(&here.args),
        env: intern_env(&here.env),
        platforms: leak_slice(platforms),
        dir_entry,
    })
}

/// Last-resort command name for a binary spec with no `cmd`: the archive file
/// name with its (possibly compound) extension removed.
fn archive_stem(url: &str) -> &str {
    let file = url
        .rsplit('/')
        .next()
        .unwrap_or(url)
        .split(['?', '#'])
        .next()
        .unwrap_or(url);
    file.split('.').next().unwrap_or(file)
}

struct Entry {
    fingerprint: String,
    meta: &'static AcpAgentMeta,
    /// Display icon, kept beside the launch metadata rather than inside
    /// [`AcpAgentMeta`]: it is presentation, not launch data, and every one of
    /// the twelve built-in metas would otherwise need a field it never uses.
    icon: Option<&'static str>,
    /// Whether the user declared the agent reads the shared `.agents/skills`
    /// store. Beside the meta for the same reason as `icon`: it drives the
    /// skills surfaces, not the launch.
    skills_shared_store: bool,
}

fn registry() -> &'static RwLock<HashMap<&'static str, Entry>> {
    static REGISTRY: OnceLock<RwLock<HashMap<&'static str, Entry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

fn fingerprint(def: &CustomAgentDef) -> String {
    serde_json::to_string(def).unwrap_or_default()
}

/// Replace the published registry with `defs`.
///
/// Definitions that fail validation are skipped with a warning rather than
/// failing the whole hydrate — one malformed row must not take down every
/// other custom agent (or app startup). Returns the errors for callers that
/// want to surface them.
pub fn hydrate(defs: &[CustomAgentDef]) -> Vec<(String, CustomAgentError)> {
    let mut errors = Vec::new();
    let mut next: HashMap<&'static str, Entry> = HashMap::new();
    let previous = registry().read().unwrap_or_else(|e| e.into_inner());

    for def in defs {
        let id = intern(&def.registry_id);
        let fp = fingerprint(def);
        // Unchanged definition — reuse the already-leaked meta.
        if let Some(prev) = previous.get(id).filter(|p| p.fingerprint == fp) {
            next.insert(
                id,
                Entry {
                    fingerprint: fp,
                    meta: prev.meta,
                    icon: prev.icon,
                    skills_shared_store: prev.skills_shared_store,
                },
            );
            continue;
        }
        match build_meta(def) {
            Ok(meta) => {
                next.insert(
                    id,
                    Entry {
                        fingerprint: fp,
                        meta: leak(meta),
                        icon: def
                            .icon_url
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(intern),
                        skills_shared_store: def.skills_shared_store,
                    },
                );
            }
            Err(e) => {
                tracing::warn!(
                    "[ACP] skipping custom agent {:?}: {e}",
                    def.registry_id.as_str()
                );
                errors.push((def.registry_id.clone(), e));
            }
        }
    }

    drop(previous);
    *registry().write().unwrap_or_else(|e| e.into_inner()) = next;
    errors
}

/// Launch metadata for a registered custom agent, or `None` when the id is not
/// registered (never registered, or deleted while conversations still point at
/// it).
pub fn get(registry_id: &str) -> Option<&'static AcpAgentMeta> {
    registry()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(registry_id)
        .map(|e| e.meta)
}

/// Display name for a registered custom agent.
pub fn display_name(registry_id: &str) -> Option<&'static str> {
    get(registry_id).map(|m| m.name)
}

/// Whether a registered custom agent declared it reads the shared
/// `.agents/skills` store. `false` for unregistered ids — a deleted agent
/// must drop out of the skills surfaces rather than keep a phantom column.
pub fn skills_shared_store(registry_id: &str) -> bool {
    registry()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(registry_id)
        .is_some_and(|e| e.skills_shared_store)
}

/// Display icon for a registered custom agent — a `data:` URL in the normal
/// case. See [`CustomAgentDef::icon_url`].
pub fn icon_for(registry_id: &str) -> Option<&'static str> {
    registry()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(registry_id)
        .and_then(|e| e.icon)
}

/// True when `registry_id` currently resolves to launch metadata.
pub fn is_registered(registry_id: &str) -> bool {
    get(registry_id).is_some()
}

/// Placeholder metadata for a custom agent id that is NOT registered — either
/// the user deleted the definition while conversations still reference it, or
/// the registry has not been hydrated yet.
///
/// [`crate::acp::registry::get_agent_meta`] is synchronous and infallible, so
/// it cannot report the miss; this keeps read-only consumers (settings list,
/// display names, cache paths) working while guaranteeing the agent cannot be
/// launched — the empty package/command is rejected up front by
/// `connection::build_agent`, which checks [`is_registered`] and fails with an
/// actionable error instead of spawning nothing.
pub fn unregistered_meta(registry_id: &'static str) -> AcpAgentMeta {
    AcpAgentMeta {
        agent_type: AgentType::Custom(registry_id),
        supports_mcp: true,
        name: registry_id,
        description: "Unregistered custom ACP agent",
        distribution: AgentDistribution::Npx {
            version: FALLBACK_VERSION,
            package: "",
            cmd: "",
            args: &[],
            env: &[],
            node_required: None,
        },
    }
}

/// Every registered custom agent, sorted by id for a stable UI order.
pub fn all() -> Vec<AgentType> {
    let mut ids: Vec<&'static str> = registry()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .copied()
        .collect();
    ids.sort_unstable();
    ids.into_iter().map(AgentType::Custom).collect()
}

/// Serializes tests that call [`hydrate`].
///
/// `hydrate` replaces a process-global map and cargo runs tests in threads, so
/// two tests publishing different registries — or one calling `hydrate(&[])` to
/// clean up — would otherwise clobber each other. Lives outside the test module
/// because tests in other modules (e.g. chat-command agent parsing) need the
/// same lock. Tests that only READ a uniquely-named id need no guard.
#[cfg(test)]
pub fn hydrate_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hydrate_guard() -> std::sync::MutexGuard<'static, ()> {
        hydrate_test_guard()
    }

    fn npx_def(id: &str) -> CustomAgentDef {
        CustomAgentDef {
            registry_id: id.to_string(),
            name: "Qwen Code".into(),
            description: "Alibaba's Qwen coding assistant".into(),
            version: "0.21.0".into(),
            distribution_kind: CustomDistributionKind::Npx,
            spec: CustomAgentSpec {
                npx: Some(NpxSpec {
                    package: "@qwen-code/qwen-code@0.21.0".into(),
                    args: vec!["--acp".into(), "--experimental-skills".into()],
                    env: BTreeMap::new(),
                    cmd: Some("qwen".into()),
                    node_required: Some("20.0.0".into()),
                }),
                ..Default::default()
            },
            icon_url: None,
            skills_shared_store: false,
        }
    }

    #[test]
    fn derives_command_names_from_package_specs() {
        assert_eq!(derive_command_name("@qwen-code/qwen-code@0.21.0"), "qwen-code");
        assert_eq!(derive_command_name("@github/copilot@1.0.75"), "copilot");
        assert_eq!(derive_command_name("cline@3.0.46"), "cline");
        assert_eq!(derive_command_name("fast-agent-acp==0.9.24"), "fast-agent-acp");
        assert_eq!(
            derive_command_name("hermes-agent[acp,mcp]==0.19.0"),
            "hermes-agent"
        );
        // A bare scoped package with no version must keep its name.
        assert_eq!(derive_command_name("@scope/thing"), "thing");
    }

    #[test]
    fn builds_npx_metadata() {
        let meta = build_meta(&npx_def("qwen-code")).expect("valid");
        assert_eq!(meta.agent_type, AgentType::custom("qwen-code").unwrap());
        assert!(meta.supports_mcp);
        assert_eq!(meta.name, "Qwen Code");
        match meta.distribution {
            AgentDistribution::Npx {
                version,
                package,
                cmd,
                args,
                node_required,
                ..
            } => {
                assert_eq!(version, "0.21.0");
                assert_eq!(package, "@qwen-code/qwen-code@0.21.0");
                assert_eq!(cmd, "qwen");
                assert_eq!(args, &["--acp", "--experimental-skills"]);
                assert_eq!(node_required, Some("20.0.0"));
            }
            other => panic!("expected npx distribution, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_definitions() {
        let mut bad_id = npx_def("qwen-code");
        bad_id.registry_id = "../escape".into();
        assert!(matches!(
            build_meta(&bad_id),
            Err(CustomAgentError::InvalidId(_))
        ));

        let mut empty_name = npx_def("qwen-code");
        empty_name.name = "   ".into();
        assert_eq!(build_meta(&empty_name).unwrap_err(), CustomAgentError::EmptyName);

        let mut wrong_channel = npx_def("qwen-code");
        wrong_channel.distribution_kind = CustomDistributionKind::Uvx;
        assert_eq!(
            build_meta(&wrong_channel).unwrap_err(),
            CustomAgentError::MissingChannel("uvx")
        );

        let mut no_package = npx_def("qwen-code");
        no_package.spec.npx.as_mut().unwrap().package = "  ".into();
        assert_eq!(
            build_meta(&no_package).unwrap_err(),
            CustomAgentError::MissingPackage("npx")
        );
    }

    fn binary_def(cmd_for_current: &str, sha: Option<&str>) -> CustomAgentDef {
        let mut binary = BTreeMap::new();
        // Cover every platform so the test runs anywhere.
        for platform in [
            "darwin-aarch64",
            "darwin-x86_64",
            "linux-aarch64",
            "linux-x86_64",
            "windows-aarch64",
            "windows-x86_64",
        ] {
            binary.insert(
                platform.to_string(),
                BinaryPlatformSpec {
                    archive: format!("https://example.com/goose-{platform}.tar.gz"),
                    cmd: cmd_for_current.to_string(),
                    args: vec!["acp".into()],
                    env: BTreeMap::new(),
                    sha256: sha.map(str::to_string),
                },
            );
        }
        CustomAgentDef {
            registry_id: "goose".into(),
            name: "goose".into(),
            description: "local agent".into(),
            version: "1.44.0".into(),
            distribution_kind: CustomDistributionKind::Binary,
            spec: CustomAgentSpec {
                binary,
                ..Default::default()
            },
            icon_url: None,
            skills_shared_store: false,
        }
    }

    #[test]
    fn maps_registry_binary_entry_to_distribution() {
        let meta = build_meta(&binary_def("./goose", Some("abc123"))).expect("valid");
        match meta.distribution {
            AgentDistribution::Binary {
                version,
                cmd,
                args,
                platforms,
                dir_entry,
                ..
            } => {
                assert_eq!(version, "1.44.0");
                // `./goose` is a single-file archive entry: strip the `./`, no
                // whole-tree extraction.
                assert_eq!(cmd, "goose");
                assert_eq!(args, &["acp"]);
                assert!(dir_entry.is_none());
                assert_eq!(platforms.len(), 6);
                assert!(platforms.iter().all(|p| p.sha256 == Some("abc123")));
                // Sorted for a stable order.
                assert_eq!(platforms[0].platform, "darwin-aarch64");
            }
            other => panic!("expected binary distribution, got {other:?}"),
        }
    }

    #[test]
    fn nested_archive_entry_switches_to_dir_tree_extraction() {
        let meta = build_meta(&binary_def("dist-package/agent", None)).expect("valid");
        match meta.distribution {
            AgentDistribution::Binary {
                cmd, dir_entry, ..
            } => {
                assert_eq!(cmd, "agent");
                let entry = dir_entry.expect("nested entry must extract the whole tree");
                assert_eq!(entry.unix, "dist-package/agent");
                assert_eq!(entry.windows, "dist-package/agent");
            }
            other => panic!("expected binary distribution, got {other:?}"),
        }
    }

    #[test]
    fn version_labels_that_would_escape_the_binary_cache_are_rejected() {
        // The version becomes a directory name under the binary cache, and that
        // directory is deleted on reinstall — so a definition must not be able
        // to steer it elsewhere.
        for bad in [
            "../../..",
            "..",
            ".",
            "a/b",
            "a\\b",
            "C:",
            ".hidden",
            "with space",
            "semi;colon",
        ] {
            let mut def = npx_def("version-guard");
            def.version = bad.to_string();
            assert!(
                matches!(build_meta(&def), Err(CustomAgentError::UnsafeVersion(_))),
                "version {bad:?} must be rejected"
            );
            assert!(validate(&def).is_err());
        }
        // Ordinary labels still pass, including an empty one (which becomes
        // FALLBACK_VERSION) and the `v`-prefixed spelling.
        for ok in ["0.21.0", "v1.2.3", "2026.07.23-e383d2b", "", "   "] {
            let mut def = npx_def("version-guard");
            def.version = ok.to_string();
            assert!(validate(&def).is_ok(), "version {ok:?} must be accepted");
        }
    }

    #[test]
    fn archive_entries_that_escape_the_extracted_tree_are_rejected() {
        // `binary_cache` joins this onto the install dir, chmod +x's the result
        // and launches it, so an absolute or `..` path would target a file of
        // the definition's choosing anywhere on disk.
        for bad in [
            "/tmp/target",
            "../../target",
            "./../../target",
            "dist/../../../target",
            "..\\..\\target.exe",
            "C:\\Windows\\System32\\cmd.exe",
            "//host/share/target",
        ] {
            let def = binary_def(bad, None);
            assert!(
                matches!(
                    build_meta(&def),
                    Err(CustomAgentError::UnsafeArchiveEntry(_))
                ),
                "archive entry {bad:?} must be rejected"
            );
        }
        // The registry's real spellings still work.
        for ok in ["./goose", "goose", "dist-package/cursor-agent", "amp-acp.exe", ""] {
            let def = binary_def(ok, None);
            assert!(
                validate(&def).is_ok(),
                "archive entry {ok:?} must be accepted"
            );
        }
    }

    #[test]
    fn an_escaping_entry_on_another_platform_is_rejected_too() {
        // `build_binary_distribution` borrows another platform family's path
        // when publishing `dir_entry`, so a hostile entry need not be this
        // machine's to reach the launch path.
        let mut def = binary_def("dist/agent", None);
        let other = if crate::acp::registry::current_platform().starts_with("windows") {
            "linux-x86_64"
        } else {
            "windows-x86_64"
        };
        def.spec.binary.get_mut(other).unwrap().cmd = "../../../../target".into();
        assert!(matches!(
            build_meta(&def),
            Err(CustomAgentError::UnsafeArchiveEntry(_))
        ));
    }

    #[test]
    fn validate_reaches_the_same_verdict_as_build_meta() {
        // The settings list and the upsert path call `validate` precisely so
        // they do not leak a throwaway meta, which only works if the two agree
        // on what is valid.
        let mut escaping_version = npx_def("agree");
        escaping_version.version = "../..".into();
        let mut no_channel = npx_def("agree");
        no_channel.distribution_kind = CustomDistributionKind::Uvx;
        let mut empty_package = npx_def("agree");
        empty_package.spec.npx.as_mut().unwrap().package = "  ".into();
        let mut bad_id = npx_def("agree");
        bad_id.registry_id = "../escape".into();
        let mut blank_name = npx_def("agree");
        blank_name.name = "  ".into();

        let cases = [
            npx_def("agree"),
            escaping_version,
            no_channel,
            empty_package,
            bad_id,
            blank_name,
            binary_def("./goose", Some("abc123")),
            binary_def("../../escape", None),
            binary_def("dist-package/agent", None),
        ];
        for def in &cases {
            assert_eq!(
                validate(def),
                build_meta(def).map(|_| ()),
                "validate and build_meta disagree about {:?}",
                def.registry_id
            );
        }
    }

    #[test]
    fn binary_without_current_platform_is_rejected() {
        let mut def = binary_def("./goose", None);
        def.spec.binary.clear();
        def.spec.binary.insert(
            "some-unreal-platform".into(),
            BinaryPlatformSpec {
                archive: "https://example.com/x.tar.gz".into(),
                cmd: "./x".into(),
                ..Default::default()
            },
        );
        assert!(matches!(
            build_meta(&def),
            Err(CustomAgentError::UnsupportedPlatform(_))
        ));
    }

    #[test]
    fn spec_json_is_paste_compatible_with_the_acp_registry() {
        // Verbatim `distribution` object from the published registry.
        let raw = r#"{
          "binary": {
            "darwin-aarch64": {
              "archive": "https://example.com/kilo-darwin-arm64.zip",
              "cmd": "./kilo",
              "args": ["acp"],
              "sha256": "0961"
            }
          },
          "npx": { "package": "kilo@7.4.16", "args": ["--acp"] }
        }"#;
        let spec: CustomAgentSpec = serde_json::from_str(raw).expect("registry shape parses");
        assert!(spec.npx.is_some());
        assert_eq!(spec.binary.len(), 1);
        assert_eq!(
            spec.binary["darwin-aarch64"].sha256.as_deref(),
            Some("0961")
        );
        // `url` is accepted as an alias for `archive` (older registry payloads).
        let aliased: CustomAgentSpec =
            serde_json::from_str(r#"{"binary":{"linux-x86_64":{"url":"https://e/x.tgz"}}}"#)
                .expect("alias parses");
        assert_eq!(aliased.binary["linux-x86_64"].archive, "https://e/x.tgz");
    }

    #[test]
    fn hydrate_publishes_and_reuses_unchanged_metadata() {
        let _guard = hydrate_guard();
        let def = npx_def("hydrate-test-agent");
        assert!(hydrate(std::slice::from_ref(&def)).is_empty());
        let first = get("hydrate-test-agent").expect("registered");
        assert_eq!(display_name("hydrate-test-agent"), Some("Qwen Code"));
        assert!(is_registered("hydrate-test-agent"));
        assert!(all().contains(&AgentType::custom("hydrate-test-agent").unwrap()));

        // Re-hydrating an unchanged definition must reuse the SAME leak.
        assert!(hydrate(std::slice::from_ref(&def)).is_empty());
        let second = get("hydrate-test-agent").expect("still registered");
        assert!(std::ptr::eq(first, second), "unchanged def must not re-leak");

        // Hydrating an empty set unregisters everything.
        assert!(hydrate(&[]).is_empty());
        assert!(get("hydrate-test-agent").is_none());
        assert!(!is_registered("hydrate-test-agent"));
    }

    #[test]
    fn the_skills_declaration_survives_hydrate_and_gates_by_registration() {
        let _guard = hydrate_guard();
        let mut def = npx_def("skills-flag-agent");
        assert!(hydrate(std::slice::from_ref(&def)).is_empty());
        assert!(
            !skills_shared_store("skills-flag-agent"),
            "undeclared agents stay out of the skills surfaces"
        );

        // Flipping the declaration changes the fingerprint, so the same
        // hydrate path republishes and the accessor flips with it.
        def.skills_shared_store = true;
        assert!(hydrate(std::slice::from_ref(&def)).is_empty());
        assert!(skills_shared_store("skills-flag-agent"));

        // An unregistered (deleted) id reads as false — a phantom column must
        // not survive its agent.
        assert!(hydrate(&[]).is_empty());
        assert!(!skills_shared_store("skills-flag-agent"));
    }

    #[test]
    fn hydrate_skips_invalid_rows_without_dropping_valid_ones() {
        let _guard = hydrate_guard();
        let good = npx_def("hydrate-good-agent");
        let mut bad = npx_def("hydrate-bad-agent");
        bad.spec.npx = None;
        let errors = hydrate(&[good, bad]);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "hydrate-bad-agent");
        assert!(is_registered("hydrate-good-agent"));
        assert!(!is_registered("hydrate-bad-agent"));
        hydrate(&[]);
    }
}
