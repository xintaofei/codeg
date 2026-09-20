use std::collections::BTreeMap;

use crate::models::{AgentType, LoopBack, PipelineGraph, PipelineRole, PipelineStep};

/// Instruction appended to reviewer/tests prompts so the agent actually
/// reports a verdict instead of leaving the step `inconclusive` by default.
const VERDICT_INSTRUCTION: &str =
    "Call the pipeline_verdict tool exactly once, right before you finish.";

fn default_agent(agent: Option<&str>) -> String {
    agent
        .filter(|slug| AgentType::from_wire(slug).is_some())
        .unwrap_or("claude_code")
        .to_string()
}

fn step(
    id: &'static str,
    role: PipelineRole,
    label: &'static str,
    read_only: bool,
    agent_type: &str,
) -> PipelineStep {
    let prompt_template = if matches!(role, PipelineRole::Reviewer | PipelineRole::Tests) {
        format!("$task\n\n$plan\n$summary\n$review\n$memory\n\n{VERDICT_INSTRUCTION}")
    } else {
        "$task\n\n$plan\n$summary\n$review\n$memory".into()
    };
    PipelineStep {
        id: id.into(),
        role,
        label: label.into(),
        agent_type: agent_type.into(),
        mode_id: None,
        config_values: BTreeMap::new(),
        prompt_template,
        timeout_secs: 1800,
        read_memory: false,
        read_only,
    }
}

/// Built-in preset graphs. `agent` selects the agent slug used for every
/// step, falling back to `claude_code` when absent or unknown (the real
/// per-agent default from `DelegationConfig.agent_defaults` is wired in by
/// the engine in a later phase).
pub fn builtin_presets(agent: Option<&str>) -> Vec<(&'static str, &'static str, PipelineGraph)> {
    let agent = default_agent(agent);
    vec![
        (
            "duet",
            "Duet",
            PipelineGraph {
                steps: vec![
                    step("coder", PipelineRole::Coder, "Coder", false, &agent),
                    step("reviewer", PipelineRole::Reviewer, "Reviewer", true, &agent),
                ],
                loops: vec![LoopBack {
                    from_step: "reviewer".into(),
                    to_step: "coder".into(),
                    max_iterations: 3,
                }],
            },
        ),
        (
            "team",
            "Team",
            PipelineGraph {
                steps: vec![
                    step("planner", PipelineRole::Planner, "Planner", true, &agent),
                    step("coder", PipelineRole::Coder, "Coder", false, &agent),
                    step("reviewer", PipelineRole::Reviewer, "Reviewer", true, &agent),
                    step("tests", PipelineRole::Tests, "Tests", false, &agent),
                ],
                loops: vec![
                    LoopBack {
                        from_step: "reviewer".into(),
                        to_step: "coder".into(),
                        max_iterations: 3,
                    },
                    LoopBack {
                        from_step: "tests".into(),
                        to_step: "coder".into(),
                        max_iterations: 3,
                    },
                ],
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::validate::validate_graph;

    #[test]
    fn builtin_presets_pass_validation() {
        for (key, _name, graph) in builtin_presets(None) {
            assert_eq!(validate_graph(&graph), Ok(()), "preset {key} is invalid");
        }
    }

    #[test]
    fn tests_step_is_not_read_only() {
        let (_, _, graph) = builtin_presets(None)
            .into_iter()
            .find(|(key, _, _)| *key == "team")
            .expect("team preset");
        let tests_step = graph
            .steps
            .iter()
            .find(|s| s.role == PipelineRole::Tests)
            .expect("tests step");
        assert!(!tests_step.read_only);
    }

    #[test]
    fn reviewer_and_tests_prompts_ask_for_verdict() {
        for (_, _, graph) in builtin_presets(None) {
            for s in &graph.steps {
                if matches!(s.role, PipelineRole::Reviewer | PipelineRole::Tests) {
                    assert!(s.prompt_template.contains("pipeline_verdict"));
                }
            }
        }
    }

    #[test]
    fn unknown_agent_falls_back_to_claude_code() {
        let (_, _, graph) = builtin_presets(Some("not-a-real-agent"))
            .into_iter()
            .next()
            .unwrap();
        assert!(graph.steps.iter().all(|s| s.agent_type == "claude_code"));
    }

    #[test]
    fn known_agent_is_used_for_every_step() {
        let (_, _, graph) = builtin_presets(Some("codex")).into_iter().next().unwrap();
        assert!(graph.steps.iter().all(|s| s.agent_type == "codex"));
    }
}
