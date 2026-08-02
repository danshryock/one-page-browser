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

    /// Fills the page's login form with `username`/`password` — identical
    /// to `render_engine::linux::WryEngine::fill_login` (see its doc comment
    /// for the full heuristic/security rationale, including the
    /// `autocomplete`-attribute-first field detection), since this is the
    /// same underlying `wry::WebView` type. Not yet called from anywhere —
    /// `browser-macos-appkit` has no password-manager UI of its own yet
    /// (see `ROADMAP.md`); added here so the capability exists once it does,
    /// rather than needing another `render-engine` change at that point.
    pub fn fill_login(&self, username: &str, password: &str) -> anyhow::Result<()> {
        let username_json = serde_json::to_string(username)?;
        let password_json = serde_json::to_string(password)?;
        let script = FILL_LOGIN_SCRIPT
            .replace("\"__USERNAME__\"", &username_json)
            .replace("\"__PASSWORD__\"", &password_json);
        self.webview.evaluate_script(&script)?;
        Ok(())
    }
}

/// Finds the password field (preferring `autocomplete="current-password"`,
/// falling back to the page's first `input[type="password"]`) and, if a
/// non-empty username was given, the identifier field (preferring
/// `autocomplete="username"`/`"email"`, falling back to positional
/// proximity) — see `render_engine::linux`'s identical constant for the
/// full rationale (same script, kept as a separate copy since these are two
/// separate files, not a shared module, mirroring how `go_back`/
/// `go_forward`/etc. are already duplicated between them rather than
/// factored out).
const FILL_LOGIN_SCRIPT: &str = r#"
(function () {
  var password = document.querySelector('input[autocomplete="current-password"]') ||
                  document.querySelector('input[type="password"]');
  if (!password) return;

  function setNativeValue(el, value) {
    var proto = Object.getPrototypeOf(el);
    var setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
    setter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  }

  var username = "__USERNAME__";
  if (username.length > 0) {
    var form = password.closest('form');
    var scope = form || document;
    var usernameField = scope.querySelector('input[autocomplete="username"], input[autocomplete="email"]');
    if (!usernameField) {
      var candidates = scope.querySelectorAll('input');
      for (var i = 0; i < candidates.length; i++) {
        var el = candidates[i];
        if (el === password) break;
        var type = (el.getAttribute('type') || 'text').toLowerCase();
        if (type === 'text' || type === 'email' || type === 'tel') {
          usernameField = el;
        }
      }
    }
    if (usernameField) setNativeValue(usernameField, username);
  }

  setNativeValue(password, "__PASSWORD__");
})();
"#;

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
