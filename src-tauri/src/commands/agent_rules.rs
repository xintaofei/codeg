//! Optional workspace rule catalogs and saved selections.
//!
//! Native agent instruction files are deliberately discovery-only here. The
//! selectable catalog lives under `.codeg/rules`, so applying a selection never
//! mutates or claims to disable `AGENTS.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::app_error::AppCommandError;

const START_PREFIX: &str = "<!-- codeg-rule";
const END_MARKER: &str = "<!-- /codeg-rule -->";
const PROFILE_PATH: &str = ".codeg/agent-rule-profiles.json";
const MAX_CATALOG_FILES: usize = 128;
const MAX_CATALOG_BYTES: usize = 2 * 1024 * 1024;
const MAX_RULES: usize = 512;
const MAX_RENDERED_BYTES: usize = 1024 * 1024;
const MAX_PROFILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
struct Rule {
    id: String,
    name: String,
    default_on: bool,
    body: String,
    source: String,
    line: usize,
}

#[derive(Debug)]
struct Catalog {
    rules: Vec<Rule>,
    source_hash: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuleSummary {
    pub id: String,
    pub name: String,
    pub default_on: bool,
    pub source: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuleProfileSummary {
    pub rule_ids: Vec<String>,
    pub source_hash: String,
    pub stale: bool,
    pub missing_rule_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRulesInspectResult {
    pub workspace: String,
    pub native_sources: Vec<String>,
    pub rules: Vec<AgentRuleSummary>,
    pub default_ids: Vec<String>,
    pub source_hash: String,
    pub profile_path: String,
    pub profiles_exist: bool,
    pub default_profile: Option<String>,
    pub profiles: BTreeMap<String, AgentRuleProfileSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRulesRenderResult {
    pub source_hash: String,
    pub rules: Vec<AgentRuleSummary>,
    pub sources: Vec<String>,
    pub text: String,
    pub envelope_nonce: String,
}

#[derive(Debug)]
struct ProfilesDocument {
    path: PathBuf,
    payload: Map<String, Value>,
    exists: bool,
}

fn start_marker_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^<!-- codeg-rule(?P<attributes>.*?)-->$").unwrap())
}

fn attribute_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?P<key>[A-Za-z][A-Za-z0-9_-]*)="(?P<value>[^"]*)""#).unwrap()
    })
}

fn rule_id_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^[a-z0-9][a-z0-9._-]*$").unwrap())
}

fn invalid(message: impl Into<String>) -> AppCommandError {
    AppCommandError::configuration_invalid(message)
}

fn diagnostic(source: &str, line: usize, message: impl AsRef<str>) -> AppCommandError {
    invalid(format!("{source}:{line}: {}", message.as_ref()))
}

fn canonical_workspace(root_path: &str) -> Result<PathBuf, AppCommandError> {
    if root_path.trim().is_empty() {
        return Err(AppCommandError::invalid_input("Workspace path is required"));
    }
    let root = fs::canonicalize(root_path).map_err(AppCommandError::io)?;
    if !root.is_dir() {
        return Err(AppCommandError::invalid_input(
            "Workspace path must be a directory",
        ));
    }
    Ok(root)
}

fn is_within(root: &Path, target: &Path) -> bool {
    target.starts_with(root)
}

