//! Parses a Conductor's free-form output into a structured task list.
//!
//! Conductors emit task plans in two stable shapes that we accept:
//!
//! 1. **Fenced JSON block** — the canonical structured format:
//!
//!    ```text
//!    ```json
//!    [
//!      {"role": "frontend", "title": "...", "description": "..."},
//!      {"role": "backend",  "title": "...", "description": "..."}
//!    ]
//!    ```
//!    ```
//!
//! 2. **Markdown checklist** — fallback when the model resists JSON:
//!
//!    ```text
//!    - [ ] [frontend] Update settings panel — wire mode toggle to backend
//!    - [ ] [backend] Add /squad/mode endpoint — accept run_id + new mode
//!    ```
//!
//! Anything we can't parse cleanly is dropped with a reason in
//! [`ParseReport::skipped`] so callers can surface it.

use serde::Deserialize;

use crate::models::squad::SquadRoleKind;

/// One task as recovered from the Conductor's reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedConductorTask {
    pub role: SquadRoleKind,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseReport {
    pub tasks: Vec<ParsedConductorTask>,
    /// Lines/blocks that *looked* task-shaped but failed validation.
    pub skipped: Vec<String>,
}

#[derive(Deserialize)]
struct JsonTask {
    #[serde(default)]
    role: Option<String>,
    #[serde(default, alias = "role_kind", alias = "roleKind")]
    role_kind: Option<String>,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

/// Parse a Conductor's full reply text. Tries fenced JSON first; if no
/// usable JSON is found, falls back to the markdown-checklist scanner.
pub fn parse_conductor_output(raw: &str) -> ParseReport {
    if let Some(report) = parse_fenced_json(raw) {
        if !report.tasks.is_empty() || !report.skipped.is_empty() {
            return report;
        }
    }
    parse_markdown_checklist(raw)
}

fn parse_fenced_json(raw: &str) -> Option<ParseReport> {
    let block = extract_json_array(raw)?;
    let parsed: Result<Vec<JsonTask>, _> = serde_json::from_str(&block);
    let mut report = ParseReport::default();
    match parsed {
        Ok(items) => {
            for item in items {
                let role_str = item
                    .role_kind
                    .as_deref()
                    .or(item.role.as_deref())
                    .unwrap_or("");
                let Some(role) = parse_role(role_str) else {
                    report.skipped.push(format!(
                        "unknown role '{role_str}' for task '{}'",
                        item.title
                    ));
                    continue;
                };
                let title = item.title.trim().to_string();
                if title.is_empty() {
                    report.skipped.push("empty title".into());
                    continue;
                }
                let description = item
                    .description
                    .or(item.summary)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                report.tasks.push(ParsedConductorTask {
                    role,
                    title,
                    description,
                });
            }
            Some(report)
        }
        Err(err) => {
            report
                .skipped
                .push(format!("fenced JSON did not deserialize: {err}"));
            Some(report)
        }
    }
}

fn extract_json_array(raw: &str) -> Option<String> {
    // Look for ```json ... ``` first, then any ``` ... ``` containing an array.
    let mut search_from = 0usize;
    while let Some(open_rel) = raw[search_from..].find("```") {
        let open = search_from + open_rel;
        let after_open = open + 3;
        // skip optional language tag
        let after_lang = raw[after_open..]
            .find('\n')
            .map(|n| after_open + n + 1)
            .unwrap_or(after_open);
        let close_rel = raw[after_lang..].find("```")?;
        let close = after_lang + close_rel;
        let body = raw[after_lang..close].trim();
        if body.starts_with('[') && body.ends_with(']') {
            return Some(body.to_string());
        }
        search_from = close + 3;
    }
    // Last-ditch: a bare top-level array.
    let trimmed = raw.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        return Some(trimmed.to_string());
    }
    None
}

