use sha2::{Digest, Sha256};

use crate::models::{ContentBlock, MessageTurn, TurnRole};

/// Maximum UTF-8 bytes kept from one text block.
pub const MAX_BLOCK_BYTES: usize = 8_192;

/// The normalized, indexable representation of one conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedDocument {
    pub text: String,
    pub content_hash: String,
}

/// Tokens for the optional short-query FTS table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShortIndexTokens {
    pub words: String,
    pub bigrams: String,
}

/// Extract only user and assistant `Text` blocks into one searchable document.
///
/// System prompts, reasoning, tool blocks, and images never reach this
/// function's input in their indexed form; the role/block filters here are the
/// second, defensive boundary.
pub fn normalize_turns(turns: &[MessageTurn]) -> NormalizedDocument {
    let mut text = String::new();
    let mut wrote_block = false;

    for turn in turns {
        if !matches!(turn.role, TurnRole::User | TurnRole::Assistant) {
            continue;
        }
        for block in &turn.blocks {
            let ContentBlock::Text { text: block_text } = block else {
                continue;
            };
            let trimmed = block_text.trim();
            if trimmed.is_empty() {
                continue;
            }
            if wrote_block {
                text.push_str("\n\n");
            }
            text.push_str(&truncate_text_block(trimmed, MAX_BLOCK_BYTES));
            wrote_block = true;
        }
    }

    NormalizedDocument {
        content_hash: sha256_hex(&text),
        text,
    }
}

/// Truncate a UTF-8 string to `max_bytes` without splitting a character.
pub fn truncate_text_block(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// Lowercase SHA-256 hex digest, used for cheap content diffing.
pub fn sha256_hex(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    format!("{digest:x}")
}

/// Build `words` and `bigrams` for the short-query FTS index.
pub fn short_index_tokens(text: &str) -> ShortIndexTokens {
    let mut words = String::new();
    let mut bigrams = String::new();
    let mut runs: Vec<(bool, Vec<char>)> = Vec::new();

    for ch in text.chars() {
        let cjk = is_cjk(ch);
        match runs.last_mut() {
            Some((kind, chars)) if *kind == cjk => chars.push(ch),
            _ => runs.push((cjk, vec![ch])),
        }
    }

    for (cjk, chars) in runs {
        if cjk {
            for &ch in &chars {
                push_token(&mut words, &ch.to_string());
            }
            for pair in chars.windows(2) {
                let token: String = pair.iter().collect();
                push_token(&mut bigrams, &token);
            }
        } else {
            let mut current = String::new();
            for ch in chars {
                if ch.is_alphanumeric() {
                    current.extend(ch.to_lowercase());
                } else if !current.is_empty() {
                    push_token(&mut words, &current);
                    current.clear();
                }
            }
            if !current.is_empty() {
                push_token(&mut words, &current);
            }
        }
    }

    ShortIndexTokens { words, bigrams }
}

fn push_token(output: &mut String, token: &str) {
    if token.is_empty() {
        return;
    }
    if !output.is_empty() {
        output.push(' ');
    }
    output.push_str(token);
}

/// CJK ranges covered by the short-query index. Fullwidth Latin, symbols, and
/// other scripts intentionally stay on the Latin/word path.
pub fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2FA1F
            | 0x3040..=0x30FF
            | 0xAC00..=0xD7AF
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MessageTurn;

    fn text_turn(role: TurnRole, text: &str) -> MessageTurn {
        MessageTurn {
            id: "turn-1".to_string(),
            role,
            blocks: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            timestamp: chrono::Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
        }
    }

    #[test]
    fn normalizes_only_user_and_assistant_text() {
        let turns = vec![
            text_turn(TurnRole::User, "  你好  "),
            text_turn(TurnRole::Assistant, "  回答  "),
            text_turn(TurnRole::System, "  系统  "),
        ];
        let doc = normalize_turns(&turns);
        assert_eq!(doc.text, "你好\n\n回答");
        assert_eq!(doc.content_hash.len(), 64);
    }

    #[test]
    fn truncation_never_splits_utf8() {
        let long = "你".repeat(4_096) + "a";
        let truncated = truncate_text_block(&long, MAX_BLOCK_BYTES);
        assert!(truncated.len() <= MAX_BLOCK_BYTES);
        assert_eq!(truncated.chars().last(), Some('你'));
    }

    #[test]
    fn short_tokens_emit_cjk_unigrams_and_bigrams() {
        let tokens = short_index_tokens("搜索chat");
        assert_eq!(tokens.words, "搜 索 chat");
        assert_eq!(tokens.bigrams, "搜索");
    }
}
