//! macOS shim on top of `WKWebView` / `WKUserContentController` (objc2).
//!
//! The helper script and the `codegBrowser` message handler live in the
//! `WKContentWorld` named `codeg`: a separate JavaScript global for the same
//! DOM, invisible to page scripts and immune to their prototype tampering.
//! `WKContentWorld` needs macOS 11; older systems fall back to the page world
//! (reported as `ChannelKind::Legacy`).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;
use std::sync::Arc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, DeclaredClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSImageCompressionFactor};
use objc2_foundation::{
    ns_string, NSArray, NSDate, NSDictionary, NSError, NSNumber, NSProcessInfo, NSString, NSURL,
    NSURLErrorFailingURLErrorKey, NSUUID,
};
use objc2_web_kit::{
    WKContentWorld, WKFindConfiguration, WKFindResult, WKNavigation, WKNavigationAction,
    WKNavigationActionPolicy, WKNavigationDelegate, WKScriptMessage, WKScriptMessageHandler,
    WKSnapshotConfiguration, WKUserContentController, WKUserScript, WKUserScriptInjectionTime,
    WKWebView, WKWebViewConfiguration, WKWebsiteDataRecord, WKWebsiteDataStore,
};
use tauri_runtime_wry::wry::{self, WebViewExtMacOS};

use super::super::channel::MessageSink;
use super::super::hooks::{classify_load_error, LoadFailure};
use super::super::profile::{BrowserProxy, ProxyScheme, DEFAULT_DATA_STORE_IDENTIFIER};

pub const WORLD_NAME: &str = "codeg";
pub const HANDLER_NAME: &str = "codegBrowser";

thread_local! {
    // Controllers that already carry our handler + scripts. A popup created
    // from an opener arrives with the opener's WKUserContentController, and
    // WebKit throws on a second handler registration under the same name.
    static INSTALLED_CONTROLLERS: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
}

pub struct HandlerIvars {
    sink: MessageSink,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = HandlerIvars]
    pub struct CodegMessageHandler;

    unsafe impl NSObjectProtocol for CodegMessageHandler {}

    unsafe impl WKScriptMessageHandler for CodegMessageHandler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(
            this: &CodegMessageHandler,
            _controller: &WKUserContentController,
            message: &WKScriptMessage,
        ) {
            // SAFETY: WebKit hands us live objects on the main thread.
            unsafe {
                let body = message.body();
                let Some(text) = body.downcast_ref::<NSString>() else {
                    return;
                };
                let frame = message.frameInfo();
                let main_frame = frame.isMainFrame();
                let source = frame
                    .webView()
                    .map(|wv| Retained::as_ptr(&wv) as usize)
                    .unwrap_or(0);
                (this.ivars().sink)(text.to_string(), main_frame, source);
            }
        }
    }
);

impl CodegMessageHandler {
    fn new(sink: MessageSink, mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(HandlerIvars { sink });
        // SAFETY: plain NSObject init.
        unsafe { msg_send![super(this), init] }
    }
}

fn mtm() -> Result<MainThreadMarker, String> {
    MainThreadMarker::new().ok_or_else(|| "not on the main thread".to_string())
}

/// Content worlds (macOS 11+): decided at runtime, not compile time, because
/// codeg sets no minimumSystemVersion.
pub fn supports_content_world(controller: &WKUserContentController) -> bool {
    controller.respondsToSelector(sel!(addScriptMessageHandler:contentWorld:name:))
}

/// Register the message handler and inject `scripts` at document start in
/// every frame. Returns `true` when an isolated world was used. Idempotent per
/// user-content controller.
pub fn install_world(
    webview: &wry::WebView,
    scripts: &[&str],
    sink: MessageSink,
) -> Result<bool, String> {
    let mtm = mtm()?;
    let controller = webview.manager();
    let key = Retained::as_ptr(&controller) as usize;
    let already = INSTALLED_CONTROLLERS.with(|set| !set.borrow_mut().insert(key));
    if already {
        // Shared controller (popup from an opener): scripts and handler are in
        // place already, and the sink resolves the tab per message.
        return Ok(supports_content_world(&controller));
    }
    let handler = CodegMessageHandler::new(sink, mtm);
    let proto = ProtocolObject::from_ref(&*handler);
    // SAFETY: main thread, live controller; WebKit retains the handler.
    unsafe {
        if supports_content_world(&controller) {
            let world = WKContentWorld::worldWithName(ns_string!("codeg"), mtm);
            controller.addScriptMessageHandler_contentWorld_name(
                proto,
                &world,
                ns_string!("codegBrowser"),
            );
            for source in scripts {
                let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
                    mtm.alloc(),
                    &NSString::from_str(source),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    false,
                    &world,
                );
                controller.addUserScript(&script);
            }
            Ok(true)
        } else {
            controller.addScriptMessageHandler_name(proto, ns_string!("codegBrowser"));
            for source in scripts {
                let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    mtm.alloc(),
                    &NSString::from_str(source),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    false,
                );
                controller.addUserScript(&script);
            }
            Ok(false)
        }
    }
}