fn parse_markdown_checklist(raw: &str) -> ParseReport {
    let mut report = ParseReport::default();
    for line in raw.lines() {
        let trimmed = line.trim_start();
        if !(trimmed.starts_with("- [ ]")
            || trimmed.starts_with("- [x]")
            || trimmed.starts_with("* [ ]"))
        {
            continue;
        }
        // strip checkbox prefix
        let after_box = trimmed
            .split_once(']')
            .map(|(_, rest)| rest.trim_start())
            .unwrap_or("");
        // role tag: "[frontend] ..." or "(frontend) ..."
        let (role_str, rest) = match after_box.strip_prefix('[') {
            Some(s) => match s.split_once(']') {
                Some((role, rest)) => (role.trim(), rest.trim()),
                None => {
                    report
                        .skipped
                        .push(format!("malformed role tag in line: {line}"));
                    continue;
                }
            },
            None => match after_box.strip_prefix('(') {
                Some(s) => match s.split_once(')') {
                    Some((role, rest)) => (role.trim(), rest.trim()),
                    None => {
                        report
                            .skipped
                            .push(format!("malformed role tag in line: {line}"));
                        continue;
                    }
                },
                None => {
                    report
                        .skipped
                        .push(format!("missing role tag in line: {line}"));
                    continue;
                }
            },
        };
        let Some(role) = parse_role(role_str) else {
            report
                .skipped
                .push(format!("unknown role '{role_str}' in line: {line}"));
            continue;
        };

        // title — description (em-dash, en-dash, or plain --)
        let (title, description) = split_title_description(rest);
        if title.is_empty() {
            report.skipped.push(format!("empty title in line: {line}"));
            continue;
        }
        report.tasks.push(ParsedConductorTask {
            role,
            title: title.to_string(),
            description: description.to_string(),
        });
    }
    report
}

fn split_title_description(text: &str) -> (&str, &str) {
    for sep in [" — ", " – ", " -- ", ": "] {
        if let Some((t, d)) = text.split_once(sep) {
            return (t.trim(), d.trim());
        }
    }
    (text.trim(), "")
}

