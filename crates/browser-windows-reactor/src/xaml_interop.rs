//! Hand-written COM/WinRT bindings for a few `Microsoft.UI.Xaml`/
//! `Microsoft.UI.Input` members `windows-reactor`'s own declarative
//! `Element`/`ElementExt` doesn't expose: `UIElement.Visibility` (see
//! `set_visible`) and `UIElement.PreviewKeyDown` (see
//! `intercept_plain_enter`).
//!
//! Unlike the earlier version of this file (which copied `IUIElement`'s
//! `SetVisibility` slot by hand from the vendored `windows-reactor` crate's
//! own `bindings.rs`), everything below was produced by actually *running*
//! `windows-bindgen` — the same codegen tool `windows-reactor` itself is
//! built with — against the real WinMD corpus already vendored at
//! `windows-reactor`'s own pinned commit
//! (`crates/tools/reactor/winmd/Microsoft.UI.Xaml.winmd` etc. in the
//! `windows-reactor`/`windows-webview` git checkout). That corpus isn't
//! reachable from *our* Cargo.toml (it's tooling-internal to that crate),
//! so this isn't wired up as a build-time codegen step here — it was a
//! one-time, reproducible generation, captured as plain source. Regenerate
//! by running, from a checkout of `windows-reactor`'s git commit:
//!
//! ```text
//! windows_bindgen::bindgen([
//!     "--in", "crates/tools/reactor/winmd",
//!             "crates/libs/bindgen/default/Windows.winmd",
//!             "crates/libs/bindgen/default/Windows.Win32.winmd",
//!     "--out", "<this file>",
//!     "--implement", "Microsoft.UI.Xaml.Input.KeyEventHandler",
//!     "--minimal", "--dead-code", "--flat",
//!     "--filter",
//!     "Microsoft.UI.Xaml.UIElement::{Visibility, PreviewKeyDown}",
//!     "Microsoft.UI.Xaml.Input.KeyRoutedEventArgs::{Key, Handled}",
//!     "Microsoft.UI.Xaml.Window::{Content}",
//!     "Microsoft.UI.Input.InputKeyboardSource::{GetKeyStateForCurrentThread}",
//! ]);
//! ```
//!
//! This is why the confidence level here is much higher than a hand-derived
//! vtable offset would be: the IIDs and vtable layouts are exactly what the
//! real Windows App SDK metadata says they are, not guessed or copied from
//! partial references. The same approach (plus `Microsoft.UI.Input.
//! InputNonClientPointerSource`, `Microsoft.UI.Xaml.FrameworkElement.
//! SizeChanged`, and the plain-COM, not-WinRT-projected `IWindowNative` for
//! the real HWND) is also how the toolbar was moved into the title bar area
//! (see `setup_titlebar_passthrough`) — the toolbar now lives in
//! `TitleBar::content(..)` and receives real clicks via a `Passthrough`
//! `SetRegionRects` carve-out, with left/right margins reserved so the
//! window stays draggable and the system's own caption buttons keep
//! working (both margins exist because of real, observed failures — see
//! `TITLEBAR_DRAG_MARGIN_DIPS`/`TITLEBAR_RIGHT_MARGIN_DIPS`'s doc comments).

use windows_core::{HRESULT, IInspectable, IUnknown, Interface, Result};

// ===== Microsoft.UI.Xaml.UIElement (Visibility + PreviewKeyDown) =====

