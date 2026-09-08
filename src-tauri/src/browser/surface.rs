//! The concrete surfaces behind a tab and the operations the command layer
//! needs from all of them. Both handle types are cheap `Send + Clone` values
//! that dispatch to the main thread internally, so callers clone a surface OUT
//! of the registry and operate on it with no lock held — holding the registry
//! mutex across a main-thread round trip would deadlock the moment the main
//! thread wants the registry too.

use tauri::Url;

use super::types::{Bounds, SurfaceKind};

#[cfg(all(
    feature = "browser-child",
    any(target_os = "macos", target_os = "windows")
))]
use super::surface_child::ChildHandle;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SurfaceError(pub String);

impl From<tauri::Error> for SurfaceError {
    fn from(err: tauri::Error) -> Self {
        SurfaceError(err.to_string())
    }
}

#[cfg(all(
    feature = "browser-child",
    any(target_os = "macos", target_os = "windows")
))]
impl From<super::surface_child::ChildError> for SurfaceError {
    fn from(err: super::surface_child::ChildError) -> Self {
        SurfaceError(err.to_string())
    }
}

#[derive(Clone)]
pub enum BrowserSurface {
    #[cfg(all(
        feature = "browser-child",
        any(target_os = "macos", target_os = "windows")
    ))]
    Child(ChildHandle),
    /// Boxed: `WebviewWindow` is ~900 bytes and the enum is cloned around.
    Window(Box<tauri::WebviewWindow>),
}

// The child arm is compiled out on Linux; the macro keeps every method to one
// match instead of three cfg-laden copies.
macro_rules! per_surface {
    ($self:ident, child: |$c:ident| $child:expr, window: |$w:ident| $window:expr) => {
        match $self {
            #[cfg(all(
                feature = "browser-child",
                any(target_os = "macos", target_os = "windows")
            ))]
            BrowserSurface::Child($c) => $child,
            BrowserSurface::Window($w) => $window,
        }
    };
}

impl BrowserSurface {
    pub fn kind(&self) -> SurfaceKind {
        per_surface!(self, child: |_c| SurfaceKind::Child, window: |_w| SurfaceKind::Window)
    }

    pub fn is_embedded(&self) -> bool {
        self.kind() != SurfaceKind::Window
    }

    pub fn label(&self) -> &str {
        per_surface!(self, child: |c| c.label(), window: |w| w.label())
    }