fn parse_role(raw: &str) -> Option<SquadRoleKind> {
    let s = raw.trim().to_ascii_lowercase();
    // Try the snake_case serde form first via JSON round-trip so the parser
    // automatically picks up new variants.
    let quoted = format!("\"{s}\"");
    if let Ok(role) = serde_json::from_str::<SquadRoleKind>(&quoted) {
        return Some(role);
    }
    // Friendly aliases the model often produces.
    match s.as_str() {
        "fe" | "front" | "front-end" | "front_end" => Some(SquadRoleKind::Frontend),
        "be" | "back" | "back-end" | "back_end" | "server" => Some(SquadRoleKind::Backend),
        "exec" | "engineer" | "impl" | "implementation" => Some(SquadRoleKind::Worker),
        "lead" | "pm" | "planner" | "orchestrator" => Some(SquadRoleKind::Conductor),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_json_array() {
        let raw = r#"
Here's the plan:

```json
[
  {"role": "frontend", "title": "Settings panel toggle", "description": "wire UI"},
  {"role": "backend",  "title": "API endpoint",          "description": "accept payload"}
]
```

Let me know.
"#;
        let report = parse_conductor_output(raw);
        assert_eq!(report.skipped.len(), 0, "skipped: {:?}", report.skipped);
        assert_eq!(report.tasks.len(), 2);
        assert_eq!(report.tasks[0].role, SquadRoleKind::Frontend);
        assert_eq!(report.tasks[0].title, "Settings panel toggle");
        assert_eq!(report.tasks[1].role, SquadRoleKind::Backend);
    }

    #[test]
    fn accepts_role_kind_alias_and_summary_fallback() {
        let raw = r#"```json
[{"roleKind": "worker", "title": "Refactor module", "summary": "split file"}]
```"#;
        let report = parse_conductor_output(raw);
        assert_eq!(report.tasks.len(), 1);
        assert_eq!(report.tasks[0].role, SquadRoleKind::Worker);
        assert_eq!(report.tasks[0].description, "split file");
    }

    #[test]
    fn parses_markdown_checklist_fallback() {
        let raw = "\
Plan:
- [ ] [frontend] Toggle button — wire mode change
- [ ] [backend] /squad/mode -- accept run_id + new mode
- [ ] [worker] Migrate tests: split into per-mode files
";
        let report = parse_conductor_output(raw);
        assert_eq!(report.skipped.len(), 0, "skipped: {:?}", report.skipped);
        assert_eq!(report.tasks.len(), 3);
        assert_eq!(report.tasks[0].role, SquadRoleKind::Frontend);
        assert_eq!(report.tasks[0].description, "wire mode change");
        assert_eq!(report.tasks[1].role, SquadRoleKind::Backend);
        assert_eq!(report.tasks[1].description, "accept run_id + new mode");
        assert_eq!(report.tasks[2].description, "split into per-mode files");
    }

    #[test]
    fn reports_unknown_role_without_panicking() {
        let raw = r#"```json
[{"role": "wizard", "title": "Cast spell"}]
```"#;
        let report = parse_conductor_output(raw);
        assert_eq!(report.tasks.len(), 0);
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].contains("wizard"));
    }

    #[test]
    fn empty_input_yields_empty_report() {
        let report = parse_conductor_output("");
        assert!(report.tasks.is_empty());
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn ignores_chatter_without_blocks() {
        let report = parse_conductor_output("I think we should split this work nicely.");
        assert!(report.tasks.is_empty());
    }

    #[test]
    fn role_aliases_resolve() {
        assert_eq!(parse_role("fe"), Some(SquadRoleKind::Frontend));
        assert_eq!(parse_role("BE"), Some(SquadRoleKind::Backend));
        assert_eq!(parse_role("planner"), Some(SquadRoleKind::Conductor));
        assert_eq!(parse_role("impl"), Some(SquadRoleKind::Worker));
        assert_eq!(parse_role("frontend"), Some(SquadRoleKind::Frontend));
    }

    #[test]
    fn parses_cjk_titles_and_descriptions() {
        // Conductors will often plan in the user's language. CJK chars
        // should round-trip cleanly through both parsers.
        let raw = r#"```json
[
  {"role": "frontend", "title": "设置面板开关", "description": "接入 UI"},
  {"role": "backend",  "title": "API 端点",     "description": "接受 payload"}
]
```"#;
        let report = parse_conductor_output(raw);
        assert_eq!(report.skipped.len(), 0);
        assert_eq!(report.tasks.len(), 2);
        assert_eq!(report.tasks[0].title, "设置面板开关");
        assert_eq!(report.tasks[1].description, "接受 payload");
    }

    #[test]
    fn checklist_skips_lines_without_role_tag() {
        // Lines that don't carry a [role] prefix should be silently
        // skipped — the conductor often interleaves prose with the list.
        let raw = "\
Plan:
- [ ] [frontend] Toggle button — wire mode change
- [ ] just a note about the layout
- [ ] [worker] Migrate tests
";
        let report = parse_conductor_output(raw);
        assert_eq!(
            report.tasks.len(),
            2,
            "untagged lines are skipped, tagged ones kept"
        );
        assert_eq!(report.tasks[0].role, SquadRoleKind::Frontend);
        assert_eq!(report.tasks[1].role, SquadRoleKind::Worker);
    }

    #[test]
    fn json_takes_precedence_over_checklist_when_both_present() {
        // If a fenced JSON array parses successfully, the checklist
        // fallback should not also run — otherwise we'd double-count.
        let raw = r#"
- [ ] [worker] checklist task

```json
[{"role": "frontend", "title": "json task", "description": "from json"}]
```
"#;
        let report = parse_conductor_output(raw);
        assert_eq!(
            report.tasks.len(),
            1,
            "JSON wins; checklist must not also fire"
        );
        assert_eq!(report.tasks[0].title, "json task");
    }

    #[test]
    fn malformed_json_falls_through_to_checklist() {
        // A fenced block that fails to parse as JSON shouldn't poison
        // the rest of the message — the markdown fallback should still
        // pick up checklist items elsewhere in the body.
        let raw = "\
```json
{this is not valid JSON
```

- [ ] [backend] still got picked up
";
        let report = parse_conductor_output(raw);
        assert_eq!(report.tasks.len(), 1);
        assert_eq!(report.tasks[0].role, SquadRoleKind::Backend);
    }

    #[test]
    fn json_object_with_tasks_array_supported() {
        // Some Conductors will wrap the array in a top-level object.
        // We require a literal array; an object is unparseable for the
        // array shape, so this should fall through to no tasks.
        let raw = r#"```json
{"tasks": [{"role": "worker", "title": "x", "description": "y"}]}
```"#;
        let report = parse_conductor_output(raw);
        // We don't currently destructure {tasks: [...]}, so we expect
        // zero tasks; this guards against accidentally accepting the
        // wrong shape later.
        assert_eq!(report.tasks.len(), 0);
    }
}