windows_core::imp::define_interface!(IUIElement, IUIElement_Vtbl, 0xc3c01020_320c_5cf6_9d24_d396bbfa4d8b);
impl windows_core::RuntimeType for IUIElement {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::for_interface::<Self>();
}
impl IUIElement {
    fn SetVisibility(&self, value: Visibility) -> Result<()> {
        unsafe { (Interface::vtable(self).SetVisibility)(Interface::as_raw(self), value).ok() }
    }
    /// DIP-to-physical-pixel scale factor — `InputNonClientPointerSource::
    /// SetRegionRects` (see `titlebar_passthrough`) wants physical pixels,
    /// everything else in this codebase (like `Rect` from `IWindow::
    /// Bounds`) is in DIPs.
    fn RasterizationScale(&self) -> Result<f64> {
        unsafe {
            let mut result__ = core::mem::zeroed();
            (Interface::vtable(self).RasterizationScale)(Interface::as_raw(self), &mut result__).map(|| result__)
        }
    }
    fn PreviewKeyDown<F>(&self, handler: F) -> Result<windows_core::EventRevoker>
    where
        F: Fn(windows_core::Ref<IInspectable>, windows_core::Ref<KeyRoutedEventArgs>) + 'static,
    {
        let handler: KeyEventHandler = {
            let com = windows_core::imp::DelegateBox::<KeyEventHandler, F>::new(&KeyEventHandlerBox::<F>::VTABLE, handler);
            unsafe { core::mem::transmute(windows_core::imp::box_new(com)) }
        };
        unsafe {
            let mut result__ = core::mem::zeroed();
            let token__ = (Interface::vtable(self).PreviewKeyDown)(Interface::as_raw(self), Interface::as_raw(&handler), &mut result__).map(|| result__)?;
            Ok(windows_core::EventRevoker::new(self.clone(), token__, Interface::vtable(self).RemovePreviewKeyDown))
        }
    }
}
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
    SetVisibility: unsafe extern "system" fn(*mut core::ffi::c_void, Visibility) -> HRESULT,
    _RenderSize: usize,
    _UseLayoutRounding: usize,
    _SetUseLayoutRounding: usize,
    _Transitions: usize,
    _SetTransitions: usize,
    _CacheMode: usize,
    _SetCacheMode: usize,
    _IsTapEnabled: usize,
    _SetIsTapEnabled: usize,
    _IsDoubleTapEnabled: usize,
    _SetIsDoubleTapEnabled: usize,
    _CanDrag: usize,
    _SetCanDrag: usize,
    _IsRightTapEnabled: usize,
    _SetIsRightTapEnabled: usize,
    _IsHoldingEnabled: usize,
    _SetIsHoldingEnabled: usize,
    _ManipulationMode: usize,
    _SetManipulationMode: usize,
    _PointerCaptures: usize,
    _ContextFlyout: usize,
    _SetContextFlyout: usize,
    _CompositeMode: usize,
    _SetCompositeMode: usize,
    _Lights: usize,
    _CanBeScrollAnchor: usize,
    _SetCanBeScrollAnchor: usize,
    _ExitDisplayModeOnAccessKeyInvoked: usize,
    _SetExitDisplayModeOnAccessKeyInvoked: usize,
    _IsAccessKeyScope: usize,
    _SetIsAccessKeyScope: usize,
    _AccessKeyScopeOwner: usize,
    _SetAccessKeyScopeOwner: usize,
    _AccessKey: usize,
    _SetAccessKey: usize,
    _KeyTipPlacementMode: usize,
    _SetKeyTipPlacementMode: usize,
    _KeyTipHorizontalOffset: usize,
    _SetKeyTipHorizontalOffset: usize,
    _KeyTipVerticalOffset: usize,
    _SetKeyTipVerticalOffset: usize,
    _KeyTipTarget: usize,
    _SetKeyTipTarget: usize,
    _XYFocusKeyboardNavigation: usize,
    _SetXYFocusKeyboardNavigation: usize,
    _XYFocusUpNavigationStrategy: usize,
    _SetXYFocusUpNavigationStrategy: usize,
    _XYFocusDownNavigationStrategy: usize,
    _SetXYFocusDownNavigationStrategy: usize,
    _XYFocusLeftNavigationStrategy: usize,
    _SetXYFocusLeftNavigationStrategy: usize,
    _XYFocusRightNavigationStrategy: usize,
    _SetXYFocusRightNavigationStrategy: usize,
    _KeyboardAccelerators: usize,
    _KeyboardAcceleratorPlacementTarget: usize,
    _SetKeyboardAcceleratorPlacementTarget: usize,
    _KeyboardAcceleratorPlacementMode: usize,
    _SetKeyboardAcceleratorPlacementMode: usize,
    _HighContrastAdjustment: usize,
    _SetHighContrastAdjustment: usize,
    _TabFocusNavigation: usize,
    _SetTabFocusNavigation: usize,
    _OpacityTransition: usize,
    _SetOpacityTransition: usize,
    _Translation: usize,
    _SetTranslation: usize,
    _TranslationTransition: usize,
    _SetTranslationTransition: usize,
    _Rotation: usize,
    _SetRotation: usize,
    _RotationTransition: usize,
    _SetRotationTransition: usize,
    _Scale: usize,
    _SetScale: usize,
    _ScaleTransition: usize,
    _SetScaleTransition: usize,
    _TransformMatrix: usize,
    _SetTransformMatrix: usize,
    _CenterPoint: usize,
    _SetCenterPoint: usize,
    _RotationAxis: usize,
    _SetRotationAxis: usize,
    _ActualOffset: usize,
    _ActualSize: usize,
    _XamlRoot: usize,
    _SetXamlRoot: usize,
    _Shadow: usize,
    _SetShadow: usize,
    RasterizationScale: unsafe extern "system" fn(*mut core::ffi::c_void, *mut f64) -> HRESULT,
    _SetRasterizationScale: usize,
    _FocusState: usize,
    _UseSystemFocusVisuals: usize,
    _SetUseSystemFocusVisuals: usize,
    _XYFocusLeft: usize,
    _SetXYFocusLeft: usize,
    _XYFocusRight: usize,
    _SetXYFocusRight: usize,
    _XYFocusUp: usize,
    _SetXYFocusUp: usize,
    _XYFocusDown: usize,
    _SetXYFocusDown: usize,
    _IsTabStop: usize,
    _SetIsTabStop: usize,
    _TabIndex: usize,
    _SetTabIndex: usize,
    _KeyUp: usize,
    _RemoveKeyUp: usize,
    _KeyDown: usize,
    _RemoveKeyDown: usize,
    _GotFocus: usize,
    _RemoveGotFocus: usize,
    _LostFocus: usize,
    _RemoveLostFocus: usize,
    _DragStarting: usize,
    _RemoveDragStarting: usize,
    _DropCompleted: usize,
    _RemoveDropCompleted: usize,
    _CharacterReceived: usize,
    _RemoveCharacterReceived: usize,
    _DragEnter: usize,
    _RemoveDragEnter: usize,
    _DragLeave: usize,
    _RemoveDragLeave: usize,
    _DragOver: usize,
    _RemoveDragOver: usize,
    _Drop: usize,
    _RemoveDrop: usize,
    _PointerPressed: usize,
    _RemovePointerPressed: usize,
    _PointerMoved: usize,
    _RemovePointerMoved: usize,
    _PointerReleased: usize,
    _RemovePointerReleased: usize,
    _PointerEntered: usize,
    _RemovePointerEntered: usize,
    _PointerExited: usize,
    _RemovePointerExited: usize,
    _PointerCaptureLost: usize,
    _RemovePointerCaptureLost: usize,
    _PointerCanceled: usize,
    _RemovePointerCanceled: usize,
    _PointerWheelChanged: usize,
    _RemovePointerWheelChanged: usize,
    _Tapped: usize,
    _RemoveTapped: usize,
    _DoubleTapped: usize,
    _RemoveDoubleTapped: usize,
    _Holding: usize,
    _RemoveHolding: usize,
    _ContextRequested: usize,
    _RemoveContextRequested: usize,
    _ContextCanceled: usize,
    _RemoveContextCanceled: usize,
    _RightTapped: usize,
    _RemoveRightTapped: usize,
    _ManipulationStarting: usize,
    _RemoveManipulationStarting: usize,
    _ManipulationInertiaStarting: usize,
    _RemoveManipulationInertiaStarting: usize,
    _ManipulationStarted: usize,
    _RemoveManipulationStarted: usize,
    _ManipulationDelta: usize,
    _RemoveManipulationDelta: usize,
    _ManipulationCompleted: usize,
    _RemoveManipulationCompleted: usize,
    _AccessKeyDisplayRequested: usize,
    _RemoveAccessKeyDisplayRequested: usize,
    _AccessKeyDisplayDismissed: usize,
    _RemoveAccessKeyDisplayDismissed: usize,
    _AccessKeyInvoked: usize,
    _RemoveAccessKeyInvoked: usize,
    _ProcessKeyboardAccelerators: usize,
    _RemoveProcessKeyboardAccelerators: usize,
    _GettingFocus: usize,
    _RemoveGettingFocus: usize,
    _LosingFocus: usize,
    _RemoveLosingFocus: usize,
    _NoFocusCandidateFound: usize,
    _RemoveNoFocusCandidateFound: usize,
    PreviewKeyDown: unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void, *mut i64) -> HRESULT,
    RemovePreviewKeyDown: unsafe extern "system" fn(*mut core::ffi::c_void, i64) -> HRESULT,
}

