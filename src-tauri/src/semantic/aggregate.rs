use crate::semantic::envelope::{AcceptState, IntentEnvelope};
use std::collections::BTreeMap;

/// Group envelopes that share an `intent` into a single envelope: ops are
/// concatenated, results joined with a separator. Raw is joined the same way
/// but kept for storage only (never shown by the UI).
pub fn aggregate(envelopes: Vec<IntentEnvelope>) -> Vec<IntentEnvelope> {
    if envelopes.len() <= 1 {
        return envelopes;
    }
    let mut groups: BTreeMap<String, IntentEnvelope> = BTreeMap::new();
    for e in envelopes {
        let entry = groups.entry(e.intent.clone()).or_insert(IntentEnvelope {
            intent: e.intent.clone(),
            why: e.why.clone(),
            ops: vec![],
            accept: AcceptState::Accepted,
            result: Some(String::new()),
            raw: Some(String::new()),
        });
        entry.ops.extend(e.ops);
        if let Some(r) = e.result {
            let cur = entry.result.as_mut().unwrap();
            if !cur.is_empty() {
                cur.push_str(" | ");
            }
            cur.push_str(&r);
        }
        if let Some(raw) = e.raw {
            let cur = entry.raw.as_mut().unwrap();
            if !cur.is_empty() {
                cur.push_str("\n---\n");
            }
            cur.push_str(&raw);
        }
    }
    groups.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::envelope::{AcceptState, IntentEnvelope, Op};

    fn env(intent: &str, op_tool: &str, result: &str) -> IntentEnvelope {
        IntentEnvelope {
            intent: intent.into(),
            why: String::new(),
            ops: vec![Op {
                tool: op_tool.into(),
                params: serde_json::json!({}),
            }],
            accept: AcceptState::Accepted,
            result: Some(result.into()),
            raw: Some(format!("raw-{result}")),
        }
    }

    #[test]
    fn parallel_ops_same_intent_merge_to_one() {
        let out = aggregate(vec![
            env("build", "shell", "compiled a"),
            env("build", "shell", "compiled b"),
            env("test", "shell", "ran t"),
        ]);
        // two distinct intents -> two envelopes
        assert_eq!(out.len(), 2);
        let build = out.iter().find(|e| e.intent == "build").unwrap();
        assert_eq!(build.ops.len(), 2);
        assert!(build.result.as_ref().unwrap().contains("compiled a"));
        assert!(build.result.as_ref().unwrap().contains("compiled b"));
    }
}
