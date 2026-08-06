//! A minimal, hand-written COM/WinRT shim for one specific gap: setting
//! `Microsoft.UI.Xaml.UIElement.Visibility` on a native XAML element handle.
//!
//! `windows-reactor`'s own declarative `Element`/`ElementExt` has no
//! `Visibility` modifier at all (checked directly in the vendored source),
//! and that gap matters more than it looks for `WebView2` specifically:
//! Microsoft's real implementation
//! (github.com/microsoft/microsoft-ui-xaml, `controls/dev/WebView2/
//! WebView2.cpp`) registers a property-changed callback on exactly
//! `UIElement::VisibilityProperty()` and forwards it to the underlying
//! `ICoreWebView2Controller::IsVisible` for us — the same real hide/show
//! primitive `browser-linux-gtk3`'s `gtk::Stack` and `browser-macos-appkit`'s
//! hidden `NSView` already give those two front ends. `Opacity` (which
//! `ElementExt` *does* expose) is not wired to anything there — confirmed by
//! reading that same source, and empirically: setting it had no visible
//! effect in real-VM testing.
//!
//! `windows-reactor`'s own vendored `bindings.rs` (`crates/libs/reactor/src/
//! bindings.rs`) already contains a complete, correctly-generated
//! `IUIElement` binding with a `SetVisibility` vtable slot at the right
//! offset — it's just `pub(crate)`-scoped there, unreachable from outside
//! that crate. Since a WinRT interface is just a stable IID plus a fixed
//! vtable layout, redeclaring the same real interface here (same IID,
//! same field order up through `SetVisibility`, copied directly from that
//! vendored definition) is a legitimate, narrow way to reach it — not a
//! workaround for a bug, just calling a real API `windows-reactor` doesn't
//! happen to expose. Every field before `SetVisibility` is left as an
//! untyped `usize` placeholder (same width as a real vtable slot, just not
//! callable) since nothing here calls them; only the exact byte offset of
//! `SetVisibility` matters.
//!
//! `Visibility` itself is a WinRT enum backed by `i32` (`Visible = 0`,
//! `Collapsed = 1`) — a stable, long-documented value mapping shared with
//! WPF/UWP's identical `Visibility` enum — so this takes a plain `bool`
//! rather than defining a whole enum type for two variants.

use windows_core::{HRESULT, IInspectable, Interface, Result};

windows_core::imp::define_interface!(IUIElement, IUIElement_Vtbl, 0xc3c01020_320c_5cf6_9d24_d396bbfa4d8b);

#[repr(C)]
pub struct IUIElement_Vtbl {
    base__: windows_core::IInspectable_Vtbl,
    _desired_size: usize,
    _allow_drop: usize,
    _set_allow_drop: usize,
    _opacity: usize,
    _set_opacity: usize,
    _clip: usize,
    _set_clip: usize,
    _render_transform: usize,
    _set_render_transform: usize,
    _projection: usize,
    _set_projection: usize,
    _transform3d: usize,
    _set_transform3d: usize,
    _render_transform_origin: usize,
    _set_render_transform_origin: usize,
    _is_hit_test_visible: usize,
    _set_is_hit_test_visible: usize,
    _visibility: usize,
    set_visibility: unsafe extern "system" fn(*mut core::ffi::c_void, i32) -> HRESULT,
}

impl IUIElement {
    fn set_visibility(&self, visible: bool) -> Result<()> {
        unsafe { (Interface::vtable(self).set_visibility)(Interface::as_raw(self), if visible { 0 } else { 1 }).ok() }
    }
}

/// Sets `Visibility` (`Visible`/`Collapsed`) on the native XAML element
/// behind `handle` — the `IInspectable` `windows-webview`'s `webview()`
/// hands back via its `on_mounted`/`on_unmounted` callbacks (see
/// `lib.rs`'s `page_element`). Silently a no-op if `handle` doesn't
/// implement `IUIElement` (shouldn't happen for a real mounted `WebView2`,
/// but this is called from event-callback contexts where surfacing an error
/// has nowhere useful to go).
pub fn set_visible(handle: &IInspectable, visible: bool) {
    if let Ok(element) = handle.cast::<IUIElement>() {
        let _ = element.set_visibility(visible);
    }
}
