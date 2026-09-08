use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::acp::types::PromptInputBlock;
use crate::models::agent::{is_valid_custom_agent_id, BUILTIN_AGENT_TYPES};

// Reserved at every Codeg prompt ingress before any routing frame is appended.
// The parser may therefore treat a structurally valid RS-bounded frame as
// Codeg-authored without deleting an API caller's control-character text.
const ROUTE_FRAME_SEPARATOR: char = '\u{001e}';
const ROUTE_FRAME_KIND: &str = "codeg_internal_agent_routes";
/// Opening bytes of the frame's descriptor line — the anchor used to find a
/// frame in a transcript that no longer has the separators around it (see
/// [`find_unseparated_frame`]). `kind` is the descriptor's first field, so this
/// is a prefix of every rendered body; a field reorder that broke that is what
/// `the_body_anchor_is_what_the_renderer_starts_with` guards.
const ROUTE_FRAME_BODY_PREFIX: &str = "{\"kind\":\"codeg_internal_agent_routes\"";
const ROUTE_FRAME_VERSION: u8 = 3;
const MAX_ROUTE_FRAME_BYTES: usize = 16 * 1024;
/// Distinct agents one frame may route, applied AFTER deduplication.
const MAX_AGENT_ROUTES: usize = 16;
const MAX_AGENT_REFERENCE_OCCURRENCES: usize = 256;

/// One routed agent inside the frame — nothing but the wire slug.
///
/// The frame is derived from the prompt's own visible `codeg://agent/...`
/// links, so there is no out-of-band claim to carry or verify. Serializing as
/// `{"agentType":"..."}` is load-bearing: the frame's bytes are its identity
/// (see [`render_internal_agent_routes`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentRoute {
    agent_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InternalAgentRoutes {
    kind: String,
    version: u8,
    nonce: String,
    #[serde(skip)]
    routes: Vec<AgentRoute>,
}

fn agent_reference_regex() -> &'static Regex {
    // Agent wire slugs are built-ins or `custom:<slug>`, where the slug is
    // limited to ASCII alphanumeric, `-`, `_`, and `.`. Requiring a visible
    // label keeps this in lockstep with the frontend reference tokenizer.
    static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\[(?:\\.|[^\]\\\r\n])+\]\(codeg://agent/([A-Za-z0-9][A-Za-z0-9._:-]{0,127})\)")
            .expect("agent reference regex is valid")
    });
    &PATTERN
}

fn visible_agent_reference_types(blocks: &[PromptInputBlock]) -> Vec<String> {
    let mut references = Vec::new();
    for block in blocks {
        let PromptInputBlock::Text { text } = block else {
            continue;
        };
        for capture in agent_reference_regex().captures_iter(text) {
            if references.len() >= MAX_AGENT_REFERENCE_OCCURRENCES {
                return references;
            }
            if let Some(agent_type) = capture.get(1) {
                references.push(agent_type.as_str().to_string());
            }
        }
    }
    references
}

/// Append prompt-local routing for every agent the user visibly linked, when
/// this connection actually received the codeg-mcp companion's delegation
/// group.
///
/// The prompt IS the input: a visible `[label](codeg://agent/<type>)` link is
/// the whole criterion. That link is text the agent reads either way, and the
/// frame grants nothing — it only names the channel a delegation should take —
/// so there is deliberately no out-of-band claim to authenticate. Deriving from
/// the text instead means copy-paste, draft restore, queue editing and a
/// remounted composer all behave identically, because they all preserve the
/// link.
///
/// No per-agent special-casing lives here either. `delegation_enabled` is the
/// single launch-time fact this needs: the connection exposed the
/// `delegate_to_agent` tool group, which already implies the agent forwards MCP
/// over the wire (`AcpAgentMeta::supports_mcp` + `agent_delivers_wire_mcp` —
/// that pair is what gates the injection). Whether the named agent is installed
/// is NOT checked here: answering that costs a filesystem sweep per launch, and
/// the delegation spawner is the hard gate for it anyway. A mention that cannot
/// be served comes back as a `delegate_to_agent` error instead.
///
/// The original blocks remain untouched until the connection loop, so
/// broadcasts, previews, and optimistic user messages never contain this
/// internal block.
pub(crate) fn append_agent_routes(blocks: &mut Vec<PromptInputBlock>, delegation_enabled: bool) {
    if !delegation_enabled {
        return;
    }

    let mut seen = BTreeSet::new();
    let routes: Vec<AgentRoute> = visible_agent_reference_types(blocks)
        .into_iter()
        .filter(|agent_type| valid_agent_wire_syntax(agent_type))
        // Dedup first, then bound: a prompt repeating one agent 200 times still
        // routes it, and the cap only bites on distinct agents.
        .filter(|agent_type| seen.insert(agent_type.clone()))
        .take(MAX_AGENT_ROUTES)
        .map(|agent_type| AgentRoute { agent_type })
        .collect();
    if routes.is_empty() {
        return;
    }

    let frame = InternalAgentRoutes {
        kind: ROUTE_FRAME_KIND.to_string(),
        version: ROUTE_FRAME_VERSION,
        nonce: uuid::Uuid::new_v4().to_string(),
        routes,
    };
    blocks.push(PromptInputBlock::Text {
        text: render_internal_agent_routes(&frame),
    });
}

