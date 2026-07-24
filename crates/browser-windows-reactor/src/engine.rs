//! `RenderEngine` impl backed by `windows-webview`'s `WebView`, so
//! `browser_core::PageManager<ReactorWebViewEngine>` can be reused as-is
//! (loaded/unloaded eviction, `matching_ids`, etc.) rather than
//! reinvented — unlike `render_engine::WebView2Engine` (the `winio-winui3`
//! equivalent), this doesn't live in the shared `render-engine` crate: it's
//! tightly coupled to `windows-reactor`'s `webview(on_ready)` element (the
//! `WebView` only exists once that callback fires, not at construction
//! time — see `Self::new`), and keeping it local avoids pulling
//! `windows-reactor`/`windows-webview`'s git dependency into
//! `browser-windows-winui`'s build via the shared crate.
use std::cell::RefCell;
use std::rc::Rc;

use render_engine::RenderEngine;
use windows_webview::{EventRegistration, WebView};

pub struct ReactorWebViewEngine {
    pub web: Rc<RefCell<Option<WebView>>>,
    /// Kept alive for as long as the engine exists — dropping an
    /// `EventRegistration` unsubscribes it immediately (see
    /// `windows-webview`'s doc comment on the type).
    pub registration: Rc<RefCell<Option<EventRegistration>>>,
}

impl ReactorWebViewEngine {
    /// The `WebView` doesn't exist yet — it's filled in later, once
    /// `webview(on_ready)`'s callback fires for whichever element ends up
    /// `.with_key()`d to this page's id (see `lib.rs`'s `page_element`).
    /// Every `RenderEngine` method below is a no-op/error until then.
    pub fn new() -> Self {
        Self { web: Rc::new(RefCell::new(None)), registration: Rc::new(RefCell::new(None)) }
    }
}

impl Default for ReactorWebViewEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderEngine for ReactorWebViewEngine {
    fn navigate(&self, url: &str) -> anyhow::Result<()> {
        match self.web.borrow().as_ref() {
            Some(w) => w.navigate(url).map_err(|err| anyhow::anyhow!("{err}")),
            // Real error, not `Ok(())` — a prior version of this code masked
            // "not ready yet" as success, which made it indistinguishable
            // from a genuine successful navigation in logs/traces. That
            // ambiguity got in the way of diagnosing a real user report of
            // pages never rendering: callers need to be able to tell "we
            // never actually asked the browser to navigate" apart from "we
            // did, and it should be showing something."
            None => anyhow::bail!("webview not ready yet"),
        }
    }
    fn go_back(&self) -> anyhow::Result<()> {
        match self.web.borrow().as_ref() {
            Some(w) => w.go_back().map_err(|err| anyhow::anyhow!("{err}")),
            None => anyhow::bail!("webview not ready yet"),
        }
    }
    fn go_forward(&self) -> anyhow::Result<()> {
        match self.web.borrow().as_ref() {
            Some(w) => w.go_forward().map_err(|err| anyhow::anyhow!("{err}")),
            None => anyhow::bail!("webview not ready yet"),
        }
    }
    fn reload(&self) -> anyhow::Result<()> {
        match self.web.borrow().as_ref() {
            Some(w) => w.reload().map_err(|err| anyhow::anyhow!("{err}")),
            None => anyhow::bail!("webview not ready yet"),
        }
    }
    fn current_url(&self) -> anyhow::Result<String> {
        match self.web.borrow().as_ref() {
            Some(w) => Ok(w.source()),
            None => anyhow::bail!("webview not ready yet"),
        }
    }
    /// `windows-webview` has no wrapped screenshot/capture-preview API yet
    /// (only a raw, unwrapped `CapturePreview` vtable slot in its
    /// `bindings.rs` — checked by reading the crate's source) — honestly
    /// unimplemented rather than faked, same as other real gaps documented
    /// elsewhere in this codebase.
    fn screenshot(&self, callback: Box<dyn Fn(anyhow::Result<Vec<u8>>)>) {
        callback(Err(anyhow::anyhow!("screenshot not yet implemented for windows-webview")));
    }
}