fn parse_start_marker(
    marker: &str,
    source: &str,
    line: usize,
) -> Result<(String, String, bool), AppCommandError> {
    let captures = start_marker_regex()
        .captures(marker)
        .ok_or_else(|| diagnostic(source, line, "malformed rule start marker"))?;
    let raw = captures
        .name("attributes")
        .map_or("", |matched| matched.as_str());
    let mut attributes = BTreeMap::new();
    let mut position = 0;
    for captures in attribute_regex().captures_iter(raw) {
        let matched = captures.get(0).expect("attribute regex has a full match");
        let separator = &raw[position..matched.start()];
        if separator.is_empty() || !separator.chars().all(char::is_whitespace) {
            return Err(diagnostic(source, line, "malformed rule attributes"));
        }
        let key = captures.name("key").expect("key capture exists").as_str();
        if attributes.contains_key(key) {
            return Err(diagnostic(
                source,
                line,
                format!("duplicate rule attribute '{key}'"),
            ));
        }
        let value = captures
            .name("value")
            .expect("value capture exists")
            .as_str();
        attributes.insert(key.to_owned(), value.to_owned());
        position = matched.end();
    }
    if !raw[position..].chars().all(char::is_whitespace) {
        return Err(diagnostic(source, line, "malformed rule attributes"));
    }

    let required = ["default", "id", "name"];
    let missing: Vec<_> = required
        .iter()
        .filter(|key| !attributes.contains_key(**key))
        .copied()
        .collect();
    let unknown: Vec<_> = attributes
        .keys()
        .filter(|key| !required.contains(&key.as_str()))
        .cloned()
        .collect();
    if !missing.is_empty() || !unknown.is_empty() {
        let mut details = Vec::new();
        if !missing.is_empty() {
            details.push(format!("missing {}", missing.join(", ")));
        }
        if !unknown.is_empty() {
            details.push(format!("unknown {}", unknown.join(", ")));
        }
        return Err(diagnostic(
            source,
            line,
            format!("malformed rule attributes ({})", details.join("; ")),
        ));
    }

    let id = attributes.remove("id").expect("required id exists");
    let name = attributes.remove("name").expect("required name exists");
    let default = attributes
        .remove("default")
        .expect("required default exists");
    if !rule_id_regex().is_match(&id) {
        return Err(diagnostic(
            source,
            line,
            "rule id must use lower-case stable ID syntax",
        ));
    }
    if name.trim().is_empty() || name.trim() != name {
        return Err(diagnostic(
            source,
            line,
            "rule name must be human-readable and trimmed",
        ));
    }
    let default_on = match default.as_str() {
        "on" => true,
        "off" => false,
        _ => {
            return Err(diagnostic(
                source,
                line,
                "malformed rule start marker: default must be 'on' or 'off'",
            ));
        }
    };
    Ok((id, name, default_on))
}