/// The frame's exact bytes ARE its identity: `parse_internal_agent_routes`
/// only accepts a candidate it can re-render byte-for-byte, which is what lets
/// the parsers strip a Codeg-authored frame without ever deleting look-alike
/// user prose.
///
/// CONSEQUENCE: editing this string orphans every frame already written into an
/// agent's on-disk transcript — they stop round-tripping and become visible
/// history. Any wording change must therefore bump [`ROUTE_FRAME_VERSION`];
/// `route_frame_wording_is_pinned_to_its_version` guards that.
///
/// No compatibility renderer exists yet: `parse_internal_agent_routes` accepts
/// the current version only, so a bump deliberately abandons the frames already
/// on disk. That is a pre-release call — there is no shipped history worth
/// preserving. Once there is, a wording change also has to keep the previous
/// version's renderer alive for PARSING, and dispatch on the candidate's own
/// `version` when round-tripping, so old transcripts keep cleaning up.
///
/// The instruction is narrow on purpose. Agents already discover
/// `delegate_to_agent` from the companion's tool schema; what they do wrong is
/// reach for their own sub-agent mechanism instead. This frame only binds the
/// channel — it does not tell the agent whether to delegate at all.
fn render_internal_agent_routes(frame: &InternalAgentRoutes) -> String {
    format!(
        "{ROUTE_FRAME_SEPARATOR}{}{ROUTE_FRAME_SEPARATOR}",
        render_internal_agent_routes_body(frame)
    )
}

/// The frame WITHOUT its separators: exactly three newline-terminated lines.
///
/// Split out because the separators are the one part of the frame an agent may
/// not give back. They are reserved and scrubbed at ingress, but that only
/// guarantees codeg never *sends* a stray one — it says nothing about what the
/// agent writes into its own store. Antigravity's ACP server, for one, joins
/// the prompt's text blocks with a space and replaces each separator with one,
/// so its trajectory holds this body and no separators at all.
///
/// Everything the identity argument needs is in here: the nonce, the routes,
/// and the full instruction wording. See [`find_unseparated_frame`].
fn render_internal_agent_routes_body(frame: &InternalAgentRoutes) -> String {
    let descriptor = serde_json::to_string(frame).expect("internal route frame is serializable");
    let routes = serde_json::to_string(&frame.routes).expect("agent routes are serializable");
    format!(
        "{descriptor}\n\
Codeg composer routing metadata (authoritative): {routes}\n\
When you delegate any of this work, route it through the `delegate_to_agent` \
tool from the `codeg-mcp` server with `agent_type` taken from the matching \
route: do not substitute your own native sub-agent, task, or spawn mechanism, \
and do not route it through any other delegation tool.\n"
    )
}

fn valid_agent_wire_syntax(agent_type: &str) -> bool {
    BUILTIN_AGENT_TYPES
        .iter()
        .any(|candidate| candidate.as_wire() == agent_type)
        || agent_type
            .strip_prefix(crate::models::agent::CUSTOM_AGENT_WIRE_PREFIX)
            .is_some_and(is_valid_custom_agent_id)
}

fn parse_internal_agent_routes(candidate: &str) -> Option<InternalAgentRoutes> {
    if !candidate.starts_with(ROUTE_FRAME_SEPARATOR) || !candidate.ends_with(ROUTE_FRAME_SEPARATOR)
    {
        return None;
    }
    parse_internal_agent_routes_body(
        candidate
            .strip_prefix(ROUTE_FRAME_SEPARATOR)?
            .strip_suffix(ROUTE_FRAME_SEPARATOR)?,
    )
}

