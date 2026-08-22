//! Live ACP session titles.
//!
//! Agents publish a session name through `session_info_update.title`. Codeg
//! used to ignore that field and only adopt a title the next time the
//! conversation was loaded from disk. These helpers extract a usable title
//! from the live notification so the lifecycle worker can write it immediately.

/// Pull a usable session title out of ACP `session_info_update.title`.
///
/// `Undefined` (passed in as `None`) means the update did not touch the title
/// and is ignored. The schema also uses `Null` to mean "clear"; we treat that
/// the same as absent on purpose so an explicit clear cannot wipe the row
/// back to Untitled. Whitespace-only strings are ignored for the same reason.
pub(crate) fn native_title_from_session_info(title: Option<&str>) -> Option<String> {
    let t = title?.trim();
    if t.is_empty() {
        None
    } else {
        Some(crate::parsers::truncate_str(t, 100))
    }
}

#[cfg(test)]
mod tests {
    use super::native_title_from_session_info;

    #[test]
    fn rejects_missing_and_blank() {
        assert_eq!(native_title_from_session_info(None), None);
        assert_eq!(native_title_from_session_info(Some("")), None);
        assert_eq!(native_title_from_session_info(Some("   ")), None);
        assert_eq!(native_title_from_session_info(Some("\n\t")), None);
    }

    #[test]
    fn trims_and_keeps_a_real_title() {
        assert_eq!(
            native_title_from_session_info(Some("  Fix login flow  ")).as_deref(),
            Some("Fix login flow")
        );
    }

    #[test]
    fn caps_at_parser_title_length() {
        let long = "a".repeat(150);
        let got = native_title_from_session_info(Some(&long)).unwrap();
        assert_eq!(got, crate::parsers::truncate_str(&long, 100));
        assert!(got.ends_with("..."));
    }
}