/// Evaluate `expression` in the `codeg` world of the main frame. The result
/// arrives as the JSON string `{"ok":true,"value":…}` or
/// `{"ok":false,"error":…}` so no platform value conversion is needed.
pub fn eval_in_world(
    webview: &wry::WebView,
    expression: &str,
    callback: impl Fn(Result<String, String>) + Send + 'static,
) -> Result<(), String> {
    let mtm = mtm()?;
    let wk = webview.webview();
    let wrapped = format!(
        "(function(){{try{{return JSON.stringify({{ok:true,value:(function(){{return ({expression});}})()}})}}catch(e){{return JSON.stringify({{ok:false,error:String(e&&e.stack||e)}})}}}})()"
    );
    let block = RcBlock::<dyn Fn(*mut AnyObject, *mut NSError)>::new(
        move |result: *mut AnyObject, error: *mut NSError| {
            // SAFETY: WebKit passes valid or null pointers; we only read.
            let outcome = unsafe {
                if !error.is_null() {
                    Err((*error).localizedDescription().to_string())
                } else if result.is_null() {
                    Ok("null".to_string())
                } else {
                    (*result)
                        .downcast_ref::<NSString>()
                        .map(|s| s.to_string())
                        .ok_or_else(|| "non-string result".to_string())
                }
            };
            callback(outcome);
        },
    );
    // SAFETY: main thread, live webview.
    unsafe {
        let world = WKContentWorld::worldWithName(ns_string!("codeg"), mtm);
        wk.evaluateJavaScript_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(&wrapped),
            None,
            &world,
            Some(&block),
        );
    }
    Ok(())
}

/// Viewport snapshot as JPEG: `(bytes, pixel width, pixel height)`. Built for
/// the freeze frame a placeholder shows while its surface is hidden under an
/// overlay, so it takes the frame as displayed now (`afterScreenUpdates:
/// false`) and encodes as JPEG through AppKit — a 5-megapixel PNG would take
/// longer to encode than the overlay's own open animation.
pub fn snapshot_jpeg(
    webview: &wry::WebView,
    quality: f64,
    callback: impl Fn(Result<(Vec<u8>, u32, u32), String>) + Send + 'static,
) -> Result<(), String> {
    let mtm = mtm()?;
    let wk = webview.webview();
    let block = RcBlock::<dyn Fn(*mut NSImage, *mut NSError)>::new(
        move |image: *mut NSImage, error: *mut NSError| {
            // SAFETY: WebKit passes valid or null pointers; we only read.
            let outcome = unsafe {
                if !error.is_null() {
                    Err((*error).localizedDescription().to_string())
                } else if image.is_null() {
                    Err("snapshot returned no image".to_string())
                } else {
                    encode_jpeg(&*image, quality, mtm)
                }
            };
            callback(outcome);
        },
    );
    // SAFETY: main thread, live webview.
    unsafe {
        let config = WKSnapshotConfiguration::new(mtm);
        config.setAfterScreenUpdates(false);
        wk.takeSnapshotWithConfiguration_completionHandler(Some(&config), &block);
    }
    Ok(())
}

