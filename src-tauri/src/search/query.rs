/// A single user query is split into at most this many terms.
pub const MAX_QUERY_TERMS: usize = 8;

/// Split a query on Unicode whitespace and drop empty terms.
pub fn split_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .take(MAX_QUERY_TERMS)
        .map(ToOwned::to_owned)
        .collect()
}

/// Escape SQLite `LIKE` metacharacters for an `ESCAPE '\'` pattern.
pub fn escape_like(term: &str) -> String {
    let mut escaped = String::with_capacity(term.len());
    for ch in term.chars() {
        match ch {
            '\\' | '%' | '_' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// A parameterized substring pattern: `%escaped%`.
pub fn like_pattern(term: &str) -> String {
    format!("%{}%", escape_like(term))
}

/// Build an FTS5 trigram `MATCH` expression for terms of at least three
/// Unicode characters. Adjacent three-character grams are ANDed; the caller
/// still applies `LIKE` as the exact substring filter.
pub fn trigram_expression(term: &str) -> Option<String> {
    let chars: Vec<char> = term.chars().collect();
    if chars.len() < 3 {
        return None;
    }
    let grams = chars
        .windows(3)
        .map(|window| format!("\"{}\"", fts_quote(&window.iter().collect::<String>())))
        .collect::<Vec<_>>()
        .join(" AND ");
    Some(grams)
}

/// How a one- or two-character term is queried against the short FTS table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortTermQuery {
    CjkUnigram { token: String },
    CjkBigram { phrase: String },
    LatinPrefix { token: String },
}

/// Build a short-term query. Callers use this only when the term is one or two
/// Unicode characters; longer terms use the trigram index.
pub fn short_query(term: &str) -> ShortTermQuery {
    let chars: Vec<char> = term.chars().collect();
    let all_cjk = !chars.is_empty()
        && chars
            .iter()
            .all(|ch| crate::search::normalizer::is_cjk(*ch));
    if all_cjk {
        if chars.len() == 1 {
            ShortTermQuery::CjkUnigram {
                token: fts_quote(&chars[0].to_string()),
            }
        } else {
            let bigrams: Vec<String> = chars
                .windows(2)
                .map(|window| fts_quote(&window.iter().collect::<String>()))
                .collect();
            ShortTermQuery::CjkBigram {
                phrase: bigrams.join(" "),
            }
        }
    } else {
        ShortTermQuery::LatinPrefix {
            token: fts_quote(&term.to_lowercase()),
        }
    }
}

/// Quote an FTS5 phrase element and double embedded quotes.
pub fn fts_quote(input: &str) -> String {
    input.replace('"', "\"\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_terms_and_caps_at_eight() {
        let terms = split_terms("  one  两个\tthree four five six seven eight nine ten ");
        assert_eq!(terms.len(), 8);
        assert_eq!(terms[0], "one");
        assert_eq!(terms[1], "两个");
    }

    #[test]
    fn escapes_like_metacharacters() {
        assert_eq!(escape_like(r"100%_ok\"), r"100\%\_ok\\");
        assert_eq!(like_pattern("a%b"), "%a\\%b%");
    }

    #[test]
    fn trigram_expression_needs_three_chars_and_quotes() {
        assert_eq!(trigram_expression("ab"), None);
        assert_eq!(
            trigram_expression("会话记录"),
            Some("\"会话记\" AND \"话记录\"".to_string())
        );
        assert_eq!(
            trigram_expression("a\"bcd"),
            Some("\"a\"\"b\" AND \"\"\"bc\" AND \"bcd\"".to_string())
        );
    }

    #[test]
    fn routes_short_queries_by_script() {
        assert_eq!(
            short_query("会"),
            ShortTermQuery::CjkUnigram {
                token: "会".to_string()
            }
        );
        assert_eq!(
            short_query("聊天"),
            ShortTermQuery::CjkBigram {
                phrase: "聊天".to_string()
            }
        );
        assert_eq!(
            short_query("Ab"),
            ShortTermQuery::LatinPrefix {
                token: "ab".to_string()
            }
        );
    }
}