/// Validate a candidate against [`render_internal_agent_routes_body`].
///
/// Shared by both finders so the separated and unseparated forms can never
/// drift into accepting different things.
fn parse_internal_agent_routes_body(candidate: &str) -> Option<InternalAgentRoutes> {
    if candidate.len() > MAX_ROUTE_FRAME_BYTES {
        return None;
    }
    let (descriptor, remainder) = candidate.split_once('\n')?;
    let mut frame: InternalAgentRoutes = serde_json::from_str(descriptor).ok()?;
    let routes_json = remainder
        .lines()
        .next()?
        .strip_prefix("Codeg composer routing metadata (authoritative): ")?;
    frame.routes = serde_json::from_str(routes_json).ok()?;
    if frame.kind != ROUTE_FRAME_KIND
        || frame.version != ROUTE_FRAME_VERSION
        || uuid::Uuid::parse_str(&frame.nonce).is_err()
        || frame.routes.is_empty()
        || frame.routes.len() > MAX_AGENT_ROUTES
        || frame
            .routes
            .iter()
            .any(|route| !valid_agent_wire_syntax(&route.agent_type))
        || render_internal_agent_routes_body(&frame) != candidate
    {
        return None;
    }
    Some(frame)
}

/// The next frame at or after `from`, in whichever form the agent stored it.
///
/// Both forms are searched and the EARLIER start wins. That ordering is what
/// keeps the separators inside the removed span whenever they survived: a
/// separated frame's own body is also a valid unseparated match one byte later,
/// so the separated hit is always the earlier one and the stray control
/// characters go with it.
fn find_internal_agent_routes(input: &str, from: usize) -> Option<(usize, usize)> {
    match (
        find_separated_frame(input, from),
        find_unseparated_frame(input, from),
    ) {
        (Some(separated), Some(bare)) if bare.0 < separated.0 => Some(bare),
        (Some(separated), _) => Some(separated),
        (None, bare) => bare,
    }
}

/// A frame the agent gave back exactly as it was sent, separators and all.
fn find_separated_frame(input: &str, from: usize) -> Option<(usize, usize)> {
    let mut search_from = from;
    while search_from < input.len() {
        let start = search_from + input[search_from..].find(ROUTE_FRAME_SEPARATOR)?;
        let after_start = start + ROUTE_FRAME_SEPARATOR.len_utf8();
        let mut bounded_end = (after_start + MAX_ROUTE_FRAME_BYTES).min(input.len());
        while !input.is_char_boundary(bounded_end) {
            bounded_end -= 1;
        }
        if let Some(close_rel) = input[after_start..bounded_end].find(ROUTE_FRAME_SEPARATOR) {
            let end = after_start + close_rel + ROUTE_FRAME_SEPARATOR.len_utf8();
            if parse_internal_agent_routes(&input[start..end]).is_some() {
                return Some((start, end));
            }
        }
        // Preserve an invalid separator and retry at the next byte. This is
        // what prevents an unmatched user opening marker from consuming a
        // later genuine frame and all prose between them.
        search_from = after_start;
    }
    None
}

/// A frame whose separators the agent dropped or rewrote, matched on its body.
///
/// WHY THIS EXISTS. Separators are the cheapest possible proof that a frame is
/// Codeg-authored, but they only reach a transcript intact if the agent stores
/// the prompt verbatim — and one does not. Antigravity's ACP server joins the
/// prompt's text blocks with a space and replaces each separator with one, so a
/// routed turn lands in `conversations/<id>.db` as
/// `…（使用codeg-mcp工具）  {"kind":…delegation tool.\n ` — the same body, both
/// separators now spaces. The separated pass cannot see that, and the whole
/// frame renders inside the user's bubble when the session is reopened.
///
/// The body's own NEWLINES do survive that rewrite, which is load-bearing:
/// [`unseparated_frame_end`] delimits a candidate by counting exactly three of
/// them. Only the separators were observed to be rewritten — an agent that
/// flattened newlines too would need a different anchor, not a wider one.
///
/// What stands in for the separators as evidence is the body itself: three
/// lines that must re-render byte-for-byte from their own parsed contents,
/// nonce and full instruction wording included. Prose that reproduces all of
/// that was copied out of a frame, not typed. The residue an agent leaves in
/// place of the separators is left to the callers' existing trims — it is
/// whitespace, and the frame is always the last thing in the record.
fn find_unseparated_frame(input: &str, from: usize) -> Option<(usize, usize)> {
    let mut search_from = from;
    while search_from < input.len() {
        let start = search_from + input[search_from..].find(ROUTE_FRAME_BODY_PREFIX)?;
        if let Some(end) = unseparated_frame_end(input, start) {
            if parse_internal_agent_routes_body(&input[start..end]).is_some() {
                return Some((start, end));
            }
        }
        // Same rule as the separated scan: a look-alike opening must not hide a
        // real frame further along.
        search_from = start + ROUTE_FRAME_BODY_PREFIX.len();
    }
    None
}