    pub fn navigate(&self, url: Url) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.load_url(url.as_str())?),
            window: |w| Ok(w.navigate(url)?))
    }

    pub fn reload(&self) -> Result<(), SurfaceError> {
        per_surface!(self, child: |c| Ok(c.reload()?), window: |w| Ok(w.reload()?))
    }

    /// Dev puppet only. Owned windows are deliberately not asked: wry 0.55's
    /// `url()` unwraps `WKWebView.URL`, which is nil until a navigation has
    /// committed (or after the first one failed), and that unwrap panics the
    /// main thread. The registry state carries the URL either way.
    pub fn url(&self) -> Result<Url, SurfaceError> {
        per_surface!(self,
            child: |c| Url::parse(&c.url()?).map_err(|e| SurfaceError(e.to_string())),
            window: |_w| Err(SurfaceError("owned windows report their URL through the tab state".into())))
    }

    pub fn eval(&self, js: &str) -> Result<(), SurfaceError> {
        per_surface!(self, child: |c| Ok(c.evaluate_script(js)?), window: |w| Ok(w.eval(js)?))
    }

    pub fn eval_with_callback(
        &self,
        js: &str,
        callback: impl Fn(String) + Send + 'static,
    ) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.evaluate_script_with_callback(js, callback)?),
            window: |w| Ok(w.eval_with_callback(js, callback)?))
    }

    /// Find in page. Only embedded surfaces have it: an owned window is a
    /// plain `WebviewWindow`, whose `WKWebView` the host does not hold.
    pub fn find(
        &self,
        query: &str,
        forward: bool,
        callback: impl Fn(bool) + Send + 'static,
    ) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.find(query, forward, callback)?),
            window: |_w| {
                let _ = (query, forward, callback);
                Err(SurfaceError("find in page needs an embedded surface".into()))
            })
    }

    pub fn clear_find(&self) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.clear_find()?),
            window: |_w| Err(SurfaceError("find in page needs an embedded surface".into())))
    }

    pub fn hide(&self) -> Result<(), SurfaceError> {
        per_surface!(self, child: |c| Ok(c.set_visible(false)?), window: |w| Ok(w.hide()?))
    }

    pub fn show(&self) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.set_visible(true)?),
            // Un-hiding alone can leave the window behind its owner; showing
            // it is a request to see it.
            window: |w| { w.show()?; w.set_focus()?; Ok(()) })
    }

    pub fn set_focus(&self) -> Result<(), SurfaceError> {
        per_surface!(self, child: |c| Ok(c.focus()?), window: |w| Ok(w.set_focus()?))
    }

    pub fn close(&self) -> Result<(), SurfaceError> {
        per_surface!(self, child: |c| Ok(c.close()?), window: |w| Ok(w.close()?))
    }

    /// Only meaningful for embedded surfaces; an owned window keeps whatever
    /// size and position the user gave it.
    pub fn set_bounds(&self, bounds: Bounds) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.set_bounds(bounds)?),
            window: |_w| { let _ = bounds; Ok(()) })
    }

    pub fn install_channel(&self) -> Result<bool, SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.install_channel()?),
            window: |_w| Err(SurfaceError("page channel for owned windows lands with the platform shims".into())))
    }

    pub fn eval_in_world(
        &self,
        expression: &str,
        callback: impl Fn(Result<String, String>) + Send + 'static,
    ) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.eval_in_world(expression, callback)?),
            window: |_w| { let _ = (expression, callback); Err(SurfaceError("world evaluation for owned windows lands with the platform shims".into())) })
    }

    pub fn snapshot_png(
        &self,
        callback: impl Fn(Result<Vec<u8>, String>) + Send + 'static,
    ) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.snapshot_png(callback)?),
            window: |_w| { let _ = callback; Err(SurfaceError("snapshots for owned windows land with the platform shims".into())) })
    }

    /// The frame as displayed now, JPEG-encoded (for the freeze frame shown
    /// while a surface is hidden under an overlay). Embedded surfaces only.
    pub fn snapshot_jpeg(
        &self,
        quality: f64,
        callback: impl Fn(Result<(Vec<u8>, u32, u32), String>) + Send + 'static,
    ) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.snapshot_jpeg(quality, callback)?),
            window: |_w| { let _ = (quality, callback); Err(SurfaceError("snapshots for owned windows land with the platform shims".into())) })
    }

    pub fn go_back(&self) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.go_back()?),
            window: |_w| Err(SurfaceError("history navigation for owned windows lands with the platform shims".into())))
    }

    pub fn go_forward(&self) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.go_forward()?),
            window: |_w| Err(SurfaceError("history navigation for owned windows lands with the platform shims".into())))
    }

    /// Engine-side "still loading", used to notice failed navigations (wry
    /// reports no failure event). Owned windows: not yet.
    pub fn is_loading(&self) -> Result<bool, SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.is_loading()?),
            window: |_w| Err(SurfaceError("load state for owned windows lands with the platform shims".into())))
    }

    pub fn can_go_back(&self) -> Result<bool, SurfaceError> {
        per_surface!(self, child: |c| Ok(c.can_go_back()?), window: |_w| Ok(false))
    }

    pub fn can_go_forward(&self) -> Result<bool, SurfaceError> {
        per_surface!(self, child: |c| Ok(c.can_go_forward()?), window: |_w| Ok(false))
    }

    pub fn stop(&self) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.stop()?),
            window: |_w| Err(SurfaceError("stop for owned windows lands with the platform shims".into())))
    }

    /// Dev puppet only.
    pub fn debug_view(&self) -> Result<serde_json::Value, SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.debug_view()?),
            window: |w| Ok(serde_json::json!({ "visible": w.is_visible().ok() })))
    }

    pub fn set_zoom(&self, factor: f64) -> Result<(), SurfaceError> {
        per_surface!(self, child: |c| Ok(c.zoom(factor)?), window: |w| Ok(w.set_zoom(factor)?))
    }

    /// Wipe cookies, caches and storage. Every tab shares one data store, so
    /// clearing through any surface clears them all.
    pub fn clear_browsing_data(&self) -> Result<(), SurfaceError> {
        per_surface!(self,
            child: |c| Ok(c.clear_all_browsing_data()?),
            window: |w| Ok(w.clear_all_browsing_data()?))
    }
}
