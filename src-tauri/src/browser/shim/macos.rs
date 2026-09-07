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
use objc2_foundation::{ns_string, NSDictionary, NSError, NSString};
use objc2_web_kit::{
    WKContentWorld, WKScriptMessage, WKScriptMessageHandler, WKSnapshotConfiguration,
    WKUserContentController, WKUserScript, WKUserScriptInjectionTime,
};
use tauri_runtime_wry::wry::{self, WebViewExtMacOS};

use super::super::channel::MessageSink;

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
pub fn webview_pointer(webview: &wry::WebView) -> usize {
    Retained::as_ptr(&webview.webview()) as usize
}