fn compute_source_hash(rules: &[Rule]) -> Result<String, AppCommandError> {
    let payload: Vec<_> = rules
        .iter()
        .map(|rule| {
            BTreeMap::from([
                ("body", Value::String(rule.body.clone())),
                (
                    "default",
                    Value::String(if rule.default_on { "on" } else { "off" }.to_owned()),
                ),
                ("id", Value::String(rule.id.clone())),
                ("name", Value::String(rule.name.clone())),
                ("source", Value::String(rule.source.clone())),
            ])
        })
        .collect();
    let encoded = serde_json::to_vec(&payload)
        .map_err(|error| invalid(format!("Failed to hash rule catalog: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn load_catalog(root: &Path) -> Result<Catalog, AppCommandError> {
    let rules_path = root.join(".codeg/rules");
    if !rules_path.exists() {
        return Ok(Catalog {
            rules: Vec::new(),
            source_hash: compute_source_hash(&[])?,
        });
    }
    let resolved_dir = fs::canonicalize(&rules_path).map_err(AppCommandError::io)?;
    if !is_within(root, &resolved_dir) {
        return Err(diagnostic(
            ".codeg/rules",
            1,
            "catalog directory resolves outside workspace",
        ));
    }
    if !resolved_dir.is_dir() {
        return Err(diagnostic(
            ".codeg/rules",
            1,
            "catalog location must be a directory",
        ));
    }

    let mut paths = Vec::new();
    for entry in fs::read_dir(&rules_path).map_err(AppCommandError::io)? {
        let path = entry.map_err(AppCommandError::io)?.path();
        if path.extension().and_then(|value| value.to_str()) == Some("md") {
            paths.push(path);
        }
    }
    paths.sort();
    if paths.len() > MAX_CATALOG_FILES {
        return Err(invalid(format!(
            "Rule catalog exceeds the {MAX_CATALOG_FILES}-file limit"
        )));
    }

    let mut total_bytes = 0usize;
    let mut rules = Vec::new();
    let mut definitions: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for path in paths {
        let source = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let resolved = fs::canonicalize(&path).map_err(|error| {
            diagnostic(&source, 1, format!("cannot resolve catalog file: {error}"))
        })?;
        if !is_within(root, &resolved) {
            return Err(diagnostic(
                &source,
                1,
                "catalog symlink resolves outside workspace",
            ));
        }
        if !resolved.is_file() {
            return Err(diagnostic(
                &source,
                1,
                "catalog entry must be a regular file",
            ));
        }
        let bytes = fs::read(&path).map_err(AppCommandError::io)?;
        total_bytes = total_bytes.saturating_add(bytes.len());
        if total_bytes > MAX_CATALOG_BYTES {
            return Err(invalid(format!(
                "Rule catalog exceeds the {MAX_CATALOG_BYTES}-byte limit"
            )));
        }
        let content = String::from_utf8(bytes)
            .map_err(|error| diagnostic(&source, 1, format!("catalog is not UTF-8: {error}")))?
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let lines: Vec<&str> = content.split_inclusive('\n').collect();
        let mut index = 0;
        while index < lines.len() {
            let marker = lines[index].trim_end_matches('\n').trim();
            if marker == END_MARKER {
                return Err(diagnostic(
                    &source,
                    index + 1,
                    "end marker without an open rule block",
                ));
            }
            if !marker.starts_with(START_PREFIX) {
                index += 1;
                continue;
            }
            let start_line = index + 1;
            let (id, name, default_on) = parse_start_marker(marker, &source, start_line)?;
            index += 1;
            let mut body = String::new();
            while index < lines.len() {
                let nested_marker = lines[index].trim_end_matches('\n').trim();
                if nested_marker == END_MARKER {
                    break;
                }
                if nested_marker.starts_with(START_PREFIX) {
                    return Err(diagnostic(&source, index + 1, "nested rule block"));
                }
                body.push_str(lines[index]);
                index += 1;
            }
            if index == lines.len() {
                return Err(diagnostic(&source, start_line, "unterminated rule block"));
            }
            if let Some((first_source, first_line)) = definitions.get(&id) {
                return Err(diagnostic(
                    &source,
                    start_line,
                    format!(
                        "duplicate rule id '{id}'; first defined at {first_source}:{first_line}"
                    ),
                ));
            }
            definitions.insert(id.clone(), (source.clone(), start_line));
            rules.push(Rule {
                id,
                name,
                default_on,
                body,
                source: source.clone(),
                line: start_line,
            });
            if rules.len() > MAX_RULES {
                return Err(invalid(format!(
                    "Rule catalog exceeds the {MAX_RULES}-rule limit"
                )));
            }
            index += 1;
        }
    }
    let source_hash = compute_source_hash(&rules)?;
    Ok(Catalog { rules, source_hash })
}

fn rule_summary(rule: &Rule) -> AgentRuleSummary {
    AgentRuleSummary {
        id: rule.id.clone(),
        name: rule.name.clone(),
        default_on: rule.default_on,
        source: rule.source.clone(),
        line: rule.line,
    }
}

fn native_sources(root: &Path) -> Vec<String> {
    let override_path = root.join("AGENTS.override.md");
    if override_path.is_file() {
        vec!["AGENTS.override.md".to_owned()]
    } else if root.join("AGENTS.md").is_file() {
        vec!["AGENTS.md".to_owned()]
    } else {
        Vec::new()
    }
}

fn profile_path(root: &Path) -> Result<PathBuf, AppCommandError> {
    let codeg_dir = root.join(".codeg");
    if codeg_dir.exists() {
        let resolved = fs::canonicalize(&codeg_dir).map_err(AppCommandError::io)?;
        if !is_within(root, &resolved) {
            return Err(invalid(".codeg directory resolves outside workspace"));
        }
        if !resolved.is_dir() {
            return Err(invalid(".codeg must be a directory"));
        }
    }
    let path = root.join(PROFILE_PATH);
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(invalid(format!(
            "{PROFILE_PATH}: profile file must not be a symlink"
        )));
    }
    Ok(path)
}

fn validate_profile_name(name: &str) -> Result<(), AppCommandError> {
    if name.is_empty()
        || name.trim() != name
        || name.chars().any(|character| character.is_control())
    {
        return Err(AppCommandError::invalid_input(
            "Profile name must be non-empty, trimmed, and printable",
        ));
    }
    Ok(())
}

fn validate_profiles(payload: &Map<String, Value>, path: &Path) -> Result<(), AppCommandError> {
    if payload.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(invalid(format!(
            "{}: unsupported or missing profile version",
            path.display()
        )));
    }
    let profiles = payload
        .get("profiles")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            invalid(format!(
                "{}: profiles must be a JSON object",
                path.display()
            ))
        })?;
    for (name, profile) in profiles {
        validate_profile_name(name)?;
        let profile = profile.as_object().ok_or_else(|| {
            invalid(format!(
                "{}: profile '{name}' must be a JSON object",
                path.display()
            ))
        })?;
        let ids = profile
            .get("ruleIds")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                invalid(format!(
                    "{}: profile '{name}' ruleIds must be unique non-empty strings",
                    path.display()
                ))
            })?;
        let mut unique = BTreeSet::new();
        for id in ids {
            let id = id
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    invalid(format!(
                        "{}: profile '{name}' ruleIds must be unique non-empty strings",
                        path.display()
                    ))
                })?;
            if !unique.insert(id) {
                return Err(invalid(format!(
                    "{}: profile '{name}' ruleIds must be unique non-empty strings",
                    path.display()
                )));
            }
        }
        if profile.get("sourceHash").and_then(Value::as_str).is_none() {
            return Err(invalid(format!(
                "{}: profile '{name}' sourceHash must be a string",
                path.display()
            )));
        }
    }
    if let Some(default_profile) = payload.get("defaultProfile") {
        let valid = default_profile
            .as_str()
            .is_some_and(|name| profiles.contains_key(name));
        if !valid {
            return Err(invalid(format!(
                "{}: defaultProfile must name an existing profile",
                path.display()
            )));
        }
    }
    Ok(())
}