/// # Safety
/// Main thread, live image.
unsafe fn encode_jpeg(image: &NSImage, quality: f64, mtm: MainThreadMarker) -> Result<(Vec<u8>, u32, u32), String> {
    let cg = image
        .CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None)
        .ok_or_else(|| "snapshot has no bitmap".to_string())?;
    let rep = NSBitmapImageRep::initWithCGImage(mtm.alloc(), &cg);
    let width = u32::try_from(rep.pixelsWide()).unwrap_or(0);
    let height = u32::try_from(rep.pixelsHigh()).unwrap_or(0);
    let factor = NSNumber::new_f64(quality);
    let factor_object: &AnyObject = &factor;
    let properties = NSDictionary::from_slices(&[NSImageCompressionFactor], &[factor_object]);
    let data = rep
        .representationUsingType_properties(NSBitmapImageFileType::JPEG, &properties)
        .ok_or_else(|| "jpeg encoding failed".to_string())?;
    Ok((data.to_vec(), width, height))
}

/// Viewport snapshot as PNG bytes.
pub fn snapshot_png(
    webview: &wry::WebView,
    callback: impl Fn(Result<Vec<u8>, String>) + Send + 'static,
) -> Result<(), String> {
    let mtm = mtm()?;
    let wk = webview.webview();
    let block = RcBlock::<dyn Fn(*mut NSImage, *mut NSError)>::new(
        move |image: *mut NSImage, error: *mut NSError| {
            // SAFETY: WebKit passes valid or null pointers; we only read.
            let outcome = unsafe {
                if !error.is_null() {
                    Err((*error).localizedDescription().to_string())
                } else if image.is_null() {
                    Err("snapshot returned no image".to_string())
                } else {
                    (*image)
                        .TIFFRepresentation()
                        .and_then(|tiff| NSBitmapImageRep::imageRepWithData(&tiff))
                        .and_then(|rep| {
                            rep.representationUsingType_properties(
                                NSBitmapImageFileType::PNG,
                                &NSDictionary::new(),
                            )
                        })
                        .map(|png| png.to_vec())
                        .ok_or_else(|| "png encoding failed".to_string())
                }
            };
            callback(outcome);
        },
    );
    // SAFETY: main thread, live webview.
    unsafe {
        let config = WKSnapshotConfiguration::new(mtm);
        config.setAfterScreenUpdates(true);
        wk.takeSnapshotWithConfiguration_completionHandler(Some(&config), &block);
    }
    Ok(())
}

/// Highlight the next (or previous) occurrence of `query` in the page, the
/// way ⌘F does in Safari: WebKit owns the search and the selection, so this
/// never touches the DOM and cannot be observed or broken by the page.
/// Wraps around, case-insensitive — the defaults a find bar is expected to
/// have. `callback` gets whether anything matched.
pub fn find_string(
    webview: &wry::WebView,
    query: &str,
    forward: bool,
    callback: impl Fn(bool) + Send + 'static,
) -> Result<(), String> {
    let mtm = mtm()?;
    let wk = webview.webview();
    let block = RcBlock::<dyn Fn(NonNull<WKFindResult>)>::new(move |result: NonNull<WKFindResult>| {
        // SAFETY: WebKit hands us a live result for the duration of the call.
        callback(unsafe { result.as_ref().matchFound() });
    });
    // SAFETY: main thread, live webview.
    unsafe {
        let configuration = WKFindConfiguration::new(mtm);
        configuration.setBackwards(!forward);
        configuration.setCaseSensitive(false);
        configuration.setWraps(true);
        wk.findString_withConfiguration_completionHandler(
            &NSString::from_str(query),
            Some(&configuration),
            &block,
        );
    }
    Ok(())
}

/// Drop the find highlight. WebKit has no "stop finding" call; clearing the
/// selection (in the isolated world, on the shared DOM) is what removes it.
pub fn clear_find(webview: &wry::WebView) -> Result<(), String> {
    eval_in_world(
        webview,
        "(function(){try{getSelection().removeAllRanges()}catch(e){}return true})()",
        |_| {},
    )
}

pub fn go_back(webview: &wry::WebView) {
    // SAFETY: main thread, live webview.
    unsafe {
        let _ = webview.webview().goBack();
    }
}

pub fn go_forward(webview: &wry::WebView) {
    // SAFETY: main thread, live webview.
    unsafe {
        let _ = webview.webview().goForward();
    }
}

pub fn can_go_back(webview: &wry::WebView) -> bool {
    // SAFETY: main thread, live webview.
    unsafe { webview.webview().canGoBack() }
}

pub fn can_go_forward(webview: &wry::WebView) -> bool {
    // SAFETY: main thread, live webview.
    unsafe { webview.webview().canGoForward() }
}

