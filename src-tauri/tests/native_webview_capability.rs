#[path = "../src/native_webview_capability.rs"]
mod native_webview_capability;

#[test]
fn main_webview_stays_authorized_without_authorizing_child_webviews() {
    let snapshot = native_webview_capability::capability_snapshot()
        .expect("capability files should be valid JSON");
    assert_eq!(snapshot["passed"], true, "{snapshot:#}");
    for file in snapshot["files"].as_array().expect("capability files") {
        assert_eq!(file["nativeBrowserAuthorized"], false, "{file:#}");
    }
}