/// Sets `Visibility` (`Visible`/`Collapsed`) on the native XAML element
/// behind `handle` — the `IInspectable` `windows-webview`'s `webview()`
/// hands back via its `on_mounted`/`on_unmounted` callbacks (see
/// `lib.rs`'s `page_element`). Silently a no-op if `handle` doesn't
/// implement `IUIElement`.
pub(crate) fn set_visible(handle: &IInspectable, visible: bool) {
    if let Ok(element) = handle.cast::<IUIElement>() {
        let _ = element.SetVisibility(if visible { Visibility::Visible } else { Visibility::Collapsed });
    }
}

/// Subscribes to `PreviewKeyDown` (tunnels from the root down to the
/// focused element, so it fires *before* a focused `TextBox`'s own
/// internal handling of a bare Enter — see this module's doc comment and
/// `lib.rs`'s `activate_search` for the gap this closes) on `root`,
/// calling `on_plain_enter` and marking the key handled whenever Enter is
/// pressed with no Ctrl held — but only marks it handled when
/// `on_plain_enter` itself reports it actually did something (returns
/// `true`); this fires for *every* Enter press app-wide (it's subscribed
/// once, on the window's root content), not just in the switcher's search
/// box, so a `false` return (e.g. no switcher search text to act on right
/// now) leaves the key alone rather than silently swallowing Enter in some
/// other, unrelated text field. Ctrl+Enter is always left alone regardless
/// (not even offered to `on_plain_enter`) so it keeps reaching the existing
/// `force_new_page_from_search` `KeyboardAccelerator` unimpeded — this only
/// fills the *specific* gap that mechanism already has, not a replacement
/// for it.
pub(crate) fn intercept_plain_enter(root: &IInspectable, on_plain_enter: impl Fn() -> bool + 'static) -> Result<windows_core::EventRevoker> {
    let element = root.cast::<IUIElement>()?;
    element.PreviewKeyDown(move |_sender, args| {
        let Some(args) = args.as_ref() else { return };
        if args.Key().unwrap_or_default() != VirtualKey::Enter {
            return;
        }
        let ctrl_down = InputKeyboardSource::GetKeyStateForCurrentThread(VirtualKey::Control).unwrap_or_default().contains(CoreVirtualKeyStates::Down);
        if ctrl_down {
            return;
        }
        if on_plain_enter() {
            let _ = args.SetHandled(true);
        }
    })
}

/// Gets the root `UIElement` (`Window.Content`) of the app's primary
/// window, the target `intercept_plain_enter` is meant to be attached to —
/// a `PreviewKeyDown` subscription anywhere in the tunneling path above the
/// switcher's search box works, and the root is the simplest choice.
/// Returns `None` if there's no active host yet or it has no content set.
pub(crate) fn root_content() -> Option<IInspectable> {
    windows_reactor::with_active_host(|host| {
        let window = host.window().cast::<IWindow>().ok()?;
        let content = window.Content().ok()?;
        content.cast::<IInspectable>().ok()
    })
    .flatten()
}

