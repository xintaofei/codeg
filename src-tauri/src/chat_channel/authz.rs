//! Inbound sender authorization for chat channels.
//!
//! Chat commands can spawn and drive real agents (`/task`, `/approve`, …) with the
//! host's full privileges, so an un-vetted inbound message is equivalent to remote
//! code execution. Every inbound command is therefore gated on a per-channel
//! allowlist of sender ids, stored in the channel `config_json` under
//! `allowed_senders` (an array of strings).
//!
//! The gate is **fail-closed**: a missing key, an empty list, or an unparseable
//! config authorizes nobody. Operators add their sender id (surfaced to them in the
//! "unauthorized" reply) via the channel settings before the bot will act.

use serde_json::Value;

/// Extract and normalize the `allowed_senders` list from a channel `config_json`.
/// Entries are trimmed; blanks are dropped. Returns empty on any parse failure.
fn allowed_senders(config_json: &str) -> Vec<String> {
    serde_json::from_str::<Value>(config_json)
        .ok()
        .as_ref()
        .and_then(|v| v.get("allowed_senders"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `sender_id` is permitted to drive the channel.
///
/// Fail-closed: an empty/absent allowlist or a blank sender authorizes no one.
pub fn is_sender_allowed(config_json: &str, sender_id: &str) -> bool {
    let sender = sender_id.trim();
    if sender.is_empty() {
        return false;
    }
    allowed_senders(config_json)
        .iter()
        .any(|allowed| allowed == sender)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_missing_allowlist_denies_everyone() {
        assert!(!is_sender_allowed("{}", "123"));
        assert!(!is_sender_allowed(
            r#"{"bot_token":"x","chat_id":"y"}"#,
            "123"
        ));
        assert!(!is_sender_allowed(r#"{"allowed_senders":[]}"#, "123"));
    }

    #[test]
    fn unparseable_config_denies() {
        assert!(!is_sender_allowed("not json", "123"));
        assert!(!is_sender_allowed("", "123"));
    }

    #[test]
    fn listed_sender_is_allowed_others_are_not() {
        let cfg = r#"{"bot_token":"x","allowed_senders":["123","456"]}"#;
        assert!(is_sender_allowed(cfg, "123"));
        assert!(is_sender_allowed(cfg, "456"));
        assert!(!is_sender_allowed(cfg, "789"));
    }

    #[test]
    fn ids_and_sender_are_trimmed() {
        let cfg = r#"{"allowed_senders":["  123  ","456"]}"#;
        assert!(is_sender_allowed(cfg, "123"));
        assert!(is_sender_allowed(cfg, "  123 "));
    }

    #[test]
    fn blank_sender_is_denied_even_if_blank_listed() {
        // blanks are stripped from the list, so an empty sender never matches
        let cfg = r#"{"allowed_senders":["","  "]}"#;
        assert!(!is_sender_allowed(cfg, ""));
        assert!(!is_sender_allowed(cfg, "  "));
    }

    #[test]
    fn non_string_entries_are_ignored() {
        let cfg = r#"{"allowed_senders":[123,"456",true,null]}"#;
        // numeric 123 is ignored (Telegram ids arrive as strings); only "456" counts
        assert!(!is_sender_allowed(cfg, "123"));
        assert!(is_sender_allowed(cfg, "456"));
    }
}