fn load_profiles(root: &Path) -> Result<ProfilesDocument, AppCommandError> {
    let path = profile_path(root)?;
    if !path.exists() {
        return Ok(ProfilesDocument {
            path,
            payload: Map::from_iter([
                ("version".to_owned(), Value::from(1)),
                ("profiles".to_owned(), Value::Object(Map::new())),
            ]),
            exists: false,
        });
    }
    let metadata = fs::metadata(&path).map_err(AppCommandError::io)?;
    if !metadata.is_file() {
        return Err(invalid(format!("{PROFILE_PATH}: must be a regular file")));
    }
    if metadata.len() > MAX_PROFILE_BYTES {
        return Err(invalid(format!(
            "Profile file exceeds the {MAX_PROFILE_BYTES}-byte limit"
        )));
    }
    let bytes = fs::read(&path).map_err(AppCommandError::io)?;
    let payload = serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| invalid(format!("{}: invalid JSON: {error}", path.display())))?
        .as_object()
        .cloned()
        .ok_or_else(|| {
            invalid(format!(
                "{}: profile file must contain a JSON object",
                path.display()
            ))
        })?;
    validate_profiles(&payload, &path)?;
    Ok(ProfilesDocument {
        path,
        payload,
        exists: true,
    })
}

fn profiles_summary(
    document: &ProfilesDocument,
    catalog: &Catalog,
) -> Result<(Option<String>, BTreeMap<String, AgentRuleProfileSummary>), AppCommandError> {
    let known: BTreeSet<_> = catalog.rules.iter().map(|rule| rule.id.as_str()).collect();
    let profiles = document
        .payload
        .get("profiles")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("Profiles document lost its profiles object"))?;
    let mut summaries = BTreeMap::new();
    for (name, profile) in profiles {
        let profile = profile
            .as_object()
            .ok_or_else(|| invalid(format!("Profile '{name}' is invalid")))?;
        let rule_ids: Vec<String> = profile
            .get("ruleIds")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid(format!("Profile '{name}' is invalid")))?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let source_hash = profile
            .get("sourceHash")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(format!("Profile '{name}' is invalid")))?
            .to_owned();
        let missing_rule_ids = rule_ids
            .iter()
            .filter(|id| !known.contains(id.as_str()))
            .cloned()
            .collect();
        summaries.insert(
            name.clone(),
            AgentRuleProfileSummary {
                rule_ids,
                stale: source_hash != catalog.source_hash,
                source_hash,
                missing_rule_ids,
            },
        );
    }
    let default_profile = document
        .payload
        .get("defaultProfile")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok((default_profile, summaries))
}