pub fn stop_loading(webview: &wry::WebView) {
    // SAFETY: main thread, live webview.
    unsafe { webview.webview().stopLoading() }
}

/// Identity of the platform webview behind a wry `WebView`, matching the
/// `source` a message sink receives.
/// `WKWebView.URL`, or `None` before any navigation has committed (and after
/// a first navigation failed). wry's own `url()` unwraps this and panics.
pub fn current_url(webview: &wry::WebView) -> Option<String> {
    let wk = WebViewExtMacOS::webview(webview);
    // SAFETY: main thread; WebKit hands back an owned NSURL / NSString.
    unsafe { wk.URL().and_then(|u| u.absoluteString()).map(|s| s.to_string()) }
}

/// `WKWebView.isLoading`. wry has no navigation-failure callback, so this is
/// the only way to notice that a load ended without finishing.
pub fn is_loading(webview: &wry::WebView) -> bool {
    let wk = WebViewExtMacOS::webview(webview);
    // SAFETY: main thread.
    unsafe { wk.isLoading() }
}

pub fn webview_pointer(webview: &wry::WebView) -> usize {
    Retained::as_ptr(&webview.webview()) as usize
}

/// Diagnostic view of the native state (dev puppet only).
pub fn debug_view(webview: &wry::WebView) -> serde_json::Value {
    let wk = webview.webview();
    // SAFETY: main thread, live view.
    let (hidden, hidden_or_ancestor, has_window, has_superview, frame, loading, has_url) = unsafe {
        let frame = wk.frame();
        (
            wk.isHidden(),
            wk.isHiddenOrHasHiddenAncestor(),
            wk.window().is_some(),
            wk.superview().is_some(),
            [frame.origin.x, frame.origin.y, frame.size.width, frame.size.height],
            wk.isLoading(),
            wk.URL().is_some(),
        )
    };
    serde_json::json!({
        "hidden": hidden,
        "isLoading": loading,
        "hasUrl": has_url,
        "hiddenOrAncestor": hidden_or_ancestor,
        "hasWindow": has_window,
        "hasSuperview": has_superview,
        "frame": frame,
    })
}

// ---------------------------------------------------------------------------
// Navigation delegate: what wry does not report
// ---------------------------------------------------------------------------
//
// wry installs its own `WKNavigationDelegate` and surfaces two of its
// callbacks (commit, finish). A tab needs three more: the provisional start
// (where a page-initiated navigation is heading), the failures (which kind,
// and at once rather than when a poll notices the spinner stopped), and, for
// the policy decision wry does forward, whether the action is for the main
// frame. Rather than re-implementing wry's delegate — its handlers reach into
// private state — the tab's `WKWebView` gets a wrapper: it answers the
// callbacks it cares about and forwards every other selector to wry's object
// (`forwardingTargetForSelector:`), which stays alive inside wry's
// `WebView` for as long as the tab does. `respondsToSelector:` is answered
// from both, so WebKit sees exactly the optional methods wry implements plus
// ours. The wrapper is dropped together with the webview: keeping it longer
// would keep wry's delegate, and through it the `WKWebView`, alive.

/// Events the wrapper reports, on the main thread.
pub enum NavigationEvent {
    /// A main-frame navigation started; the URL it is heading for.
    Started(String),
    Failed(LoadFailure),
}

pub type NavigationSink = Arc<dyn Fn(NavigationEvent) + Send + Sync>;

