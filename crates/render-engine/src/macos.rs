use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_web_kit::{WKWebView, WKWebViewConfiguration};
use wry::raw_window_handle::{AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use wry::{NewWindowResponse, Rect, WebContext, WebView, WebViewBuilder, WebViewBuilderExtMacos, WebViewExtMacOS};

use crate::{RenderEngine, CONSOLE_CAPTURE_SCRIPT, FILL_LOGIN_SCRIPT};

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
    /// strip `browser-macos-appkit` places above it). `web_context` is
    /// shared across every page in the same profile (see `wry::WebContext`'s
    /// own doc comment: "A browser would have a context for all the normal
    /// tabs and a different context for all the private/incognito tabs") —
    /// the caller owns it for the lifetime of the whole profile, not
    /// per-page, so cookies/localStorage/cache persist (and are shared
    /// between tabs) instead of each page silently getting its own
    /// throwaway context (`WebViewBuilder::new()`'s default when no context
    /// is supplied at all).
    /// `on_new_window_requested` fires for a `target="_blank"` link click, a
    /// `window.open()` call, or the engine's own default right-click "Open
    /// Link in New Window" context-menu item — all three route through
    /// `WKUIDelegate::createWebViewWithConfiguration`, so one handler covers
    /// all of them. Stays inside `wry`'s own `with_new_window_req_handler`
    /// hook here (unlike `render_engine::linux`, which bypasses the
    /// equivalent `wry` hook entirely): `WKUIDelegate` is a single-dispatch
    /// Objective-C delegate, not a multi-subscriber signal like GTK's, and
    /// `wry`'s own generated delegate class already implements this exact
    /// method (also handling file-upload panels and permission prompts in
    /// the same object) — replacing it would be a real regression for no
    /// benefit. **Real, honest gap, unlike `render_engine::linux`'s**: this
    /// means there's no way to tell a real click apart from an unprompted
    /// `window.open()` script call here at all (`wry`'s hook only ever
    /// exposes `(uri, NewWindowFeatures)`, and the real underlying
    /// `WKNavigationAction` has no `isUserInitiated`-equivalent property
    /// either, confirmed by reading `objc2-web-kit`'s bindings directly) —
    /// every request that reaches this handler is accepted.
    ///
    /// On accept, returns `Some(configuration)` — the exact
    /// `WKWebViewConfiguration` this app's own new-window closure should
    /// build the popup's `WryEngine` from via `new_related`, handing its
    /// raw `webview()` back to `wry` as `NewWindowResponse::Create`. This is
    /// the mechanism Apple's own docs specify for preserving the popup's
    /// real `window.opener`/`postMessage` relationship ("The web view
    /// returned must be created with the specified configuration... WebKit
    /// will load the request in the returned web view" — so `new_related`
    /// takes no `initial_url` either, same reasoning). Returning `None`
    /// denies the popup outright.
    pub fn new(
        parent: &NSView,
        initial_url: &str,
        web_context: &mut WebContext,
        on_title_changed: impl Fn(String) + 'static,
        on_new_window_requested: impl Fn(String, Retained<WKWebViewConfiguration>) -> Option<Retained<WKWebView>> + 'static,
        on_console_message: impl Fn(String) + 'static,
    ) -> anyhow::Result<Self> {
        let handle = NsViewHandle(NonNull::from(parent));
        let webview = WebViewBuilder::new_with_web_context(web_context)
            .with_url(initial_url)
            .with_document_title_changed_handler(move |title| on_title_changed(title))
            .with_new_window_req_handler(move |uri, features| {
                match on_new_window_requested(uri, features.opener.target_configuration) {
                    Some(webview) => NewWindowResponse::Create { webview },
                    None => NewWindowResponse::Deny,
                }
            })
            .with_initialization_script(CONSOLE_CAPTURE_SCRIPT)
            .with_ipc_handler(move |request| on_console_message(request.body().clone()))
            .build_as_child(&handle)?;
        Ok(Self { webview })
    }

    /// Same as `new`, but for a page opened via `window.open()`/
    /// `target="_blank"`/"open in new tab" that should preserve a real
    /// opener relationship — see `new`'s `on_new_window_requested` doc
    /// comment for where `target_configuration` comes from and why there's
    /// no `initial_url` parameter here (WebKit performs the navigation
    /// itself once this page's raw webview is handed back to it).
    pub fn new_related(
        parent: &NSView,
        target_configuration: Retained<WKWebViewConfiguration>,
        on_title_changed: impl Fn(String) + 'static,
        on_new_window_requested: impl Fn(String, Retained<WKWebViewConfiguration>) -> Option<Retained<WKWebView>> + 'static,
        on_console_message: impl Fn(String) + 'static,
    ) -> anyhow::Result<Self> {
        let handle = NsViewHandle(NonNull::from(parent));
        let webview = WebViewBuilder::new()
            .with_webview_configuration(target_configuration)
            .with_document_title_changed_handler(move |title| on_title_changed(title))
            .with_new_window_req_handler(move |uri, features| {
                match on_new_window_requested(uri, features.opener.target_configuration) {
                    Some(webview) => NewWindowResponse::Create { webview },
                    None => NewWindowResponse::Deny,
                }
            })
            .with_initialization_script(CONSOLE_CAPTURE_SCRIPT)
            .with_ipc_handler(move |request| on_console_message(request.body().clone()))
            .build_as_child(&handle)?;
        Ok(Self { webview })
    }

    /// The webview's own raw `WKWebView` — lets a caller return the result
    /// of `new_related` from `on_new_window_requested` (via
    /// `NewWindowResponse::Create`) without needing to know about `wry`'s
    /// own `WryWebView` subclass type.
    pub fn raw_webview(&self) -> Retained<WKWebView> {
        (&self.webview.webview()).into()
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

    /// Test-only escape hatch for running arbitrary script against the
    /// active page — see `browser-macos-appkit`'s `_for_test`-suffixed
    /// accessor convention (`browser-linux-gtk3`'s own
    /// `evaluate_script_on_active_page_for_test` established the same
    /// naming pattern first).
    pub fn evaluate_script_for_test(&self, script: &str) -> anyhow::Result<()> {
        self.webview.evaluate_script(script)?;
        Ok(())
    }
}

impl RenderEngine for WryEngine {
    fn navigate(&self, url: &str) -> anyhow::Result<()> {
        self.webview.load_url(url)?;
        Ok(())
    }

    /// Deliberately bypasses `wry::WebView::url()` (which calls this crate's
    /// own pinned `url_from_webview`, `wkwebview/mod.rs`) — that helper does
    /// `webview.URL().unwrap()`, and `WKWebView.URL` is genuinely `nil`
    /// (Apple's own documented behavior) until a navigation actually starts,
    /// which a webview freshly (re)created by `ensure_engine_loaded` for a
    /// previously-unloaded page hasn't necessarily done yet by the time
    /// something reads its URL in the same call stack — confirmed directly,
    /// not theoretical: reproduced a real crash this way (switching to an
    /// evicted page via the switcher's keyboard path, which reads the newly
    /// active page's URL from inside `close_all_overlays` right after
    /// `ensure_engine_loaded` recreates its engine). `objc2_web_kit`'s own
    /// binding already types `URL()` as `Option`, so going straight through
    /// it sidesteps wry's `unwrap` entirely instead of needing to patch (or
    /// fork further) the pinned git dependency.
    fn current_url(&self) -> anyhow::Result<String> {
        let url = unsafe { self.raw_webview().URL() }.and_then(|u| u.absoluteString());
        match url {
            Some(url) => Ok(url.to_string()),
            None => Err(anyhow::anyhow!("webview has no URL yet (no navigation started)")),
        }
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