fn inspect_blocking(root_path: &str) -> Result<AgentRulesInspectResult, AppCommandError> {
    let root = canonical_workspace(root_path)?;
    let catalog = load_catalog(&root)?;
    let profiles = load_profiles(&root)?;
    let (default_profile, profile_summaries) = profiles_summary(&profiles, &catalog)?;
    Ok(AgentRulesInspectResult {
        workspace: root.to_string_lossy().into_owned(),
        native_sources: native_sources(&root),
        rules: catalog.rules.iter().map(rule_summary).collect(),
        default_ids: catalog
            .rules
            .iter()
            .filter(|rule| rule.default_on)
            .map(|rule| rule.id.clone())
            .collect(),
        source_hash: catalog.source_hash,
        profile_path: PROFILE_PATH.to_owned(),
        profiles_exist: profiles.exists,
        default_profile,
        profiles: profile_summaries,
    })
}

fn selected_rules<'a>(
    catalog: &'a Catalog,
    rule_ids: &[String],
) -> Result<Vec<&'a Rule>, AppCommandError> {
    let requested: BTreeSet<_> = rule_ids.iter().map(String::as_str).collect();
    if requested.len() != rule_ids.len() {
        return Err(AppCommandError::invalid_input(
            "Rule IDs must not contain duplicates",
        ));
    }
    let known: BTreeSet<_> = catalog.rules.iter().map(|rule| rule.id.as_str()).collect();
    let unknown: Vec<_> = requested.difference(&known).copied().collect();
    if !unknown.is_empty() {
        return Err(AppCommandError::invalid_input(format!(
            "Unknown rule {}: {}",
            if unknown.len() == 1 { "ID" } else { "IDs" },
            unknown.join(", ")
        )));
    }
    Ok(catalog
        .rules
        .iter()
        .filter(|rule| requested.contains(rule.id.as_str()))
        .collect())
}

fn ensure_expected_hash(catalog: &Catalog, expected: &str) -> Result<(), AppCommandError> {
    if catalog.source_hash != expected {
        return Err(AppCommandError::already_exists(
            "The optional rule catalog changed. Refresh the picker before applying.",
        )
        .with_detail(catalog.source_hash.clone()));
    }
    Ok(())
}

fn envelope_nonce(source_hash: &str, text: &str) -> String {
    for counter in 0u32.. {
        let digest = Sha256::digest(format!("{source_hash}\0{counter}\0{text}"));
        let nonce = format!("{:x}", digest)[..24].to_owned();
        if !text.contains(&format!("<!-- /codeg-agent-rules-selection:{nonce} -->")) {
            return nonce;
        }
    }
    unreachable!("the nonce space cannot be exhausted")
}

