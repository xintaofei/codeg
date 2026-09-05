//! Strict session recovery: re-attach an EXISTING external agent session
//! without ever falling back to `session/new` (v2 design §5.3).
//!
//! The ordinary spawn path recovers with the chain resume → load → new: any
//! failure falls through to a brand-new session, which is the right behavior
//! for a fresh conversation but silently WRONG for a continuation round — a
//! "new session" would masquerade as the child's prior context. The strict
//! entry inverts that: only `session/resume` / `session/load` against the
//! recorded external session id may establish the session, readiness is a
//! typed verdict delivered AFTER recovery, replay drain, and successful
//! config application, and any failure is reported instead of papered over.

pub mod types;

pub use types::*;
