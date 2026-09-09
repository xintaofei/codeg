//! Content translation: turning an agent's English prose into the user's
//! language without touching the code, links, formulas, or markup inside it.
//!
//! Split of responsibility with the frontend, which matters for reading the
//! cache keys here: **masking and splitting happen before this module sees a
//! text**. The renderer masks literal spans (`markdown-mask.ts`) into opaque
//! `[[CBLK<n>]]` placeholders and splits over-long messages on paragraph
//! boundaries, then sends the resulting pieces here. So every `text` reaching
//! this module is already masked, and hashing it directly is what makes the
//! cache key stable across the `parts`-array replacement the message list
//! performs when a turn settles.
//!
//! PR1 shape: ONE endpoint (see [`endpoint`]), settled texts only, and a
//! single acceptance owner — `client::translate_one` runs the quality gates
//! BEFORE anything is reported or returned, so a parseable but refused reply
//! can never be reported as success or take root in the cache. This module
//! dispatches, caches gate-accepted replies, and owns the identity
//! short-circuit (already in the target language → returned verbatim,
//! `skipped: true`, no request, no cache write, no metrics).

pub mod cache;
pub mod client;
pub mod endpoint;
pub mod metrics;
pub mod prompt;
pub mod settings;

use std::sync::OnceLock;

/// Hard ceiling on a single text's length before translation. The frontend
/// splits on paragraph boundaries, so this only trips on an unreasonable input
/// (or a masked span that grew past the guard): refuse rather than send a huge
/// body that would blow the request timeout.
pub const MAX_SINGLE_TEXT_CHARS: usize = 20_000;

use serde::Serialize;

use crate::app_error::AppCommandError;
use crate::translation::cache::TranslationCache;
use crate::translation::metrics::{translation_metrics, GateRejection};
use crate::translation::settings::TranslationSettings;

pub use cache::TranslationCacheStats;
pub use settings::TRANSLATION_SETTINGS_KEY;

/// Process-wide cache. Rooted under the regenerable cache dir, so wiping it is
/// a supported action that costs only refetches.
pub fn translation_cache() -> &'static TranslationCache {
    static CACHE: OnceLock<TranslationCache> = OnceLock::new();
    CACHE.get_or_init(|| {
        TranslationCache::new(crate::paths::codeg_cache_dir().join("translation"))
    })
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranslationResult {
    /// The content-addressed cache key, so a caller can correlate a result
    /// with what it asked for without relying on position alone.
    pub key: String,
    pub text: String,
    pub from_cache: bool,
    /// The identity short-circuit fired: the text is already written in the
    /// target language and was returned verbatim — no request was made, no
    /// cache entry written, no metrics recorded. The caller renders it as-is
    /// and must not retry it.
    #[serde(default)]
    pub skipped: bool,
    /// Why this chunk has no translation, when the endpoint failed on it.
    /// A batch is per-chunk fault tolerant — the successful chunks are
    /// cached and returned even when a sibling hit the endpoint's rate
    /// limit — so a `Some` here means "discard this result and retry";
    /// the cached siblings make that retry cheap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The endpoint that produced (or failed) this slot — cache, native-skip,
    /// and unconfigured slots have none. Observability only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Round-trip of the deciding attempt, milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

/// What the model is told to translate *into*. The BCP-47 tags the interface
/// uses are ambiguous to a model asked in prose ("zh-CN" invites Pinyin more
/// often than it invites 简体中文), so each supported locale states its name.
/// An unknown tag passes through unchanged — a user pointing at their own
/// endpoint may well want a language codeg's UI does not ship.
pub fn display_language(locale: &str) -> &str {
    match locale {
        "en" => "English",
        "zh-CN" => "Simplified Chinese",
        "zh-TW" => "Traditional Chinese",
        "ja" => "Japanese",
        "ko" => "Korean",
        "es" => "Spanish",
        "de" => "German",
        "fr" => "French",
        "pt" => "Portuguese",
        "ar" => "Arabic",
        other => other,
    }
}

/// The language to translate into: the explicit setting when the user picked
/// one, otherwise whatever locale the interface is currently in.
pub fn resolve_target_lang(settings: &TranslationSettings, ui_locale: &str) -> String {
    settings
        .target_lang
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(ui_locale)
        .to_string()
}

/// How far a translation may exceed its source before it is refused. Real
/// expansion is tight: English→Chinese comes out SHORTER in characters
/// (Chinese is denser), and even the widest pair in practice (CJK→English)
/// sits under ~2× — placeholders contribute equally to both sides and
/// Markdown markers survive the trip. A model asked to translate a
/// self-contained chunk sometimes ANSWERS the question the text discusses
/// instead; that reply is real Chinese and passes the script gate, but it is
/// several times the source length. 2.5× + 200 catches the answer-shaped
/// replies (observed 5-7×) while never clipping a genuine translation.
fn length_sanity_error(source: &str, translated: &str) -> Option<String> {
    let source_len = source.chars().count();
    let translated_len = translated.chars().count();
    if translated_len > source_len * 5 / 2 + 200 {
        return Some(format!(
            "The translation is far longer than its source ({translated_len} vs {source_len} characters) — the endpoint answered with invented content"
        ));
    }
    None
}

/// The placeholder token the frontend's mask emits, plus the loose shapes a
/// model may produce while imitating it (stray whitespace inside the
/// brackets, a dropped outer bracket pair). Used only to EXCLUDE placeholder
/// bytes from source-side analysis — validation of the reply's tokens lives
/// in the frontend, which owns the mask.
fn strip_translation_placeholders(text: &str) -> String {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re =
        RE.get_or_init(|| regex::Regex::new(r"\[\s*\[?_?CBLK\d+\s*\]\s*\]?").expect("valid regex"));
    re.replace_all(text, "").into_owned()
}

/// Whitespace-insensitive text for the exact-echo comparison: trim plus
/// collapse every whitespace run to a single space. An endpoint's reflow of
/// the same words is still an echo. Mirrors `normalizeEchoText` in
/// src/lib/translation.ts — the two gates must agree or one reply passes one
/// side and fails the other.
fn normalize_echo_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `translated` looks like an echo or a refusal rather than a
/// translation: the target language is CJK, the source carries real prose
/// (≥30 Latin letters outside masked placeholders), and the reply contains
/// zero target-script characters. Both shapes were served by a real relay —
/// an English source "translated" into English unchanged, and a bare refusal
/// ("I am not able to comply with this request." for a Git-merge explanation)
/// — and the length gate cannot see either: an echo is 1:1, a refusal is
/// shorter. A legitimate translation of that much prose always lands in the
/// target script.
fn echo_or_refusal_error(source: &str, translated: &str, target_lang: &str) -> Option<String> {
    let lang = target_lang.trim().to_ascii_lowercase();
    let cjk_target = lang == "zh" || lang == "ja" || lang == "ko" || lang.starts_with("zh-");
    if !cjk_target {
        return None;
    }
    // Placeholder tokens (`[[CBLK<n>]]`, loose imitations thereof) stand in
    // for code and must not count as prose.
    let prose = strip_translation_placeholders(source);
    // Exact echo, judged before the letters bar: a code-heavy chunk masks
    // down to placeholders plus a few words, so its verbatim echo never
    // reaches 30 letters and the script gate below cannot see it either
    // (observed on a relay). Content equality is what catches it — tokens
    // stripped from both sides, because an echo carries the same tokens the
    // source does. A placeholder-only chunk skips: echoing `[[CBLK0]]` back
    // IS the correct translation.
    if !prose.trim().is_empty()
        && normalize_echo_text(&prose)
            == normalize_echo_text(&strip_translation_placeholders(translated))
    {
        return Some(
            "The reply is the source returned verbatim — the endpoint echoed the chunk".to_string(),
        );
    }
    let letters = prose.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if letters < 30 {
        return None;
    }
    let has_target_script = translated.chars().any(|c| {
        matches!(c,
            '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}'
            | '\u{3040}'..='\u{30ff}' | '\u{ac00}'..='\u{d7af}')
    });
    if !has_target_script {
        return Some(
            "The reply contains no target-language script — the endpoint echoed or refused the chunk".to_string(),
        );
    }
    None
}

/// Digit runs (two or more digits) the source prose carries that the
/// translation dropped. A model that answers the text instead of translating
/// it routinely sheds the concrete numbers ("Git 2.34" → "Git 较新版本");
/// a faithful translation keeps them verbatim in every language codeg ships.
/// Only runs of ≥2 digits count — a lone "v5"-style digit is too noisy — and
/// masked regions (code, URLs, math) never reach this gate: they were replaced
/// by placeholders before the request. A false positive costs one discarded
/// attempt and a retry; a missed invention poisons the cache for every later
/// render of the block.
fn missing_source_numbers(source: &str, translated: &str) -> Option<String> {
    let prose = normalize_number_text(&strip_translation_placeholders(source));
    let translated = normalize_number_text(translated);
    let mut runs: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut flush = |current: &mut String| {
        if current.chars().count() >= 2 && !runs.contains(current) {
            runs.push(current.clone());
        }
        current.clear();
    };
    for ch in prose.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else {
            flush(&mut current);
        }
    }
    flush(&mut current);
    if runs.is_empty() {
        return None;
    }
    let missing = runs
        .iter()
        .filter(|run| !translated.contains(run.as_str()))
        .count();
    // 一两处"缺失"多半是归一化覆盖不到的排版差或无害省略；只有过半
    // 缺失才说明模型在回答而不是翻译。
    if missing < 2 || missing * 2 < runs.len() {
        return None;
    }
    Some(format!(
        "the reply dropped {missing} of {} numbers present in the source — the endpoint likely answered instead of translating",
        runs.len()
    ))
}