fn render_blocking(
    root_path: &str,
    rule_ids: &[String],
    expected_source_hash: &str,
) -> Result<AgentRulesRenderResult, AppCommandError> {
    let root = canonical_workspace(root_path)?;
    let catalog = load_catalog(&root)?;
    ensure_expected_hash(&catalog, expected_source_hash)?;
    let selected = selected_rules(&catalog, rule_ids)?;
    let rendered_bytes: usize = selected.iter().map(|rule| rule.body.len()).sum();
    if rendered_bytes > MAX_RENDERED_BYTES {
        return Err(AppCommandError::invalid_input(format!(
            "Selected rules exceed the {MAX_RENDERED_BYTES}-byte rendered limit"
        )));
    }
    let text: String = selected.iter().map(|rule| rule.body.as_str()).collect();
    let sources: Vec<String> = selected
        .iter()
        .map(|rule| rule.source.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(AgentRulesRenderResult {
        source_hash: catalog.source_hash.clone(),
        rules: selected.iter().map(|rule| rule_summary(rule)).collect(),
        sources,
        envelope_nonce: envelope_nonce(&catalog.source_hash, &text),
        text,
    })
}

fn write_profiles_atomic(document: &ProfilesDocument) -> Result<(), AppCommandError> {
    let parent = document
        .path
        .parent()
        .ok_or_else(|| invalid("Profile path has no parent directory"))?;
    fs::create_dir_all(parent).map_err(AppCommandError::io)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".agent-rule-profiles.")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(AppCommandError::io)?;
    serde_json::to_writer_pretty(&mut temporary, &document.payload)
        .map_err(|error| invalid(format!("Failed to serialize profiles: {error}")))?;
    temporary.write_all(b"\n").map_err(AppCommandError::io)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(AppCommandError::io)?;
    temporary
        .persist(&document.path)
        .map_err(|error| AppCommandError::io(error.error))?;
    Ok(())
}

fn save_profile_blocking(
    root_path: &str,
    name: &str,
    rule_ids: &[String],
    expected_source_hash: &str,
    set_default: bool,
    overwrite: bool,
) -> Result<AgentRulesInspectResult, AppCommandError> {
    validate_profile_name(name)?;
    let root = canonical_workspace(root_path)?;
    let catalog = load_catalog(&root)?;
    ensure_expected_hash(&catalog, expected_source_hash)?;
    let ordered_ids: Vec<_> = selected_rules(&catalog, rule_ids)?
        .into_iter()
        .map(|rule| Value::String(rule.id.clone()))
        .collect();
    let mut document = load_profiles(&root)?;
    let profiles = document
        .payload
        .get_mut("profiles")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| invalid("Profiles document lost its profiles object"))?;
    if profiles.contains_key(name) && !overwrite {
        return Err(AppCommandError::already_exists(format!(
            "Profile '{name}' already exists"
        )));
    }
    profiles.insert(
        name.to_owned(),
        Value::Object(Map::from_iter([
            ("ruleIds".to_owned(), Value::Array(ordered_ids)),
            (
                "sourceHash".to_owned(),
                Value::String(catalog.source_hash.clone()),
            ),
        ])),
    );
    if set_default {
        document
            .payload
            .insert("defaultProfile".to_owned(), Value::String(name.to_owned()));
    }
    write_profiles_atomic(&document)?;
    inspect_blocking(root.to_string_lossy().as_ref())
}

fn rename_profile_blocking(
    root_path: &str,
    old_name: &str,
    new_name: &str,
    overwrite: bool,
) -> Result<AgentRulesInspectResult, AppCommandError> {
    validate_profile_name(old_name)?;
    validate_profile_name(new_name)?;
    let root = canonical_workspace(root_path)?;
    let mut document = load_profiles(&root)?;
    let profiles = document
        .payload
        .get_mut("profiles")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| invalid("Profiles document lost its profiles object"))?;
    if !profiles.contains_key(old_name) {
        return Err(AppCommandError::not_found(format!(
            "Profile '{old_name}' does not exist"
        )));
    }
    if old_name != new_name && profiles.contains_key(new_name) && !overwrite {
        return Err(AppCommandError::already_exists(format!(
            "Profile '{new_name}' already exists"
        )));
    }
    let profile = profiles
        .remove(old_name)
        .ok_or_else(|| AppCommandError::not_found("Profile disappeared during rename"))?;
    profiles.insert(new_name.to_owned(), profile);
    if document
        .payload
        .get("defaultProfile")
        .and_then(Value::as_str)
        == Some(old_name)
    {
        document.payload.insert(
            "defaultProfile".to_owned(),
            Value::String(new_name.to_owned()),
        );
    }
    write_profiles_atomic(&document)?;
    inspect_blocking(root.to_string_lossy().as_ref())
}

