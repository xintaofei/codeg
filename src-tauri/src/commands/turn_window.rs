//! Turn-window slicing for conversation detail responses.
//!
//! The parsers always parse a session transcript in full (local parsing is
//! fast); what hurts web / remote-workspace mode is *transferring* every turn
//! of a long conversation on each detail fetch. These helpers slice the fully
//! parsed + fully post-processed turn list down to a requested window right
//! before serialization, plus compute the window metadata the frontend's
//! consistency protocol needs (offset/total, assistant count before the
//! window, a structural prefix fingerprint, and the max timestamp of the
//! uncovered prefix).
//!
//! Invariant shared with `get_folder_conversation_core`: every existing
//! post-processing pass (delegation-meta injection, auto-title, in-flight id
//! stamping) runs on the FULL turn list first; slicing is strictly a
//! serialization concern, so a windowed response is byte-identical to the
//! corresponding region of the full response.

use chrono::{DateTime, Utc};

use crate::models::{MessageTurn, TurnRole};

/// Server-side clamp for `tailTurns` / page `limit`.
pub const MAX_WINDOW_TURNS: usize = 500;

/// Upper bound on how far a window start walks backward looking for a
/// user-round boundary. Keeps a single degenerate round (one prompt followed
/// by hundreds of assistant/tool turns) from silently re-expanding a window
/// into a full-transcript transfer; when the cap is hit the boundary may fall
/// mid-round and the frontend renders a partial round.
pub const ROUND_ALIGN_CAP: usize = 200;

/// FNV-1a 64-bit offset basis / prime (public-domain constants).
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Requested window for a conversation-detail fetch. Mutually exclusive
/// selectors; validation lives in the command layer so both transports report
/// the same error.
#[derive(Debug, Clone, Copy)]
pub enum TurnWindowReq {
    /// Last `n` turns, start aligned backward to a user-round boundary.
    Tail(usize),
    /// Exact slice `turns[min(from, total)..]` — NEVER round-aligned, so the
    /// response offset matches the coordinate the client refreshed against
    /// (its prefix fingerprint would be incomparable otherwise).
    FromIndex(usize),
}

/// Window metadata attached to a sliced response.
#[derive(Debug, Clone)]
pub struct WindowMeta {
    pub offset: usize,
    pub total: usize,
    pub assistant_before: usize,
    /// Fingerprint of `turns[0..offset)` as a fixed-width 16-hex string.
    /// Serialized as a string because a raw u64 JSON number is rounded by
    /// JS `response.json()` past 2^53-1, which would systematically break
    /// the client's chained-seed comparison.
    pub prefix_hash: String,
    /// Max `timestamp` across `turns[0..offset)`; `None` when the window
    /// covers the whole list. Drives the frontend's overlay-retirement
    /// coverage check (strictly-greater comparison).
    pub uncovered_prefix_max_ts: Option<DateTime<Utc>>,
}

fn role_tag(role: &TurnRole) -> u8 {
    match role {
        TurnRole::User => 0,
        TurnRole::Assistant => 1,
        TurnRole::System => 2,
    }
}

