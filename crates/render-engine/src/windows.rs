use std::num::NonZeroIsize;

use windows::Win32::Foundation::HWND;
use wry::dpi::{PhysicalPosition, PhysicalSize, Position, Size};
use wry::raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use wry::{Rect, WebView, WebViewBuilder};

use crate::RenderEngine;

/// Thin `HasWindowHandle` wrapper around a raw `HWND`. Needed because we
/// create the window directly via Win32 (no `tao`/`winit`, which normally
/// provide this impl themselves).
struct HwndHandle(HWND);

impl HasWindowHandle for HwndHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0 .0 as isize).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

pub struct WryEngine {
    webview: WebView,
}

impl WryEngine {
    pub fn new(
        parent: HWND,
        initial_url: &str,
        on_title_changed: impl Fn(String) + 'static,
    ) -> anyhow::Result<Self> {
        let handle = HwndHandle(parent);
        let webview = WebViewBuilder::new()
            .with_url(initial_url)
            .with_document_title_changed_handler(move |title| on_title_changed(title))
            .build_as_child(&handle)?;
        Ok(Self { webview })
    }

    /// Win32 has no layout manager to resize the embedded webview the way
    /// GTK's box layout does on Linux — the app must call this from its
    /// `WM_SIZE` handler with the content area's new bounds.
    pub fn set_bounds(&self, x: i32, y: i32, width: u32, height: u32) -> anyhow::Result<()> {
        self.webview.set_bounds(Rect {
            position: Position::Physical(PhysicalPosition::new(x, y)),
            size: Size::Physical(PhysicalSize::new(width, height)),
        })?;
        Ok(())
    }

    /// Win32 has no `Stack`-like widget to show only the active page's
    /// webview the way GTK's does on Linux — every page's webview is a
    /// sibling child window of the same parent, so exactly one must be
    /// shown (and the rest hidden) by hand whenever the active page changes.
    pub fn set_visible(&self, visible: bool) -> anyhow::Result<()> {
        self.webview.set_visible(visible)?;
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
}
