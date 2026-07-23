use std::ptr::NonNull;

use objc2_app_kit::NSView;
use wry::raw_window_handle::{AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use wry::{Rect, WebView, WebViewBuilder};

use crate::RenderEngine;

/// Thin `HasWindowHandle` wrapper around a raw `NSView` pointer. Needed
/// because we create the window/view hierarchy directly via AppKit (no
/// `tao`/`winit`, which normally provide this impl themselves) — same
/// pattern as `windows.rs`'s `HwndHandle`.
struct NsViewHandle(NonNull<NSView>);

impl HasWindowHandle for NsViewHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(self.0.cast()));
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

pub struct WryEngine {
    webview: WebView,
}

impl WryEngine {
    /// `parent` is the `NSView` this webview is embedded into as a subview
    /// (typically the window's content view, below whatever native toolbar
    /// strip `browser-macos-appkit` places above it).
    pub fn new(
        parent: &NSView,
        initial_url: &str,
        on_title_changed: impl Fn(String) + 'static,
    ) -> anyhow::Result<Self> {
        let handle = NsViewHandle(NonNull::from(parent));
        let webview = WebViewBuilder::new()
            .with_url(initial_url)
            .with_document_title_changed_handler(move |title| on_title_changed(title))
            .build_as_child(&handle)?;
        Ok(Self { webview })
    }

    /// AppKit has no layout manager to resize the embedded webview the way
    /// GTK's box layout does on Linux — the app must call this whenever the
    /// window resizes, with the content area's new bounds (below the
    /// toolbar strip).
    pub fn set_bounds(&self, x: i32, y: i32, width: u32, height: u32) -> anyhow::Result<()> {
        self.webview.set_bounds(Rect {
            position: wry::dpi::Position::Physical(wry::dpi::PhysicalPosition::new(x, y)),
            size: wry::dpi::Size::Physical(wry::dpi::PhysicalSize::new(width, height)),
        })?;
        Ok(())
    }
}

impl RenderEngine for WryEngine {
    fn navigate(&self, url: &str) -> anyhow::Result<()> {
        self.webview.load_url(url)?;
        Ok(())
    }

    fn current_url(&self) -> anyhow::Result<String> {
        Ok(self.webview.url()?)
    }

    fn go_back(&self) -> anyhow::Result<()> {
        self.webview.evaluate_script("window.history.back()")?;
        Ok(())
    }

    fn go_forward(&self) -> anyhow::Result<()> {
        self.webview.evaluate_script("window.history.forward()")?;
        Ok(())
    }

    fn reload(&self) -> anyhow::Result<()> {
        self.webview.reload()?;
        Ok(())
    }

    // Not yet implemented for this backend — this crate is a fresh scaffold
    // (see ROADMAP.md); a real implementation would use WKWebView's
    // `takeSnapshotWithConfiguration:completionHandler:` via
    // `WebViewExtMacOS::webview()`, matching how `render-engine::linux` uses
    // WebKitGTK's `snapshot`.
    fn screenshot(&self, callback: Box<dyn Fn(anyhow::Result<Vec<u8>>)>) {
        callback(Err(anyhow::anyhow!("screenshot is not yet implemented on this platform")));
    }
}
