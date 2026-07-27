//! Process-lifetime string interning for **runtime-defined agent metadata**.
//!
//! `AgentType` is `Copy` and threaded by value through ~900 call sites, and
//! [`crate::acp::registry::AcpAgentMeta`] / [`crate::acp::registry::AgentDistribution`]
//! are built entirely from `&'static str` because every agent used to be a
//! compile-time constant. Custom ACP agents break that assumption: their id,
//! name, package spec and download URLs come from the database at runtime.
//!
//! Rather than widen `AgentType` to an owned `String` (which would cost `Copy`
//! and cascade through the whole crate) or convert every registry field to
//! `Cow<'static, str>` (57 destructuring sites across 5 files), we promote the
//! handful of runtime strings to `&'static` by leaking them ONCE and reusing
//! the leak forever.
//!
//! ## Why leaking is bounded here
//!
//! [`intern`] deduplicates: a given string is leaked at most once per process,
//! no matter how many times it is interned. The live set is therefore bounded
//! by the number of DISTINCT strings a user's custom-agent definitions have
//! ever contained during one run of the app — single digits in practice, and
//! it only grows when the user edits a definition. Callers that build whole
//! metadata structs additionally fingerprint them (see
//! `crate::acp::custom_registry`) so re-hydrating an unchanged registry leaks
//! nothing at all.
//!
//! This module is deliberately narrow: do NOT intern user prompts, file
//! contents, session ids, or anything else whose cardinality grows with usage.

use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

fn pool() -> &'static RwLock<HashSet<&'static str>> {
    static POOL: OnceLock<RwLock<HashSet<&'static str>>> = OnceLock::new();
    POOL.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Return a `&'static str` with the same contents as `s`, leaking the
/// allocation on first sight and reusing it on every subsequent call.
///
/// Idempotent by value: `intern("x") == intern("x")` and — because the pool
/// hands back the same leaked slice — the two results are also pointer-equal.
pub fn intern(s: &str) -> &'static str {
    let pool = pool();
    // Fast path: already interned. A poisoned lock is harmless here (the pool
    // is a plain set of leaked strs with no cross-field invariant), so recover
    // rather than propagate a panic into the agent registry.
    if let Some(found) = pool.read().unwrap_or_else(|e| e.into_inner()).get(s) {
        return found;
    }
    let mut guard = pool.write().unwrap_or_else(|e| e.into_inner());
    // Re-check: another thread may have interned `s` between the two locks.
    if let Some(found) = guard.get(s) {
        return found;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    guard.insert(leaked);
    leaked
}

/// Leak a vector into a `&'static` slice.
///
/// Unlike [`intern`] this does NOT deduplicate — slices have no cheap
/// structural key — so callers must gate it behind their own change detection
/// (the custom-agent registry fingerprints each definition before rebuilding).
pub fn leak_slice<T>(items: Vec<T>) -> &'static [T] {
    Vec::leak(items)
}

/// Leak a single value into a `&'static` reference. Same non-deduplicating
/// caveat as [`leak_slice`].
pub fn leak<T>(value: T) -> &'static T {
    Box::leak(Box::new(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_returns_pointer_identical_slices() {
        let a = intern("codeg-intern-test-alpha");
        let b = intern(&String::from("codeg-intern-test-alpha"));
        assert_eq!(a, b);
        assert!(
            std::ptr::eq(a.as_ptr(), b.as_ptr()),
            "interning the same value twice must reuse the same leak"
        );
    }

    #[test]
    fn intern_keeps_distinct_values_distinct() {
        let a = intern("codeg-intern-test-one");
        let b = intern("codeg-intern-test-two");
        assert_ne!(a, b);
    }

    #[test]
    fn intern_handles_empty_and_unicode() {
        assert_eq!(intern(""), "");
        assert_eq!(intern("智能体-🤖"), "智能体-🤖");
    }

    #[test]
    fn leak_slice_preserves_contents() {
        let leaked: &'static [&'static str] = leak_slice(vec![intern("acp"), intern("stdio")]);
        assert_eq!(leaked, &["acp", "stdio"]);
    }
}
