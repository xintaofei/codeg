//! Concise human-readable rendering of an agent tool call's `raw_input`.
//!
//! Shared by the session relay (`session_event_subscriber`) and the global
//! event push (`event_subscriber` permission-request notifications) so a tool
//! call reads the same way — `"Bash: npm test"`, `"Write: src/main.rs"` —
//! wherever it surfaces in a chat channel.

/// Extract a concise detail string from a tool call's `raw_input` JSON.
///
/// Returns a formatted string like `"Read: src/main.rs"` or `"Bash: npm test"`.
/// Falls back to the original title if no detail can be extracted.
pub(crate) fn format_tool_call_detail(title: &str, raw_input: Option<&str>) -> String {
    let parsed = raw_input.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());

    let normalized_title = title.to_lowercase().replace([' ', '-'], "_");

    if let Some(ref obj) = parsed {
        // File operations: read, edit, write, delete
        if let Some(path) = obj
            .get("file_path")
            .or_else(|| obj.get("path"))
            .or_else(|| obj.get("notebook_path"))
            .and_then(|v| v.as_str())
        {
            let short = short_path(path);
            let label = match normalized_title.as_str() {
                s if s.contains("write") => "Write",
                s if s.contains("edit") || s.contains("change") || s.contains("update") => "Edit",
                s if s.contains("delete") => "Delete",
                _ => "Read",
            };
            return format!("{label}: {short}");
        }

        // Bash / shell commands
        if let Some(cmd) = obj
            .get("command")
            .or_else(|| obj.get("cmd"))
            .and_then(|v| v.as_str())
        {
            let short = truncate_str(cmd.lines().next().unwrap_or(cmd), 80);
            return format!("Bash: {short}");
        }

        // Grep / search
        if let Some(pattern) = obj.get("pattern").and_then(|v| v.as_str()) {
            let path = obj.get("path").and_then(|v| v.as_str());
            return if let Some(p) = path {
                format!(
                    "Grep: \"{}\" in {}",
                    truncate_str(pattern, 40),
                    short_path(p)
                )
            } else {
                format!("Grep: \"{}\"", truncate_str(pattern, 60))
            };
        }

        // Glob
        if let Some(pat) = obj.get("glob").and_then(|v| v.as_str()) {
            return format!("Glob: {pat}");
        }

        // Agent / task
        if obj.get("subagent_type").is_some()
            || obj.get("task_id").is_some()
            || obj.get("subject").is_some()
        {
            let desc = obj
                .get("description")
                .or_else(|| obj.get("subject"))
                .or_else(|| obj.get("prompt"))
                .and_then(|v| v.as_str());
            if let Some(d) = desc {
                return format!("Agent: {}", truncate_str(d, 60));
            }
        }

        // Web fetch
        if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
            return format!("Fetch: {}", truncate_str(url, 80));
        }

        // Web search
        if let Some(query) = obj.get("query").and_then(|v| v.as_str()) {
            return format!("Search: {}", truncate_str(query, 60));
        }

        // TodoWrite
        if obj.get("todos").is_some() {
            return "TodoWrite".to_string();
        }
    }

    // Fallback: if raw_input is a plain string (e.g. a bare command), use it directly
    if let Some(raw) = raw_input {
        if !raw.starts_with('{') && !raw.starts_with('[') {
            let short = truncate_str(raw.lines().next().unwrap_or(raw), 80);
            if normalized_title.contains("bash")
                || normalized_title.contains("shell")
                || normalized_title.contains("exec")
            {
                return format!("Bash: {short}");
            }
        }
    }

    title.to_string()
}

fn short_path(path: &str) -> &str {
    // Show last 2 path components at most, or the full path if short enough
    if path.len() <= 60 {
        return path;
    }
    let parts: Vec<&str> = path.rsplitn(3, '/').collect();
    if parts.len() >= 2 {
        // e.g. "src/main.rs" from "/very/long/path/src/main.rs"
        let tail = &path[path.len() - parts[0].len() - parts[1].len() - 1..];
        if tail.len() < path.len() {
            return tail;
        }
    }
    path
}

pub(crate) fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_file_ops_by_title() {
        assert_eq!(
            format_tool_call_detail("Write", Some(r#"{"file_path":"src/main.rs"}"#)),
            "Write: src/main.rs"
        );
        assert_eq!(
            format_tool_call_detail("Edit file", Some(r#"{"file_path":"a/b.rs"}"#)),
            "Edit: a/b.rs"
        );
        // Unknown title with a file_path defaults to Read.
        assert_eq!(
            format_tool_call_detail("View", Some(r#"{"path":"README.md"}"#)),
            "Read: README.md"
        );
    }

    #[test]
    fn formats_bash_first_line_only() {
        assert_eq!(
            format_tool_call_detail("Bash", Some(r#"{"command":"npm test\nsecond line"}"#)),
            "Bash: npm test"
        );
    }

    #[test]
    fn formats_grep_without_path() {
        // A `pattern` with no `path`/`file_path` renders as Grep. (When a `path`
        // is present the file-op branch wins first by design — see
        // `path_precedes_pattern`.)
        assert_eq!(
            format_tool_call_detail("Grep", Some(r#"{"pattern":"bar"}"#)),
            "Grep: \"bar\""
        );
    }

    #[test]
    fn path_precedes_pattern() {
        // Documents the intentional precedence: any `path` is treated as a file
        // operation before the grep branch is considered.
        assert_eq!(
            format_tool_call_detail("Grep", Some(r#"{"pattern":"foo","path":"src"}"#)),
            "Read: src"
        );
    }

    #[test]
    fn falls_back_to_title_when_no_detail() {
        assert_eq!(format_tool_call_detail("Mystery", None), "Mystery");
        assert_eq!(
            format_tool_call_detail("Mystery", Some("not json")),
            "Mystery"
        );
    }

    #[test]
    fn bare_command_string_with_bash_title() {
        assert_eq!(
            format_tool_call_detail("Bash shell", Some("ls -la")),
            "Bash: ls -la"
        );
    }

    #[test]
    fn truncate_str_respects_char_boundaries() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello", 4), "h...");
        // Multi-byte chars must not panic.
        let s = "日本語のテキスト";
        let out = truncate_str(s, 5);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn short_path_keeps_short_and_tails_long() {
        assert_eq!(short_path("src/main.rs"), "src/main.rs");
        let long = "/very/long/absolute/path/that/exceeds/the/sixty/character/limit/src/main.rs";
        let out = short_path(long);
        assert!(out.len() < long.len());
        assert!(out.ends_with("src/main.rs"));
    }
}
