//! `RenderEngine` impl wrapping wxDragon's built-in `wxWebView` widget.
//!
//! Unlike every other front end in this workspace, this doesn't touch `wry`
//! at all: wxWebView already wraps the OS's native webview itself (WebView2
//! on Windows, WebKitGTK on Linux), through wxWidgets' own C++ layer instead
//! of wry's. `render-engine` is depended on only for the `RenderEngine` trait
//! definition — this is a wholly separate implementation, local to this
//! crate rather than added to `render-engine`, since wxWebView is one API
//! across every OS wxDragon supports and doesn't fit `render-engine`'s
//! existing per-`target_os` module split.

use std::cell::RefCell;
use std::rc::Rc;

use render_engine::RenderEngine;
use wxdragon::event::WebViewEvents;
use wxdragon::sizers::{BoxSizer, Orientation, SizerFlag};
use wxdragon::widgets::panel::Panel;
use wxdragon::widgets::webview::{WebView, WebViewBackend, WebViewReloadFlags};
use wxdragon::window::WxWidget;

pub struct WxEngine {
    webview: WebView,
    /// Tracked in Rust state rather than always calling
    /// `webview.get_current_url()` live: that call segfaults when made on a
    /// webview that was constructed moments earlier in the same call stack
    /// (confirmed by reproduction — the underlying native widget isn't fully
    /// realized yet, the same class of "freshly-built webview isn't ready"
    /// issue `render-engine`'s GTK/wry backend works around by pumping the
    /// GTK event loop once; wxdragon exposes no equivalent pump). Updated
    /// eagerly on construction/`navigate()`, then corrected once the
    /// `Navigated` event fires with the real post-navigation URL.
    current_url: Rc<RefCell<String>>,
}

impl WxEngine {
    pub fn new(
        parent: &Panel,
        initial_url: &str,
        on_title_changed: impl Fn(String) + 'static,
    ) -> anyhow::Result<Self> {
        // Prefer Edge (WebView2/Chromium) when available. Matters on
        // Windows, where the fallback IE/Trident backend can't render
        // modern pages; a no-op preference on Linux, where WebKitGTK is the
        // only backend wx offers anyway.
        let backend =
            if WebView::is_backend_available(WebViewBackend::Edge) { WebViewBackend::Edge } else { WebViewBackend::Default };

        let webview = WebView::builder(parent).with_url(Some(initial_url.to_string())).with_backend(backend).build();

        // The container `Panel` has no layout of its own otherwise, so the
        // webview would sit at its default (small, fixed) size rather than
        // filling it.
        let sizer = BoxSizer::builder(Orientation::Vertical).build();
        sizer.add(&webview, 1, SizerFlag::Expand, 0);
        parent.set_sizer(sizer, true);

        webview.on_title_changed(move |event| {
            if let Some(title) = event.get_string() {
                on_title_changed(title);
            }
        });

        let current_url = Rc::new(RefCell::new(initial_url.to_string()));
        {
            let current_url = Rc::clone(&current_url);
            webview.on_navigated(move |event| {
                if let Some(url) = event.get_string() {
                    *current_url.borrow_mut() = url;
                }
            });
        }

        Ok(Self { webview, current_url })
    }
}

impl RenderEngine for WxEngine {
    fn navigate(&self, url: &str) -> anyhow::Result<()> {
        self.webview.load_url(url);
        *self.current_url.borrow_mut() = url.to_string();
        Ok(())
    }

    fn current_url(&self) -> anyhow::Result<String> {
        Ok(self.current_url.borrow().clone())
    }

    fn go_back(&self) -> anyhow::Result<()> {
        self.webview.go_back();
        Ok(())
    }

    fn go_forward(&self) -> anyhow::Result<()> {
        self.webview.go_forward();
        Ok(())
    }

    fn reload(&self) -> anyhow::Result<()> {
        self.webview.reload(WebViewReloadFlags::Default);
        Ok(())
    }

    // Not yet implemented for this backend — browser-wx is unmaintained
    // (see ROADMAP.md); landed for browser-linux-gtk3/render-engine::linux
    // first. wxWebView has no built-in screenshot call of its own either,
    // so this would need a widget-level Cairo/GDI capture.
    fn screenshot(&self, callback: Box<dyn Fn(anyhow::Result<Vec<u8>>)>) {
        callback(Err(anyhow::anyhow!("screenshot is not yet implemented on this platform")));
    }
}