/// 数字比较前的归一化：全角数字/句点/逗号折叠为半角，再剥掉夹在数字
/// 中间的千分位逗号。模型输出 `2．34` 或 `1,234`/`1234` 的差异是排版，
/// 不是丢数字。
fn normalize_number_text(text: &str) -> String {
    let folded: String = text
        .chars()
        .map(|ch| match ch {
            '０'..='９' => char::from_u32('0' as u32 + (ch as u32 - '０' as u32)).unwrap_or(ch),
            '．' => '.',
            '，' => ',',
            _ => ch,
        })
        .collect();
    let chars: Vec<char> = folded.chars().collect();
    let mut out = String::with_capacity(chars.len());
    for (i, &ch) in chars.iter().enumerate() {
        let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
        let next_digit = chars.get(i + 1).is_some_and(|c| c.is_ascii_digit());
        if ch == ',' && prev_digit && next_digit {
            continue;
        }
        out.push(ch);
    }
    out
}

/// Restores the paragraph boundaries a model destroyed when it answered a
/// multi-part source inline: it compressed the segments into one line and
/// emitted the protocol markers `[1] [2] …` itself (a shape only a numbered
/// request is allowed to carry). Observed on a real relay replying to a plain
/// list: `- one\n- two\n- three` came back as `[1] 一 [2] 二 [3] 三` on a
/// single line; the renderer strips only the leading marker, so the inner
/// `[2] [3]` leak into the rendered text. The restoration replaces each
/// marker with the `\n\n` segment boundary the model was supposed to emit.
///
/// Mirrors the frontend's `normalizeProtocolMarkerEcho` in
/// src/lib/translation.ts — the two must agree token for token or one reply
/// is restored on one side and leaks on the other. The digit class is pinned
/// to `[0-9]` (not `\d`) because Rust's regex `\d` is Unicode-aware while
/// JavaScript's is ASCII-only; the byte-exact match keeps the mirror honest.
///
/// Deliberately conservative — every ambiguity returns the text unchanged:
/// the source itself carrying a marker shape (a numbered request, or prose
/// that quotes one), fewer than two markers (a lone `[1]` is too noisy), and
/// a non-consecutive numbering run (a real segmented reply counts by one).
pub fn normalize_protocol_marker_echo(translated: &str, source: &str) -> String {
    static MARKER_RE: OnceLock<regex::Regex> = OnceLock::new();
    static SOURCE_MARKER_RE: OnceLock<regex::Regex> = OnceLock::new();
    let marker_re =
        MARKER_RE.get_or_init(|| regex::Regex::new(r"\[([0-9]{1,3})\][ \t]").expect("valid regex"));
    let source_marker_re = SOURCE_MARKER_RE.get_or_init(|| {
        regex::Regex::new(r"\[[0-9]{1,3}\][ \t]").expect("valid regex")
    });
    // A source carrying the marker shape means this IS a numbered exchange —
    // the markers belong to the protocol, not to a flattened echo.
    if source_marker_re.is_match(source) {
        return translated.to_string();
    }
    let hits: Vec<(usize, usize, u32)> = marker_re
        .captures_iter(translated)
        .map(|caps| {
            let whole = caps.get(0).expect("the whole match");
            let number = caps[1].parse::<u32>().expect("1-3 digits fit u32");
            (whole.start(), whole.end(), number)
        })
        .collect();
    if hits.len() < 2 {
        return translated.to_string();
    }
    // Strictly consecutive numbering is the fingerprint of a segmented reply
    // the model flattened; anything else (repeats, gaps, descending) is prose
    // that happens to quote bracketed numbers and must not be touched.
    if hits.windows(2).any(|pair| pair[1].2 != pair[0].2 + 1) {
        return translated.to_string();
    }
    let mut out = String::with_capacity(translated.len());
    let mut cursor = 0usize;
    for (start, end, _) in &hits {
        out.push_str(&translated[cursor..*start]);
        out.push_str("\n\n");
        cursor = *end;
    }
    out.push_str(&translated[cursor..]);
    out.trim_start().to_string()
}

