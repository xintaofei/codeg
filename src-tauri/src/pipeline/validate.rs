use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::models::{AgentType, PipelineGraph, PipelineRole};

#[derive(Debug, Clone, Serialize, Deserialize, Error, PartialEq, Eq)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum PipelineValidationError {
    #[error("pipeline must contain at least one step")]
    Empty,
    #[error("pipeline cannot contain more than 8 steps")]
    TooManySteps,
    #[error("duplicate step id: {id}")]
    DuplicateStepId { id: String },
    #[error("invalid step id: {id}")]
    BadStepId { id: String },
    #[error("unknown agent for step {id}")]
    UnknownAgent { id: String },
    #[error("loop source has an invalid role: {from}")]
    LoopFromWrongRole { from: String },
    #[error("loop target is not earlier: {from} -> {to}")]
    LoopTargetNotEarlier { from: String, to: String },
    #[error("duplicate loop from step: {from}")]
    DuplicateLoop { from: String },
    #[error("invalid max iterations for loop from step: {from}")]
    BadMaxIterations { from: String },
    #[error("step prompt is empty: {id}")]
    EmptyPrompt { id: String },
    #[error("invalid timeout for step: {id}")]
    BadTimeout { id: String },
}

pub fn validate_graph(graph: &PipelineGraph) -> Result<(), PipelineValidationError> {
    if graph.steps.is_empty() {
        return Err(PipelineValidationError::Empty);
    }
    if graph.steps.len() > 8 {
        return Err(PipelineValidationError::TooManySteps);
    }

    let mut ids = HashSet::new();
    for step in &graph.steps {
        let valid_id = !step.id.is_empty()
            && step.id.len() <= 32
            && step.id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
            });
        if !valid_id {
            return Err(PipelineValidationError::BadStepId {
                id: step.id.clone(),
            });
        }
        if !ids.insert(step.id.clone()) {
            return Err(PipelineValidationError::DuplicateStepId {
                id: step.id.clone(),
            });
        }
        if AgentType::from_wire(&step.agent_type).is_none() {
            return Err(PipelineValidationError::UnknownAgent {
                id: step.id.clone(),
            });
        }
        if step.prompt_template.trim().is_empty() {
            return Err(PipelineValidationError::EmptyPrompt {
                id: step.id.clone(),
            });
        }
        if !(1..=86_400).contains(&step.timeout_secs) {
            return Err(PipelineValidationError::BadTimeout {
                id: step.id.clone(),
            });
        }
    }

    let positions = graph
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.id.as_str(), index))
        .collect::<std::collections::HashMap<_, _>>();
    let mut sources = HashSet::new();
    for loop_back in &graph.loops {
        if !sources.insert(loop_back.from_step.clone()) {
            return Err(PipelineValidationError::DuplicateLoop {
                from: loop_back.from_step.clone(),
            });
        }
        let source_index = positions.get(loop_back.from_step.as_str()).copied();
        let target_index = positions.get(loop_back.to_step.as_str()).copied();
        let Some(source_index) = source_index else {
            return Err(PipelineValidationError::LoopFromWrongRole {
                from: loop_back.from_step.clone(),
            });
        };
        let source_role = graph.steps[source_index].role;
        if !matches!(source_role, PipelineRole::Reviewer | PipelineRole::Tests) {
            return Err(PipelineValidationError::LoopFromWrongRole {
                from: loop_back.from_step.clone(),
            });
        }
        let Some(target_index) = target_index else {
            return Err(PipelineValidationError::LoopTargetNotEarlier {
                from: loop_back.from_step.clone(),
                to: loop_back.to_step.clone(),
            });
        };
        if target_index >= source_index {
            return Err(PipelineValidationError::LoopTargetNotEarlier {
                from: loop_back.from_step.clone(),
                to: loop_back.to_step.clone(),
            });
        }
        if !(1..=10).contains(&loop_back.max_iterations) {
            return Err(PipelineValidationError::BadMaxIterations {
                from: loop_back.from_step.clone(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PipelineStep;

    fn step(id: &str, role: PipelineRole) -> PipelineStep {
        PipelineStep {
            id: id.to_string(),
            role,
            label: id.to_string(),
            agent_type: "claude_code".to_string(),
            mode_id: None,
            config_values: Default::default(),
            prompt_template: "$task".to_string(),
            timeout_secs: 1800,
            read_memory: false,
            read_only: false,
        }
    }

    #[test]
    fn validates_loops_and_defaults() {
        let graph = PipelineGraph {
            steps: vec![
                step("coder", PipelineRole::Coder),
                step("review", PipelineRole::Reviewer),
            ],
            loops: vec![crate::models::LoopBack {
                from_step: "review".into(),
                to_step: "coder".into(),
                max_iterations: 3,
            }],
        };
        assert_eq!(validate_graph(&graph), Ok(()));
        assert_eq!(serde_json::from_str::<PipelineStep>(r#"{"id":"x","role":"coder","label":"x","agent_type":"codex","prompt_template":"$task"}"#).unwrap().timeout_secs, 1800);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert_eq!(
            validate_graph(&PipelineGraph::default()),
            Err(PipelineValidationError::Empty)
        );
        let mut graph = PipelineGraph {
            steps: vec![step("Coder", PipelineRole::Coder)],
            loops: vec![],
        };
        assert!(matches!(
            validate_graph(&graph),
            Err(PipelineValidationError::BadStepId { .. })
        ));
        graph.steps[0].id = "coder".into();
        graph.steps[0].agent_type = "unknown".into();
        assert!(matches!(
            validate_graph(&graph),
            Err(PipelineValidationError::UnknownAgent { .. })
        ));
    }

    #[test]
    fn rejects_too_many_steps() {
        let graph = PipelineGraph {
            steps: (0..9)
                .map(|i| step(&format!("s{i}"), PipelineRole::Coder))
                .collect(),
            loops: vec![],
        };
        assert_eq!(
            validate_graph(&graph),
            Err(PipelineValidationError::TooManySteps)
        );
    }

    #[test]
    fn rejects_duplicate_step_id() {
        let graph = PipelineGraph {
            steps: vec![
                step("coder", PipelineRole::Coder),
                step("coder", PipelineRole::Reviewer),
            ],
            loops: vec![],
        };
        assert!(matches!(
            validate_graph(&graph),
            Err(PipelineValidationError::DuplicateStepId { .. })
        ));
    }

    #[test]
    fn rejects_empty_prompt() {
        let mut graph = PipelineGraph {
            steps: vec![step("coder", PipelineRole::Coder)],
            loops: vec![],
        };
        graph.steps[0].prompt_template = "   ".into();
        assert!(matches!(
            validate_graph(&graph),
            Err(PipelineValidationError::EmptyPrompt { .. })
        ));
    }

    #[test]
    fn rejects_bad_timeout() {
        let mut graph = PipelineGraph {
            steps: vec![step("coder", PipelineRole::Coder)],
            loops: vec![],
        };
        graph.steps[0].timeout_secs = 0;
        assert!(matches!(
            validate_graph(&graph),
            Err(PipelineValidationError::BadTimeout { .. })
        ));
    }

    #[test]
    fn rejects_loop_from_wrong_role() {
        let graph = PipelineGraph {
            steps: vec![
                step("coder", PipelineRole::Coder),
                step("coder2", PipelineRole::Coder),
            ],
            loops: vec![crate::models::LoopBack {
                from_step: "coder2".into(),
                to_step: "coder".into(),
                max_iterations: 3,
            }],
        };
        assert!(matches!(
            validate_graph(&graph),
            Err(PipelineValidationError::LoopFromWrongRole { .. })
        ));
    }

    #[test]
    fn rejects_loop_target_not_earlier() {
        let graph = PipelineGraph {
            steps: vec![
                step("review", PipelineRole::Reviewer),
                step("coder", PipelineRole::Coder),
            ],
            loops: vec![crate::models::LoopBack {
                from_step: "review".into(),
                to_step: "coder".into(),
                max_iterations: 3,
            }],
        };
        assert!(matches!(
            validate_graph(&graph),
            Err(PipelineValidationError::LoopTargetNotEarlier { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_loop() {
        let graph = PipelineGraph {
            steps: vec![
                step("coder", PipelineRole::Coder),
                step("review", PipelineRole::Reviewer),
                step("tests", PipelineRole::Tests),
            ],
            loops: vec![
                crate::models::LoopBack {
                    from_step: "review".into(),
                    to_step: "coder".into(),
                    max_iterations: 3,
                },
                crate::models::LoopBack {
                    from_step: "review".into(),
                    to_step: "coder".into(),
                    max_iterations: 2,
                },
            ],
        };
        assert!(matches!(
            validate_graph(&graph),
            Err(PipelineValidationError::DuplicateLoop { .. })
        ));
    }

    #[test]
    fn rejects_bad_max_iterations() {
        let graph = PipelineGraph {
            steps: vec![
                step("coder", PipelineRole::Coder),
                step("review", PipelineRole::Reviewer),
            ],
            loops: vec![crate::models::LoopBack {
                from_step: "review".into(),
                to_step: "coder".into(),
                max_iterations: 0,
            }],
        };
        assert!(matches!(
            validate_graph(&graph),
            Err(PipelineValidationError::BadMaxIterations { .. })
        ));
    }

    #[test]
    fn accepts_custom_agent_with_nonempty_id() {
        let mut graph = PipelineGraph {
            steps: vec![step("coder", PipelineRole::Coder)],
            loops: vec![],
        };
        graph.steps[0].agent_type = "custom:my-agent".into();
        assert_eq!(validate_graph(&graph), Ok(()));
    }

    #[test]
    fn rejects_custom_agent_with_empty_id() {
        let mut graph = PipelineGraph {
            steps: vec![step("coder", PipelineRole::Coder)],
            loops: vec![],
        };
        graph.steps[0].agent_type = "custom:".into();
        assert!(matches!(
            validate_graph(&graph),
            Err(PipelineValidationError::UnknownAgent { .. })
        ));
    }
}
