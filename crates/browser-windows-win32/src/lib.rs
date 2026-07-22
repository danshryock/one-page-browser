//! Native Win32 chrome for the browser, brought to feature parity with
//! `browser-linux-gtk3`: multi-page `browser_core::PageManager`-backed
//! navigation, a native page switcher, `Settings` with disk persistence, and
//! real resource reclamation for unloaded pages.
//!
//! Two deliberate departures from a literal port of `browser-linux-gtk3`, both
//! confirmed with the user up front given this crate's verification ceiling
//! (see below): the switcher is a plain `LISTBOX` of open pages, not a port
//! of Linux's GTK FlowBox tile grid (Win32 has no flow-layout widget), and
//! the settings screen is a hand-rolled popup window with its own small
//! message loop, not a resource-template (`.rc`/`DLGTEMPLATE`) dialog (this
//! project has no resource-compiler pipeline, and building one blind is
//! riskier than reusing the plain-window pattern already proven in this
//! file for the main window).
//!
//! Unlike `browser-linux-gtk3`, this crate has never been linked or run: there is
//! no Windows/WebView2 toolchain (nor a real linker for the target) in the
//! environment it was written in. It has been type-checked successfully
//! against the real `windows` 0.62 crate via `cargo check --target
//! x86_64-pc-windows-gnu` (which caught and fixed several real API
//! mismatches — see git history), so the code compiles and its types line
//! up, but WebView2 embedding, layout, and actual runtime behavior are still
//! unverified. Build and run it on a real Windows machine and report back
//! anything that breaks.
//!
//! Gated on the whole crate so a bare `cargo build`/`cross build --target
//! x86_64-pc-windows-gnu` across the whole workspace succeeds everywhere:
//! this crate compiles to an empty no-op on any other platform.
#![cfg(target_os = "windows")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Once;

use browser_core::{domain_of, resolve_address_input, PageManager, Profile, Settings};
use render_engine::{RenderEngine, WryEngine};
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{BST_CHECKED, BST_UNCHECKED};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus, VK_DELETE, VK_ESCAPE, VK_F1};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateAcceleratorTableW, CreateWindowExW, DefWindowProcW, DestroyAcceleratorTable, DestroyWindow,
    DispatchMessageW, GetClientRect, GetDlgItem, GetMessageW, GetWindowLongPtrW, IsWindow, IsWindowVisible,
    LoadCursorW, MoveWindow, PostMessageW, PostQuitMessage, RegisterClassExW, SendMessageW, SetWindowLongPtrW,
    ShowWindow, TranslateAcceleratorW, TranslateMessage, ACCEL, BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX,
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW,
    CW_USEDEFAULT, EN_CHANGE, ES_NUMBER, FCONTROL, FVIRTKEY, GWLP_USERDATA, HMENU, IDC_ARROW, LBN_DBLCLK,
    LBS_NOTIFY, LB_ADDSTRING, LB_GETCURSEL, LB_RESETCONTENT, MSG, SW_HIDE, SW_SHOW, WINDOW_STYLE, WM_APP,
    WM_COMMAND, WM_CREATE, WM_DESTROY, WM_KEYDOWN, WM_SIZE, WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE,
    WS_OVERLAPPEDWINDOW, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

const ID_BACK: u16 = 1001;
const ID_FORWARD: u16 = 1002;
const ID_RELOAD: u16 = 1003;
const ID_GO: u16 = 1004;
const ID_ADDRESS: u16 = 1005;
const ID_SWITCHER_TOGGLE: u16 = 1006;
const ID_SETTINGS: u16 = 1007;
const ID_SWITCHER_SEARCH: u16 = 1008;
const ID_SWITCHER_LIST: u16 = 1009;
const ID_SWITCHER_ADD: u16 = 1010;
const ID_SWITCHER_PROFILE_LABEL: u16 = 1011;

/// Command ids delivered via `WM_COMMAND` by the accelerator table (see
/// `run_message_loop`), rather than by a real control — chosen well outside
/// the 1001-1010 control-id range above so the two never collide.
const ID_ACCEL_OPEN_SWITCHER: u16 = 2001;
const ID_ACCEL_CLOSE_PAGE: u16 = 2002;
const ID_ACCEL_ESCAPE: u16 = 2003;

const ID_SETTINGS_START_PAGE_EDIT: u16 = 3001;
const ID_SETTINGS_ENGINE_COMBO: u16 = 3002;
const ID_SETTINGS_UNLIMITED_CHECK: u16 = 3003;
const ID_SETTINGS_LIMIT_EDIT: u16 = 3004;
const ID_SETTINGS_OK: u16 = 3005;
const ID_SETTINGS_CANCEL: u16 = 3006;