/// Whether a line carries structure a translation must keep: either a
/// sentence-terminal line (terminal punctuation, then at most two closing
/// quotes/brackets) or a list item (`- `, `* `, or `1.`/`1、`/`1)` with up to
/// three leading spaces). Hard-wrapped prose lines — no terminal punctuation,
/// no marker — do not count, so a legal rewrap never trips the gate below.
fn is_structural_line(line: &str) -> bool {
    static LIST_RE: OnceLock<regex::Regex> = OnceLock::new();
    let list_re = LIST_RE
        .get_or_init(|| regex::Regex::new(r"^ {0,3}(?:[-*] |[0-9]{1,2}[.、)] )").expect("valid regex"));
    if line.trim().is_empty() {
        return false;
    }
    // Sentence-terminal: strip up to two trailing closers ("he said.") and
    // look for the punctuation underneath.
    let mut end = line.trim_end();
    for _ in 0..2 {
        match end.chars().last() {
            Some(c @ ('"' | '\'' | '”' | '’' | '」' | '』' | '）' | ')' | '】' | '》' | '〉')) => {
                end = &end[..end.len() - c.len_utf8()];
            }
            _ => break,
        }
    }
    if matches!(
        end.chars().last(),
        Some('.' | '!' | '?' | '…' | '。' | '！' | '？')
    ) {
        return true;
    }
    list_re.is_match(line)
}

/// A reply that collapsed the source's line structure: the source carries
/// `s` structural lines (see [`is_structural_line`]) but the reply keeps
/// fewer than half of them as lines. Observed on a real relay — an
/// eight-item list came back as one line with `[1] [2]` inline — and a
/// flattened reply poisons the cache for every later render of the block.
/// The bar is lenient (half, and only from three structural lines up, with
/// hard-wrapped prose not counting) so a legitimate reflow is never refused;
/// a false negative costs a corrupted render, a false positive costs one
/// retry of a fine reply.
fn structure_flatten_error(source: &str, translated: &str) -> Option<String> {
    let structural_lines = source.lines().filter(|line| is_structural_line(line)).count();
    // Below three the signal is too thin: one or two structural lines say
    // nothing about a reply's line structure.
    if structural_lines < 3 {
        return None;
    }
    let reply_lines = translated
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    // t >= ceil(s / 2) passes.
    if reply_lines >= structural_lines.div_ceil(2) {
        return None;
    }
    Some(format!(
        "the reply flattened {structural_lines} structural lines of the source into {reply_lines} — the endpoint dropped the line structure"
    ))
}

/// Whether `source` is the frontend's numbered-segment request shape
/// (`buildNumberedRequest` in src/lib/translation.ts): lines opening with
/// `[n]` headers. Its replies are reassembled by the frontend's
/// `parseNumberedTranslation`, so the numbered protocol owns both the
/// markers and the line structure — the local restore/gates below must not
/// second-guess it.
pub fn is_numbered_request(source: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re =
        RE.get_or_init(|| regex::Regex::new(r"(?m)^\[[0-9]+\][ \t]").expect("valid regex"));
    re.is_match(source)
}

/// Strips the leading context-reference block the frontend prepends to an
/// outbound request (`buildContextPrefix` in `src/lib/translation.ts`): the
/// previous segment's source + translation, framed between the
/// `[Reference for consistency only…]` and `[End of reference…]` scaffolding
/// lines. That block is the MODEL's consistency anchor and still rides to the
/// endpoint with the request — but it is not content, so every LOCAL judgment
/// here (skip detection, cache key, quality-gate source) must see only the
/// body after it. Without stripping, the reference's numbers count as source
/// numbers the model was told not to output, so `missing_source_numbers`
/// rejects a faithful translation every time; its English boilerplate also
/// unconditionally clears the echo gate's ≥30-Latin-letters bar. Texts
/// without the prefix pass through unchanged.
fn strip_context_reference(text: &str) -> String {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"\A\[Reference for consistency only[\s\S]*?\[End of reference[^\n]*\n")
            .expect("valid regex")
    });
    re.replace(text, "").into_owned()
}

/// Strips the `<translate target="…">…</translate>` envelope the frontend
/// wraps around every outbound body — the hard content/instruction boundary
/// that suppresses echo-mode answers at the request-shape level. The envelope
/// rides to the endpoint (it IS the request shape), but the local judgments —
/// skip detection, cache key, quality-gate source — must see only the inner
/// text: the tag boilerplate's Latin letters would otherwise clear the echo
/// gate's ≥30-letter prose bar on code-heavy chunks. Texts without the
/// envelope pass through unchanged.
fn strip_translate_envelope(text: &str) -> String {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // Anchored at the end: a source that itself quotes `</translate>`
        // mid-text extends the match to the real, final closing tag.
        regex::Regex::new(r"<translate[^>]*>\n([\s\S]*?)\n?</translate>\s*\z").expect("valid regex")
    });
    match re.captures(text) {
        Some(caps) => caps[1].to_string(),
        None => text.to_string(),
    }
}

/// The four gates in their evaluation order, each tagged with the rejection
/// bucket the metrics record. The user-facing message is unchanged; the tag
/// is what the metrics' rejection buckets count.
///
/// The reply text arriving here must already have gone through
/// [`normalize_protocol_marker_echo`] (done at the acceptance site in
/// `client.rs`, which owns the text that reaches the cache). Numbered-segment
/// requests are exempt from the flatten gate: their `[n]` headers are the
/// frontend's `buildNumberedRequest` scaffolding and the reply's line groups
/// are reassembled by the frontend's `parseNumberedTranslation`, so the
/// line-count heuristic is not calibrated for that shape and must stand
/// down.
pub fn quality_gate_error(
    source: &str,
    translated: &str,
    target_lang: &str,
) -> Option<(GateRejection, String)> {
    length_sanity_error(source, translated)
        .map(|message| (GateRejection::Invented, message))
        .or_else(|| {
            echo_or_refusal_error(source, translated, target_lang)
                .map(|message| (GateRejection::EchoOrRefusal, message))
        })
        .or_else(|| {
            missing_source_numbers(source, translated)
                .map(|message| (GateRejection::DroppedNumbers, message))
        })
        .or_else(|| {
            if is_numbered_request(source) {
                return None;
            }
            structure_flatten_error(source, translated)
                .map(|message| (GateRejection::Invented, message))
        })
}