// ===== Microsoft.UI.Xaml.Window (Content getter only) =====

windows_core::imp::define_interface!(IWindow, IWindow_Vtbl, 0x61f0ec79_5d52_56b5_86fb_40fa4af288b0);
impl windows_core::RuntimeType for IWindow {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::for_interface::<Self>();
}
impl IWindow {
    fn Content(&self) -> Result<UIElement> {
        unsafe {
            let mut result__ = core::mem::zeroed();
            (Interface::vtable(self).Content)(Interface::as_raw(self), &mut result__).and_then(|| windows_core::Type::from_abi(result__))
        }
    }
    /// The window's content-area size, in DIPs.
    fn Bounds(&self) -> Result<Rect> {
        unsafe {
            let mut result__ = core::mem::zeroed();
            (Interface::vtable(self).Bounds)(Interface::as_raw(self), &mut result__).map(|| result__)
        }
    }
}
#[repr(C)]
pub struct IWindow_Vtbl {
    base__: windows_core::IInspectable_Vtbl,
    Bounds: unsafe extern "system" fn(*mut core::ffi::c_void, *mut Rect) -> HRESULT,
    _visible: usize,
    Content: unsafe extern "system" fn(*mut core::ffi::c_void, *mut *mut core::ffi::c_void) -> HRESULT,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl windows_core::TypeKind for Rect {
    type TypeKind = windows_core::CopyType;
}
impl windows_core::RuntimeType for Rect {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::from_slice(b"struct(Windows.Foundation.Rect;f4;f4;f4;f4)");
}

// ===== IWindowNative (plain COM, not WinRT — no WinMD entry for this one) =====
//
// `Microsoft.UI.Xaml.Window` doesn't publicly expose its own HWND through
// any WinRT-projected API — this is the standard, long-documented way
// every WinAppSDK app (C++, C#, now this one) gets it: query the Window
// object for `IWindowNative`, a plain `IUnknown`-based COM interface
// declared in the Windows App SDK's `microsoft.ui.xaml.window.h` header,
// not in any `.winmd` (WinRT metadata only covers WinRT-projected types,
// and this interface deliberately isn't one — see Microsoft Learn's
// `IWindowNative::get_WindowHandle` reference page). Its IID is a
// long-stable, widely-published public constant, not something private or
// guessed.
windows_core::imp::define_interface!(IWindowNative, IWindowNative_Vtbl, 0xeecdbf0e_bae9_4cb6_a68e_9598e1cb57bb);
impl IWindowNative {
    fn GetWindowHandle(&self) -> Result<isize> {
        unsafe {
            let mut result__: isize = 0;
            (Interface::vtable(self).GetWindowHandle)(Interface::as_raw(self), &mut result__).ok()?;
            Ok(result__)
        }
    }
}
#[repr(C)]
pub struct IWindowNative_Vtbl {
    base__: windows_core::IUnknown_Vtbl,
    GetWindowHandle: unsafe extern "system" fn(*mut core::ffi::c_void, *mut isize) -> HRESULT,
}

// ===== Microsoft.UI.Input.InputNonClientPointerSource (title bar hit-testing) =====

windows_core::imp::define_interface!(IInputNonClientPointerSource, IInputNonClientPointerSource_Vtbl, 0x471732b4_3d07_5104_b192_ebacf71e86df);
impl windows_core::RuntimeType for IInputNonClientPointerSource {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::for_interface::<Self>();
}
impl IInputNonClientPointerSource {
    fn SetRegionRects(&self, region: NonClientRegionKind, rects: &[RectInt32]) -> Result<()> {
        unsafe { (Interface::vtable(self).SetRegionRects)(Interface::as_raw(self), region, rects.len().try_into().unwrap(), rects.as_ptr()).ok() }
    }
}
#[repr(C)]
pub struct IInputNonClientPointerSource_Vtbl {
    base__: windows_core::IInspectable_Vtbl,
    _dispatcher_queue: usize,
    _clear_all_region_rects: usize,
    _clear_region_rects: usize,
    _get_region_rects: usize,
    SetRegionRects: unsafe extern "system" fn(*mut core::ffi::c_void, NonClientRegionKind, u32, *const RectInt32) -> HRESULT,
}

windows_core::imp::define_interface!(IInputNonClientPointerSourceStatics, IInputNonClientPointerSourceStatics_Vtbl, 0x7d0b775c_1903_5dc7_bd2f_7a4b31f0cff2);
impl windows_core::RuntimeType for IInputNonClientPointerSourceStatics {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::for_interface::<Self>();
}
#[repr(C)]
pub struct IInputNonClientPointerSourceStatics_Vtbl {
    base__: windows_core::IInspectable_Vtbl,
    GetForWindowId: unsafe extern "system" fn(*mut core::ffi::c_void, WindowId, *mut *mut core::ffi::c_void) -> HRESULT,
}

#[repr(transparent)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputNonClientPointerSource(IUnknown);
windows_core::imp::interface_hierarchy!(InputNonClientPointerSource, IUnknown, IInspectable);
unsafe impl Interface for InputNonClientPointerSource {
    type Vtable = <IInputNonClientPointerSource as Interface>::Vtable;
    const IID: windows_core::GUID = <IInputNonClientPointerSource as Interface>::IID;
}
impl windows_core::RuntimeName for InputNonClientPointerSource {
    const NAME: &'static str = "Microsoft.UI.Input.InputNonClientPointerSource";
}
impl core::ops::Deref for InputNonClientPointerSource {
    type Target = IInputNonClientPointerSource;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
impl InputNonClientPointerSource {
    fn GetForWindowId(windowid: WindowId) -> Result<Self> {
        Self::IInputNonClientPointerSourceStatics(|this| unsafe {
            let mut result__ = core::mem::zeroed();
            (Interface::vtable(this).GetForWindowId)(Interface::as_raw(this), windowid, &mut result__).and_then(|| windows_core::Type::from_abi(result__))
        })
    }
    fn IInputNonClientPointerSourceStatics<R, F: FnOnce(&IInputNonClientPointerSourceStatics) -> Result<R>>(callback: F) -> Result<R> {
        static SHARED: windows_core::imp::FactoryCache<InputNonClientPointerSource, IInputNonClientPointerSourceStatics> = windows_core::imp::FactoryCache::new();
        SHARED.call(callback)
    }
}

// ===== Microsoft.UI.Xaml.FrameworkElement (SizeChanged) =====

windows_core::imp::define_interface!(IFrameworkElement, IFrameworkElement_Vtbl, 0xfe08f13d_dc6a_5495_ad44_c2d8d21863b0);
impl windows_core::RuntimeType for IFrameworkElement {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::for_interface::<Self>();
}
impl IFrameworkElement {
    fn SizeChanged<F>(&self, handler: F) -> Result<windows_core::EventRevoker>
    where
        F: Fn(windows_core::Ref<IInspectable>, windows_core::Ref<IInspectable>) + 'static,
    {
        let handler: SizeChangedEventHandler = {
            let com = windows_core::imp::DelegateBox::<SizeChangedEventHandler, F>::new(&SizeChangedEventHandlerBox::<F>::VTABLE, handler);
            unsafe { core::mem::transmute(windows_core::imp::box_new(com)) }
        };
        unsafe {
            let mut result__ = core::mem::zeroed();
            let token__ = (Interface::vtable(self).SizeChanged)(Interface::as_raw(self), Interface::as_raw(&handler), &mut result__).map(|| result__)?;
            Ok(windows_core::EventRevoker::new(self.clone(), token__, Interface::vtable(self).RemoveSizeChanged))
        }
    }
}
#[repr(C)]
pub struct IFrameworkElement_Vtbl {
    base__: windows_core::IInspectable_Vtbl,
    _Triggers: usize,
    _Resources: usize,
    _SetResources: usize,
    _Tag: usize,
    _SetTag: usize,
    _Language: usize,
    _SetLanguage: usize,
    _ActualWidth: usize,
    _ActualHeight: usize,
    _Width: usize,
    _SetWidth: usize,
    _Height: usize,
    _SetHeight: usize,
    _MinWidth: usize,
    _SetMinWidth: usize,
    _MaxWidth: usize,
    _SetMaxWidth: usize,
    _MinHeight: usize,
    _SetMinHeight: usize,
    _MaxHeight: usize,
    _SetMaxHeight: usize,
    _HorizontalAlignment: usize,
    _SetHorizontalAlignment: usize,
    _VerticalAlignment: usize,
    _SetVerticalAlignment: usize,
    _Margin: usize,
    _SetMargin: usize,
    _Name: usize,
    _SetName: usize,
    _BaseUri: usize,
    _DataContext: usize,
    _SetDataContext: usize,
    _AllowFocusOnInteraction: usize,
    _SetAllowFocusOnInteraction: usize,
    _FocusVisualMargin: usize,
    _SetFocusVisualMargin: usize,
    _FocusVisualSecondaryThickness: usize,
    _SetFocusVisualSecondaryThickness: usize,
    _FocusVisualPrimaryThickness: usize,
    _SetFocusVisualPrimaryThickness: usize,
    _FocusVisualSecondaryBrush: usize,
    _SetFocusVisualSecondaryBrush: usize,
    _FocusVisualPrimaryBrush: usize,
    _SetFocusVisualPrimaryBrush: usize,
    _AllowFocusWhenDisabled: usize,
    _SetAllowFocusWhenDisabled: usize,
    _Style: usize,
    _SetStyle: usize,
    _Parent: usize,
    _FlowDirection: usize,
    _SetFlowDirection: usize,
    _RequestedTheme: usize,
    _SetRequestedTheme: usize,
    _IsLoaded: usize,
    _ActualTheme: usize,
    _Loaded: usize,
    _RemoveLoaded: usize,
    _Unloaded: usize,
    _RemoveUnloaded: usize,
    _DataContextChanged: usize,
    _RemoveDataContextChanged: usize,
    SizeChanged: unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void, *mut i64) -> HRESULT,
    RemoveSizeChanged: unsafe extern "system" fn(*mut core::ffi::c_void, i64) -> HRESULT,
}

windows_core::imp::define_interface!(SizeChangedEventHandler, SizeChangedEventHandler_Vtbl, 0x8d7b1a58_14c6_51c9_892c_9fcce368e77d);
impl windows_core::RuntimeType for SizeChangedEventHandler {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::for_interface::<Self>();
}
#[repr(C)]
pub struct SizeChangedEventHandler_Vtbl {
    base__: windows_core::IUnknown_Vtbl,
    Invoke: unsafe extern "system" fn(this: *mut core::ffi::c_void, sender: *mut core::ffi::c_void, e: *mut core::ffi::c_void) -> HRESULT,
}
struct SizeChangedEventHandlerBox<F: Fn(windows_core::Ref<IInspectable>, windows_core::Ref<IInspectable>) + 'static>(core::marker::PhantomData<(fn() -> F,)>);
impl<F: Fn(windows_core::Ref<IInspectable>, windows_core::Ref<IInspectable>) + 'static> SizeChangedEventHandlerBox<F> {
    const VTABLE: SizeChangedEventHandler_Vtbl = SizeChangedEventHandler_Vtbl {
        base__: windows_core::IUnknown_Vtbl {
            QueryInterface: windows_core::imp::DelegateBox::<SizeChangedEventHandler, F>::QueryInterface,
            AddRef: windows_core::imp::DelegateBox::<SizeChangedEventHandler, F>::AddRef,
            Release: windows_core::imp::DelegateBox::<SizeChangedEventHandler, F>::Release,
        },
        Invoke: Self::Invoke,
    };
    unsafe extern "system" fn Invoke(this: *mut core::ffi::c_void, sender: *mut core::ffi::c_void, e: *mut core::ffi::c_void) -> HRESULT {
        unsafe {
            let this = &mut *(this as *mut *mut core::ffi::c_void as *mut windows_core::imp::DelegateBox<SizeChangedEventHandler, F>);
            (this.invoke)(core::mem::transmute_copy(&sender), core::mem::transmute_copy(&e));
            HRESULT(0)
        }
    }
}