thread_local! {
    /// Wrappers by `WKWebView` pointer, kept alive here (`navigationDelegate`
    /// is a weak property).
    static NAV_DELEGATES: RefCell<HashMap<usize, Retained<CodegNavigationDelegate>>> =
        RefCell::new(HashMap::new());
    /// Whether the navigation action currently being decided targets the main
    /// frame. Set around the forward to wry, whose synchronous call into the
    /// navigation handler is the only place the host learns about the action
    /// — and wry's handler signature carries the URL alone.
    static CURRENT_ACTION_MAIN_FRAME: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Inside a navigation handler: does the action being decided target the
/// main frame? `None` outside a decision (other platforms, or a call from
/// elsewhere), which callers treat as "main frame" — the strict reading.
pub fn current_navigation_is_main_frame() -> Option<bool> {
    CURRENT_ACTION_MAIN_FRAME.with(|flag| flag.get())
}

pub struct NavigationDelegateIvars {
    inner: Retained<ProtocolObject<dyn WKNavigationDelegate>>,
    sink: NavigationSink,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = NavigationDelegateIvars]
    pub struct CodegNavigationDelegate;

    unsafe impl NSObjectProtocol for CodegNavigationDelegate {}

    impl CodegNavigationDelegate {
        #[unsafe(method(respondsToSelector:))]
        fn responds_to_selector(&self, selector: Sel) -> bool {
            self.class().responds_to(selector) || self.ivars().inner.respondsToSelector(selector)
        }

        #[unsafe(method(forwardingTargetForSelector:))]
        fn forwarding_target_for_selector(&self, _selector: Sel) -> *mut AnyObject {
            Retained::as_ptr(&self.ivars().inner) as *mut AnyObject
        }
    }

    unsafe impl WKNavigationDelegate for CodegNavigationDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            webview: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::Block<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // SAFETY: WebKit hands us live objects on the main thread.
            let main_frame = unsafe { action.targetFrame().map(|frame| frame.isMainFrame()) };
            // No target frame = a new window; the strict reading applies.
            CURRENT_ACTION_MAIN_FRAME.with(|flag| flag.set(Some(main_frame.unwrap_or(true))));
            let inner = &self.ivars().inner;
            // SAFETY: forwarding the exact selector and arguments WebKit gave
            // us to the delegate that implements it.
            unsafe {
                let _: () = msg_send![
                    &**inner,
                    webView: webview,
                    decidePolicyForNavigationAction: action,
                    decisionHandler: handler
                ];
            }
            CURRENT_ACTION_MAIN_FRAME.with(|flag| flag.set(None));
        }

        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn did_start_provisional(&self, webview: &WKWebView, _navigation: Option<&WKNavigation>) {
            // `URL` is the active URL: the provisional one while a load is in
            // flight, so this is where the navigation is heading.
            // SAFETY: main thread, live webview.
            let url = unsafe { webview.URL().and_then(|u| u.absoluteString()) }.map(|s| s.to_string());
            tracing::debug!("[browser] navigation started: {url:?}");
            if let Some(url) = url {
                (self.ivars().sink)(NavigationEvent::Started(url));
            }
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(&self, _webview: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.report_failure(error, true);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, _webview: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.report_failure(error, false);
        }
    }
);

impl CodegNavigationDelegate {
    fn new(
        inner: Retained<ProtocolObject<dyn WKNavigationDelegate>>,
        sink: NavigationSink,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let this = mtm
            .alloc::<Self>()
            .set_ivars(NavigationDelegateIvars { inner, sink });
        // SAFETY: plain NSObject init.
        unsafe { msg_send![super(this), init] }
    }

    fn report_failure(&self, error: &NSError, provisional: bool) {
        let domain = error.domain().to_string();
        let code = i64::try_from(error.code()).unwrap_or(i64::MAX);
        let kind = classify_load_error(&domain, code);
        tracing::debug!(
            "[browser] navigation failed (provisional: {provisional}): {domain} {code} -> {kind:?}: {}",
            error.localizedDescription()
        );
        let Some(kind) = kind else {
            return;
        };
        // SAFETY: main thread; the dictionary and its values are live.
        let url = unsafe {
            error
                .userInfo()
                .objectForKey(NSURLErrorFailingURLErrorKey)
                .and_then(|value| value.downcast::<NSURL>().ok())
                .and_then(|url| url.absoluteString())
                .map(|s| s.to_string())
        };
        (self.ivars().sink)(NavigationEvent::Failed(LoadFailure {
            kind,
            message: error.localizedDescription().to_string(),
            url,
            provisional,
        }));
    }
}

