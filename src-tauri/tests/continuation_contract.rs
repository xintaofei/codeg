//! Wire-contract guard for the collaboration DTOs (v2 design §7 / A26):
//! the SAME JSON fixtures under `tests/fixtures/collaboration/` are loaded by
//! the TypeScript parser test (`src/lib/collaboration.test.ts`), so the Rust
//! serialization and the TS runtime validation can never drift. Any field
//! rename, nullability change, or state rename fails BOTH sides.

use codeg_lib::acp::delegation::continuation::{
    CollaborationSessionState, ContinuationError, ContinuationErrorCode, SessionSummary,
    TurnReport, TurnState,
};

const FIXTURE_DIR: &str = "tests/fixtures/collaboration";

fn load(name: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURE_DIR}/{name}"))
        .unwrap_or_else(|e| panic!("fixture {name} missing: {e}"))
}

#[test]
fn rust_deserializes_the_shared_turn_fixtures() {
    for state in [
        "accepted",
        "preparing",
        "dispatching",
        "running",
        "cancel_requested",
        "completed",
        "failed",
        "canceled",
        "interrupted",
        "outcome_unknown",
    ] {
        let raw = load(&format!("turn_{state}.json"));
        let report: TurnReport =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("turn_{state}: {e}"));
        assert_eq!(report.state.as_str(), state);
        assert_eq!(report.turn_id, "turn-1");
        assert_eq!(report.session_id, "session-1");
        assert_eq!(report.source_task_id, "task-0");
        assert_eq!(report.initiator_kind, "parent_agent");
        assert!(!report.text_truncated);
    }
}

/// The strongest direction of the contract: a Rust-built TurnReport
/// round-trips to EXACTLY the fixture bytes (modulo key order) — i.e. what
/// the TS parser was frozen against is what Rust actually emits.
#[test]
fn rust_serialization_matches_the_fixture_shape() {
    let raw = load("turn_completed.json");
    let report: TurnReport = serde_json::from_str(&raw).expect("fixture parses");
    let re_emitted: TurnReport =
        serde_json::from_str(&serde_json::to_string(&report).expect("serialize"))
            .expect("round-trips");
    assert_eq!(report, re_emitted);
    // And the TS fixture's own fields survive the round trip untouched.
    let fixture: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let emitted: serde_json::Value =
        serde_json::to_value(&report).expect("serialize to value");
    assert_eq!(fixture, emitted, "Rust output must equal the frozen fixture");
}

#[test]
fn rust_deserializes_the_shared_session_fixture() {
    let raw = load("session_open.json");
    let summary: SessionSummary = serde_json::from_str(&raw).expect("session fixture");
    assert_eq!(summary.state, CollaborationSessionState::Open);
}

#[test]
fn rust_deserializes_the_shared_snapshot_fixtures() {
    let empty: codeg_lib::commands::collaboration::CollaborationSnapshot =
        serde_json::from_str(&load("snapshot_empty.json")).expect("empty snapshot");
    assert!(empty.session.is_none());
    assert!(empty.turns.is_empty());

    let page: codeg_lib::commands::collaboration::CollaborationSnapshot =
        serde_json::from_str(&load("snapshot_page.json")).expect("page snapshot");
    assert_eq!(page.turns.len(), 2);
    assert_eq!(page.turns[0].state, TurnState::Completed);
    assert_eq!(page.turns[1].state, TurnState::Running);
    assert!(page.turns[1].result_text.is_none());
}

#[test]
fn unknown_schema_version_is_rejected_on_the_rust_side_too() {
    let raw = load("turn_completed.json").replace("\"schema_version\": 1", "\"schema_version\": 2");
    let result: Result<TurnReport, _> = serde_json::from_str(&raw);
    assert!(result.is_err(), "an unknown schema_version must fail parsing");
}

#[test]
fn continuation_error_round_trips_with_its_code_vocabulary() {
    let err = ContinuationError::new(
        ContinuationErrorCode::SessionReservedForDelegation,
        "reserved",
    )
    .with_ids(Some("session-1"), Some("turn-1"));
    let json = serde_json::to_value(&err).expect("serialize");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["error_code"], "session_reserved_for_delegation");
    assert_eq!(json["session_id"], "session-1");
    assert_eq!(json["turn_id"], "turn-1");
    let back: ContinuationError = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, err);
}