/// The toolbar row's height, in DIPs — generous on purpose (see
/// `setup_titlebar_passthrough`'s doc comment on why precision doesn't
/// matter here).
const TITLEBAR_PASSTHROUGH_HEIGHT_DIPS: f64 = 56.0;

/// Width, in DIPs, of a strip reserved at the *left* edge of the title bar
/// that is deliberately left *out* of the `Passthrough` region — i.e. left
/// draggable. `TitleBar` renders its own title text (and, at the system's
/// discretion, an app icon) in that strip before our `content()` begins, so
/// this also happens to line up with where a user's eye would look for "the
/// title bar" as a drag handle. Without this, a `Passthrough` rect spanning
/// the *entire* window width leaves literally nowhere in the whole title
/// bar strip draggable — confirmed as a real regression, not a hypothetical
/// one: a synthetic click-drag against the un-narrowed rect left the window
/// stationary. The exact value doesn't need to be pixel-perfect (same
/// "generous is safe" reasoning as the rest of this function) — it only
/// needs to (a) stay clear of the leftmost real toolbar button (the back
/// button) so it doesn't swallow a click, and (b) be wide enough to grab.
const TITLEBAR_DRAG_MARGIN_DIPS: f64 = 120.0;

/// Width, in DIPs, of a strip reserved at the *right* edge of the title bar
/// that is deliberately left *out* of the `Passthrough` region, so it
/// doesn't cover the system's own minimize/maximize/close caption-button
/// cluster (rendered by `TitleBar` itself, in that same top-right corner).
/// This contradicts what Microsoft's own title-bar customization guide
/// seems to promise ("The system retains control of the caption button
/// area... regardless" of what an app marks) — but empirically, a
/// `Passthrough` rect extending under the caption buttons *does* break
/// them: real synthetic clicks directly on the minimize glyph, at several
/// nearby pixels, silently did nothing until this margin was added. Rather
/// than trust the docs over an observed, repeatable failure, this reserves
/// real clearance. Generous on purpose, same reasoning as the left margin.
const TITLEBAR_RIGHT_MARGIN_DIPS: f64 = 150.0;

