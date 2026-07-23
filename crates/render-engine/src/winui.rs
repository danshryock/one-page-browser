//! WinUI 3 backend: unlike `linux.rs`/`windows.rs` (both wry-based), this
//! wraps the native `Microsoft.UI.Xaml.Controls.WebView2` XAML control
//! directly — no wry involved at all. It's a real `FrameworkElement`, so it
//! participates in ordinary Grid/StackPanel layout like any other XAML
//! control: no manual `WM_SIZE` → `set_bounds` plumbing is needed the way
//! `windows.rs`'s HWND-child approach requires.

use std::cell::Cell;
use std::rc::Rc;

use windows::Foundation::{TypedEventHandler, Uri};
use windows_core::HSTRING;
use winui3::Microsoft::UI::Xaml::Controls::{Grid, WebView2};
use winui3::Microsoft::Web::WebView2::Core::CoreWebView2;

use crate::RenderEngine;

/// Every WinRT delegate constructor in this app (`TypedEventHandler::new`,
/// `RoutedEventHandler::new`, etc.) requires `F: ... + Send`, even though
/// this whole app is genuinely single-threaded — one STA apartment (see
/// `init_apartment(ApartmentType::SingleThreaded)` in `browser-windows-winui`),
/// and every delegate here is only ever invoked from that same thread. WinRT's
/// type system has no way to know that. Wraps a non-`Send` closure (typically
/// one capturing `Rc`/`RefCell` state, this codebase's usual pattern) so it
/// can be handed to those constructors — sound only because nothing in this
/// app ever calls it from a second thread.
pub struct AssertSend<T>(pub T);
unsafe impl<T> Send for AssertSend<T> {}

pub struct WebView2Engine {
    control: WebView2,
}

impl WebView2Engine {
    /// `parent` must already be in the live visual tree (or about to be) —
    /// the control is appended to its `Children` collection here.
    pub fn new(
        parent: &Grid,
        initial_url: &str,
        on_title_changed: impl Fn(String) + 'static,
    ) -> anyhow::Result<Self> {
        let control = WebView2::new()?;
        parent.Children()?.Append(&control)?;

        // `WebView2` has no public `CoreWebView2Initialized` adder in this
        // crate's bindings (only `RemoveCoreWebView2Initialized` exists —
        // the delegate type it needs was never generated). `NavigationCompleted`
        // *is* public, and by the time it fires `CoreWebView2()` is
        // guaranteed to exist, so title tracking is wired lazily from there
        // instead. Guarded so the `DocumentTitleChanged` subscription only
        // happens once, since `NavigationCompleted` fires on every navigation.
        let on_title_changed: Rc<dyn Fn(String)> = Rc::new(on_title_changed);
        let title_wired = Rc::new(Cell::new(false));
        let state = AssertSend((on_title_changed, title_wired));
        control.NavigationCompleted(&TypedEventHandler::new(move |sender: windows_core::Ref<'_, WebView2>, _args| {
            // Forces whole-value capture of `state` (rather than Rust 2021's
            // disjoint capture grabbing just `state.0`, which would silently
            // defeat `AssertSend`'s Send-ness — it only covers the wrapper,
            // not whatever field access bypasses it).
            let state = &state;
            let (on_title_changed, title_wired) = &state.0;
            let Some(webview) = sender.as_ref() else { return Ok(()) };
            let Ok(core) = webview.CoreWebView2() else { return Ok(()) };

            if !title_wired.get() {
                title_wired.set(true);
                let inner_state = AssertSend(Rc::clone(on_title_changed));
                core.DocumentTitleChanged(&TypedEventHandler::new(move |sender: windows_core::Ref<'_, CoreWebView2>, _args| {
                    let inner_state = &inner_state;
                    let on_title_changed = &inner_state.0;
                    if let Some(core) = sender.as_ref() {
                        if let Ok(title) = core.DocumentTitle() {
                            on_title_changed(title.to_string());
                        }
                    }
                    Ok(())
                }))?;
            }
            if let Ok(title) = core.DocumentTitle() {
                on_title_changed(title.to_string());
            }
            Ok(())
        }))?;

        control.SetSource(&Uri::CreateUri(&HSTRING::from(initial_url))?)?;
        Ok(Self { control })
    }

    /// Removes the control from `parent`'s `Children` collection and closes
    /// its `CoreWebView2` to actually reclaim resources. Unlike `WryEngine`
    /// (dropping it destroys the underlying widget via wry's own `Drop`),
    /// `WebView2` is a ref-counted WinRT object still referenced by its
    /// parent's `Children` collection — dropping this wrapper alone
    /// wouldn't free anything. Must be called explicitly before dropping.
    pub fn close(&self, parent: &Grid) -> anyhow::Result<()> {
        let children = parent.Children()?;
        let mut index = 0u32;
        if children.IndexOf(&self.control, &mut index)? {
            children.RemoveAt(index)?;
        }
        self.control.Close()?;
        Ok(())
    }
}

impl RenderEngine for WebView2Engine {
    fn navigate(&self, url: &str) -> anyhow::Result<()> {
        self.control.SetSource(&Uri::CreateUri(&HSTRING::from(url))?)?;
        Ok(())
    }

    fn current_url(&self) -> anyhow::Result<String> {
        Ok(self.control.Source()?.AbsoluteUri()?.to_string())
    }

    fn go_back(&self) -> anyhow::Result<()> {
        self.control.GoBack()?;
        Ok(())
    }

    fn go_forward(&self) -> anyhow::Result<()> {
        self.control.GoForward()?;
        Ok(())
    }

    fn reload(&self) -> anyhow::Result<()> {
        self.control.Reload()?;
        Ok(())
    }

    // Not yet implemented for this backend — landed for
    // browser-linux-gtk3/render-engine::linux first (see ROADMAP.md).
    // WebView2 does have a real async capture API of its own
    // (`CoreWebView2.CapturePreview`) that a future pass could wire up here.
    fn screenshot(&self, callback: Box<dyn Fn(anyhow::Result<Vec<u8>>)>) {
        callback(Err(anyhow::anyhow!("screenshot is not yet implemented on this platform")));
    }
}