/// A private, app-defined message the address bar's subclass procedure
/// posts to the main window when Enter is pressed, since a plain (non-dialog)
/// window doesn't get a WM_COMMAND notification for that on its own.
const WM_APP_NAVIGATE: u32 = WM_APP + 1;
/// Posted by the switcher's search-edit subclass on Enter — mirrors
/// `browser-linux-gtk3`'s `search_entry.connect_activate`.
const WM_APP_SWITCHER_ACTIVATE: u32 = WM_APP + 2;
/// Posted by the switcher list's subclass on Delete — mirrors
/// `browser-linux-gtk3`'s flowbox `Delete`-closes-the-selected-tile binding.
const WM_APP_SWITCHER_DELETE: u32 = WM_APP + 3;
/// Posted by a page's title-changed callback so the switcher list (if open)
/// can refresh — GTK's signal closures let Linux capture a weak self
/// reference directly; a plain custom message is the equivalent Win32
/// idiom, already established here by `WM_APP_NAVIGATE`.
const WM_APP_TITLE_CHANGED: u32 = WM_APP + 4;

const TOOLBAR_HEIGHT: i32 = 36;
const BUTTON_WIDTH: i32 = 60;
const GO_BUTTON_WIDTH: i32 = 40;
const MARGIN: i32 = 4;

pub struct AppState {
    /// Also used as the parent for every page's `WryEngine` — every page's
    /// webview is a plain sibling child window of the main window, not
    /// nested inside a per-page container the way `browser-linux-gtk3` nests
    /// each page in its own `gtk::Box` (Win32 has no `Stack` widget to
    /// automate "show only the active one", so it's done by hand — see
    /// `layout_children`).
    hwnd: HWND,
    address_edit: HWND,
    switcher_search_edit: HWND,
    switcher_listbox: HWND,
    switcher_add_btn: HWND,
    switcher_hint_label: HWND,
    /// Shows the active profile's name in the upper-right corner of the
    /// switcher — shown/hidden alongside the other switcher-only controls.
    switcher_profile_label: HWND,
    core: RefCell<PageManager<WryEngine>>,
    settings: RefCell<Settings>,
    /// Ids in the same order as `switcher_listbox`'s rows — a plain LISTBOX
    /// only carries a display string per row, so this is how a selected row
    /// index maps back to a page id. Rebuilt every time the list is.
    switcher_row_ids: RefCell<Vec<String>>,
    /// Resolved once at startup (from `--profile`, defaulting to
    /// `"default"`) — kept around so the settings dialog's Save action can
    /// re-save to the same place `Settings::load` read from.
    profile: Profile,
}

