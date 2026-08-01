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

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::WryEngine;

#[cfg(all(target_os = "windows", target_env = "msvc"))]
mod winui;
#[cfg(all(target_os = "windows", target_env = "msvc"))]
pub use winui::{AssertSend, WebView2Engine};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::WryEngine;
