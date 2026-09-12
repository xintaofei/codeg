//! Web-mode mirror of `commands::semantic::semantic_submit`. Re-exports the
//! handler so the router can register it under `handlers::semantic::`.

pub use crate::commands::semantic::semantic_submit_handler;