/// Marks the title bar's toolbar-content row as an interactive
/// (`Passthrough`) region within the window's otherwise fully-draggable
/// custom title bar (see `lib.rs`'s `app`, where the toolbar moves into
/// `TitleBar::content(..)`) — without this, every click there would just
/// drag the window instead of reaching a button. Reapplies on every
/// `SizeChanged` (the region is in physical pixels, tied to the window's
/// current size), including once immediately. Returns the `SizeChanged`
/// subscription — keep it alive for the app's lifetime, same as
/// `intercept_plain_enter`'s revoker.
///
/// Deliberately generous — spans from `TITLEBAR_DRAG_MARGIN_DIPS` to
/// `TITLEBAR_RIGHT_MARGIN_DIPS` short of the window's right edge, using a
/// fixed, taller-than-necessary height — rather than tracking the toolbar's
/// exact on-screen rect: there's no `on_mounted`-style hook for a bare `Grid`
/// (only a few special-cased native-control-hosting widgets like
/// `WebView2` get one — see `set_visible`'s doc comment for that same
/// gap), so getting the toolbar's precise bounds would need walking the
/// real visual tree via `VisualTreeHelper`, real work with no real payoff
/// here. Both margins were tuned against real, repeatable failures (see
/// their own doc comments) rather than assumed safe. Extending below the
/// toolbar's real bottom edge remains harmless, though: `SetRegionRects`
/// only has any effect *within* the area `Window::SetTitleBar` already
/// claims as title-bar hit-testing territory (`TitleBar`'s own rendered
/// bounds) in the first place, so this can't reach into ordinary page
/// content below it.
pub(crate) fn setup_titlebar_passthrough() -> Option<windows_core::EventRevoker> {
    apply_titlebar_passthrough();
    let root = root_content()?;
    let element = root.cast::<IFrameworkElement>().ok()?;
    element.SizeChanged(move |_sender, _args| apply_titlebar_passthrough()).ok()
}