/// Wrap the webview's navigation delegate. Idempotent per webview.
pub fn install_navigation_delegate(webview: &wry::WebView, sink: NavigationSink) -> Result<(), String> {
    let mtm = mtm()?;
    let wk = webview.webview();
    let key = Retained::as_ptr(&wk) as usize;
    let installed = NAV_DELEGATES.with(|map| map.borrow().contains_key(&key));
    if installed {
        return Ok(());
    }
    // SAFETY: main thread, live webview.
    let inner = unsafe { wk.navigationDelegate() }
        .ok_or_else(|| "the webview has no navigation delegate to wrap".to_string())?;
    let delegate = CodegNavigationDelegate::new(inner, sink, mtm);
    // SAFETY: main thread; the wrapper is retained in `NAV_DELEGATES` below,
    // which is what keeps the weak `navigationDelegate` valid.
    unsafe { wk.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    NAV_DELEGATES.with(|map| map.borrow_mut().insert(key, delegate));
    Ok(())
}

/// Drop the wrapper of a webview that is going away (call before the wry
/// `WebView` is dropped, on the main thread).
pub fn forget_navigation_delegate(webview: &wry::WebView) {
    let key = webview_pointer(webview);
    NAV_DELEGATES.with(|map| map.borrow_mut().remove(&key));
}

// ---------------------------------------------------------------------------
// Profile: the tabs' own data store and its proxy
// ---------------------------------------------------------------------------

struct ProfileStore {
    store: Retained<WKWebsiteDataStore>,
    /// A store of the profile's own (macOS 14+) rather than WebKit's default
    /// store, which the app's own webviews live in.
    isolated: bool,
    /// Proxy last written to the store, to skip rewriting the same value.
    proxy: Option<BrowserProxy>,
}

thread_local! {
    static PROFILE: RefCell<Option<ProfileStore>> = const { RefCell::new(None) };
}

fn macos_major_version() -> isize {
    NSProcessInfo::processInfo().operatingSystemVersion().majorVersion
}

/// `WKWebsiteDataStore(forIdentifier:)` and `proxyConfigurations` both arrived
/// in macOS 14; before that the tabs share WebKit's default store with the app
/// and cannot be proxied. Safe from any thread.
pub fn supports_isolated_profile() -> bool {
    macos_major_version() >= 14
}

/// The profile's data store, created on first use and kept for the life of
/// the main thread. WebKit hands back the same store for the same identifier,
/// so owned windows built by tauri with that identifier share it too.
fn profile_store(mtm: MainThreadMarker) -> Retained<WKWebsiteDataStore> {
    PROFILE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let entry = slot.get_or_insert_with(|| {
            let isolated = supports_isolated_profile();
            // SAFETY: main thread; WebKit owns the store.
            let store = unsafe {
                if isolated {
                    let identifier = NSUUID::from_bytes(DEFAULT_DATA_STORE_IDENTIFIER);
                    WKWebsiteDataStore::dataStoreForIdentifier(&identifier, mtm)
                } else {
                    WKWebsiteDataStore::defaultDataStore(mtm)
                }
            };
            ProfileStore {
                store,
                isolated,
                proxy: None,
            }
        });
        entry.store.clone()
    })
}

fn profile_is_isolated() -> bool {
    PROFILE.with(|slot| slot.borrow().as_ref().map(|entry| entry.isolated).unwrap_or(false))
}

/// A `WKWebViewConfiguration` whose data store is the profile's. Every regular
/// tab is built from one; popups inherit their opener's instead.
pub fn profile_configuration(mtm: MainThreadMarker) -> Retained<WKWebViewConfiguration> {
    let store = profile_store(mtm);
    // SAFETY: main thread; both objects are live.
    unsafe {
        let configuration = WKWebViewConfiguration::new(mtm);
        configuration.setWebsiteDataStore(&store);
        configuration
    }
}

/// Create the profile's store if needed and point it at `proxy` (or at no
/// proxy). Open tabs use the new value for their next connections; setting the
/// same value again does nothing, so callers can be liberal.
pub fn ensure_profile(proxy: Option<BrowserProxy>) -> Result<(), String> {
    let mtm = mtm()?;
    let store = profile_store(mtm);
    let unchanged = PROFILE.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|entry| entry.proxy == proxy)
    });
    if unchanged {
        return Ok(());
    }
    if !profile_is_isolated() {
        return match proxy {
            Some(_) => Err("proxying browser tabs needs macOS 14 or later".to_string()),
            None => Ok(()),
        };
    }
    let configurations: Retained<NSArray<NSObject>> = match &proxy {
        Some(proxy) => NSArray::from_retained_slice(&[network::proxy_config(proxy)?]),
        None => NSArray::new(),
    };
    // SAFETY: main thread; `proxyConfigurations` is a public property on
    // macOS 14+ (checked above). Written through KVC because objc2-web-kit
    // does not bind Network.framework's types.
    unsafe {
        let _: () = msg_send![&*store, setValue: &*configurations, forKey: ns_string!("proxyConfigurations")];
    }
    PROFILE.with(|slot| {
        if let Some(entry) = slot.borrow_mut().as_mut() {
            entry.proxy = proxy;
        }
    });
    Ok(())
}

