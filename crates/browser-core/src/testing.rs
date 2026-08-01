//! A trivial `RenderEngine` for testing page/tab-management logic without
//! any real webview/GTK/AppKit/WinUI involved — used by `browser-core`'s own
//! `PageManager` tests, and exposed here (not `#[cfg(test)]`-gated, unlike a
//! same-crate test helper) so other crates' tests can depend on it too:
//! `#[cfg(test)]` only applies within the crate that declares it, not to a
//! downstream crate importing it as a dependency, so a genuinely-shared mock
//! has to live in an always-compiled module like this one. Small enough
//! (under 40 lines) that always compiling it is not worth gating behind a
//! Cargo feature.

use std::cell::RefCell;

use render_engine::RenderEngine;

pub struct MockEngine {
    url: RefCell<String>,
}

impl MockEngine {
    pub fn new(url: &str) -> Self {
        Self { url: RefCell::new(url.to_string()) }
    }
}

impl RenderEngine for MockEngine {
    fn navigate(&self, url: &str) -> anyhow::Result<()> {
        *self.url.borrow_mut() = url.to_string();
        Ok(())
    }
    fn current_url(&self) -> anyhow::Result<String> {
        Ok(self.url.borrow().clone())
    }
    fn go_back(&self) -> anyhow::Result<()> {
        Ok(())
    }
    fn go_forward(&self) -> anyhow::Result<()> {
        Ok(())
    }
    fn reload(&self) -> anyhow::Result<()> {
        Ok(())
    }
    fn screenshot(&self, callback: Box<dyn Fn(anyhow::Result<Vec<u8>>)>) {
        callback(Ok(Vec::new()));
    }
}