/// End of an unseparated candidate: the body is exactly three newline-terminated
/// lines, so there is nothing to search for — the third newline IS the close.
fn unseparated_frame_end(input: &str, start: usize) -> Option<usize> {
    let mut bounded_end = (start + MAX_ROUTE_FRAME_BYTES).min(input.len());
    while !input.is_char_boundary(bounded_end) {
        bounded_end -= 1;
    }
    let mut cursor = start;
    for _ in 0..3 {
        cursor += input.get(cursor..bounded_end)?.find('\n')? + 1;
    }
    Some(cursor)
}

/// Remove only complete frames that can be parsed and reproduced byte-for-byte.
/// Codeg scrubs the reserved separator from every prompt at ingress, so a frame
/// that still round-trips can only have been appended at the final boundary —
/// true for any agent's transcript, not just the one this was first written for.
///
/// A frame the agent stored without its separators is removed too, on the
/// strength of its body alone (see [`find_unseparated_frame`]); only the body
/// goes, and the whitespace an agent put where the separators were is left to
/// the caller's trim.
pub(crate) fn strip_internal_agent_routes(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some((start, end)) = find_internal_agent_routes(input, cursor) {
        let prefix = &input[cursor..start];
        let prefix_end = if start > cursor
            && input.as_bytes()[start - 1] == b'\n'
            && !prefix.contains(ROUTE_FRAME_SEPARATOR)
        {
            start - 1
        } else {
            start
        };
        output.push_str(&input[cursor..prefix_end]);
        cursor = end;
        if prefix_end == start
            && cursor < input.len()
            && input.as_bytes().get(cursor) == Some(&b'\n')
        {
            cursor += 1;
        }
    }
    output.push_str(&input[cursor..]);
    output
}

pub(crate) fn contains_internal_agent_routes(input: &str) -> bool {
    find_internal_agent_routes(input, 0).is_some()
}

/// Cut a string at a frame marker left behind by TRUNCATION, for the title path.
///
/// [`strip_internal_agent_routes`] only removes a byte-exact *complete* frame,
/// which is right for turn text but not for a title: parsers derive one by
/// joining a prompt's text blocks and capping the result (80 chars in
/// `acp_native`, 100 in [`title_from_user_text`]), and that cap lands wherever it
/// lands. A short first prompt therefore truncates mid-frame, leaving an opening
/// marker and half a JSON descriptor with no close — which the strip pass skips,
/// putting internal route metadata in the sidebar. The frame's body is 453
/// bytes even for the shortest agent slug, well past either cap, so a title that
/// reaches a frame at all is always sliced inside one.
///
/// Two markers, because either one can be the first thing left of a frame.
/// Cutting at the SEPARATOR is sound because RS is reserved:
/// `strip_route_separator_from_prompt` removes it from every prompt field at
/// ingress, so a separator that survives into persisted history can only be one
/// Codeg appended itself. The descriptor prefix carries no such guarantee — it
/// is ordinary text a user could in principle type — but it is the only handle
/// left on the agents that rewrite the separators away (Antigravity), and the
/// blast radius of a false positive is one truncated sidebar title, against a
/// certainty of leaking route metadata into every routed conversation's title.
/// Turn text keeps demanding the full round-trip either way; the asymmetry is
/// the same one the separator cut has always had.
///
/// [`title_from_user_text`]: crate::parsers::title_from_user_text
pub(crate) fn cut_at_route_frame_marker(text: &mut String) {
    let Some(cut) = text
        .find(ROUTE_FRAME_SEPARATOR)
        .into_iter()
        .chain(text.find(ROUTE_FRAME_BODY_PREFIX))
        .chain(find_halved_body_prefix(text))
        .min()
    else {
        return;
    };
    text.truncate(cut);
    text.truncate(text.trim_end().len());
}

/// Start of a descriptor prefix the CAP itself cut in half.
///
/// The separator is one character and cannot be halved: either it is inside the
/// capped window or no frame content is. The descriptor prefix is 37 bytes and
/// very much can be, so a first prompt whose prose length falls in a 37-wide
/// band leaves `…prose  {"kind":"codeg_int...` — a fragment [`str::find`] cannot
/// see, and the leak this whole cut exists to stop.
///
/// Matched at the END of the string with a trailing run of `.` discarded,
/// because both title paths cap with [`truncate_str`], which marks the cut by
/// appending `...`.
///
/// A fragment shorter than `{"kind":"c` is deliberately left alone: what
/// survives at that length is a generic JSON opening rather than anything that
/// identifies codeg, and matching it would start trimming ordinary prose that
/// happens to end in `{`.
///
/// [`truncate_str`]: crate::parsers::truncate_str
fn find_halved_body_prefix(text: &str) -> Option<usize> {
    const SHORTEST_FRAGMENT: usize = "{\"kind\":\"c".len();
    let capped = text.trim_end_matches('.');
    // Longest fragment first, so the cut lands at the earliest byte. The full
    // marker is excluded — `find` already covers that case.
    (SHORTEST_FRAGMENT..ROUTE_FRAME_BODY_PREFIX.len())
        .rev()
        .find(|&length| capped.ends_with(&ROUTE_FRAME_BODY_PREFIX[..length]))
        .map(|length| capped.len() - length)
}