/// Remove every kind of website data (cookies, caches, storage, …) from the
/// profile, whether or not a tab is open. With a store of its own (macOS 14+)
/// that is the whole store; when the tabs still share WebKit's default store
/// with the app, see `clear_shared_store_except_app`. `done` runs on the main
/// thread once WebKit has finished.
pub fn clear_profile_store(done: impl Fn() + 'static) -> Result<(), String> {
    let mtm = mtm()?;
    let store = profile_store(mtm);
    if !profile_is_isolated() {
        return clear_shared_store_except_app(done);
    }
    // SAFETY: main thread; WebKit owns every object handed back.
    unsafe {
        let types = WKWebsiteDataStore::allWebsiteDataTypes(mtm);
        let since = NSDate::dateWithTimeIntervalSince1970(0.0);
        let handler = RcBlock::new(done);
        store.removeDataOfTypes_modifiedSince_completionHandler(&types, &since, &handler);
    }
    Ok(())
}

/// Clear WebKit's default store one origin at a time, leaving the app's own
/// origins alone: below macOS 14 the tabs have no store of their own, and a
/// blanket removal would wipe the workspace's localStorage along with the
/// pages' cookies.
pub fn clear_shared_store_except_app(done: impl Fn() + 'static) -> Result<(), String> {
    let mtm = mtm()?;
    // SAFETY: main thread; WebKit owns the store.
    let store = unsafe { WKWebsiteDataStore::defaultDataStore(mtm) };
    let types = unsafe { WKWebsiteDataStore::allWebsiteDataTypes(mtm) };
    let done = std::rc::Rc::new(done);
    let removing_store = store.clone();
    let removing_types = types.clone();
    let fetched = RcBlock::new(move |records: std::ptr::NonNull<NSArray<WKWebsiteDataRecord>>| {
        // SAFETY: WebKit passes a live array on the main thread.
        let records = unsafe { records.as_ref() };
        let victims: Vec<Retained<WKWebsiteDataRecord>> = records
            .iter()
            .filter(|record| {
                // SAFETY: live record.
                let name = unsafe { record.displayName() };
                !is_app_origin(&name.to_string())
            })
            .collect();
        let victims = NSArray::from_retained_slice(&victims);
        let done = done.clone();
        let finished = RcBlock::new(move || done());
        // SAFETY: main thread; all three arguments are live.
        unsafe {
            removing_store.removeDataOfTypes_forDataRecords_completionHandler(
                &removing_types,
                &victims,
                &finished,
            );
        }
    });
    // SAFETY: main thread.
    unsafe { store.fetchDataRecordsOfTypes_completionHandler(&types, &fetched) };
    Ok(())
}

/// Display names of every record in WebKit's default store (dev puppet only:
/// evidence that the per-record path spares the app's origins).
pub fn default_store_record_names(done: impl Fn(Vec<String>) + 'static) -> Result<(), String> {
    let mtm = mtm()?;
    // SAFETY: main thread; WebKit owns the store.
    let store = unsafe { WKWebsiteDataStore::defaultDataStore(mtm) };
    let types = unsafe { WKWebsiteDataStore::allWebsiteDataTypes(mtm) };
    let fetched = RcBlock::new(move |records: std::ptr::NonNull<NSArray<WKWebsiteDataRecord>>| {
        // SAFETY: WebKit passes a live array on the main thread.
        let records = unsafe { records.as_ref() };
        let names = records
            .iter()
            // SAFETY: live record.
            .map(|record| unsafe { record.displayName() }.to_string())
            .collect();
        done(names);
    });
    // SAFETY: main thread.
    unsafe { store.fetchDataRecordsOfTypes_completionHandler(&types, &fetched) };
    Ok(())
}

/// Hosts the app's own webviews are served from — `http://localhost:<port>`
/// in development, `tauri://localhost` and `http://tauri.localhost` in release
/// — as WebKit names their data records.
fn is_app_origin(display_name: &str) -> bool {
    display_name == "localhost" || display_name.ends_with(".localhost")
}

