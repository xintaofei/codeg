/// Secret redaction before memory write.
use regex::Regex;

/// Redact API keys, tokens, passwords, and private key material from text.
/// Returns (sanitized_text, number_of_replacements).
pub fn sanitize_secrets(text: &str) -> (String, usize) {
    let mut result = text.to_string();
    let mut count = 0;

    // Patterns for common secret types.
    let patterns = [
        // Standalone OpenAI/Anthropic keys (e.g. sk-..., sk-ant-..., sk-proj-...)
        (r#"sk-(?:ant-|proj-)?[A-Za-z0-9_-]{20,}"#, "AI API key"),
        // Bearer tokens in headers or prose ("Authorization: Bearer <token>")
        (r#"(?i)bearer\s+[A-Za-z0-9\-._~+/]{8,}=*"#, "bearer token"),
        // API keys: "api_key=..." or "apiKey: ..." or standalone hex keys
        (
            r#"(?i)(api[_-]?key|api[_-]?token)\s*[:=]\s*['\"]?[\w\-]+['\"]?"#,
            "api key",
        ),
        // Tokens: "token: ..." or Bearer/AWS/GitHub token patterns
        (
            r#"(?i)(token|access[_-]?token|refresh[_-]?token)\s*[:=]\s*['\"]?[\w\-./+]+['\"]?"#,
            "token",
        ),
        // AWS keys
        (
            r#"(?i)(aws[_-]?access|aws[_-]?secret|AKIA[0-9A-Z]{16})"#,
            "AWS key",
        ),
        // Passwords in URLs or assignments
        (
            r#"(?i)(password|passwd)\s*[:=]\s*['\"]?[^\s'\"]+['\"]?"#,
            "password",
        ),
        // Private keys (basic pattern)
        (
            r#"-----BEGIN[\s\w-]+PRIVATE[\s\w-]+-----[\s\S]*?-----END[\s\w-]+PRIVATE[\s\w-]+-----"#,
            "private key",
        ),
        // GitHub tokens (ghp_, gho_, ghu_, ghs_, ghr_)
        (r#"gh[pousr]_[A-Za-z0-9_]{20,255}"#, "GitHub token"),
        // Slack token
        (
            r#"xox[baprs]-[0-9]{10,13}-[0-9]{10,13}-[^\s]+['\"]?"#,
            "Slack token",
        ),
    ];

    for (pattern_str, _name) in &patterns {
        if let Ok(re) = Regex::new(pattern_str) {
            let new_result = re.replace_all(&result, "[REDACTED]").to_string();
            count += re.find_iter(&result).count();
            result = new_result;
        }
    }

    (result, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_api_keys() {
        let (result, count) = sanitize_secrets("Use api_key=my-secret-key123");
        assert!(result.contains("[REDACTED]"));
        assert!(count > 0);
    }

    #[test]
    fn redacts_bearer_tokens() {
        let (result, count) = sanitize_secrets("auth: Bearer sk-12345abcde67890");
        assert!(result.contains("[REDACTED]"));
        assert!(count > 0);
    }

    #[test]
    fn redacts_passwords() {
        let (result, count) = sanitize_secrets("password=supersecret123");
        assert!(result.contains("[REDACTED]"));
        assert!(count > 0);
    }

    #[test]
    fn redacts_standalone_sk_and_ghp_tokens() {
        let (res1, c1) = sanitize_secrets("sk-123456789012345678901234567890");
        assert_eq!(res1, "[REDACTED]");
        assert!(c1 > 0);

        let (res2, c2) = sanitize_secrets("ghp_123456789012345678901234567890123456");
        assert_eq!(res2, "[REDACTED]");
        assert!(c2 > 0);
    }
}
