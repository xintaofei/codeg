use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptState {
    Pending,
    Accepted,
    Denied,
    Countered,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Op {
    pub tool: String,
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentEnvelope {
    pub intent: String,
    pub why: String,
    pub ops: Vec<Op>,
    pub accept: AcceptState,
    pub result: Option<String>,
    pub raw: Option<String>,
}

impl IntentEnvelope {
    pub fn denied(reason: &str) -> Self {
        IntentEnvelope {
            intent: String::new(),
            why: String::new(),
            ops: vec![],
            accept: AcceptState::Denied,
            result: Some(format!("denied: {reason}")),
            raw: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denied_carrier_reason_in_result() {
        let e = IntentEnvelope::denied("too abstract");
        assert_eq!(e.accept, AcceptState::Denied);
        assert!(e.result.as_ref().unwrap().contains("too abstract"));
        assert!(e.raw.is_none());
    }
    #[test]
    fn serde_round_trips() {
        let e = IntentEnvelope {
            intent: "list files".into(),
            why: "see layout".into(),
            ops: vec![Op {
                tool: "shell".into(),
                params: serde_json::json!({"cmd":"ls"}),
            }],
            accept: AcceptState::Accepted,
            result: Some("3 dirs".into()),
            raw: Some("dir1 dir2 dir3".into()),
        };
        let j = serde_json::to_string(&e).unwrap();
        let back: IntentEnvelope = serde_json::from_str(&j).unwrap();
        assert_eq!(back.intent, "list files");
        assert_eq!(back.ops.len(), 1);
    }
}