fn delete_profile_blocking(
    root_path: &str,
    name: &str,
) -> Result<AgentRulesInspectResult, AppCommandError> {
    validate_profile_name(name)?;
    let root = canonical_workspace(root_path)?;
    let mut document = load_profiles(&root)?;
    let profiles = document
        .payload
        .get_mut("profiles")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| invalid("Profiles document lost its profiles object"))?;
    if profiles.remove(name).is_none() {
        return Err(AppCommandError::not_found(format!(
            "Profile '{name}' does not exist"
        )));
    }
    if document
        .payload
        .get("defaultProfile")
        .and_then(Value::as_str)
        == Some(name)
    {
        document.payload.remove("defaultProfile");
    }
    write_profiles_atomic(&document)?;
    inspect_blocking(root.to_string_lossy().as_ref())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn agent_rules_inspect(
    root_path: String,
) -> Result<AgentRulesInspectResult, AppCommandError> {
    tokio::task::spawn_blocking(move || inspect_blocking(&root_path))
        .await
        .map_err(|error| AppCommandError::task_execution_failed(error.to_string()))?
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn agent_rules_render(
    root_path: String,
    rule_ids: Vec<String>,
    expected_source_hash: String,
) -> Result<AgentRulesRenderResult, AppCommandError> {
    tokio::task::spawn_blocking(move || {
        render_blocking(&root_path, &rule_ids, &expected_source_hash)
    })
    .await
    .map_err(|error| AppCommandError::task_execution_failed(error.to_string()))?
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn agent_rules_save_profile(
    root_path: String,
    name: String,
    rule_ids: Vec<String>,
    expected_source_hash: String,
    set_default: bool,
    overwrite: bool,
) -> Result<AgentRulesInspectResult, AppCommandError> {
    tokio::task::spawn_blocking(move || {
        save_profile_blocking(
            &root_path,
            &name,
            &rule_ids,
            &expected_source_hash,
            set_default,
            overwrite,
        )
    })
    .await
    .map_err(|error| AppCommandError::task_execution_failed(error.to_string()))?
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn agent_rules_rename_profile(
    root_path: String,
    old_name: String,
    new_name: String,
    overwrite: bool,
) -> Result<AgentRulesInspectResult, AppCommandError> {
    tokio::task::spawn_blocking(move || {
        rename_profile_blocking(&root_path, &old_name, &new_name, overwrite)
    })
    .await
    .map_err(|error| AppCommandError::task_execution_failed(error.to_string()))?
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn agent_rules_delete_profile(
    root_path: String,
    name: String,
) -> Result<AgentRulesInspectResult, AppCommandError> {
    tokio::task::spawn_blocking(move || delete_profile_blocking(&root_path, &name))
        .await
        .map_err(|error| AppCommandError::task_execution_failed(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_catalog_for_test(root: &Path) -> Result<Catalog, AppCommandError> {
        let root = fs::canonicalize(root).expect("canonical temp workspace");
        load_catalog(&root)
    }

    fn write_catalog(root: &Path, name: &str, content: &[u8]) {
        let rules = root.join(".codeg/rules");
        fs::create_dir_all(&rules).expect("create rules directory");
        fs::write(rules.join(name), content).expect("write rule catalog");
    }

    #[test]
    fn catalog_preserves_exact_bodies_and_lexical_order() {
        let root = tempfile::tempdir().expect("temp workspace");
        write_catalog(
            root.path(),
            "b.md",
            b"<!-- codeg-rule name=\"Second\" id=\"second\" default=\"off\" -->\r\nB\r\n<!-- /codeg-rule -->\r\n",
        );
        write_catalog(
            root.path(),
            "a.md",
            "<!-- codeg-rule id=\"first\" name=\"Primo \u{2713}\" default=\"on\" -->\nA\n\n<!-- /codeg-rule -->\n".as_bytes(),
        );

        let catalog = load_catalog_for_test(root.path()).expect("valid catalog");

        assert_eq!(
            catalog
                .rules
                .iter()
                .map(|rule| rule.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(catalog.rules[0].body, "A\n\n");
        assert_eq!(catalog.rules[1].body, "B\n");
    }

    #[test]
    fn catalog_rejects_nested_and_duplicate_rules_with_locations() {
        let root = tempfile::tempdir().expect("temp workspace");
        write_catalog(
            root.path(),
            "rules.md",
            b"<!-- codeg-rule id=\"one\" name=\"One\" default=\"on\" -->\n<!-- codeg-rule id=\"two\" name=\"Two\" default=\"off\" -->\n<!-- /codeg-rule -->\n",
        );

        let error = load_catalog_for_test(root.path()).expect_err("nested marker must fail");

        assert!(error
            .to_string()
            .contains(".codeg/rules/rules.md:2: nested rule block"));
    }

    #[test]
    fn render_rejects_a_stale_snapshot() {
        let root = tempfile::tempdir().expect("temp workspace");
        write_catalog(
            root.path(),
            "rules.md",
            b"<!-- codeg-rule id=\"one\" name=\"One\" default=\"on\" -->\nOne\n<!-- /codeg-rule -->\n",
        );

        let error = render_blocking(
            root.path().to_string_lossy().as_ref(),
            &["one".to_owned()],
            "stale",
        )
        .expect_err("stale hash must fail");

        assert!(error.to_string().contains("catalog changed"));
    }

    #[test]
    fn profiles_preserve_unknown_top_level_fields() {
        let root = tempfile::tempdir().expect("temp workspace");
        write_catalog(
            root.path(),
            "rules.md",
            b"<!-- codeg-rule id=\"one\" name=\"One\" default=\"on\" -->\nOne\n<!-- /codeg-rule -->\n",
        );
        let catalog = load_catalog_for_test(root.path()).expect("valid catalog");
        fs::write(
            root.path().join(".codeg/agent-rule-profiles.json"),
            br#"{"version":1,"profiles":{},"teamMetadata":{"owner":"tools"}}"#,
        )
        .expect("write profiles");

        save_profile_blocking(
            root.path().to_string_lossy().as_ref(),
            "default",
            &["one".to_owned()],
            &catalog.source_hash,
            true,
            false,
        )
        .expect("save profile");
        let written: Value = serde_json::from_slice(
            &fs::read(root.path().join(".codeg/agent-rule-profiles.json")).expect("read profiles"),
        )
        .expect("valid JSON");

        assert_eq!(written["teamMetadata"]["owner"], "tools");
    }

    #[cfg(unix)]
    #[test]
    fn catalog_rejects_symlinks_outside_workspace() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temp workspace");
        let outside = tempfile::tempdir().expect("outside directory");
        fs::create_dir_all(root.path().join(".codeg/rules")).expect("create rules");
        let outside_file = outside.path().join("outside.md");
        fs::write(&outside_file, b"outside").expect("write outside file");
        symlink(&outside_file, root.path().join(".codeg/rules/escape.md")).expect("create symlink");

        let error = load_catalog_for_test(root.path()).expect_err("escaping link must fail");

        assert!(error
            .to_string()
            .contains(".codeg/rules/escape.md:1: catalog symlink resolves outside workspace"));
    }
}
