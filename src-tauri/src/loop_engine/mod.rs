//! Loop engineering engine: drives each running issue through triage → refine →
//! design → plan → implement → verify → review → finalize, autonomously.
//!
//! M2.0 lands only the error type; the driver, dispatch, gates, worktree
//! lifecycle, briefing, recovery and MCP ingest modules arrive in later phases.

pub mod error;
pub mod transitions;

pub use error::LoopError;
