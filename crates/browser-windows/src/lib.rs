//! Native Win32 chrome for the browser, following the same shape as
//! `browser-linux`: a plain window + toolbar embedding a `render_engine`
//! webview. This is a first milestone only — single page, no switcher, no
//! shortcuts beyond Enter-to-navigate — matching the scope `browser-linux`
//! itself started at before growing incrementally.
//!
//! Unlike `browser-linux`, this crate has never been linked or run: there is
//! no Windows/WebView2 toolchain (nor a real linker for the target) in the
//! environment it was written in. It has been type-checked successfully
//! against the real `windows` 0.62 crate via `cargo check --target
//! x86_64-pc-windows-gnu` (which caught and fixed several real API
//! mismatches — see git history), so the code compiles and its types line
//! up, but WebView2 embedding, layout, and actual runtime behavior are still
//! unverified. Build and run it on a real Windows machine and report back
//! anything that breaks.

use std::rc::Rc;

use render_engine::{RenderEngine, WryEngine};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW, GetWindowLongPtrW,
    LoadCursorW, PostMessageW, RegisterClassExW, SetWindowLongPtrW, ShowWindow, TranslateMessage,
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, IDC_ARROW, MSG, SW_SHOW,
    WM_COMMAND, WM_CREATE, WM_DESTROY, WM_KEYDOWN, WM_SIZE, WNDCLASSEXW, WS_CHILD, WS_EX_CLIENTEDGE,
    WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};

pub const HOME_URL: &str = "about:blank";

const ID_BACK: u16 = 1001;
const ID_FORWARD: u16 = 1002;
const ID_RELOAD: u16 = 1003;
const ID_GO: u16 = 1004;
const ID_ADDRESS: u16 = 1005;

/// VK_RETURN — not pulled from `windows::Win32::UI::Input::KeyboardAndMouse`
/// to avoid guessing at one more feature-gate name; this numeric value is a
/// stable, decades-old Win32 constant.
const VK_RETURN: u16 = 0x0D;

/// A private, app-defined message the address bar's subclass procedure
/// posts to the main window when Enter is pressed, since a plain (non-dialog)
/// window doesn't get a WM_COMMAND notification for that on its own.
const WM_APP_NAVIGATE: u32 = 0x8000 + 1;

const TOOLBAR_HEIGHT: i32 = 36;
const BUTTON_WIDTH: i32 = 60;
const GO_BUTTON_WIDTH: i32 = 40;
const MARGIN: i32 = 4;

struct AppState {
    engine: WryEngine,
    address_edit: HWND,
}

impl AppState {
    fn navigate_from_address_bar(&self) {
        let text = get_window_text(self.address_edit);
        if let Err(err) = self.engine.navigate(&text) {
            eprintln!("navigation failed: {err:?}");
        }
    }
}

/// Reads the current text out of an Edit control.
fn get_window_text(hwnd: HWND) -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..copied as usize])
    }
}

fn set_window_text(hwnd: HWND, text: &str) {
    use windows::Win32::UI::WindowsAndMessaging::SetWindowTextW;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SetWindowTextW(hwnd, windows::core::PCWSTR(wide.as_ptr()));
    }
}

/// Creates the main window and all of its chrome, including the embedded
/// webview (opened at `HOME_URL`). Assumes this runs on the thread that will
/// pump the message loop (`run_message_loop`).
pub fn create_window() -> anyhow::Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class_name = w!("ClaudeBrowserWindowClass");

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 as isize + 1) as *mut _),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            Default::default(),
            class_name,
            w!("claude-browser"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1024,
            768,
            None,
            None,
            Some(instance.into()),
            None,
        )?;

        let _ = ShowWindow(hwnd, SW_SHOW);

        Ok(hwnd)
    }
}

/// Standard Win32 message loop. Blocks until the window is closed.
pub fn run_message_loop() {
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn app_state(hwnd: HWND) -> Option<&'static AppState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if ptr == 0 {
        None
    } else {
        Some(unsafe { &*(ptr as *const AppState) })
    }
}

