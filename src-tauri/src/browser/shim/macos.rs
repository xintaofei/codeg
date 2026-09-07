//! macOS shim on top of `WKWebView` / `WKUserContentController` (objc2).
//!
//! The helper script and the `codegBrowser` message handler live in the
//! `WKContentWorld` named `codeg`: a separate JavaScript global for the same
//! DOM, invisible to page scripts and immune to their prototype tampering.
//! `WKContentWorld` needs macOS 11; older systems fall back to the page world
//! (reported as `ChannelKind::Legacy`).

use std::cell::RefCell;
use std::collections::HashSet;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, DeclaredClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
use objc2_foundation::{ns_string, NSArray, NSDate, NSDictionary, NSError, NSProcessInfo, NSString, NSUUID};
use objc2_web_kit::{
    WKContentWorld, WKScriptMessage, WKScriptMessageHandler, WKSnapshotConfiguration,
    WKUserContentController, WKUserScript, WKUserScriptInjectionTime, WKWebViewConfiguration,
    WKWebsiteDataRecord, WKWebsiteDataStore,
};
use tauri_runtime_wry::wry::{self, WebViewExtMacOS};

use super::super::channel::MessageSink;
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