fn apply_titlebar_passthrough() {
    let result = windows_reactor::with_active_host(|host| -> Result<()> {
        let window = host.window();
        let iwindow = window.cast::<IWindow>()?;
        let content = iwindow.Content()?;
        let scale = content.RasterizationScale()?;
        let bounds = iwindow.Bounds()?;
        let hwnd = window.cast::<IWindowNative>()?.GetWindowHandle()?;
        let source = InputNonClientPointerSource::GetForWindowId(WindowId { value: hwnd as u64 })?;
        let left_margin_px = (TITLEBAR_DRAG_MARGIN_DIPS * scale).round() as i32;
        let right_margin_px = (TITLEBAR_RIGHT_MARGIN_DIPS * scale).round() as i32;
        let total_width_px = (f64::from(bounds.width) * scale).round() as i32;
        let rect = RectInt32 {
            x: left_margin_px,
            y: 0,
            width: (total_width_px - left_margin_px - right_margin_px).max(0),
            height: (TITLEBAR_PASSTHROUGH_HEIGHT_DIPS * scale).round() as i32,
        };
        source.SetRegionRects(NonClientRegionKind::Passthrough, &[rect])
    });
    if let Some(Err(_err)) = result {
        // Best-effort: worst case the toolbar stays click-dead until the
        // next resize retries this, same as it always was before this
        // feature existed. Nowhere useful to surface an error from here.
    }
}

#[repr(transparent)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UIElement(IUnknown);
windows_core::imp::interface_hierarchy!(UIElement, IUnknown, IInspectable);
unsafe impl Interface for UIElement {
    type Vtable = <IUIElement as Interface>::Vtable;
    const IID: windows_core::GUID = <IUIElement as Interface>::IID;
}
impl core::ops::Deref for UIElement {
    type Target = IUIElement;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}

// ===== Microsoft.UI.Xaml.Input.KeyRoutedEventArgs =====

windows_core::imp::define_interface!(IKeyRoutedEventArgs, IKeyRoutedEventArgs_Vtbl, 0xee357007_a2d6_5c75_9431_05fd66ec7915);
impl windows_core::RuntimeType for IKeyRoutedEventArgs {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::for_interface::<Self>();
}
impl IKeyRoutedEventArgs {
    fn Key(&self) -> Result<VirtualKey> {
        unsafe {
            let mut result__ = core::mem::zeroed();
            (Interface::vtable(self).Key)(Interface::as_raw(self), &mut result__).map(|| result__)
        }
    }
    fn SetHandled(&self, value: bool) -> Result<()> {
        unsafe { (Interface::vtable(self).SetHandled)(Interface::as_raw(self), value).ok() }
    }
}
#[repr(C)]
pub struct IKeyRoutedEventArgs_Vtbl {
    base__: windows_core::IInspectable_Vtbl,
    Key: unsafe extern "system" fn(*mut core::ffi::c_void, *mut VirtualKey) -> HRESULT,
    _key_status: usize,
    _handled: usize,
    SetHandled: unsafe extern "system" fn(*mut core::ffi::c_void, bool) -> HRESULT,
}

#[repr(transparent)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyRoutedEventArgs(IUnknown);
windows_core::imp::interface_hierarchy!(KeyRoutedEventArgs, IUnknown, IInspectable);
unsafe impl Interface for KeyRoutedEventArgs {
    type Vtable = <IKeyRoutedEventArgs as Interface>::Vtable;
    const IID: windows_core::GUID = <IKeyRoutedEventArgs as Interface>::IID;
}
impl core::ops::Deref for KeyRoutedEventArgs {
    type Target = IKeyRoutedEventArgs;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}

// ===== Microsoft.UI.Xaml.Input.KeyEventHandler (delegate) =====

windows_core::imp::define_interface!(KeyEventHandler, KeyEventHandler_Vtbl, 0xdb68e7cc_9a2b_527d_9989_25284daccc03);
impl windows_core::RuntimeType for KeyEventHandler {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::for_interface::<Self>();
}
#[repr(C)]
pub struct KeyEventHandler_Vtbl {
    base__: windows_core::IUnknown_Vtbl,
    Invoke: unsafe extern "system" fn(this: *mut core::ffi::c_void, sender: *mut core::ffi::c_void, e: *mut core::ffi::c_void) -> HRESULT,
}
struct KeyEventHandlerBox<F: Fn(windows_core::Ref<IInspectable>, windows_core::Ref<KeyRoutedEventArgs>) + 'static>(core::marker::PhantomData<(fn() -> F,)>);
impl<F: Fn(windows_core::Ref<IInspectable>, windows_core::Ref<KeyRoutedEventArgs>) + 'static> KeyEventHandlerBox<F> {
    const VTABLE: KeyEventHandler_Vtbl = KeyEventHandler_Vtbl {
        base__: windows_core::IUnknown_Vtbl {
            QueryInterface: windows_core::imp::DelegateBox::<KeyEventHandler, F>::QueryInterface,
            AddRef: windows_core::imp::DelegateBox::<KeyEventHandler, F>::AddRef,
            Release: windows_core::imp::DelegateBox::<KeyEventHandler, F>::Release,
        },
        Invoke: Self::Invoke,
    };
    unsafe extern "system" fn Invoke(this: *mut core::ffi::c_void, sender: *mut core::ffi::c_void, e: *mut core::ffi::c_void) -> HRESULT {
        unsafe {
            let this = &mut *(this as *mut *mut core::ffi::c_void as *mut windows_core::imp::DelegateBox<KeyEventHandler, F>);
            (this.invoke)(core::mem::transmute_copy(&sender), core::mem::transmute_copy(&e));
            HRESULT(0)
        }
    }
}

