//! Custom title bar for Windows: strip the native caption from the live
//! `HWND` wxWidgets' own `Frame` created, and subclass it to answer
//! `WM_NCHITTEST` so the existing toolbar row acts as the window's caption —
//! this is the standard "borderless but still resizable/moveable" recipe
//! (the same one Windows Terminal/VS Code/Chromium use), not a speculative
//! hack, but it *is* genuinely more fragile than the rest of this app: we're
//! subclassing a window wxWidgets itself created and still manages
//! internally, not one we own.
//!
//! Unlike Linux, no new widgets are needed here — the existing wx
//! `toolbar_panel` (buttons + address bar, built in `build_frame_and_app`)
//! keeps serving as the toolbar row; only the *window's* behavior changes
//! (plus three new wx buttons for minimize/maximize/close, added directly to
//! the existing `toolbar_sizer` in `build_frame_and_app` since native ones
//! disappear along with the caption).
//!
//! # Verified under Wine, with one caveat
//!
//! Confirmed under this project's Wine 11.0 setup: the native caption is
//! genuinely gone, and clicks on the toolbar's buttons/address bar keep
//! working normally (that part doesn't depend on `WM_NCHITTEST` at all —
//! ordinary child-window click routing handles it regardless). Drag-to-move
//! via the toolbar row's blank space could *not* be confirmed under Wine:
//! `WM_NCHITTEST` never reaches this subclass there at all (confirmed by
//! logging every message the subclass receives — others arrive fine, that
//! one never does), even though `SetWindowSubclass` itself reports success.
//! This looks like a Wine-specific gap in how its X11 driver handles
//! non-client hit-testing for a window it didn't expect to have its caption
//! stripped post-creation, rather than a bug in this code (which follows the
//! standard technique) — real Windows should honor `WM_NCHITTEST` normally,
//! but that's unverified here without a real Windows machine to test on.

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    ChildWindowFromPointEx, GetClientRect, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, CWP_SKIPINVISIBLE,
    CWP_SKIPTRANSPARENT, GWL_STYLE, HTCAPTION, HTCLIENT, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, WM_NCDESTROY, WM_NCHITTEST, WS_CAPTION,
};
use wxdragon::widgets::frame::Frame;
use wxdragon::widgets::panel::Panel;
use wxdragon::window::WxWidget;

/// Arbitrary but unique-within-this-app subclass id, required by
/// `SetWindowSubclass`/`RemoveWindowSubclass` to identify which subclass to
/// (re)move — this app only ever installs one, so any fixed constant works.
const SUBCLASS_ID: usize = 0x8b40_5757;

/// Strips the native title bar from `frame`'s window and installs a subclass
/// that answers `WM_NCHITTEST` so `toolbar_panel`'s row (background only —
/// its buttons/address bar keep receiving normal clicks, checked via a
/// second, nested `ChildWindowFromPointEx`) acts as the caption: drag-to-move,
/// Aero Snap, and double-click-to-maximize all then come from Windows' own
/// non-client handling, no manual drag-tracking code needed.
///
/// Deliberately does *not* also override `WM_NCCALCSIZE` to zero out the
/// non-client frame entirely (the other half of the usual "borderless
/// window" recipe): tried it, and it collapsed the window to a sliver under
/// Wine (confirmed by reverting just that one piece and seeing the window
/// render at full size again). Removing `WS_CAPTION` alone already removes
/// the native title bar and achieves the goal; the thin remaining border
/// `WS_THICKFRAME` draws is a reasonable trade against that breakage.
///
/// Call after `toolbar_panel` and its children already exist.
pub fn install(frame: &Frame, toolbar_panel: &Panel) {
    let hwnd = HWND(frame.get_handle());
    let toolbar_hwnd = HWND(toolbar_panel.get_handle());

    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_CAPTION.0 as isize));
        // Force Windows to recompute the frame immediately rather than
        // waiting for some other trigger to notice the style change.
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);

        let _ = SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, toolbar_hwnd.0 as usize);
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => {
            let default = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            if default.0 as u32 != HTCLIENT {
                // DefSubclassProc already identified an edge/corner resize
                // region — respect it as-is.
                return default;
            }

            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut point = POINT { x, y };
            if unsafe { ScreenToClient(hwnd, &mut point) }.as_bool() {
                let toolbar_hwnd = HWND(ref_data as *mut core::ffi::c_void);
                let mut toolbar_rect = RECT::default();
                // toolbar_panel is anchored at the frame's client-area
                // origin (root_sizer places it first, full width, at the
                // top), so its own client rect's width/height directly
                // bounds the "is this row" check with no coordinate
                // translation needed beyond what's already done above.
                let has_toolbar_rect = unsafe { GetClientRect(toolbar_hwnd, &mut toolbar_rect) }.is_ok();
                if has_toolbar_rect && point.x >= 0 && point.x < toolbar_rect.right && point.y >= 0 && point.y < toolbar_rect.bottom {
                    // Nested hit-test *within* the toolbar row: if a button
                    // or the address bar (a grandchild of `hwnd`, hence
                    // invisible to the top-level ChildWindowFromPointEx
                    // DefSubclassProc already effectively did) is at this
                    // exact point, let it receive the click normally.
                    let hit = unsafe { ChildWindowFromPointEx(toolbar_hwnd, point, CWP_SKIPINVISIBLE | CWP_SKIPTRANSPARENT) };
                    if hit.0 == toolbar_hwnd.0 || hit.is_invalid() {
                        return LRESULT(HTCAPTION as isize);
                    }
                }
            }
            default
        }
        WM_NCDESTROY => {
            // Must remove the subclass before the window actually goes away
            // — the standard, documented pattern for SetWindowSubclass,
            // avoiding a dangling callback pointer after teardown.
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
    }
}