impl AppState {
    pub fn settings(&self) -> std::cell::Ref<'_, Settings> {
        self.settings.borrow()
    }

    fn with_active<F: FnOnce(&WryEngine) -> anyhow::Result<()>>(&self, f: F) {
        let core = self.core.borrow();
        if let Some(page) = core.active() {
            if let Some(engine) = &page.engine {
                if let Err(err) = f(engine) {
                    eprintln!("action failed: {err:?}");
                }
            }
        }
    }

    fn navigate_from_address_bar(&self) {
        let text = get_window_text(self.address_edit);
        let url = resolve_address_input(&text, &self.settings());
        self.with_active(|engine| engine.navigate(&url));
    }

    fn active_id(&self) -> String {
        self.core.borrow().active_id().to_string()
    }

    pub fn add_page(&self, url: &str) -> anyhow::Result<()> {
        let id = self.core.borrow_mut().allocate_id();
        let title = Rc::new(RefCell::new(String::new()));
        let engine = WryEngine::new(self.hwnd, url, build_title_changed_callback(self.hwnd, Rc::clone(&title)))?;

        let evicted = self.core.borrow_mut().insert(id.clone(), engine, title);
        self.unload_engines(&evicted);

        self.set_active(&id);
        if self.is_switcher_open() {
            self.refresh_switcher_list();
        }
        Ok(())
    }

    /// Actually tears down the engines for pages `PageManager` just flipped
    /// to unloaded. Dropping a `wry::WebView` on Windows tears down its
    /// WebView2 controller — the analogous "the drop is the reclamation"
    /// mechanism `browser-linux-gtk3` relies on (there, confirmed directly via
    /// wry's WebKitGTK `Drop` impl; here, the expected/documented behavior
    /// of dropping a `WebView2Controller`).
    fn unload_engines(&self, ids: &[String]) {
        let mut core = self.core.borrow_mut();
        for id in ids {
            if let Some(page) = core.page_mut(id) {
                if let Some(engine) = page.engine.take() {
                    page.last_url = engine.current_url().unwrap_or_else(|_| page.last_url.clone());
                    drop(engine);
                }
            }
        }
    }

    /// Rebuilds the engine for a page that was previously unloaded, loading
    /// it back to its `last_url`. No-op if the page already has a live
    /// engine.
    fn ensure_engine_loaded(&self, id: &str) {
        let needs_engine = self.core.borrow().page(id).map(|p| p.engine.is_none()).unwrap_or(false);
        if !needs_engine {
            return;
        }
        let Some((last_url, title)) =
            self.core.borrow().page(id).map(|p| (p.last_url.clone(), Rc::clone(&p.title)))
        else {
            return;
        };

        match WryEngine::new(self.hwnd, &last_url, build_title_changed_callback(self.hwnd, title)) {
            Ok(engine) => self.core.borrow_mut().install_engine(id, engine),
            Err(err) => eprintln!("failed to reload unloaded page: {err:?}"),
        }
    }

    /// Makes `id` the active/visible page.
    fn set_active(&self, id: &str) {
        self.ensure_engine_loaded(id);
        self.core.borrow_mut().set_active(id);
        self.layout_children();
        let url = self.core.borrow().page(id).map(|p| p.current_url()).unwrap_or_default();
        set_window_text(self.address_edit, &url);
    }

    /// User explicitly picked a page to view — updates the active page and
    /// closes the switcher, mirroring `browser-linux-gtk3`'s `switch_to`.
    fn switch_to(&self, id: &str) {
        self.set_active(id);
        self.close_switcher();
    }

    fn close_page(&self, id: &str) {
        let was_active = self.core.borrow().active_id() == id;
        self.core.borrow_mut().remove(id); // dropping its Option<WryEngine> tears down the widget, same as an unload

        if was_active {
            let next_id = self.core.borrow().pages().first().map(|p| p.id.clone());
            match next_id {
                Some(next_id) => self.set_active(&next_id),
                None => {
                    let start_page = self.settings().start_page.clone();
                    if let Err(err) = self.add_page(&start_page) {
                        eprintln!("failed to open replacement page: {err:?}");
                    }
                }
            }
        }
        if self.is_switcher_open() {
            self.refresh_switcher_list();
        }
    }

    /// Changes the loaded-pages limit and enforces it immediately, tearing
    /// down any newly-evicted engines — used by the settings dialog's OK
    /// handler.
    fn set_max_loaded_pages(&self, limit: Option<usize>) {
        self.settings.borrow_mut().max_loaded_pages = limit;
        let evicted = self.core.borrow_mut().set_max_loaded_pages(limit);
        self.unload_engines(&evicted);
        if self.is_switcher_open() {
            self.refresh_switcher_list();
        }
    }

    /// Mirrors `browser-linux-gtk3`'s own approach: derive from real widget
    /// state instead of tracking a separate bool that could drift.
    fn is_switcher_open(&self) -> bool {
        unsafe { IsWindowVisible(self.switcher_listbox).as_bool() }
    }

    fn open_switcher(&self) {
        set_window_text(self.switcher_search_edit, "");
        self.refresh_switcher_list();
        unsafe {
            let _ = ShowWindow(self.switcher_search_edit, SW_SHOW);
            let _ = ShowWindow(self.switcher_listbox, SW_SHOW);
            let _ = ShowWindow(self.switcher_add_btn, SW_SHOW);
            let _ = ShowWindow(self.switcher_hint_label, SW_SHOW);
            let _ = ShowWindow(self.switcher_profile_label, SW_SHOW);
        }
        self.layout_children();
        unsafe {
            let _ = SetFocus(Some(self.switcher_search_edit));
        }
    }

    fn close_switcher(&self) {
        unsafe {
            let _ = ShowWindow(self.switcher_search_edit, SW_HIDE);
            let _ = ShowWindow(self.switcher_listbox, SW_HIDE);
            let _ = ShowWindow(self.switcher_add_btn, SW_HIDE);
            let _ = ShowWindow(self.switcher_hint_label, SW_HIDE);
            let _ = ShowWindow(self.switcher_profile_label, SW_HIDE);
        }
        self.layout_children();
    }

    /// Rebuilds `switcher_listbox`'s rows (and `switcher_row_ids` alongside
    /// them) from whatever's currently in the search box, via the same
    /// `matching_ids` substring match `browser-linux-gtk3`'s filter uses.
    fn refresh_switcher_list(&self) {
        let query = get_window_text(self.switcher_search_edit);
        let core = self.core.borrow();
        let ids = core.matching_ids(query.trim());

        unsafe {
            SendMessageW(self.switcher_listbox, LB_RESETCONTENT, None, None);
        }
        let mut row_ids = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(page) = core.page(&id) else { continue };
            let title = {
                let t = page.title.borrow();
                if t.is_empty() { "New Page".to_string() } else { t.clone() }
            };
            let domain = domain_of(&page.current_url());
            let label =
                if page.loaded { format!("{title} \u{2014} {domain}") } else { format!("{title} \u{2014} {domain} (unloaded)") };
            let wide: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                SendMessageW(self.switcher_listbox, LB_ADDSTRING, None, Some(LPARAM(wide.as_ptr() as isize)));
            }
            row_ids.push(id);
        }
        *self.switcher_row_ids.borrow_mut() = row_ids;
    }

    /// Maps the listbox's currently-selected row back to a page id via
    /// `switcher_row_ids` — `None` if nothing is selected.
    fn switcher_selected_id(&self) -> Option<String> {
        let index = unsafe { SendMessageW(self.switcher_listbox, LB_GETCURSEL, None, None) }.0;
        if index < 0 {
            return None;
        }
        self.switcher_row_ids.borrow().get(index as usize).cloned()
    }

    /// Positions the toolbar row, and then either the switcher's controls
    /// (when open) or the active page's webview (when closed) in the
    /// content area below it. Also the single place that enforces "exactly
    /// one page's webview is visible, and only while the switcher is
    /// closed" — Win32 has no `Stack` widget to do this automatically, so
    /// every page must be shown/hidden by hand here. Called on `WM_SIZE`
    /// and whenever the active page or switcher visibility changes.
    fn layout_children(&self) {
        let mut rect = Default::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rect);
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;

        unsafe {
            let back = GetDlgItem(Some(self.hwnd), ID_BACK as i32).unwrap_or_default();
            let forward = GetDlgItem(Some(self.hwnd), ID_FORWARD as i32).unwrap_or_default();
            let reload = GetDlgItem(Some(self.hwnd), ID_RELOAD as i32).unwrap_or_default();
            let go = GetDlgItem(Some(self.hwnd), ID_GO as i32).unwrap_or_default();
            let switcher_toggle = GetDlgItem(Some(self.hwnd), ID_SWITCHER_TOGGLE as i32).unwrap_or_default();
            let settings_btn = GetDlgItem(Some(self.hwnd), ID_SETTINGS as i32).unwrap_or_default();

            let mut x = MARGIN;
            let y = MARGIN;
            let button_h = TOOLBAR_HEIGHT - 2 * MARGIN;

            let _ = MoveWindow(back, x, y, BUTTON_WIDTH, button_h, true);
            x += BUTTON_WIDTH + MARGIN;
            let _ = MoveWindow(forward, x, y, BUTTON_WIDTH, button_h, true);
            x += BUTTON_WIDTH + MARGIN;
            let _ = MoveWindow(reload, x, y, BUTTON_WIDTH, button_h, true);
            x += BUTTON_WIDTH + MARGIN;

            let settings_x = width - MARGIN - BUTTON_WIDTH;
            let switcher_x = settings_x - MARGIN - BUTTON_WIDTH;
            let go_x = switcher_x - MARGIN - GO_BUTTON_WIDTH;
            let address_width = (go_x - MARGIN - x).max(0);

            let _ = MoveWindow(self.address_edit, x, y, address_width, button_h, true);
            let _ = MoveWindow(go, go_x, y, GO_BUTTON_WIDTH, button_h, true);
            let _ = MoveWindow(switcher_toggle, switcher_x, y, BUTTON_WIDTH, button_h, true);
            let _ = MoveWindow(settings_btn, settings_x, y, BUTTON_WIDTH, button_h, true);
        }

        let content_y = TOOLBAR_HEIGHT;
        let content_h = (height - TOOLBAR_HEIGHT).max(0);
        let switcher_open = self.is_switcher_open();

        if switcher_open {
            const SEARCH_H: i32 = 28;
            const ROW_H: i32 = 28;
            const HINT_H: i32 = 20;
            const PROFILE_LABEL_W: i32 = 140;
            let content_width = (width - 2 * MARGIN).max(0);
            let search_width = (content_width - PROFILE_LABEL_W - MARGIN).max(0);
            unsafe {
                let _ = MoveWindow(self.switcher_search_edit, MARGIN, content_y + MARGIN, search_width, SEARCH_H, true);
                let _ = MoveWindow(
                    self.switcher_profile_label,
                    MARGIN + search_width + MARGIN,
                    content_y + MARGIN,
                    (PROFILE_LABEL_W - MARGIN).max(0),
                    SEARCH_H,
                    true,
                );
                let list_y = content_y + MARGIN + SEARCH_H + MARGIN;
                let list_h = (content_h - (SEARCH_H + ROW_H + HINT_H + MARGIN * 4)).max(0);
                let _ = MoveWindow(self.switcher_listbox, MARGIN, list_y, content_width, list_h, true);
                let add_y = list_y + list_h + MARGIN;
                let _ = MoveWindow(self.switcher_add_btn, MARGIN, add_y, 140, ROW_H, true);
                let hint_y = add_y + ROW_H + MARGIN;
                let _ = MoveWindow(self.switcher_hint_label, MARGIN, hint_y, content_width, HINT_H, true);
            }
        }

        let core = self.core.borrow();
        let active_id = core.active_id().to_string();
        for page in core.pages() {
            let Some(engine) = &page.engine else { continue };
            let should_show = !switcher_open && page.id == active_id;
            let _ = engine.set_visible(should_show);
            if should_show {
                let _ = engine.set_bounds(0, content_y, width.max(0) as u32, content_h as u32);
            }
        }
    }
}