/// Whether `text` is already written in `target_lang` closely enough that a
/// "translation" can only damage it. The failure modes are all observed on a
/// real relay: an already-Chinese chunk came back empty (erasing the source),
/// truncated to its first sentence (dropping the rest), or expanded into a
/// self-written essay (grafting content the source never had). A chunk that is
/// predominantly target-language script is returned verbatim instead — no
/// request, no cache write, nothing to go wrong.
///
/// Script ranges only, and deliberately narrow: Simplified Chinese targets
/// skip on a majority of CJK ideographs (kana marks Japanese apart), Japanese
/// requires kana, Korean requires hangul. Traditional Chinese (`zh-TW`) never
/// skips — Simplified→Traditional IS a conversion, and script detection cannot
/// see it. Latin-script targets have no reliable test and never skip.
fn already_in_target_language(text: &str, target_lang: &str) -> bool {
    let lang = target_lang.trim().to_ascii_lowercase();
    let zh_hans = lang == "zh"
        || lang.starts_with("zh-cn")
        || lang.starts_with("zh-hans")
        || lang.starts_with("zh-sg");
    let ja = lang == "ja";
    let ko = lang == "ko";
    if !zh_hans && !ja && !ko {
        return false;
    }

    let mut total = 0usize;
    let mut cjk = 0usize;
    let mut kana = 0usize;
    let mut hangul = 0usize;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        total += 1;
        if matches!(ch, '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}')
            || matches!(ch, '\u{3000}'..='\u{303f}' | '\u{ff00}'..='\u{ffef}')
        {
            cjk += 1;
        }
        if matches!(ch, '\u{3040}'..='\u{30ff}') {
            kana += 1;
        }
        if matches!(ch, '\u{ac00}'..='\u{d7af}' | '\u{1100}'..='\u{11ff}') {
            hangul += 1;
        }
    }
    if total == 0 {
        return false;
    }
    let frac = |count: usize| count as f64 / total as f64;

    if zh_hans {
        return frac(cjk) > 0.5 && frac(kana) < 0.02;
    }
    if ja {
        return frac(cjk) + frac(kana) > 0.5 && frac(kana) >= 0.02;
    }
    frac(hangul) > 0.5
}