fn fnv1a_extend(mut hash: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Chained structural fingerprint over `(role_tag, timestamp_millis)` per
/// turn. Deliberately id-free: parser turn ids are index-derived (`turn-N`),
/// so they cannot witness a prefix rewrite anyway, and the in-flight user
/// turn's id is legitimately rewritten to the broadcast id while a turn is
/// running — an id-bearing fingerprint would make every mid-stream viewer
/// batch look like a prefix rewrite once the turn completes and the parser id
/// returns. `(role, millis)` is invariant under both id rewrites and in-place
/// content backfills (background-task ack previews), while insertions,
/// deletions and shifts — the actual shapes of a compact/rewrite — still
/// change the chain.
///
/// Mirrored bit-for-bit by the TypeScript implementation (BigInt); a shared
/// test vector pins the two.
pub fn prefix_fingerprint(turns: &[MessageTurn]) -> String {
    let mut hash = FNV_OFFSET;
    for turn in turns {
        hash = fnv1a_extend(hash, &[role_tag(&turn.role)]);
        hash = fnv1a_extend(hash, &turn.timestamp.timestamp_millis().to_le_bytes());
    }
    format!("{hash:016x}")
}

/// Walk `start` backward until the window begins at a `User` turn (or index
/// 0), bounded by [`ROUND_ALIGN_CAP`]. Alignment only ever ADDS earlier turns.
fn round_align_backward(turns: &[MessageTurn], mut start: usize) -> usize {
    let mut steps = 0;
    while start > 0 && steps < ROUND_ALIGN_CAP && !matches!(turns[start].role, TurnRole::User) {
        start -= 1;
        steps += 1;
    }
    start
}

/// Resolve a window request against the full turn list, returning the slice
/// start (`offset`). `Tail` clamps to `1..=MAX_WINDOW_TURNS` then aligns
/// backward; `FromIndex` is exact (clamped to `total` only).
pub fn resolve_window_offset(turns: &[MessageTurn], req: TurnWindowReq) -> usize {
    match req {
        TurnWindowReq::Tail(n) => {
            let n = n.clamp(1, MAX_WINDOW_TURNS);
            round_align_backward(turns, turns.len().saturating_sub(n))
        }
        TurnWindowReq::FromIndex(from) => from.min(turns.len()),
    }
}

/// Resolve a history-page request: returns `(start, end)` for
/// `turns[start..end)` where `end = min(before_index, total)` and `start`
/// walks back `limit` turns then aligns to a user-round boundary.
pub fn resolve_page_bounds(
    turns: &[MessageTurn],
    before_index: usize,
    limit: usize,
) -> (usize, usize) {
    let end = before_index.min(turns.len());
    let limit = limit.clamp(1, MAX_WINDOW_TURNS);
    let start = round_align_backward(turns, end.saturating_sub(limit));
    (start.min(end), end)
}

/// Compute the response metadata for a window starting at `offset`.
pub fn window_meta(turns: &[MessageTurn], offset: usize) -> WindowMeta {
    let prefix = &turns[..offset.min(turns.len())];
    WindowMeta {
        offset,
        total: turns.len(),
        assistant_before: prefix
            .iter()
            .filter(|t| matches!(t.role, TurnRole::Assistant))
            .count(),
        prefix_hash: prefix_fingerprint(prefix),
        uncovered_prefix_max_ts: prefix.iter().map(|t| t.timestamp).max(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn turn(role: TurnRole, secs: i64) -> MessageTurn {
        MessageTurn {
            id: format!("turn-{secs}"),
            role,
            blocks: vec![],
            timestamp: Utc.timestamp_opt(secs, 0).unwrap(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
        }
    }

    fn sample(roles: &[TurnRole]) -> Vec<MessageTurn> {
        roles
            .iter()
            .enumerate()
            .map(|(i, r)| turn(r.clone(), 1_700_000_000 + i as i64))
            .collect()
    }

    use TurnRole::{Assistant as A, User as U};

    #[test]
    fn tail_window_aligns_back_to_user_turn() {
        let turns = sample(&[U, A, A, U, A, A]);
        // tail 2 → raw start 4 (Assistant) → aligned back to index 3 (User)
        assert_eq!(resolve_window_offset(&turns, TurnWindowReq::Tail(2)), 3);
        // tail covering everything → 0
        assert_eq!(resolve_window_offset(&turns, TurnWindowReq::Tail(500)), 0);
    }

    #[test]
    fn tail_alignment_respects_cap() {
        let mut roles = vec![U];
        roles.extend(std::iter::repeat_n(A, ROUND_ALIGN_CAP + 50));
        let turns = sample(&roles);
        let offset = resolve_window_offset(&turns, TurnWindowReq::Tail(1));
        // Walked ROUND_ALIGN_CAP steps back from the raw start without
        // reaching the user turn: boundary falls mid-round, never index 0.
        assert_eq!(offset, turns.len() - 1 - ROUND_ALIGN_CAP);
        assert!(offset > 0);
    }

    #[test]
    fn from_index_is_exact_and_clamped() {
        let turns = sample(&[U, A, A, U, A]);
        // Index 4 is an Assistant turn — NO round alignment happens.
        assert_eq!(
            resolve_window_offset(&turns, TurnWindowReq::FromIndex(4)),
            4
        );
        assert_eq!(
            resolve_window_offset(&turns, TurnWindowReq::FromIndex(99)),
            5
        );
        assert_eq!(
            resolve_window_offset(&turns, TurnWindowReq::FromIndex(0)),
            0
        );
    }

    #[test]
    fn page_bounds_clamp_and_align() {
        let turns = sample(&[U, A, U, A, A, U, A]);
        // before=5, limit=2 → raw start 3 (Assistant) → aligned to 2 (User)
        assert_eq!(resolve_page_bounds(&turns, 5, 2), (2, 5));
        // before beyond total clamps to total
        assert_eq!(resolve_page_bounds(&turns, 99, 2), (5, 7));
        // before=0 → empty page
        assert_eq!(resolve_page_bounds(&turns, 0, 2), (0, 0));
    }

    #[test]
    fn window_meta_counts_and_bounds() {
        let turns = sample(&[U, A, A, U, A]);
        let meta = window_meta(&turns, 3);
        assert_eq!(meta.offset, 3);
        assert_eq!(meta.total, 5);
        assert_eq!(meta.assistant_before, 2);
        assert_eq!(meta.uncovered_prefix_max_ts, Some(turns[2].timestamp));

        let full = window_meta(&turns, 0);
        assert_eq!(full.assistant_before, 0);
        assert_eq!(full.uncovered_prefix_max_ts, None);
        assert_eq!(full.prefix_hash, format!("{FNV_OFFSET:016x}"));
    }

    #[test]
    fn fingerprint_is_id_free_and_order_sensitive() {
        let turns = sample(&[U, A, A]);
        let mut renamed = turns.clone();
        renamed[0].id = "totally-different-id".to_string();
        assert_eq!(
            prefix_fingerprint(&turns),
            prefix_fingerprint(&renamed),
            "id rewrites must not change the fingerprint"
        );

        let mut shifted = turns.clone();
        shifted.remove(0);
        assert_ne!(
            prefix_fingerprint(&turns),
            prefix_fingerprint(&shifted),
            "removals/shifts must change the fingerprint"
        );
    }

    /// Shared Rust/TS test vector — the TypeScript implementation must
    /// produce these exact strings (see `src/lib/turn-window.test.ts`).
    #[test]
    fn fingerprint_shared_test_vector() {
        assert_eq!(prefix_fingerprint(&[]), "cbf29ce484222325");
        let turns = vec![
            turn(U, 1_700_000_001),
            turn(A, 1_700_000_002),
            turn(TurnRole::System, 1_700_000_003),
        ];
        assert_eq!(prefix_fingerprint(&turns[..1]), "7060d8542023feb2");
        assert_eq!(prefix_fingerprint(&turns), "f171a62320fc10d3");
    }
}