/// Builds the closure passed to `WryEngine::new` for a page's document-
/// title-changed handler: updates the page's shared title cell, then posts
/// `WM_APP_TITLE_CHANGED` so the switcher list (if open) can refresh.
/// `browser-linux-gtk3` captures a weak `Rc<AppState>` for the equivalent
/// refresh; Win32 has no per-widget signal-closure mechanism the way GTK
/// does, so posting a message to the main window and re-resolving
/// `app_state` there (the same idiom `WM_APP_NAVIGATE` already uses) is the
/// natural fit — it also means this closure only needs to capture a `Copy`
/// `HWND`, not an `Rc`/`Weak`.
fn build_title_changed_callback(hwnd: HWND, title: Rc<RefCell<String>>) -> impl Fn(String) + 'static {
    move |new_title| {
        *title.borrow_mut() = new_title;
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_APP_TITLE_CHANGED, WPARAM(0), LPARAM(0));
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

fn app_state(hwnd: HWND) -> Option<&'static AppState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if ptr == 0 {
        None
    } else {
        Some(unsafe { &*(ptr as *const AppState) })
    }
}

/// Creates the main window and all of its chrome, including the switcher's
/// (initially hidden) controls. Does not open any page — call
/// `app.add_page(&app.settings().start_page.clone())` afterward, same as
/// `browser-linux-gtk3`'s `build_window_and_app`/`main.rs` split.
///
/// `profile` scopes where `Settings` is loaded from/saved to (see
/// `browser_core::Profile`) — pass `Profile::default()` for the implicit
/// `"default"` profile, or a profile resolved from `--profile` via
/// `browser_core::resolve_profile_name`. Threaded into `WM_CREATE` via
/// `CreateWindowExW`'s `lpParam` (the same mechanism the settings dialog
/// already uses — see `show_settings_dialog`): a raw pointer to `profile`
/// itself is valid for this since `WM_CREATE` fires synchronously inside
/// this same call, well before `profile` (a local) could be dropped.
///
/// Returns an owned `Rc<AppState>` handle alongside the window: `WM_CREATE`
/// leaks one strong reference into the window's `GWLP_USERDATA` slot (which
/// `WM_DESTROY` reclaims via `Rc::from_raw` when the window closes, as
/// before); this function takes one more reference from that same
/// allocation (`Rc::increment_strong_count` + `Rc::from_raw`) to hand back
/// to the caller — the standard pattern for getting an extra owned handle
/// out of a raw pointer that already represents one, without disturbing the
/// original owner's bookkeeping.
pub fn create_window(profile: Profile) -> anyhow::Result<(HWND, Rc<AppState>)> {
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

        let profile_ptr = &profile as *const Profile as *const std::ffi::c_void;
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
            Some(profile_ptr),
        )?;

        let _ = ShowWindow(hwnd, SW_SHOW);

        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const AppState;
        Rc::increment_strong_count(ptr);
        let app = Rc::from_raw(ptr);

        Ok((hwnd, app))
    }
}