// ===== Microsoft.UI.Input.InputKeyboardSource (live modifier-key state) =====

windows_core::imp::define_interface!(IInputKeyboardSource, IInputKeyboardSource_Vtbl, 0xed61b906_16ad_5df7_a550_5e6f7d2229f7);
impl windows_core::RuntimeType for IInputKeyboardSource {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::for_interface::<Self>();
}
#[repr(C)]
pub struct IInputKeyboardSource_Vtbl {
    base__: windows_core::IInspectable_Vtbl,
}

windows_core::imp::define_interface!(IInputKeyboardSourceStatics, IInputKeyboardSourceStatics_Vtbl, 0xf4e1563d_8c2e_5bcd_b784_47adeaa3cd7e);
impl windows_core::RuntimeType for IInputKeyboardSourceStatics {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::for_interface::<Self>();
}
#[repr(C)]
pub struct IInputKeyboardSourceStatics_Vtbl {
    base__: windows_core::IInspectable_Vtbl,
    GetKeyStateForCurrentThread: unsafe extern "system" fn(*mut core::ffi::c_void, VirtualKey, *mut CoreVirtualKeyStates) -> HRESULT,
}

#[repr(transparent)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputKeyboardSource(IUnknown);
windows_core::imp::interface_hierarchy!(InputKeyboardSource, IUnknown, IInspectable);
unsafe impl Interface for InputKeyboardSource {
    type Vtable = <IInputKeyboardSource as Interface>::Vtable;
    const IID: windows_core::GUID = <IInputKeyboardSource as Interface>::IID;
}
impl windows_core::RuntimeName for InputKeyboardSource {
    const NAME: &'static str = "Microsoft.UI.Input.InputKeyboardSource";
}
impl InputKeyboardSource {
    fn GetKeyStateForCurrentThread(virtualkey: VirtualKey) -> Result<CoreVirtualKeyStates> {
        Self::IInputKeyboardSourceStatics(|this| unsafe {
            let mut result__ = core::mem::zeroed();
            (Interface::vtable(this).GetKeyStateForCurrentThread)(Interface::as_raw(this), virtualkey, &mut result__).map(|| result__)
        })
    }
    fn IInputKeyboardSourceStatics<R, F: FnOnce(&IInputKeyboardSourceStatics) -> Result<R>>(callback: F) -> Result<R> {
        static SHARED: windows_core::imp::FactoryCache<InputKeyboardSource, IInputKeyboardSourceStatics> = windows_core::imp::FactoryCache::new();
        SHARED.call(callback)
    }
}

// ===== Value types =====

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Visibility(pub i32);
impl Visibility {
    pub const Visible: Self = Self(0);
    pub const Collapsed: Self = Self(1);
}
impl windows_core::TypeKind for Visibility {
    type TypeKind = windows_core::CopyType;
}
impl windows_core::RuntimeType for Visibility {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::from_slice(b"enum(Microsoft.UI.Xaml.Visibility;i4)");
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VirtualKey(pub i32);
impl VirtualKey {
    pub const Enter: Self = Self(13);
    pub const Control: Self = Self(17);
}
impl windows_core::TypeKind for VirtualKey {
    type TypeKind = windows_core::CopyType;
}
impl windows_core::RuntimeType for VirtualKey {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::from_slice(b"enum(Windows.System.VirtualKey;i4)");
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CoreVirtualKeyStates(pub u32);
impl CoreVirtualKeyStates {
    pub const Down: Self = Self(1);
    pub const fn contains(&self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}
impl windows_core::TypeKind for CoreVirtualKeyStates {
    type TypeKind = windows_core::CopyType;
}
impl windows_core::RuntimeType for CoreVirtualKeyStates {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::from_slice(b"enum(Windows.UI.Core.CoreVirtualKeyStates;u4)");
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WindowId {
    pub value: u64,
}
impl windows_core::TypeKind for WindowId {
    type TypeKind = windows_core::CopyType;
}
impl windows_core::RuntimeType for WindowId {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::from_slice(b"struct(Microsoft.UI.WindowId;u8)");
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RectInt32 {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
impl windows_core::TypeKind for RectInt32 {
    type TypeKind = windows_core::CopyType;
}
impl windows_core::RuntimeType for RectInt32 {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::from_slice(b"struct(Windows.Graphics.RectInt32;i4;i4;i4;i4)");
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NonClientRegionKind(pub i32);
impl NonClientRegionKind {
    pub const Passthrough: Self = Self(9);
}
impl windows_core::TypeKind for NonClientRegionKind {
    type TypeKind = windows_core::CopyType;
}
impl windows_core::RuntimeType for NonClientRegionKind {
    const SIGNATURE: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::from_slice(b"enum(Microsoft.UI.Input.NonClientRegionKind;i4)");
}