/// Translate `texts` in order, serving what the cache already holds and
/// requesting only the rest.
///
/// Every text is expected to be **masked** already (see the module docs).
/// Returns one result per input, in the same order. `priority` picks the
/// concurrency lane: reader-facing prose and user-initiated calls queue
/// separately from background thinking-block polish so a backlog in one can
/// never starve the other. `override_target_lang` lets a caller (the
/// selection-translation card) aim at a language other than the configured
/// target for just that request; the cache keys stay per-language, so the two
/// never serve each other. `variant` is the caller's retry counter — a bumped
/// variant gets a fresh request instead of the cached reply the caller is
/// retrying away from.
pub async fn translate_with_cache(
    texts: &[String],
    ui_locale: &str,
    settings: &TranslationSettings,
    priority: client::Priority,
    override_target_lang: Option<&str>,
    variant: u32,
    trace: Option<&str>,
) -> Result<Vec<TranslationResult>, AppCommandError> {
    if !settings.enabled {
        return Err(AppCommandError::configuration_missing(
            "Translation is not enabled",
        ));
    }

    if let Some(over) = texts
        .iter()
        .find(|text| text.chars().count() > MAX_SINGLE_TEXT_CHARS)
    {
        let n = over.chars().count();
        return Err(AppCommandError::invalid_input(format!(
            "Translation text is too long ({n} characters; the limit is {MAX_SINGLE_TEXT_CHARS})"
        )));
    }

    // The one endpoint every request in this batch goes to. An enabled
    // feature with no callable endpoint is a configuration hole: fail here,
    // before touching the cache, with the actionable message.
    let active = endpoint::select_endpoint(settings)?;
    let provider_id = active.provider_id();

    let target_lang = override_target_lang
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| resolve_target_lang(settings, ui_locale));
    let cache = translation_cache();

    // Resolve hits first so only the misses reach the network, and so a batch
    // that is entirely cached makes no request at all.
    let mut results: Vec<Option<TranslationResult>> = Vec::with_capacity(texts.len());
    let mut misses: Vec<String> = Vec::new();
    // The stripped bodies, positionally aligned with `misses`: the endpoint
    // gets the full outbound (reference block included), while the gates and
    // the cache judge the body only.
    let mut miss_bodies: Vec<String> = Vec::new();
    let mut miss_positions: Vec<usize> = Vec::new();

    for text in texts.iter() {
        // The context reference rides in the same request body as the
        // consistency anchor for the MODEL, but it is not content: skip
        // detection and the cache key must judge the body after it, or the
        // reference could skew both. The <translate> envelope is the same
        // story one layer out: it IS the request shape the model sees, and
        // the judgments must see only what is inside it.
        let body = strip_translate_envelope(&strip_context_reference(text));
        let key = TranslationCache::key_for(&body, &target_lang, &provider_id, variant);
        // An already-target-language chunk skips the endpoint entirely: every
        // failure mode it has (empty, truncated, invented) damages text that
        // was already what the reader wanted to see. No request, no cache
        // write, no metrics — nothing happened.
        if already_in_target_language(&body, &target_lang) {
            results.push(Some(TranslationResult {
                key,
                text: body,
                from_cache: false,
                skipped: true,
                error: None,
                provider_id: None,
                latency_ms: None,
            }));
            continue;
        }
        match cache.get(&body, &target_lang, &provider_id, variant) {
            Some(hit) => {
                translation_metrics().record_cache_hit();
                translation_metrics().record_served();
                results.push(Some(TranslationResult {
                    key,
                    text: hit,
                    from_cache: true,
                    skipped: false,
                    error: None,
                    provider_id: None,
                    latency_ms: None,
                }))
            }
            None => {
                results.push(None);
                miss_positions.push(results.len() - 1);
                misses.push(text.clone());
                miss_bodies.push(body);
            }
        }
    }

    if !misses.is_empty() {
        // The cooldown is judged only when a request would actually leave:
        // cache hits and identity skips must keep working while the endpoint
        // sits out its bench.
        endpoint::ensure_available()?;

        let outcomes = client::translate_batch(
            &misses,
            &miss_bodies,
            &target_lang,
            &active,
            settings,
            priority,
            trace,
        )
        .await;

        // Per-chunk fault tolerance: cache and return every accepted reply
        // even when a sibling chunk failed (a strict requests-per-minute
        // quota fails *some* of a large burst, and failed attempts count
        // against it). The caller discards the failed slots but the cached
        // successes make its bounded retry converge — it re-requests only
        // what is still missing.
        //
        // The outcomes arrive already ACCEPTED: the gate ran inside
        // `client::translate_one` before any report was made, so an `Ok`
        // here is exactly a gate-passed reply and the only remaining job is
        // cache-then-return. A refused reply can therefore never take root
        // under this chunk's key.
        let mut failures = 0usize;
        for (i, outcome) in outcomes.into_iter().enumerate() {
            let slot = miss_positions[i];
            let key = TranslationCache::key_for(
                &miss_bodies[i],
                &target_lang,
                &provider_id,
                variant,
            );
            match outcome.result {
                Ok(translation) => {
                    cache.insert(
                        &miss_bodies[i],
                        &target_lang,
                        &provider_id,
                        variant,
                        &translation,
                    );
                    translation_metrics().record_served();
                    results[slot] = Some(TranslationResult {
                        key,
                        text: translation,
                        from_cache: false,
                        skipped: false,
                        error: None,
                        provider_id: Some(provider_id.clone()),
                        latency_ms: Some(outcome.latency_ms),
                    });
                }
                Err(err) => {
                    failures += 1;
                    tracing::warn!(
                        "[translation] chunk {}/{} failed: {}",
                        failures,
                        misses.len(),
                        err.message
                    );
                    results[slot] = Some(TranslationResult {
                        key,
                        text: String::new(),
                        from_cache: false,
                        skipped: false,
                        error: Some(err.message),
                        provider_id: Some(provider_id.clone()),
                        latency_ms: Some(outcome.latency_ms),
                    });
                }
            }
        }
        if failures > 0 {
            tracing::warn!(
                "[translation] batch of {} chunk(s): {} failed, {} served",
                misses.len(),
                failures,
                misses.len() - failures
            );
        }
    }

    // Every slot was filled either from cache, from the identity short-
    // circuit, or from the batch above.
    results
        .into_iter()
        .map(|slot| {
            slot.ok_or_else(|| {
                AppCommandError::task_execution_failed(
                    "The translation service returned fewer results than requested",
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_locales_are_named_for_the_model() {
        assert_eq!(display_language("zh-CN"), "Simplified Chinese");
        assert_eq!(display_language("zh-TW"), "Traditional Chinese");
        assert_eq!(display_language("ja"), "Japanese");
    }

    /// A translation cannot be an order of magnitude longer than its source:
    /// past the gate lies the distill that answered a one-line source with a
    /// self-written essay, which must be refused before it reaches the cache.
    #[test]
    fn an_invented_essay_is_refused_by_the_length_gate() {
        // The observed shape: a 65-character source, a 1000+-character essay.
        let source = "下面按要求用英文分多段详细展开。\n\n";
        let essay = "合".repeat(1081);
        assert!(length_sanity_error(source, &essay).is_some());
    }

    #[test]
    fn legitimate_expansion_passes_the_length_gate() {
        // English prose expands into Chinese at well under 4× in characters,
        // and the +200 slack keeps short sources from tripping on rounding.
        let source = "A merge in Git is the process of integrating two divergent \
lines of development into a single, unified snapshot.";
        let translated = "Git 中的合并是将两条分化的开发路径集成为单一统一快照的过程。";
        assert!(length_sanity_error(source, translated).is_none());
        // A placeholder-heavy chunk translates with the tokens intact; the
        // equal contribution keeps the ratio stable.
        let masked = "[[CBLK0]] merges [[CBLK1]] heads.";
        assert!(length_sanity_error(masked, "把两个分支头合并起来。").is_none());
    }

    /// A translation that shed the source's concrete numbers is answering the
    /// text rather than translating it; the gate refuses it before the cache
    /// write so the invention can never take root under this chunk's key.
    #[test]
    fn a_translation_that_dropped_source_numbers_is_refused() {
        let source = "Since Git 2.34 the default strategy is ort, introduced in 2021.";
        assert!(missing_source_numbers(source, "自较新版本起，默认策略已经是新的实现。").is_some());
        // A faithful translation keeps every run.
        assert!(
            missing_source_numbers(source, "自 Git 2.34 起默认策略是 ort，于 2021 年引入。")
                .is_none()
        );
        // Single digits are too noisy to gate: "v5" alone never trips it.
        assert!(missing_source_numbers("update to v5", "升级到 v5").is_none());
        // Numbers inside masked placeholders never reach the gate.
        assert!(missing_source_numbers("[[CBLK12]] explains it", "详见 [[CBLK12]]").is_none());
    }

    #[test]
    fn fullwidth_numbers_and_thousand_separators_are_not_drops() {
        // 全角数字与全角小数点只是排版差异，不是编造。
        assert!(missing_source_numbers(
            "Git 2.34 shipped in 2023 with 15 fixes",
            "Git 2．34 于 2023 年发布，包含 15 项修复",
        )
        .is_none());
        // 千分位逗号 vs 无分隔符，同一数字。
        assert!(missing_source_numbers(
            "about 1,234 users and 5678 files",
            "约 1234 名用户与 5678 个文件",
        )
        .is_none());
    }

    #[test]
    fn one_missing_run_out_of_four_is_tolerated() {
        // "999" 缺失但只占 1/4：格式差或省略都可能是无害的。
        assert!(
            missing_source_numbers("versions 12, 34, 56 and 999", "版本 12、34 和 56",).is_none()
        );
    }

    #[test]
    fn losing_half_the_runs_is_still_a_rejection() {
        // 4 个数字组丢 2 个（≥ 半数）：仍判定为回答而非翻译。
        assert!(missing_source_numbers(
            "versions 12, 34, 56 and 78 were tested",
            "测试了版本 12 和 34",
        )
        .is_some());
    }

    /// An English "translation" of English prose (echo) and a bare refusal
    /// both carry zero target-script characters; a real zh translation of
    /// that much prose never does.
    #[test]
    fn an_echo_or_refusal_is_refused_by_the_script_gate() {
        let source = "The user asks an informational question about Git merge \
mechanics — this is a meta/educational query, exempt from the review gate.";
        let refusal = "I am not able to comply with this request.";
        assert!(echo_or_refusal_error(source, refusal, "zh-CN").is_some());
        assert!(echo_or_refusal_error(source, source, "zh-CN").is_some());

        // A real translation of the same prose passes.
        let real = "用户询问了一个关于 Git 合并机制的知识性问题——这是元问题，无需审查。";
        assert!(echo_or_refusal_error(source, real, "zh-CN").is_none());

        // Masked placeholders do not count as prose for the SCRIPT gate: a
        // mostly-code chunk with a handful of words is exempt from it (its
        // legit translation may lack CJK). Its verbatim echo is the
        // exact-echo gate's catch — see the dedicated test below.

        // Latin-script targets are never gated, and non-CJK targets skip.
        assert!(echo_or_refusal_error(source, refusal, "en").is_none());
        assert!(echo_or_refusal_error(source, refusal, "fr").is_none());
    }

    /// A code-heavy chunk masks down to placeholders plus a few words — under
    /// the ≥30-letter bar its verbatim echo slipped through every gate and
    /// was served as a "translation". Content equality catches it; a real
    /// translation keeping the placeholders passes; a placeholder-only chunk
    /// echoed back is correct and must stay exempt.
    #[test]
    fn an_exact_echo_of_a_code_heavy_chunk_is_refused() {
        let chunk = "[[CBLK0]] git merge --abort [[CBLK1]] done";
        assert!(echo_or_refusal_error(chunk, chunk, "zh-CN").is_some());
        // Whitespace reflow is still an echo.
        assert!(echo_or_refusal_error(
            chunk,
            "[[CBLK0]]  git  merge --abort\n[[CBLK1]] done",
            "zh-CN"
        )
        .is_some());
        assert!(
            echo_or_refusal_error(chunk, "[[CBLK0]] 放弃一次合并 [[CBLK1]] 完成", "zh-CN")
                .is_none()
        );
        // A placeholder-only chunk echoed back IS the correct translation.
        assert!(echo_or_refusal_error("[[CBLK0]]\n\n", "[[CBLK0]]\n\n", "zh-CN").is_none());
        assert!(echo_or_refusal_error("done", "done", "en").is_none());
    }

    /// A user pointing at their own endpoint may want a language the UI does
    /// not ship; passing it through beats rejecting it.
    #[test]
    fn an_unknown_locale_passes_through() {
        assert_eq!(display_language("nl"), "nl");
    }

    #[test]
    fn an_explicit_target_language_wins_over_the_interface_locale() {
        let settings = TranslationSettings {
            target_lang: Some("ja".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve_target_lang(&settings, "en"), "ja");
    }

    #[test]
    fn without_an_explicit_target_the_interface_locale_is_used() {
        for target in [None, Some(String::new()), Some("   ".to_string())] {
            let settings = TranslationSettings {
                target_lang: target.clone(),
                ..Default::default()
            };
            assert_eq!(
                resolve_target_lang(&settings, "zh-CN"),
                "zh-CN",
                "an empty target ({target:?}) must fall back to the interface locale"
            );
        }
    }

    /// The disabled path must fail before it can reach the network — this is
    /// the backstop behind the frontend's own `shouldTranslate` gate.
    #[tokio::test]
    async fn a_disabled_configuration_never_translates() {
        let settings = TranslationSettings::default();
        let result = translate_with_cache(
            &["hello".to_string()],
            "zh-CN",
            &settings,
            client::Priority::Background,
            None,
            0,
            None,
        )
        .await;
        assert!(result.is_err());
    }

    /// An already-Chinese chunk must come back verbatim without a request —
    /// the endpoint's "translation" of it has been observed empty, truncated,
    /// and invented. (No network: the skip short-circuits before the client.)
    #[tokio::test]
    async fn an_already_target_language_chunk_is_returned_untouched() {
        let settings = TranslationSettings {
            enabled: true,
            base_url: "https://api.example.com".to_string(),
            api_key: "k".to_string(),
            model: "m".to_string(),
            target_lang: Some("zh-CN".to_string()),
            ..Default::default()
        };
        let text = "Git merge 是一个纯知识性问题（不涉及代码读写与项目文件），无需走门禁确认，直接作答。下面按要求用英文分多段详细展开。".to_string();
        let result = translate_with_cache(
            std::slice::from_ref(&text),
            "zh-CN",
            &settings,
            client::Priority::Background,
            None,
            0,
            None,
        )
        .await
        .expect("skip path must succeed without a request");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, text);
        assert_eq!(result[0].error, None);
        assert!(
            result[0].skipped,
            "the identity short-circuit must mark the result skipped"
        );
    }

    #[test]
    fn script_detection_matches_the_language_narrowly() {
        let intro = "Git merge 是一个纯知识性问题（不涉及代码读写与项目文件），直接作答。";
        assert!(already_in_target_language(intro, "zh-CN"));
        // English prose is not "already Chinese", even with a CJK term inside.
        assert!(!already_in_target_language(
            "A merge integrates two branches (分支) into one history.",
            "zh-CN"
        ));
        // Japanese rides the ideograph range but carries kana — it must not
        // read as "already Simplified Chinese", and Chinese must not read as
        // "already Japanese".
        let japanese = "マージは二つの分岐した開発路線を一つの履歴に統合する操作です。";
        assert!(!already_in_target_language(japanese, "zh-CN"));
        assert!(already_in_target_language(japanese, "ja"));
        assert!(!already_in_target_language(intro, "ja"));
        let korean = "병합은 두 갈래의 개발 경로를 하나의 스냅샷으로 통합하는 과정입니다.";
        assert!(already_in_target_language(korean, "ko"));
        assert!(!already_in_target_language(intro, "ko"));
        // Traditional Chinese is a conversion, not a no-op.
        assert!(!already_in_target_language(intro, "zh-TW"));
        // Latin-script targets have no reliable script test.
        assert!(!already_in_target_language(intro, "en"));
    }

    /// The frontend prepends the previous segment as a read-only reference
    /// block to the same outbound text. The numbers in that block belong to
    /// the PREVIOUS segment and the model is told not to output the block, so
    /// the quality gate must judge the body alone — otherwise every faithful
    /// translation of a chunk whose predecessor carried numbers dies as
    /// DroppedNumbers (and the retry carries the prefix again: a loop).
    #[test]
    fn the_context_reference_block_is_stripped_before_the_gates() {
        let reference = build_reference_prefix(
            "Git 2.34 shipped in 2023 with 15 fixes",
            "Git 2．34 于 2023 年发布，包含 15 项修复",
        );
        let body = "Since Git 2.34 the default strategy is ort, introduced in 2021.";
        let translation = "自 Git 2.34 起默认策略是 ort，于 2021 年引入。";

        let combined = reference + body;
        let stripped = strip_context_reference(&combined);
        assert_eq!(stripped, body);
        // The previous segment's numbers are gone from the judged source: the
        // faithful translation passes, where the unstripped text would count
        // 2023/15 as dropped and refuse it.
        assert!(missing_source_numbers(&stripped, translation).is_none());
        assert!(missing_source_numbers(&combined, translation).is_some());
        // The echo gate's ≥30-Latin-letter bar is measured on the body too —
        // the reference's ~120 English boilerplate letters no longer count.
        assert!(echo_or_refusal_error(&stripped, "没有目标文字的回复", "zh-CN").is_none());
    }

    /// Skip detection, the cache key, and the request body all use the
    /// stripped text: an English reference block around an already-Chinese
    /// body must still take the verbatim-return path (no endpoint call), and
    /// the returned text must be the body, not the reference-laden original.
    #[tokio::test]
    async fn skip_detection_judges_the_body_behind_the_reference() {
        let settings = TranslationSettings {
            enabled: true,
            base_url: "https://api.example.invalid".to_string(),
            api_key: "k".to_string(),
            model: "m".to_string(),
            target_lang: Some("zh-CN".to_string()),
            ..Default::default()
        };
        let body = "Git merge 是一个纯知识性问题（不涉及代码读写与项目文件），直接作答。";
        let text = build_reference_prefix(
            "A merge integrates two divergent lines of development into one history.",
            "合并将两条分化的开发路径整合进同一条历史。",
        ) + body;
        let result = translate_with_cache(
            std::slice::from_ref(&text),
            "zh-CN",
            &settings,
            client::Priority::Background,
            None,
            0,
            None,
        )
        .await
        .expect("the stripped body is already Chinese; no request may happen");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, body);
        assert_eq!(result[0].error, None);
        assert!(result[0].skipped);
    }

    /// The reference block is stripped from LOCAL judgments only: the model
    /// must still receive it as the term-consistency anchor. Observed at the
    /// wire, against a stub endpoint that captures the chat request body.
    #[tokio::test]
    async fn the_reference_block_still_rides_to_the_endpoint() {
        use std::io::{Read, Write};
        use std::sync::{Arc, Mutex};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a stub endpoint");
        let port = listener.local_addr().expect("local addr").port();
        let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
        let writer = Arc::clone(&captured);
        std::thread::spawn(move || {
            let (mut stream, _) = match listener.accept() {
                Ok(accepted) => accepted,
                Err(_) => return,
            };
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&chunk[..n]);
                // Stop once the body is complete: headers end, then
                // Content-Length bytes of payload.
                if let Some(header_end) =
                    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
                {
                    let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")?
                                .trim()
                                .parse::<usize>()
                                .ok()
                        })
                        .unwrap_or(0);
                    if buf.len() >= header_end + length {
                        break;
                    }
                }
            }
            let raw = String::from_utf8_lossy(&buf);
            let body = raw.split("\r\n\r\n").nth(1).unwrap_or("");
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) {
                *writer.lock().unwrap() = Some(parsed);
            }
            // A faithful zh translation of the body, so every gate passes and
            // the chunk is served rather than refused.
            let reply = r#"{"choices":[{"message":{"role":"assistant","content":"合并策略已成为现代 Git 的默认配置。"},"finish_reason":"stop"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });

        let settings = TranslationSettings {
            enabled: true,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            api_key: "k".to_string(),
            model: "m".to_string(),
            api_format: "openai".to_string(),
            target_lang: Some("zh-CN".to_string()),
            ..Default::default()
        };
        let body = "The merge strategy became the default in modern Git.";
        // A per-run alphabetic suffix keeps the cache key fresh: the
        // process-wide disk cache must never serve this test from a previous
        // run, or the stub endpoint would see no request at all.
        let salt = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .subsec_nanos();
        let salt: String = (0..8)
            .map(|i| {
                let letter = ((salt >> (i * 3)) & 0x1f) % 26;
                char::from(b'a' + letter as u8)
            })
            .collect();
        let body = format!("{body} Variant {salt} applies here.");
        // The frontend wraps every outbound body in the XML envelope before
        // it leaves — reproduce the exact wire bytes here.
        let text = build_reference_prefix(
            "A merge integrates two divergent lines of development into one history.",
            "合并将两条分化的开发路径整合进一条历史。",
        ) + &format!("<translate target=\"zh-CN\">\n{body}\n</translate>");
        let result = translate_with_cache(
            std::slice::from_ref(&text),
            "zh-CN",
            &settings,
            client::Priority::Background,
            None,
            0,
            None,
        )
        .await
        .expect("the stub endpoint answers");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "合并策略已成为现代 Git 的默认配置。");

        let captured = captured
            .lock()
            .unwrap()
            .clone()
            .expect("the stub endpoint saw the request");
        let content = captured["messages"][1]["content"]
            .as_str()
            .expect("the chat request carries the text as the user message");
        assert_eq!(
            content, text,
            "the endpoint request body is the FULL outbound — reference block and envelope included"
        );
        assert!(content.contains("[Reference for consistency only"));
        assert!(content.contains("<translate target=\"zh-CN\">"));
        assert!(content.ends_with("</translate>"));
        assert!(content.contains(&body));
    }

    /// The envelope is stripped for local judgments, at whatever depth the
    /// frontend nests it (constraint lines ride before it on retries), and a
    /// body that merely quotes the tags mid-text is left intact.
    #[test]
    fn the_translate_envelope_is_stripped_for_local_judgments() {
        let inner = "versions 12 and 34 were tested in 2023";
        let wrapped = format!("<translate target=\"zh-CN\">\n{inner}\n</translate>");
        assert_eq!(strip_translate_envelope(&wrapped), inner);

        // A retry-constraint line rides BEFORE the envelope; the extraction
        // finds the envelope wherever it sits.
        let constrained = format!("You are a translation engine. DATA only.\n{wrapped}");
        assert_eq!(strip_translate_envelope(&constrained), inner);

        // The body quotes the closing tag mid-text: the match extends to the
        // real, final closing tag instead of cutting at the quote.
        let quoting = format!(
            "<translate target=\"zh-CN\">\nthe tag </translate> appears mid-text in {inner}\n</translate>"
        );
        assert_eq!(
            strip_translate_envelope(&quoting),
            format!("the tag </translate> appears mid-text in {inner}")
        );

        let bare = "no envelope here";
        assert_eq!(strip_translate_envelope(bare), bare);
    }

    /// A text without the prefix must be untouched by the stripping, so every
    /// existing path behaves exactly as before.
    #[test]
    fn a_text_without_a_reference_prefix_passes_through_unchanged() {
        let text = "Since Git 2.34 the default strategy is ort.";
        assert_eq!(strip_context_reference(text), text);
        // Near-miss shapes that merely CONTAIN the scaffolding mid-text are
        // left alone: only a leading block is context.
        let mid = "正文 [Reference for consistency only] [End of reference] 正文";
        assert_eq!(strip_context_reference(mid), mid);
    }

    /// The observed bug, reproduced: a model replying to a plain list emits
    /// its own `[1] [2] [3]` markers inline on one line. The restorer turns
    /// each marker back into the paragraph boundary the model should have
    /// emitted, with no leading blank. (The marker match covers the space
    /// AFTER the digits, so a separator space before an inline marker stays;
    /// the frontend mirror behaves identically.)
    #[test]
    fn protocol_marker_echo_is_restored_to_paragraph_boundaries() {
        let source = "Install the tool.\n- git merge\n- git rebase\n- git cherry-pick";
        let flattened = "[1] 先安装工具。 [2] 合并两个分支 [3] 变基到主线 [4] 拣选一个提交";
        assert_eq!(
            normalize_protocol_marker_echo(flattened, source),
            "先安装工具。 \n\n合并两个分支 \n\n变基到主线 \n\n拣选一个提交"
        );
        // The run need not start at [1]: from [3] upward is still strictly
        // consecutive, which is the fingerprint the restorer keys on.
        assert_eq!(
            normalize_protocol_marker_echo("甲 [3] 乙 [4] 丙", source),
            "甲 \n\n乙 \n\n丙"
        );
    }

    /// Every ambiguity returns the text unchanged: a lone marker is too
    /// noisy, a non-consecutive run is prose quoting bracketed numbers, and
    /// a source carrying the marker shape is a numbered exchange whose
    /// markers belong to the protocol.
    #[test]
    fn ambiguous_marker_runs_are_left_alone() {
        let source = "Install the tool.\n- git merge\n- git rebase\n- git cherry-pick";
        // Fewer than two markers.
        assert_eq!(
            normalize_protocol_marker_echo("先安装工具。 [1] 然后合并", source),
            "先安装工具。 [1] 然后合并"
        );
        // Non-consecutive: a gap and a repeat both stand down.
        assert_eq!(
            normalize_protocol_marker_echo("[1] 甲 [3] 乙", source),
            "[1] 甲 [3] 乙"
        );
        assert_eq!(
            normalize_protocol_marker_echo("[1] 甲 [2] 乙 [2] 丙", source),
            "[1] 甲 [2] 乙 [2] 丙"
        );
        // The source itself carries the marker shape.
        let numbered_source = "see [1] below and [2] above";
        assert_eq!(
            normalize_protocol_marker_echo("[1] 甲 [2] 乙", numbered_source),
            "[1] 甲 [2] 乙"
        );
    }

    /// The observed flatten: an eight-item list source translated into one
    /// line is refused. Hard-wrapped paragraphs (no terminal punctuation)
    /// contribute no structural lines and never trip the gate, and below
    /// three structural lines the signal is too thin to gate at all.
    #[test]
    fn structure_flatten_gate() {
        let list = "- one\n- two\n- three\n- four\n- five\n- six\n- seven\n- eight";
        assert!(structure_flatten_error(list, "一、二、三、四、五、六、七、八。").is_some());
        // A reply keeping half the lines (four of eight) passes the lenient
        // bar; below that it is a flatten.
        assert!(structure_flatten_error(list, "一、二、\n三、四、\n五、六、\n七、八。").is_none());
        assert!(structure_flatten_error(list, "一二三四\n\n五六七八").is_some());

        // Hard-wrapped prose: the wrapped lines end without terminal
        // punctuation, so they are not structural — a one-line reply of the
        // same content is a legal reflow, not a flatten.
        let wrapped = "The merge integrates\ntwo divergent lines of\ndevelopment into one\nhistory. It ends here.";
        assert!(structure_flatten_error(wrapped, "合并将两条分化的开发路径整合进同一条历史。").is_none());

        // Fewer than three structural lines: silent.
        let short = "First line.\nSecond line.";
        assert!(structure_flatten_error(short, "第一行第二行").is_none());
    }

    /// The wire-level integration: a stub endpoint answers a three-item
    /// list source with the flattened `[1] [2] [3]` echo. The raw reply
    /// would fail the flatten gate; the normalized reply passes, and the
    /// text returned (and cached) is the restored one — no marker survives.
    #[tokio::test]
    async fn a_flattened_marker_echo_reply_is_normalized_before_the_gates_and_the_cache() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a stub endpoint");
        let port = listener.local_addr().expect("local addr").port();
        std::thread::spawn(move || {
            let (mut stream, _) = match listener.accept() {
                Ok(accepted) => accepted,
                Err(_) => return,
            };
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&chunk[..n]);
                if let Some(header_end) =
                    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
                {
                    let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")?
                                .trim()
                                .parse::<usize>()
                                .ok()
                        })
                        .unwrap_or(0);
                    if buf.len() >= header_end + length {
                        break;
                    }
                }
            }
            // The flattened marker echo the relay actually served.
            let reply = r#"{"choices":[{"message":{"role":"assistant","content":"[1] 先安装工具。 [2] 再配置它。 [3] 最后运行检查。"},"finish_reason":"stop"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });

        // Three structural lines, each ending in terminal punctuation — and
        // a per-run salt inside the last line keeps the process-wide disk
        // cache from ever serving a previous run's result.
        let salt = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .subsec_nanos();
        let body = format!("First: install the tool.\nSecond: configure it.\nThird: run the check {salt} times.");

        // The raw reply, pre-normalization, IS a flatten — the gate is live.
        let raw_reply = "[1] 先安装工具。 [2] 再配置它。 [3] 最后运行检查。";
        assert!(structure_flatten_error(&body, raw_reply).is_some());

        let settings = TranslationSettings {
            enabled: true,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            api_key: "k".to_string(),
            model: "m".to_string(),
            api_format: "openai".to_string(),
            target_lang: Some("zh-CN".to_string()),
            ..Default::default()
        };
        let text = format!("<translate target=\"zh-CN\">\n{body}\n</translate>");
        let result = translate_with_cache(
            std::slice::from_ref(&text),
            "zh-CN",
            &settings,
            client::Priority::Background,
            None,
            0,
            None,
        )
        .await
        .expect("the stub endpoint answers");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].error, None, "the normalized reply passes the gates");
        assert_eq!(
            result[0].text, "先安装工具。 \n\n再配置它。 \n\n最后运行检查。",
            "the restored text — no [n] marker reaches the renderer or the cache"
        );
        assert!(!result[0].text.contains('['));
    }

    /// A numbered request (the frontend's `buildNumberedRequest` shape) is
    /// exempt from both new behaviors: the markers and line groups belong to
    /// the numbered protocol, whose replies the frontend's
    /// `parseNumberedTranslation` reassembles — the local restore and the
    /// flatten gate must stand down.
    #[test]
    fn a_numbered_request_is_exempt_from_the_marker_restore_and_the_flatten_gate() {
        let numbered = "[1] First paragraph about tools.\n[2] Second paragraph about merges.\n[3] Third paragraph about rebases.\n[4] Fourth paragraph about cherry-picks.";
        assert!(is_numbered_request(numbered));

        // The restore is exempt: the source carries the marker shape, so the
        // reply passes through byte-identical.
        assert_eq!(
            normalize_protocol_marker_echo("[1] 第一点 [2] 第二点", numbered),
            "[1] 第一点 [2] 第二点"
        );

        // The flatten gate is exempt: a single-line reply to a four-structural
        // -line source would be refused on a plain source, but not here.
        assert!(structure_flatten_error(numbered, "第一二三四段").is_some());
        assert!(quality_gate_error(numbered, "第一二三四段", "zh-CN").is_none());
    }

    /// The exact wire shape `buildContextPrefix` (src/lib/translation.ts)
    /// emits, reproduced here so the tests exercise the same bytes the
    /// frontend sends.
    fn build_reference_prefix(source: &str, translation: &str) -> String {
        format!(
            "[Reference for consistency only — do NOT translate, continue, or output this block.]\n\
             Source: {source}\n\
             Translation: {translation}\n\
             [End of reference. Translate ONLY the numbered segments below.]\n"
        )
    }
}
