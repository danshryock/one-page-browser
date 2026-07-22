//! Abstraction over the platform's native webview so the rest of the
//! app (chrome, history) never depends on `wry` directly. This is
//! what lets an alternative engine (Servo, CEF, custom) be swapped in later.

pub trait RenderEngine {
    fn navigate(&self, url: &str) -> anyhow::Result<()>;
    fn go_back(&self) -> anyhow::Result<()>;
    fn go_forward(&self) -> anyhow::Result<()>;
    fn reload(&self) -> anyhow::Result<()>;
    fn current_url(&self) -> anyhow::Result<String>;
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::WryEngine;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::WryEngine;