/// Standard Win32 message loop, extended with an accelerator table so
/// F1/Ctrl+T/Ctrl+L/Ctrl+W/Escape work regardless of which child control
/// currently has keyboard focus — `WM_KEYDOWN` is only delivered to the
/// focused control, not the main window, so a per-control subclass (as the
/// address bar and switcher controls already need for Enter/Delete) isn't
/// enough for shortcuts that must work everywhere. Blocks until the window
/// is closed.
pub fn run_message_loop(hwnd: HWND) {
    let haccel = unsafe {
        CreateAcceleratorTableW(&[
            ACCEL { fVirt: FVIRTKEY, key: VK_F1.0, cmd: ID_ACCEL_OPEN_SWITCHER },
            ACCEL { fVirt: FVIRTKEY | FCONTROL, key: b'T' as u16, cmd: ID_ACCEL_OPEN_SWITCHER },
            ACCEL { fVirt: FVIRTKEY | FCONTROL, key: b'L' as u16, cmd: ID_ACCEL_OPEN_SWITCHER },
            ACCEL { fVirt: FVIRTKEY | FCONTROL, key: b'W' as u16, cmd: ID_ACCEL_CLOSE_PAGE },
            ACCEL { fVirt: FVIRTKEY, key: VK_ESCAPE.0, cmd: ID_ACCEL_ESCAPE },
        ])
    };
    let haccel = match haccel {
        Ok(h) => Some(h),
        Err(err) => {
            eprintln!("failed to create accelerator table: {err:?}");
            None
        }
    };

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let translated = haccel.map(|h| TranslateAcceleratorW(hwnd, h, &msg) != 0).unwrap_or(false);
            if !translated {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if let Some(h) = haccel {
            let _ = DestroyAcceleratorTable(h);
        }
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

/// VK_RETURN — not pulled from `windows::Win32::UI::Input::KeyboardAndMouse`
/// to avoid guessing at one more feature-gate name (this file's original
/// rationale, from before this crate depended on that module for anything
/// else); this numeric value is a stable, decades-old Win32 constant.
const VK_RETURN: u16 = 0x0D;

/// Same pattern as `edit_subclass_proc`, for the switcher's search box:
/// posts `WM_APP_SWITCHER_ACTIVATE` on Enter instead of `WM_APP_NAVIGATE`.
unsafe extern "system" fn search_edit_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> LRESULT {
    if msg == WM_KEYDOWN && wparam.0 as u16 == VK_RETURN {
        let parent = HWND(ref_data as *mut _);
        let _ = PostMessageW(Some(parent), WM_APP_SWITCHER_ACTIVATE, WPARAM(0), LPARAM(0));
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// Catches Delete while the switcher's listbox has keyboard focus and posts
/// `WM_APP_SWITCHER_DELETE` — mirrors `browser-linux-gtk3`'s flowbox
/// `connect_key_press_event` Delete binding.
unsafe extern "system" fn switcher_list_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> LRESULT {
    if msg == WM_KEYDOWN && wparam.0 as u16 == VK_DELETE.0 {
        let parent = HWND(ref_data as *mut _);
        let _ = PostMessageW(Some(parent), WM_APP_SWITCHER_DELETE, WPARAM(0), LPARAM(0));
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            let instance = create.hInstance;
            let profile = &*(create.lpCreateParams as *const Profile);

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
                Some(HMENU(ID_BACK as usize as *mut _)),
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
                Some(HMENU(ID_FORWARD as usize as *mut _)),
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
                Some(HMENU(ID_RELOAD as usize as *mut _)),
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
                Some(HMENU(ID_GO as usize as *mut _)),
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
                Some(HMENU(ID_ADDRESS as usize as *mut _)),
                Some(instance),
                None,
            );
            let switcher_toggle = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                w!("Pages"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(ID_SWITCHER_TOGGLE as usize as *mut _)),
                Some(instance),
                None,
            );
            let settings_btn = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                w!("Settings"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(ID_SETTINGS as usize as *mut _)),
                Some(instance),
                None,
            );
            // The switcher's own controls start hidden (no WS_VISIBLE) —
            // `AppState::open_switcher`/`close_switcher` toggle them.
            let switcher_search_edit = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_TABSTOP,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(ID_SWITCHER_SEARCH as usize as *mut _)),
                Some(instance),
                None,
            );
            let switcher_listbox = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("LISTBOX"),
                w!(""),
                WINDOW_STYLE((WS_CHILD | WS_TABSTOP).0 | LBS_NOTIFY as u32),
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(ID_SWITCHER_LIST as usize as *mut _)),
                Some(instance),
                None,
            );
            let switcher_add_btn = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                w!("+ New Page"),
                WS_CHILD | WS_TABSTOP,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(ID_SWITCHER_ADD as usize as *mut _)),
                Some(instance),
                None,
            );
            let switcher_hint_label = CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!("Enter: switch page   Delete: close page"),
                WS_CHILD,
                0,
                0,
                0,
                0,
                Some(hwnd),
                None,
                Some(instance),
                None,
            );
            let switcher_profile_label = CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!(""),
                WS_CHILD,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(ID_SWITCHER_PROFILE_LABEL as usize as *mut _)),
                Some(instance),
                None,
            );

            let (
                Ok(back),
                Ok(forward),
                Ok(reload),
                Ok(go),
                Ok(address_edit),
                Ok(switcher_toggle),
                Ok(settings_btn),
                Ok(switcher_search_edit),
                Ok(switcher_listbox),
                Ok(switcher_add_btn),
                Ok(switcher_hint_label),
                Ok(switcher_profile_label),
            ) = (
                back,
                forward,
                reload,
                go,
                address_edit,
                switcher_toggle,
                settings_btn,
                switcher_search_edit,
                switcher_listbox,
                switcher_add_btn,
                switcher_hint_label,
                switcher_profile_label,
            )
            else {
                return LRESULT(-1);
            };
            let _ = (back, forward, reload, go, switcher_toggle, settings_btn);

            let _ = SetWindowSubclass(address_edit, Some(edit_subclass_proc), 1, hwnd.0 as usize);
            let _ = SetWindowSubclass(switcher_search_edit, Some(search_edit_subclass_proc), 1, hwnd.0 as usize);
            let _ = SetWindowSubclass(switcher_listbox, Some(switcher_list_subclass_proc), 1, hwnd.0 as usize);

            set_window_text(switcher_profile_label, &profile.name);

            let settings = Settings::load(profile);
            let core = PageManager::new(settings.max_loaded_pages);
            let app = Rc::new(AppState {
                hwnd,
                address_edit,
                switcher_search_edit,
                switcher_listbox,
                switcher_add_btn,
                switcher_hint_label,
                switcher_profile_label,
                core: RefCell::new(core),
                settings: RefCell::new(settings),
                switcher_row_ids: RefCell::new(Vec::new()),
                profile: profile.clone(),
            });
            let raw = Rc::into_raw(app);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);

            LRESULT(0)
        }

        WM_SIZE => {
            if let Some(app) = app_state(hwnd) {
                app.layout_children();
            }
            LRESULT(0)
        }

        WM_COMMAND => {
            let control_id = (wparam.0 & 0xFFFF) as u16;
            let notify_code = ((wparam.0 >> 16) & 0xFFFF) as u16;
            if let Some(app) = app_state(hwnd) {
                match control_id {
                    ID_BACK => app.with_active(|p| p.go_back()),
                    ID_FORWARD => app.with_active(|p| p.go_forward()),
                    ID_RELOAD => app.with_active(|p| p.reload()),
                    ID_GO => app.navigate_from_address_bar(),
                    ID_SWITCHER_TOGGLE => {
                        if app.is_switcher_open() {
                            app.close_switcher();
                        } else {
                            app.open_switcher();
                        }
                    }
                    ID_SETTINGS => show_settings_dialog(hwnd, app),
                    ID_SWITCHER_SEARCH if notify_code == EN_CHANGE as u16 => app.refresh_switcher_list(),
                    ID_SWITCHER_LIST if notify_code == LBN_DBLCLK as u16 => {
                        if let Some(id) = app.switcher_selected_id() {
                            app.switch_to(&id);
                        }
                    }
                    ID_SWITCHER_ADD => {
                        let start_page = app.settings().start_page.clone();
                        if let Err(err) = app.add_page(&start_page) {
                            eprintln!("failed to open new page: {err:?}");
                        }
                        app.close_switcher();
                    }
                    ID_ACCEL_OPEN_SWITCHER => app.open_switcher(),
                    ID_ACCEL_CLOSE_PAGE => {
                        let active = app.active_id();
                        app.close_page(&active);
                    }
                    ID_ACCEL_ESCAPE if app.is_switcher_open() => app.close_switcher(),
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

        WM_APP_SWITCHER_ACTIVATE => {
            if let Some(app) = app_state(hwnd) {
                let text = get_window_text(app.switcher_search_edit);
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    let matches = app.core.borrow().matching_ids(trimmed);
                    match matches.as_slice() {
                        [] => {
                            let url = resolve_address_input(trimmed, &app.settings());
                            if let Err(err) = app.add_page(&url) {
                                eprintln!("failed to open new page: {err:?}");
                            }
                            app.close_switcher();
                        }
                        [only] => app.switch_to(only),
                        _ => {}
                    }
                }
            }
            LRESULT(0)
        }

        WM_APP_SWITCHER_DELETE => {
            if let Some(app) = app_state(hwnd) {
                if let Some(id) = app.switcher_selected_id() {
                    app.close_page(&id);
                }
            }
            LRESULT(0)
        }

        WM_APP_TITLE_CHANGED => {
            if let Some(app) = app_state(hwnd) {
                if app.is_switcher_open() {
                    app.refresh_switcher_list();
                }
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
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

static SETTINGS_CLASS_REGISTERED: Once = Once::new();

fn register_settings_class(instance: HINSTANCE) {
    SETTINGS_CLASS_REGISTERED.call_once(|| unsafe {
        let class_name = w!("ClaudeBrowserSettingsDialogClass");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(settings_wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_WINDOW.0 as isize + 1) as *mut _),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    });
}

/// Shows a "Settings" popup for editing `app`'s start page, default search
/// engine, and loaded-pages limit — a hand-rolled window with its own small
/// message loop rather than a resource-template dialog (see module doc).
/// Disables `main_hwnd` while open (the standard hand-rolled-modal idiom,
/// and what actually blocks background interaction — playing the same role
/// `browser-linux-gtk3`'s `dialog.set_modal(true)` does) and re-enables it after.
fn show_settings_dialog(main_hwnd: HWND, app: &'static AppState) {
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return };
        let instance: HINSTANCE = module.into();
        register_settings_class(instance);

        let class_name = w!("ClaudeBrowserSettingsDialogClass");
        let lparam_ctx = app as *const AppState as *const std::ffi::c_void;
        let dialog_hwnd = match CreateWindowExW(
            Default::default(),
            class_name,
            w!("Settings"),
            WS_POPUP | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            420,
            270,
            Some(main_hwnd),
            None,
            Some(instance),
            Some(lparam_ctx),
        ) {
            Ok(h) => h,
            Err(err) => {
                eprintln!("failed to create settings dialog: {err:?}");
                return;
            }
        };

        let _ = EnableWindow(main_hwnd, false);
        let _ = ShowWindow(dialog_hwnd, SW_SHOW);

        let mut msg = MSG::default();
        while IsWindow(Some(dialog_hwnd)).as_bool() {
            if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = EnableWindow(main_hwnd, true);
    }
}

unsafe extern "system" fn settings_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            let instance = create.hInstance;
            // Threaded through `CreateWindowExW`'s `lpParam` (Win32's own
            // mechanism for exactly this: passing context a window needs
            // during its own WM_CREATE, before any other way to reach it
            // exists) rather than `GWLP_USERDATA`, which the *caller* only
            // gets to set *after* `CreateWindowExW` returns — too late for
            // WM_CREATE itself to use it.
            let app = &*(create.lpCreateParams as *const AppState);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as *const AppState as isize);

            let (start_page, engine_names, current_engine, current_limit) = {
                let settings = app.settings();
                (
                    settings.start_page.clone(),
                    settings.search_engines.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
                    settings.default_search_engine.clone(),
                    settings.max_loaded_pages,
                )
            };

            let _ = CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!("Start page"),
                WS_CHILD | WS_VISIBLE,
                10,
                14,
                90,
                20,
                Some(hwnd),
                None,
                Some(instance),
                None,
            );
            let start_page_edit = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                110,
                10,
                290,
                22,
                Some(hwnd),
                Some(HMENU(ID_SETTINGS_START_PAGE_EDIT as usize as *mut _)),
                Some(instance),
                None,
            )
            .unwrap_or_default();
            set_window_text(start_page_edit, &start_page);

            let _ = CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!("Search engine"),
                WS_CHILD | WS_VISIBLE,
                10,
                46,
                90,
                20,
                Some(hwnd),
                None,
                Some(instance),
                None,
            );
            let engine_combo = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("COMBOBOX"),
                w!(""),
                WINDOW_STYLE((WS_CHILD | WS_VISIBLE | WS_TABSTOP).0 | CBS_DROPDOWNLIST as u32),
                110,
                42,
                290,
                150,
                Some(hwnd),
                Some(HMENU(ID_SETTINGS_ENGINE_COMBO as usize as *mut _)),
                Some(instance),
                None,
            )
            .unwrap_or_default();
            let mut current_engine_index: i32 = 0;
            for (index, name) in engine_names.iter().enumerate() {
                let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
                SendMessageW(engine_combo, CB_ADDSTRING, None, Some(LPARAM(wide.as_ptr() as isize)));
                if *name == current_engine {
                    current_engine_index = index as i32;
                }
            }
            SendMessageW(engine_combo, CB_SETCURSEL, Some(WPARAM(current_engine_index as usize)), None);

            let _ = CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!("Loaded pages limit"),
                WS_CHILD | WS_VISIBLE,
                10,
                80,
                150,
                20,
                Some(hwnd),
                None,
                Some(instance),
                None,
            );
            let limit_edit = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                w!(""),
                WINDOW_STYLE((WS_CHILD | WS_VISIBLE | WS_TABSTOP).0 | ES_NUMBER as u32),
                160,
                76,
                60,
                22,
                Some(hwnd),
                Some(HMENU(ID_SETTINGS_LIMIT_EDIT as usize as *mut _)),
                Some(instance),
                None,
            )
            .unwrap_or_default();
            let unlimited_check = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                w!("Unlimited"),
                WINDOW_STYLE((WS_CHILD | WS_VISIBLE | WS_TABSTOP).0 | BS_AUTOCHECKBOX as u32),
                230,
                76,
                100,
                22,
                Some(hwnd),
                Some(HMENU(ID_SETTINGS_UNLIMITED_CHECK as usize as *mut _)),
                Some(instance),
                None,
            )
            .unwrap_or_default();
            match current_limit {
                Some(n) => {
                    set_window_text(limit_edit, &n.to_string());
                    SendMessageW(unlimited_check, BM_SETCHECK, Some(WPARAM(BST_UNCHECKED.0 as usize)), None);
                }
                None => {
                    set_window_text(limit_edit, "1");
                    SendMessageW(unlimited_check, BM_SETCHECK, Some(WPARAM(BST_CHECKED.0 as usize)), None);
                    let _ = EnableWindow(limit_edit, false);
                }
            }

            let _ = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                w!("OK"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                240,
                150,
                80,
                28,
                Some(hwnd),
                Some(HMENU(ID_SETTINGS_OK as usize as *mut _)),
                Some(instance),
                None,
            );
            let _ = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                w!("Cancel"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                330,
                150,
                80,
                28,
                Some(hwnd),
                Some(HMENU(ID_SETTINGS_CANCEL as usize as *mut _)),
                Some(instance),
                None,
            );

            LRESULT(0)
        }

        WM_COMMAND => {
            let control_id = (wparam.0 & 0xFFFF) as u16;
            match control_id {
                ID_SETTINGS_UNLIMITED_CHECK => {
                    let check = GetDlgItem(Some(hwnd), ID_SETTINGS_UNLIMITED_CHECK as i32).unwrap_or_default();
                    let limit_edit = GetDlgItem(Some(hwnd), ID_SETTINGS_LIMIT_EDIT as i32).unwrap_or_default();
                    let checked = SendMessageW(check, BM_GETCHECK, None, None).0 == BST_CHECKED.0 as isize;
                    let _ = EnableWindow(limit_edit, !checked);
                }
                ID_SETTINGS_OK => {
                    if let Some(app) = app_state(hwnd) {
                        let start_page_edit =
                            GetDlgItem(Some(hwnd), ID_SETTINGS_START_PAGE_EDIT as i32).unwrap_or_default();
                        let engine_combo = GetDlgItem(Some(hwnd), ID_SETTINGS_ENGINE_COMBO as i32).unwrap_or_default();
                        let unlimited_check =
                            GetDlgItem(Some(hwnd), ID_SETTINGS_UNLIMITED_CHECK as i32).unwrap_or_default();
                        let limit_edit = GetDlgItem(Some(hwnd), ID_SETTINGS_LIMIT_EDIT as i32).unwrap_or_default();

                        let new_start_page = get_window_text(start_page_edit);
                        let engine_index = SendMessageW(engine_combo, CB_GETCURSEL, None, None).0;
                        let is_unlimited =
                            SendMessageW(unlimited_check, BM_GETCHECK, None, None).0 == BST_CHECKED.0 as isize;
                        let limit_value = get_window_text(limit_edit).trim().parse::<usize>().unwrap_or(1).max(1);

                        {
                            let mut settings = app.settings.borrow_mut();
                            settings.start_page = new_start_page;
                            if engine_index >= 0 {
                                if let Some(name) =
                                    settings.search_engines.get(engine_index as usize).map(|e| e.name.clone())
                                {
                                    settings.default_search_engine = name;
                                }
                            }
                        }
                        let new_limit = if is_unlimited { None } else { Some(limit_value) };
                        app.set_max_loaded_pages(new_limit);
                        if let Err(err) = app.settings().save(&app.profile) {
                            eprintln!("failed to save settings: {err:?}");
                        }
                    }
                    let _ = DestroyWindow(hwnd);
                }
                ID_SETTINGS_CANCEL => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }

        WM_DESTROY => LRESULT(0),

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
