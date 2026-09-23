//! Parse `VERDICT:` markers and template variable substitution for pipeline prompts.

use crate::models::PipelineVerdict;

/// Attempt to parse a `VERDICT: ...` marker from text.
/// Returns `(verdict, notes)` on success.
/// Notes = text after the marker (if non-empty) or before (if after is empty).
pub fn parse_marker(text: &str) -> Option<(PipelineVerdict, Option<String>)> {
    let lines: Vec<&str> = text.lines().collect();
    for (idx, line) in lines.iter().enumerate().rev() {
        let trimmed = line.trim();
        let clean = trimmed.trim_matches(|c: char| c == '*' || c == '_' || c == '`' || c == '#');
        if clean.starts_with("VERDICT:") {
            let verdict_part = clean.strip_prefix("VERDICT:")?.trim();
            let verdict_token =
                verdict_part.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
            let verdict = match verdict_token.to_uppercase().as_str() {
                "PASS" => PipelineVerdict::Pass,
                "CHANGES_REQUESTED" | "FAIL" => PipelineVerdict::ChangesRequested,
                "INCONCLUSIVE" => PipelineVerdict::Inconclusive,
                _ => return None,
            };

            // Notes: text after marker (if non-empty), else all text before marker
            let notes = if idx + 1 < lines.len() {
                let after = lines[idx + 1..].join("\n").trim().to_string();
                if !after.is_empty() {
                    Some(after)
                } else {
                    let before = lines[..idx].join("\n").trim().to_string();
                    if !before.is_empty() {
                        Some(before)
                    } else {
                        None
                    }
                }
            } else {
                let before = lines[..idx].join("\n").trim().to_string();
                if !before.is_empty() {
                    Some(before)
                } else {
                    None
                }
            };

            return Some((verdict, notes));
        }
    }
    None
}

pub struct PromptVars {
    pub task: String,
    pub plan: Option<String>,
    pub summary: Option<String>,
    pub review: Option<String>,
    pub memory: String,
}

/// Substitute `$` placeholders in a prompt template.
pub fn render_prompt(template: &str, vars: &PromptVars) -> String {
    template
        .replace("$task", &vars.task)
        .replace("$plan", vars.plan.as_deref().unwrap_or(""))
        .replace("$summary", vars.summary.as_deref().unwrap_or(""))
        .replace("$review", vars.review.as_deref().unwrap_or(""))
        .replace("$memory", &vars.memory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_marker_recognizes_pass() {
        let text = "All looks good!\n\nVERDICT: PASS";
        let (verdict, notes) = parse_marker(text).expect("parse");
        assert_eq!(verdict, PipelineVerdict::Pass);
        assert_eq!(notes, Some("All looks good!".into()));
    }

    #[test]
    fn parse_marker_recognizes_fail_as_changes_requested() {
        let text = "Line 5 is wrong.\n\nVERDICT: FAIL";
        let (verdict, notes) = parse_marker(text).expect("parse");
        assert_eq!(verdict, PipelineVerdict::ChangesRequested);
        assert_eq!(notes, Some("Line 5 is wrong.".into()));
    }

    #[test]
    fn parse_marker_recognizes_inconclusive() {
        let text = "Not sure what's happening here.\n\nVERDICT: INCONCLUSIVE";
        let (verdict, notes) = parse_marker(text).expect("parse");
        assert_eq!(verdict, PipelineVerdict::Inconclusive);
        assert_eq!(notes, Some("Not sure what's happening here.".into()));
    }

    #[test]
    fn parse_marker_uses_text_after_if_present() {
        let text = "Context before.\n\nVERDICT: PASS\n\nNotes after marker.";
        let (verdict, notes) = parse_marker(text).expect("parse");
        assert_eq!(verdict, PipelineVerdict::Pass);
        assert_eq!(notes, Some("Notes after marker.".into()));
    }

    #[test]
    fn parse_marker_returns_none_for_missing_verdict() {
        let text = "No verdict here.";
        assert_eq!(parse_marker(text), None);
    }

    #[test]
    fn render_prompt_substitutes_all_placeholders() {
        let template = "Task: $task\nPlan: $plan\nSummary: $summary\nReview: $review\nMemory: $memory";
        let vars = PromptVars {
            task: "write code".into(),
            plan: Some("plan text".into()),
            summary: Some("summary text".into()),
            review: Some("review text".into()),
            memory: "memory block".into(),
        };
        let result = render_prompt(template, &vars);
        assert!(result.contains("Task: write code"));
        assert!(result.contains("Plan: plan text"));
        assert!(result.contains("Summary: summary text"));
        assert!(result.contains("Review: review text"));
        assert!(result.contains("Memory: memory block"));
    }

    #[test]
    fn render_prompt_handles_missing_optional_vars() {
        let template = "Plan: $plan";
        let vars = PromptVars {
            task: "x".into(),
            plan: None,
            summary: None,
            review: None,
            memory: "".into(),
        };
        let result = render_prompt(template, &vars);
        assert_eq!(result, "Plan: ");
    }
}