mod network {
    //! Network.framework's proxy-config C API, resolved at run time: the
    //! symbols exist only on macOS 14+, and a load-time reference would keep
    //! the whole binary from launching on older systems. The objects it hands
    //! back are Objective-C objects (`OS_object`), which is what lets them
    //! ride in an `NSArray` and be retained like any other.

    use std::ffi::{c_char, c_void, CString};

    use objc2::rc::Retained;
    use objc2::runtime::NSObject;

    use super::{BrowserProxy, ProxyScheme};

    type CreateHost = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut NSObject;
    type CreateSocks5 = unsafe extern "C" fn(*mut NSObject) -> *mut NSObject;
    type CreateHttpConnect = unsafe extern "C" fn(*mut NSObject, *mut NSObject) -> *mut NSObject;
    type AddExcludedDomain = unsafe extern "C" fn(*mut NSObject, *const c_char);

    /// Connections to these hosts never go through the proxy: pages served
    /// from this machine (dev servers, codeg's own bridges) are the point of
    /// the built-in browser and must keep working whatever the proxy would do
    /// with them — the exception every browser and the `NO_PROXY` convention
    /// make. (A remote-egress profile will want the opposite; it gets its own
    /// configuration.)
    const EXCLUDED_DOMAINS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

    fn symbol(name: &str) -> Result<*mut c_void, String> {
        let c_name = CString::new(name).map_err(|e| e.to_string())?;
        // SAFETY: a valid C string; RTLD_DEFAULT searches every loaded image.
        let pointer = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c_name.as_ptr()) };
        if pointer.is_null() {
            Err(format!("{name} is not available on this macOS"))
        } else {
            Ok(pointer)
        }
    }

    pub fn proxy_config(proxy: &BrowserProxy) -> Result<Retained<NSObject>, String> {
        // SAFETY: the signatures are Network.framework's declared ones.
        let (create_host, create_socks5, create_http_connect, add_excluded_domain) = unsafe {
            (
                std::mem::transmute::<*mut c_void, CreateHost>(symbol("nw_endpoint_create_host")?),
                std::mem::transmute::<*mut c_void, CreateSocks5>(symbol("nw_proxy_config_create_socksv5")?),
                std::mem::transmute::<*mut c_void, CreateHttpConnect>(symbol("nw_proxy_config_create_http_connect")?),
                std::mem::transmute::<*mut c_void, AddExcludedDomain>(symbol("nw_proxy_config_add_excluded_domain")?),
            )
        };
        let host = CString::new(proxy.host.as_str()).map_err(|e| format!("proxy host: {e}"))?;
        let port = CString::new(proxy.port.to_string()).map_err(|e| format!("proxy port: {e}"))?;
        // SAFETY: valid C strings; every `create` follows the create rule
        // (+1), which `Retained::from_raw` takes over.
        unsafe {
            let endpoint = Retained::from_raw(create_host(host.as_ptr(), port.as_ptr()))
                .ok_or_else(|| format!("cannot describe proxy endpoint {}:{}", proxy.host, proxy.port))?;
            let endpoint_ptr = Retained::as_ptr(&endpoint) as *mut NSObject;
            let config = match proxy.scheme {
                ProxyScheme::Http => create_http_connect(endpoint_ptr, std::ptr::null_mut()),
                ProxyScheme::Socks5 => create_socks5(endpoint_ptr),
            };
            let config = Retained::from_raw(config).ok_or("cannot create proxy configuration")?;
            for domain in EXCLUDED_DOMAINS {
                let domain = CString::new(domain).expect("static");
                add_excluded_domain(Retained::as_ptr(&config) as *mut NSObject, domain.as_ptr());
            }
            Ok(config)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_app_origin;

    /// The per-record clear must spare every host the app's own webviews are
    /// served from and nothing else.
    #[test]
    fn app_origins_are_recognised_by_record_name() {
        assert!(is_app_origin("localhost"));
        assert!(is_app_origin("tauri.localhost"));
        assert!(!is_app_origin("example.com"));
        assert!(!is_app_origin("127.0.0.1"));
        assert!(!is_app_origin("localhost.example.com"));
    }
}