/// The record contains one or more valid internal frames and no user-visible
/// content. Used by the Codex parser to avoid phantom turns and promotion
/// coverage from adapters that persist ACP text blocks separately.
pub(crate) fn contains_only_internal_agent_routes(input: &str) -> bool {
    contains_internal_agent_routes(input) && strip_internal_agent_routes(input).trim().is_empty()
}

/// RS is reserved for the internal frame, so it is REMOVED from every prompt
/// at ingress — never used to reject the prompt.
///
/// The character is invisible and mostly arrives inside content the user did
/// not author (an attached file's bytes land in `Resource.text`; ASCII-delimited
/// data and some export formats carry RS legitimately). Rejecting made such a
/// message permanently unsendable with no way for the user to find the offending
/// byte, and it also let a model echo the frame back into a `delegate_to_agent`
/// task string and kill the child prompt. Stripping keeps the invariant that
/// motivated the check — a byte-exact frame inside a Codeg-authored turn can
/// only have been appended by [`append_agent_routes`] — and makes it stronger,
/// since no user-supplied RS reaches the wire at all.
///
/// That last claim is only true if EVERY field is covered, so the nominally
/// base64 ones (`Image.data`, `Resource.blob`) are scrubbed too. Nothing
/// validates them as base64 before they are forwarded to the agent, and both are
/// plain `String`s an API caller can fill freely; scrubbing is a no-op on
/// well-formed input, since RS is outside the base64 alphabet.
///
/// Must run before any conversation DB, broadcast, title, ledger, or
/// command-channel side effect, so the stored / broadcast / on-the-wire copies
/// of the message stay identical.
pub(crate) fn strip_route_separator_from_prompt(blocks: &mut [PromptInputBlock]) -> bool {
    fn scrub(value: &mut String) -> bool {
        if !value.contains(ROUTE_FRAME_SEPARATOR) {
            return false;
        }
        value.retain(|c| c != ROUTE_FRAME_SEPARATOR);
        true
    }
    fn scrub_opt(value: &mut Option<String>) -> bool {
        value.as_mut().is_some_and(scrub)
    }

    let mut stripped = false;
    for block in blocks.iter_mut() {
        // Every textual field, not just `text`: a sentinel uri or an attachment
        // name is echoed back into the prompt by the composer's serialization.
        match block {
            PromptInputBlock::Text { text } => stripped |= scrub(text),
            PromptInputBlock::Image {
                data,
                mime_type,
                uri,
            } => {
                stripped |= scrub(data);
                stripped |= scrub(mime_type);
                stripped |= scrub_opt(uri);
            }
            PromptInputBlock::Resource {
                uri,
                mime_type,
                text,
                blob,
            } => {
                stripped |= scrub(uri);
                stripped |= scrub_opt(mime_type);
                stripped |= scrub_opt(text);
                stripped |= scrub_opt(blob);
            }
            PromptInputBlock::ResourceLink {
                uri,
                name,
                mime_type,
                description,
            } => {
                stripped |= scrub(uri);
                stripped |= scrub(name);
                stripped |= scrub_opt(mime_type);
                stripped |= scrub_opt(description);
            }
        }
    }
    stripped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(block: &PromptInputBlock) -> &str {
        match block {
            PromptInputBlock::Text { text } => text,
            _ => panic!("expected text block"),
        }
    }

    #[test]
    fn wire_shape_retains_camel_case_agent_type() {
        let value = serde_json::to_value(AgentRoute {
            agent_type: "antigravity".into(),
        })
        .unwrap();
        assert_eq!(value, serde_json::json!({"agentType": "antigravity"}));
    }

    #[test]
    fn a_prompt_with_no_agent_link_is_left_exactly_as_it_came_in() {
        let mut blocks = vec![PromptInputBlock::Text {
            text: "just run the tests".into(),
        }];
        append_agent_routes(&mut blocks, true);
        assert_eq!(blocks.len(), 1, "no mention means no appended block at all");
        assert_eq!(text(&blocks[0]), "just run the tests");
    }

    #[test]
    fn every_mentioned_agent_gets_exactly_one_deduplicated_route() {
        let visible = "ask [@Antigravity](codeg://agent/antigravity) and \
[@Claude](codeg://agent/claude_code), then [@Antigravity](codeg://agent/antigravity) again";
        let mut blocks = vec![PromptInputBlock::Text {
            text: visible.into(),
        }];
        append_agent_routes(&mut blocks, true);
        assert_eq!(blocks.len(), 2);
        let routing = text(&blocks[1]);
        assert!(routing.contains(r#"[{"agentType":"antigravity"},{"agentType":"claude_code"}]"#));
        assert_eq!(routing.matches(r#"agentType":"antigravity"#).count(), 1);
    }

    #[test]
    fn any_visible_link_routes_regardless_of_how_the_badge_was_produced() {
        // The accepted trade-off of deriving from the text: a link pasted or
        // quoted from an earlier transcript routes exactly like one just picked
        // from the `@` panel. That is the POINT — the previous design
        // authenticated the badge, so re-sending a copied message silently
        // dropped the reminder while still showing the badge. The frame grants
        // nothing the link itself doesn't, so uniform behavior wins.
        let mut blocks = vec![PromptInputBlock::Text {
            text: "quoting an old turn: raw [@Antigravity](codeg://agent/antigravity) and \
[@Claude](codeg://agent/claude_code)"
                .into(),
        }];
        append_agent_routes(&mut blocks, true);
        assert_eq!(blocks.len(), 2);
        let routing = text(&blocks[1]);
        assert!(routing.contains(r#"{"agentType":"antigravity"}"#));
        assert!(routing.contains(r#"{"agentType":"claude_code"}"#));
    }

    #[test]
    fn a_closed_delegation_gate_is_the_only_check_on_the_parent_agent() {
        // A parent that never received codeg-mcp (OpenClaw's supports_mcp=false,
        // pi's wire exclusion) reaches this function with the gate closed. There
        // is no agent-type check here on purpose: the injection gate is the
        // single source of truth.
        let mut blocks = vec![PromptInputBlock::Text {
            text: "ask [@Antigravity](codeg://agent/antigravity)".into(),
        }];
        append_agent_routes(&mut blocks, false);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn route_frame_wording_is_pinned_to_its_version() {
        // The instruction text is part of the frame's byte-exact identity —
        // parsing only accepts a candidate the CURRENT renderer reproduces
        // verbatim — so a reworded frame stops being strippable and surfaces in
        // every transcript already written with the old wording. There is no
        // compat renderer: the version bump this test pins is a deliberate
        // marker for that break, not a fix for it.
        let mut blocks = vec![PromptInputBlock::Text {
            text: "ask [@Codex](codeg://agent/codex)".into(),
        }];
        append_agent_routes(&mut blocks, true);
        let routing = text(&blocks[1]);
        assert_eq!(ROUTE_FRAME_VERSION, 3);
        assert!(routing.contains(
            "do not substitute your own native sub-agent, task, or spawn mechanism, \
             and do not route it through any other delegation tool."
        ));
        assert!(
            !routing.contains("Codex native"),
            "the routing instruction must stay agent-neutral"
        );
        // The frame binds the CHANNEL, not the decision: agents already know the
        // tool exists and go around it. Commanding a delegation here made every
        // mention an imperative, which is a different feature.
        assert!(
            !routing.contains("immediately"),
            "the frame must not order a delegation, only constrain how one is routed"
        );
    }

    #[test]
    fn transcript_strip_keeps_only_user_prose() {
        let mut blocks = vec![PromptInputBlock::Text {
            text: "Please ask [@Antigravity](codeg://agent/antigravity) to review this".into(),
        }];
        append_agent_routes(&mut blocks, true);
        let joined = format!("{}\n{}", text(&blocks[0]), text(&blocks[1]));
        assert_eq!(
            strip_internal_agent_routes(&joined),
            "Please ask [@Antigravity](codeg://agent/antigravity) to review this"
        );
        assert_eq!(strip_internal_agent_routes(text(&blocks[1])), "");
    }

    #[test]
    fn invalid_rs_text_is_preserved_and_cannot_swallow_a_genuine_frame() {
        let mut blocks = vec![PromptInputBlock::Text {
            text: "ask [@Codex](codeg://agent/codex)".into(),
        }];
        append_agent_routes(&mut blocks, true);
        let forged_prefix = "user\u{001e}<codeg_internal_agent_routes version=\"2\">keep me\n";
        let joined = format!("{forged_prefix}{}", text(&blocks[1]));
        assert_eq!(strip_internal_agent_routes(&joined), forged_prefix);
    }

    #[test]
    fn invalid_rs_before_long_unicode_text_never_panics_or_loses_text() {
        let input = format!(
            "{ROUTE_FRAME_SEPARATOR}{}",
            "界".repeat(MAX_ROUTE_FRAME_BYTES / 3 + 2)
        );
        assert_eq!(strip_internal_agent_routes(&input), input);
    }

    #[test]
    fn unknown_and_uri_prefix_targets_are_ignored() {
        // A link whose slug is neither a built-in nor a valid `custom:` id is
        // dropped: naming it would only produce a `delegate_to_agent` call the
        // companion's enum rejects.
        let mut unknown = vec![PromptInputBlock::Text {
            text: "ask [@Old](codeg://agent/not-an-agent)".into(),
        }];
        append_agent_routes(&mut unknown, true);
        assert_eq!(unknown.len(), 1);

        // A bare uri is not a reference (the regex requires a visible label),
        // and `codex-other` must not be accepted as `codex` by prefix.
        let mut prefix = vec![PromptInputBlock::Text {
            text: "codeg://agent/codex [@Other](codeg://agent/codex-other)".into(),
        }];
        append_agent_routes(&mut prefix, true);
        assert_eq!(prefix.len(), 1);
    }

    #[test]
    fn empty_visible_label_is_not_a_routing_reference() {
        let mut blocks = vec![PromptInputBlock::Text {
            text: "[](codeg://agent/codex)".into(),
        }];
        append_agent_routes(&mut blocks, true);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn the_route_cap_counts_distinct_agents_and_keeps_the_frame_parseable() {
        let mut prompt = String::from("ask ");
        for index in 0..(MAX_AGENT_ROUTES + 4) {
            prompt.push_str(&format!("[@A{index}](codeg://agent/custom:agent-{index}) "));
        }
        let mut blocks = vec![PromptInputBlock::Text { text: prompt }];
        append_agent_routes(&mut blocks, true);
        assert_eq!(blocks.len(), 2);
        let routing = text(&blocks[1]);
        assert_eq!(routing.matches(r#""agentType""#).count(), MAX_AGENT_ROUTES);
        // A truncated frame must still round-trip, or the parsers would leave it
        // in the transcript.
        assert_eq!(strip_internal_agent_routes(routing), "");
    }

    #[test]
    fn prompt_ingress_scrubs_rs_across_every_textual_block_field() {
        let mut blocks = vec![
            PromptInputBlock::Text {
                text: "user\u{001e}text".into(),
            },
            PromptInputBlock::Image {
                // Nominally base64, but nothing validates that before the block
                // is forwarded to the agent, and an API caller fills it freely.
                data: "QUJ\u{001e}D".into(),
                mime_type: "image/\u{001e}png".into(),
                uri: Some("clipboard://a\u{001e}b".into()),
            },
            PromptInputBlock::Resource {
                uri: "file:///tmp/\u{001e}a".into(),
                mime_type: Some("text/\u{001e}plain".into()),
                text: Some("user\u{001e}resource".into()),
                blob: Some("SGVsb\u{001e}G8=".into()),
            },
            PromptInputBlock::ResourceLink {
                uri: "file:///tmp/\u{001e}b".into(),
                name: "re\u{001e}port".into(),
                mime_type: Some("text/\u{001e}csv".into()),
                description: Some("row\u{001e}sep".into()),
            },
        ];
        assert!(strip_route_separator_from_prompt(&mut blocks));
        assert!(
            !prompt_has_route_separator(&blocks),
            "no user-supplied separator may survive to the wire"
        );
        assert!(matches!(
            &blocks[0],
            PromptInputBlock::Text { text } if text == "usertext"
        ));
        assert!(matches!(
            &blocks[2],
            PromptInputBlock::Resource { text: Some(text), .. } if text == "userresource"
        ));

        // Clean prompts are reported untouched so the caller can stay quiet.
        let mut clean = vec![PromptInputBlock::Text {
            text: "ordinary prompt".into(),
        }];
        assert!(!strip_route_separator_from_prompt(&mut clean));
    }

    /// Test-only inverse of [`strip_route_separator_from_prompt`]: walks the
    /// serialized form so a field the scrub forgot fails the test, instead of
    /// asserting the scrub against its own field list.
    fn prompt_has_route_separator(blocks: &[PromptInputBlock]) -> bool {
        fn any_string(value: &serde_json::Value) -> bool {
            match value {
                serde_json::Value::String(text) => text.contains(ROUTE_FRAME_SEPARATOR),
                serde_json::Value::Array(items) => items.iter().any(any_string),
                serde_json::Value::Object(map) => map.values().any(any_string),
                _ => false,
            }
        }
        any_string(&serde_json::to_value(blocks).expect("prompt blocks are serializable"))
    }

    /// Exactly what Antigravity's ACP server writes into its trajectory: the
    /// prompt's text blocks joined with a space, and each separator replaced by
    /// one. Verified against a real `conversations/<id>.db`, where a routed turn
    /// reads `…（使用codeg-mcp工具）  {"kind":…delegation tool.\n ` — note the
    /// body's own newlines came through, which is why the scan can count them.
    fn as_antigravity_persists(blocks: &[PromptInputBlock]) -> String {
        blocks
            .iter()
            .map(|block| text(block).replace(ROUTE_FRAME_SEPARATOR, " "))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn a_frame_stored_without_its_separators_is_still_stripped() {
        let visible = "调用[@Claude Code](codeg://agent/claude_code) 看下这个文件夹干什么的";
        let mut blocks = vec![PromptInputBlock::Text {
            text: visible.into(),
        }];
        append_agent_routes(&mut blocks, true);
        let persisted = as_antigravity_persists(&blocks);
        assert!(
            !persisted.contains(ROUTE_FRAME_SEPARATOR),
            "the fixture must have LOST its separators, or it proves nothing"
        );

        assert!(contains_internal_agent_routes(&persisted));
        // Only the body goes; the spaces that replaced the separators are the
        // caller's trim to make (`parsers::sanitize_text`).
        assert_eq!(strip_internal_agent_routes(&persisted).trim_end(), visible);
    }

    #[test]
    fn a_separator_less_frame_still_has_to_round_trip_byte_for_byte() {
        let mut blocks = vec![PromptInputBlock::Text {
            text: "ask [@Codex](codeg://agent/codex)".into(),
        }];
        append_agent_routes(&mut blocks, true);
        let body = text(&blocks[1]).replace(ROUTE_FRAME_SEPARATOR, "");
        assert_eq!(strip_internal_agent_routes(&body), "");

        // One byte off anywhere in the wording, and it is prose again.
        let reworded = body.replace("delegation tool.", "delegation tool!");
        assert_eq!(strip_internal_agent_routes(&reworded), reworded);
        // A nonce that is not a UUID is not a nonce this renderer produced.
        let forged = body.replacen("\"nonce\":\"", "\"nonce\":\"x", 1);
        assert_eq!(strip_internal_agent_routes(&forged), forged);
        // Truncated mid-body there is no third line, so nothing matches — which
        // is why the title path cuts rather than strips.
        let truncated = &body[..body.len() - 20];
        assert_eq!(strip_internal_agent_routes(truncated), truncated);
    }

    #[test]
    fn the_body_anchor_is_what_the_renderer_starts_with() {
        let mut blocks = vec![PromptInputBlock::Text {
            text: "ask [@Codex](codeg://agent/codex)".into(),
        }];
        append_agent_routes(&mut blocks, true);
        assert!(
            text(&blocks[1])
                .starts_with(&format!("{ROUTE_FRAME_SEPARATOR}{ROUTE_FRAME_BODY_PREFIX}")),
            "reordering the descriptor's fields would silently unanchor the \
             separator-less scan"
        );
    }

    #[test]
    fn a_title_is_cut_at_whichever_frame_marker_comes_first() {
        // Separator intact: the cut lands on it, as it always has.
        let mut separated =
            String::from("hi \u{001e}{\"kind\":\"codeg_internal_agent_routes\",\"ve");
        cut_at_route_frame_marker(&mut separated);
        assert_eq!(separated, "hi");

        // Separator rewritten away: the descriptor's own opening is the only
        // handle left.
        let mut bare = String::from("hi  {\"kind\":\"codeg_internal_agent_routes\",\"ve");
        cut_at_route_frame_marker(&mut bare);
        assert_eq!(bare, "hi");

        let mut clean = String::from("just a normal title");
        cut_at_route_frame_marker(&mut clean);
        assert_eq!(clean, "just a normal title");
    }

    #[test]
    fn a_title_whose_cap_halved_the_marker_is_cut_as_well() {
        // The separator is one char and lands inside the window or not at all;
        // the 37-byte descriptor prefix can be sliced through, and `find` is
        // blind to the fragment that survives.
        let mut halved = String::from("hi  {\"kind\":\"codeg_int...");
        cut_at_route_frame_marker(&mut halved);
        assert_eq!(halved, "hi");

        // A parser that caps without an ellipsis leaves the same fragment bare.
        let mut bare = String::from("hi  {\"kind\":\"codeg");
        cut_at_route_frame_marker(&mut bare);
        assert_eq!(bare, "hi");

        // Below the floor it is generic punctuation, not route metadata, and
        // trimming it would eat ordinary prose.
        for prose in ["what does this do: {", "see the shape {\"kind\":\""] {
            let mut title = String::from(prose);
            cut_at_route_frame_marker(&mut title);
            assert_eq!(title, prose);
        }
    }

    #[test]
    fn user_authored_envelope_text_is_left_untouched() {
        let input = "hello <codeg_internal_agent_routes version=\"1\">\nuser text\n</codeg_internal_agent_routes>";
        assert_eq!(strip_internal_agent_routes(input), input);
        assert!(!contains_internal_agent_routes(input));
    }
}
