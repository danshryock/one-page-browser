//! Abstraction over the platform's native webview so the rest of the
//! app (chrome, history) never depends on `wry` directly. This is
//! what lets an alternative engine (Servo, CEF, custom) be swapped in later.

pub trait RenderEngine {
    fn navigate(&self, url: &str) -> anyhow::Result<()>;
    fn go_back(&self) -> anyhow::Result<()>;
    fn go_forward(&self) -> anyhow::Result<()>;
    fn reload(&self) -> anyhow::Result<()>;
    fn current_url(&self) -> anyhow::Result<String>;
    /// Captures the current page as PNG-encoded image bytes, delivered to
    /// `callback` — every platform's native screenshot capability is
    /// async/callback-based (WebKitGTK's `snapshot`, WebView2's
    /// `CapturePreview`), not synchronous, so this stays async here too
    /// rather than forcing callers to block on it.
    fn screenshot(&self, callback: Box<dyn Fn(anyhow::Result<Vec<u8>>)>);
}

/// A generic `Send`-asserting wrapper — for a value that's provably only
/// ever touched from one thread (a single-threaded UI event loop), but
/// needs to satisfy a `Send` bound some framework imposes regardless (e.g.
/// `windows-reactor`'s `App::render`, even though the whole app is one STA
/// UI thread — see `browser-windows-reactor`'s own use of this). Not tied
/// to any one platform's engine — kept here, ungated, since more than one
/// frontend needs the exact same escape hatch.
pub struct AssertSend<T>(pub T);
unsafe impl<T> Send for AssertSend<T> {}

mod scripts;
pub use scripts::{CONSOLE_CAPTURE_SCRIPT, FILL_LOGIN_SCRIPT};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{NewWindowInfo, WryEngine};

/// Re-exported the same way `WebContext` is below — a page's own raw
/// `webkit2gtk::WebView`, needed as the `related_to` argument for
/// `WryEngine::new_related` (see `linux::WryEngine::new`'s
/// `on_new_window_requested` doc comment: the callback receives one of
/// these for exactly this purpose), without the caller needing its own
/// direct `webkit2gtk` dependency.
#[cfg(target_os = "linux")]
pub use webkit2gtk::WebView as WebKitWebView;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::WryEngine;

/// Re-exported the same way `WebContext`/`WebKitWebView` are — the real
/// `WKWebView`/`WKWebViewConfiguration` types needed for
/// `WryEngine::new_related` (see `macos::WryEngine::new`'s
/// `on_new_window_requested` doc comment), without the caller needing its
/// own direct `objc2-web-kit` dependency.
#[cfg(target_os = "macos")]
pub use objc2_web_kit::{WKWebView, WKWebViewConfiguration};

/// Re-exported rather than left implicit so callers can hold one
/// `WebContext` per profile (shared across every page it opens — see
/// `WryEngine::new`'s doc comment) without depending on `wry` directly,
/// preserving the "never depends on `wry` directly" boundary this module
/// doc comment describes — it's still literally `wry`'s type, just named
/// through here the same way `WryEngine` itself is.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use wry::WebContext;