fn layout_toolbar(hwnd: HWND, app: &AppState) {
    let mut rect = Default::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;

    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{MoveWindow, GetDlgItem};

        let back = GetDlgItem(Some(hwnd), ID_BACK as i32).unwrap_or_default();
        let forward = GetDlgItem(Some(hwnd), ID_FORWARD as i32).unwrap_or_default();
        let reload = GetDlgItem(Some(hwnd), ID_RELOAD as i32).unwrap_or_default();
        let go = GetDlgItem(Some(hwnd), ID_GO as i32).unwrap_or_default();

        let mut x = MARGIN;
        let y = MARGIN;
        let button_h = TOOLBAR_HEIGHT - 2 * MARGIN;

        let _ = MoveWindow(back, x, y, BUTTON_WIDTH, button_h, true);
        x += BUTTON_WIDTH + MARGIN;
        let _ = MoveWindow(forward, x, y, BUTTON_WIDTH, button_h, true);
        x += BUTTON_WIDTH + MARGIN;
        let _ = MoveWindow(reload, x, y, BUTTON_WIDTH, button_h, true);
        x += BUTTON_WIDTH + MARGIN;

        let go_x = width - MARGIN - GO_BUTTON_WIDTH;
        let _ = MoveWindow(go, go_x, y, GO_BUTTON_WIDTH, button_h, true);

        let address_width = (go_x - MARGIN - x).max(0);
        let _ = MoveWindow(app.address_edit, x, y, address_width, button_h, true);
    }

    let content_y = TOOLBAR_HEIGHT;
    let content_h = (height - TOOLBAR_HEIGHT).max(0);
    if let Err(err) = app.engine.set_bounds(0, content_y, width.max(0) as u32, content_h as u32) {
        eprintln!("failed to resize webview: {err:?}");
    }
}

unsafe extern "system" fn edit_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> LRESULT {
    if msg == WM_KEYDOWN && wparam.0 as u16 == VK_RETURN {
        let parent = HWND(ref_data as *mut _);
        let _ = PostMessageW(Some(parent), WM_APP_NAVIGATE, WPARAM(0), LPARAM(0));
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            let instance = create.hInstance;

            let back = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                w!("Back"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(windows::Win32::UI::WindowsAndMessaging::HMENU(ID_BACK as usize as *mut _)),
                Some(instance),
                None,
            );
            let forward = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                w!("Forward"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(windows::Win32::UI::WindowsAndMessaging::HMENU(ID_FORWARD as usize as *mut _)),
                Some(instance),
                None,
            );
            let reload = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                w!("Reload"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(windows::Win32::UI::WindowsAndMessaging::HMENU(ID_RELOAD as usize as *mut _)),
                Some(instance),
                None,
            );
            let go = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                w!("Go"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(windows::Win32::UI::WindowsAndMessaging::HMENU(ID_GO as usize as *mut _)),
                Some(instance),
                None,
            );
            let address_edit = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(windows::Win32::UI::WindowsAndMessaging::HMENU(ID_ADDRESS as usize as *mut _)),
                Some(instance),
                None,
            );

            let (Ok(back), Ok(forward), Ok(reload), Ok(go), Ok(address_edit)) =
                (back, forward, reload, go, address_edit)
            else {
                return LRESULT(-1);
            };
            let _ = (back, forward, reload, go);

            let _ = SetWindowSubclass(address_edit, Some(edit_subclass_proc), 1, hwnd.0 as usize);
            set_window_text(address_edit, HOME_URL);

            let engine = match WryEngine::new(hwnd, HOME_URL, move |_title| {
                // Title tracking has no UI consumer yet in this first
                // milestone (no switcher grid) — nothing to update.
            }) {
                Ok(engine) => engine,
                Err(err) => {
                    eprintln!("failed to create webview: {err:?}");
                    return LRESULT(-1);
                }
            };

            let app = Rc::new(AppState { engine, address_edit });
            let raw = Rc::into_raw(app);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);

            LRESULT(0)
        }

        WM_SIZE => {
            if let Some(app) = app_state(hwnd) {
                layout_toolbar(hwnd, app);
            }
            LRESULT(0)
        }

        WM_COMMAND => {
            let control_id = (wparam.0 & 0xFFFF) as u16;
            if let Some(app) = app_state(hwnd) {
                match control_id {
                    ID_BACK => {
                        let _ = app.engine.go_back();
                    }
                    ID_FORWARD => {
                        let _ = app.engine.go_forward();
                    }
                    ID_RELOAD => {
                        let _ = app.engine.reload();
                    }
                    ID_GO => app.navigate_from_address_bar(),
                    _ => {}
                }
            }
            LRESULT(0)
        }

        WM_APP_NAVIGATE => {
            if let Some(app) = app_state(hwnd) {
                app.navigate_from_address_bar();
            }
            LRESULT(0)
        }

        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if ptr != 0 {
                let app = Rc::from_raw(ptr as *const AppState);
                drop(app);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
