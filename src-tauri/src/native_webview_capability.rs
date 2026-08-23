use serde_json::{json, Value};

pub(crate) const SPIKE_LABEL: &str = "browser-native-webview2-spike";
pub(crate) const NATIVE_BROWSER_LABEL: &str = "native-browser-0123456789abcdef";
const MAIN_LABEL: &str = "main";
const CAPABILITIES: [(&str, &str); 2] = [
    ("default.json", include_str!("../capabilities/default.json")),
    ("desktop.json", include_str!("../capabilities/desktop.json")),
];

pub(crate) fn capability_snapshot() -> Result<Value, String> {
    let mut files = Vec::new();
    let mut passed = true;
    for (name, source) in CAPABILITIES {
        let value: Value = serde_json::from_str(source).map_err(|error| error.to_string())?;
        let windows = string_array(&value, "windows");
        let webviews = string_array(&value, "webviews");
        let main_window_covers_child = windows
            .iter()
            .any(|pattern| wildcard_matches(pattern, MAIN_LABEL));
        let main_webview_authorized = webviews
            .iter()
            .any(|pattern| wildcard_matches(pattern, MAIN_LABEL));
        let child_webview_authorized = webviews
            .iter()
            .any(|pattern| wildcard_matches(pattern, SPIKE_LABEL));
        let native_browser_authorized = webviews
            .iter()
            .any(|pattern| wildcard_matches(pattern, NATIVE_BROWSER_LABEL));
        let file_passed = !main_window_covers_child
            && main_webview_authorized
            && !child_webview_authorized
            && !native_browser_authorized;
        passed &= file_passed;
        files.push(json!({
            "file": name,
            "passed": file_passed,
            "mainWindowCoversChild": main_window_covers_child,
            "mainWebviewAuthorized": main_webview_authorized,
            "childWebviewAuthorized": child_webview_authorized,
            "nativeBrowserAuthorized": native_browser_authorized,
        }));
    }
    Ok(json!({ "passed": passed, "files": files }))
}

fn string_array<'a>(value: &'a Value, key: &str) -> Vec<&'a str> {
    value[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut pattern_index, mut value_index) = (0, 0);
    let (mut star_index, mut star_value_index) = (None, 0);
    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matching_covers_tauri_label_patterns() {
        assert!(wildcard_matches("main", "main"));
        assert!(wildcard_matches("browser-*", SPIKE_LABEL));
        assert!(wildcard_matches("*", SPIKE_LABEL));
        assert!(!wildcard_matches("commit-*", SPIKE_LABEL));
        assert!(!wildcard_matches("main", NATIVE_BROWSER_LABEL));
    }

    #[test]
    fn main_permissions_do_not_flow_to_the_spike_child() {
        let snapshot = capability_snapshot().expect("valid capability files");
        assert_eq!(snapshot["passed"], true, "{snapshot:#}");
    }
}
