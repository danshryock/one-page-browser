//! A third front-end for the same feature set `browser-linux-gtk3` and
//! `browser-windows-win32` already have — multi-page `PageManager`, a
//! switcher, `Settings` persistence, real resource reclamation — built on
//! `native-windows-gui` (NWG) instead of raw Win32 calls, to compare a
//! higher-level Windows GUI toolkit against the hand-rolled approach.
//!
//! NWG's last published version (1.0.13) is from September 2022 and wraps
//! the older `winapi` crate rather than `windows-rs`; its own optional
//! `raw-window-handle` feature is pinned to an incompatible ancient version,
//! so it's unused here — every page's `WryEngine` is still built from a
//! plain `windows::Win32::Foundation::HWND`, obtained by converting the raw
//! `winapi::shared::windef::HWND` NWG's `Frame::handle.hwnd()` returns (see
//! `to_windows_hwnd`). Like `browser-windows-win32`, this crate has never
//! been linked or run — no WebView2 Runtime under Wine — so it's
//! cross-compile type-checked and manually reviewed only.
//!
//! NWG's `Frame` gives this front-end a real per-page container, unlike
//! `browser-windows-win32` (which had no widget analogous to GTK's per-page
//! `gtk::Box` and had to show/hide each page's webview directly) — hiding a
//! page's `Frame` hides its embedded webview too, via ordinary Win32
//! parent/child visibility, closer to `browser-linux-gtk3`'s actual model.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use browser_core::{domain_of, resolve_address_input, PageManager, Settings};
use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use render_engine::{RenderEngine, WryEngine};

const TOOLBAR_HEIGHT: i32 = 36;
const BUTTON_WIDTH: i32 = 60;
const GO_BUTTON_WIDTH: i32 = 40;
const MARGIN: i32 = 4;

/// Converts NWG's raw handle (a `winapi` type, since NWG predates
/// `windows-rs`) into the `windows`-crate `HWND` type
/// `render_engine::WryEngine::new` expects — both are just wrappers around
/// the same OS-level handle value, so this is a plain pointer cast.
fn to_windows_hwnd(hwnd: winapi::shared::windef::HWND) -> windows::Win32::Foundation::HWND {
    windows::Win32::Foundation::HWND(hwnd as *mut _)
}

/// Updates a page's shared title cell when its document title changes.
/// `browser-linux-gtk3`/`browser-windows-win32` also refresh their switcher
/// list live from this callback (via a captured `Weak` self-reference /
/// a posted custom window message respectively) — deliberately not done
/// here: doing so would require every `add_page` caller to also thread an
/// `Rc<App>`/`Weak<App>` through (this method only ever has plain `&self`,
/// unlike the settings dialog's button handler, which specifically needs
/// `RC_SELF` — see `on_settings`). Known, minor limitation: the switcher
/// list won't live-update a title while open; it's always fresh the next
/// time it's opened, since `open_switcher` calls `refresh_switcher_list`.
fn build_title_changed_callback(title: std::rc::Rc<RefCell<String>>) -> impl Fn(String) + 'static {
    move |new_title| {
        *title.borrow_mut() = new_title;
    }
}

#[derive(Default, NwgUi)]
pub struct App {
    #[nwg_control(size: (1024, 768), title: "claude-browser", flags: "MAIN_WINDOW|VISIBLE")]
    #[nwg_events(OnWindowClose: [App::on_window_close], OnResize: [App::layout_children], OnKeyPress: [App::on_key_press_general(SELF, EVT_DATA)], OnKeyRelease: [App::on_key_release(SELF, EVT_DATA)])]
    window: nwg::Window,

    #[nwg_control(text: "Back")]
    #[nwg_events(OnButtonClick: [App::on_back])]
    back_btn: nwg::Button,

    #[nwg_control(text: "Forward")]
    #[nwg_events(OnButtonClick: [App::on_forward])]
    forward_btn: nwg::Button,

    #[nwg_control(text: "Reload")]
    #[nwg_events(OnButtonClick: [App::on_reload])]
    reload_btn: nwg::Button,

    #[nwg_control(text: "")]
    #[nwg_events(OnKeyEnter: [App::on_go], OnKeyPress: [App::on_key_press_general(SELF, EVT_DATA)], OnKeyRelease: [App::on_key_release(SELF, EVT_DATA)])]
    address_edit: nwg::TextInput,

    #[nwg_control(text: "Go")]
    #[nwg_events(OnButtonClick: [App::on_go])]
    go_btn: nwg::Button,

    #[nwg_control(text: "Pages")]
    #[nwg_events(OnButtonClick: [App::on_switcher_toggle])]
    switcher_toggle_btn: nwg::Button,

    #[nwg_control(text: "Settings")]
    #[nwg_events(OnButtonClick: [App::on_settings(RC_SELF)])]
    settings_btn: nwg::Button,

    #[nwg_control(text: "")]
    #[nwg_events(OnKeyEnter: [App::on_switcher_activate], OnKeyEsc: [App::on_switcher_escape], OnTextInput: [App::on_switcher_search_changed], OnKeyPress: [App::on_key_press_general(SELF, EVT_DATA)], OnKeyRelease: [App::on_key_release(SELF, EVT_DATA)])]
    switcher_search_edit: nwg::TextInput,

    #[nwg_control]
    #[nwg_events(OnListBoxDoubleClick: [App::on_switcher_list_activate], OnKeyPress: [App::on_key_press_listbox(SELF, EVT_DATA)], OnKeyRelease: [App::on_key_release(SELF, EVT_DATA)])]
    switcher_listbox: nwg::ListBox<String>,

    #[nwg_control(text: "+ New Page")]
    #[nwg_events(OnButtonClick: [App::on_switcher_add])]
    switcher_add_btn: nwg::Button,

    #[nwg_control(text: "Enter: switch page   Delete: close page")]
    switcher_hint_label: nwg::Label,

    core: RefCell<PageManager<WryEngine>>,
    settings: RefCell<Settings>,
    /// Per-page containers — the NWG analogue of `browser-linux-gtk3`'s
    /// `containers: RefCell<HashMap<String, gtk::Box>>`. Hiding a page's
    /// `Frame` hides its embedded `WryEngine` too (ordinary Win32
    /// parent/child visibility), so unlike `browser-windows-win32` this
    /// front-end doesn't need to juggle each engine's own visibility.
    page_frames: RefCell<HashMap<String, nwg::Frame>>,
    /// Ids in the same order as `switcher_listbox`'s rows, for mapping a
    /// selected row back to a page id — same pattern
    /// `browser-windows-win32::switcher_row_ids` uses.
    switcher_row_ids: RefCell<Vec<String>>,
    /// Tracks whether Ctrl is currently held, updated by `on_key_press_general`/
    /// `on_key_release` — Win32 delivers a key event only to whichever
    /// control has focus, and NWG has no accelerator-table equivalent, so
    /// Ctrl+T/Ctrl+L/Ctrl+W are recognized this way instead (see module doc
    /// and the plan this was built from for why an accelerator table wasn't
    /// used: it would mean replacing NWG's own message loop, which can't be
    /// verified safe here).
    ctrl_held: Cell<bool>,
}

impl App {
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

    fn on_back(&self) {
        self.with_active(|p| p.go_back());
    }

    fn on_forward(&self) {
        self.with_active(|p| p.go_forward());
    }

    fn on_reload(&self) {
        self.with_active(|p| p.reload());
    }

    fn on_go(&self) {
        let text = self.address_edit.text();
        let url = resolve_address_input(&text, &self.settings());
        self.with_active(|engine| engine.navigate(&url));
    }

    fn active_id(&self) -> String {
        self.core.borrow().active_id().to_string()
    }

    pub fn add_page(&self, url: &str) -> anyhow::Result<()> {
        let id = self.core.borrow_mut().allocate_id();

        let mut frame = nwg::Frame::default();
        nwg::Frame::builder().flags(nwg::FrameFlags::VISIBLE).parent(&self.window).build(&mut frame)?;

        let title = std::rc::Rc::new(RefCell::new(String::new()));
        let raw_hwnd = frame.handle.hwnd().ok_or_else(|| anyhow::anyhow!("frame has no HWND"))?;
        let engine = WryEngine::new(to_windows_hwnd(raw_hwnd), url, build_title_changed_callback(std::rc::Rc::clone(&title)))?;

        self.page_frames.borrow_mut().insert(id.clone(), frame);

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
    /// WebView2 controller — same reclamation mechanism
    /// `browser-windows-win32::unload_engines` relies on.
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
    /// it back to its `last_url` into its still-tracked `Frame`. No-op if
    /// the page already has a live engine.
    fn ensure_engine_loaded(&self, id: &str) {
        let needs_engine = self.core.borrow().page(id).map(|p| p.engine.is_none()).unwrap_or(false);
        if !needs_engine {
            return;
        }
        let Some((last_url, title)) =
            self.core.borrow().page(id).map(|p| (p.last_url.clone(), std::rc::Rc::clone(&p.title)))
        else {
            return;
        };
        let Some(raw_hwnd) = self.page_frames.borrow().get(id).and_then(|f| f.handle.hwnd()) else {
            return;
        };

        match WryEngine::new(to_windows_hwnd(raw_hwnd), &last_url, build_title_changed_callback(title)) {
            Ok(engine) => self.core.borrow_mut().install_engine(id, engine),
            Err(err) => eprintln!("failed to reload unloaded page: {err:?}"),
        }
    }

    /// Makes `id` the active/visible page: hides every other page's `Frame`
    /// and shows+positions this one's.
    fn set_active(&self, id: &str) {
        self.ensure_engine_loaded(id);
        self.core.borrow_mut().set_active(id);
        self.layout_children();
        let url = self.core.borrow().page(id).map(|p| p.current_url()).unwrap_or_default();
        self.address_edit.set_text(&url);
    }

    fn switch_to(&self, id: &str) {
        self.set_active(id);
        self.close_switcher();
    }

    fn close_page(&self, id: &str) {
        let was_active = self.core.borrow().active_id() == id;
        self.core.borrow_mut().remove(id); // drops its Option<WryEngine>, tearing the widget down
        self.page_frames.borrow_mut().remove(id); // drops the Frame itself

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

    fn is_switcher_open(&self) -> bool {
        self.switcher_listbox.visible()
    }

    fn on_switcher_toggle(&self) {
        if self.is_switcher_open() {
            self.close_switcher();
        } else {
            self.open_switcher();
        }
    }

    fn open_switcher(&self) {
        self.switcher_search_edit.set_text("");
        self.refresh_switcher_list();
        self.switcher_search_edit.set_visible(true);
        self.switcher_listbox.set_visible(true);
        self.switcher_add_btn.set_visible(true);
        self.switcher_hint_label.set_visible(true);
        self.layout_children();
        self.switcher_search_edit.set_focus();
    }

    fn close_switcher(&self) {
        self.switcher_search_edit.set_visible(false);
        self.switcher_listbox.set_visible(false);
        self.switcher_add_btn.set_visible(false);
        self.switcher_hint_label.set_visible(false);
        self.layout_children();
    }

    /// Every NWG control defaults to visible on construction — `main.rs`
    /// calls this once right after `build_ui` to establish the switcher's
    /// actual initial (hidden) state. `close_switcher` is already
    /// idempotent, so this is just a `pub` door into it.
    pub fn close_switcher_for_startup(&self) {
        self.close_switcher();
    }

    /// Rebuilds `switcher_listbox`'s rows (and `switcher_row_ids` alongside
    /// them) from whatever's currently in the search box, via the same
    /// `matching_ids` substring match every front-end shares.
    fn refresh_switcher_list(&self) {
        let query = self.switcher_search_edit.text();
        let core = self.core.borrow();
        let ids = core.matching_ids(query.trim());

        let mut rows = Vec::with_capacity(ids.len());
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
            rows.push(label);
            row_ids.push(id);
        }
        self.switcher_listbox.set_collection(rows);
        *self.switcher_row_ids.borrow_mut() = row_ids;
    }

    fn switcher_selected_id(&self) -> Option<String> {
        let index = self.switcher_listbox.selection()?;
        self.switcher_row_ids.borrow().get(index).cloned()
    }

    fn on_switcher_list_activate(&self) {
        if let Some(id) = self.switcher_selected_id() {
            self.switch_to(&id);
        }
    }

    fn on_switcher_search_changed(&self) {
        self.refresh_switcher_list();
    }

    fn on_switcher_escape(&self) {
        if self.is_switcher_open() {
            self.close_switcher();
        }
    }

    fn on_switcher_activate(&self) {
        let text = self.switcher_search_edit.text();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let matches = self.core.borrow().matching_ids(trimmed);
        match matches.as_slice() {
            [] => {
                let url = resolve_address_input(trimmed, &self.settings());
                if let Err(err) = self.add_page(&url) {
                    eprintln!("failed to open new page: {err:?}");
                }
                self.close_switcher();
            }
            [only] => self.switch_to(only),
            _ => {}
        }
    }

    fn on_switcher_add(&self) {
        let start_page = self.settings().start_page.clone();
        if let Err(err) = self.add_page(&start_page) {
            eprintln!("failed to open new page: {err:?}");
        }
        self.close_switcher();
    }

    fn on_window_close(&self) {
        nwg::stop_thread_dispatch();
    }

    /// Positions the toolbar row, then either the switcher's controls (when
    /// open) or the active page's `Frame` in the content area below it.
    /// Called on `OnResize` and whenever the active page or switcher
    /// visibility changes — the single place that keeps "exactly one page
    /// visible, only while the switcher is closed" consistent, same role
    /// `browser-windows-win32::layout_children` plays.
    fn layout_children(&self) {
        let (width, height) = self.window.size();
        let (width, height) = (width as i32, height as i32);

        let mut x = MARGIN;
        let y = MARGIN;
        let button_h = TOOLBAR_HEIGHT - 2 * MARGIN;

        self.back_btn.set_position(x, y);
        self.back_btn.set_size(BUTTON_WIDTH as u32, button_h as u32);
        x += BUTTON_WIDTH + MARGIN;
        self.forward_btn.set_position(x, y);
        self.forward_btn.set_size(BUTTON_WIDTH as u32, button_h as u32);
        x += BUTTON_WIDTH + MARGIN;
        self.reload_btn.set_position(x, y);
        self.reload_btn.set_size(BUTTON_WIDTH as u32, button_h as u32);
        x += BUTTON_WIDTH + MARGIN;

        let settings_x = width - MARGIN - BUTTON_WIDTH;
        let switcher_x = settings_x - MARGIN - BUTTON_WIDTH;
        let go_x = switcher_x - MARGIN - GO_BUTTON_WIDTH;
        let address_width = (go_x - MARGIN - x).max(0);

        self.address_edit.set_position(x, y);
        self.address_edit.set_size(address_width as u32, button_h as u32);
        self.go_btn.set_position(go_x, y);
        self.go_btn.set_size(GO_BUTTON_WIDTH as u32, button_h as u32);
        self.switcher_toggle_btn.set_position(switcher_x, y);
        self.switcher_toggle_btn.set_size(BUTTON_WIDTH as u32, button_h as u32);
        self.settings_btn.set_position(settings_x, y);
        self.settings_btn.set_size(BUTTON_WIDTH as u32, button_h as u32);

        let content_y = TOOLBAR_HEIGHT;
        let content_h = (height - TOOLBAR_HEIGHT).max(0);
        let switcher_open = self.is_switcher_open();

        if switcher_open {
            const SEARCH_H: i32 = 28;
            const ROW_H: i32 = 28;
            const HINT_H: i32 = 20;
            let content_width = (width - 2 * MARGIN).max(0);

            self.switcher_search_edit.set_position(MARGIN, content_y + MARGIN);
            self.switcher_search_edit.set_size(content_width as u32, SEARCH_H as u32);

            let list_y = content_y + MARGIN + SEARCH_H + MARGIN;
            let list_h = (content_h - (SEARCH_H + ROW_H + HINT_H + MARGIN * 4)).max(0);
            self.switcher_listbox.set_position(MARGIN, list_y);
            self.switcher_listbox.set_size(content_width as u32, list_h as u32);

            let add_y = list_y + list_h + MARGIN;
            self.switcher_add_btn.set_position(MARGIN, add_y);
            self.switcher_add_btn.set_size(140, ROW_H as u32);

            let hint_y = add_y + ROW_H + MARGIN;
            self.switcher_hint_label.set_position(MARGIN, hint_y);
            self.switcher_hint_label.set_size(content_width as u32, HINT_H as u32);
        }

        let core = self.core.borrow();
        let active_id = core.active_id().to_string();
        let frames = self.page_frames.borrow();
        for page in core.pages() {
            let Some(frame) = frames.get(&page.id) else { continue };
            let should_show = !switcher_open && page.id == active_id;
            frame.set_visible(should_show);
            if should_show {
                frame.set_position(0, content_y);
                frame.set_size(width.max(0) as u32, content_h as u32);
            }
            if let Some(engine) = &page.engine {
                let _ = engine.set_visible(should_show);
            }
        }
    }

    /// Shared by every focusable fixed control except the switcher listbox
    /// (see `on_key_press_listbox`) — recognizes F1/Ctrl+T/Ctrl+L/Ctrl+W and
    /// tracks whether Ctrl is currently held (there's no accelerator-table
    /// equivalent wired up here — see the `ctrl_held` field doc).
    fn on_key_press_general(&self, data: &nwg::EventData) {
        let vk = data.on_key();
        if vk == nwg::keys::CONTROL {
            self.ctrl_held.set(true);
        }
        let ctrl = self.ctrl_held.get();
        if vk == nwg::keys::F1 || (ctrl && vk == nwg::keys::_T) || (ctrl && vk == nwg::keys::_L) {
            self.open_switcher();
        } else if ctrl && vk == nwg::keys::_W {
            let id = self.active_id();
            self.close_page(&id);
        }
    }

    /// Same as `on_key_press_general`, plus Delete-closes-the-selected-page
    /// — scoped to the listbox specifically so pressing Delete anywhere
    /// else (e.g. editing the address bar) doesn't accidentally close a
    /// page.
    fn on_key_press_listbox(&self, data: &nwg::EventData) {
        self.on_key_press_general(data);
        if data.on_key() == nwg::keys::DELETE {
            if let Some(id) = self.switcher_selected_id() {
                self.close_page(&id);
            }
        }
    }

    fn on_key_release(&self, data: &nwg::EventData) {
        if data.on_key() == nwg::keys::CONTROL {
            self.ctrl_held.set(false);
        }
    }

    /// Bound via the `RC_SELF` marker (not plain `SELF`): the settings
    /// dialog's OK/Cancel/checkbox handlers need to call back into `App`
    /// whenever the user eventually clicks them, which outlives this
    /// button-click event itself — an ordinary `&self` (what `SELF` gives)
    /// can't be moved into another closure with an independent lifetime,
    /// but `RC_SELF` gives `&Rc<App>`, which can be cloned into one. Every
    /// other handler in this file only acts within its own call, so plain
    /// `SELF`/`&self` suffices there (the derive macro's generated
    /// dispatcher already holds an `Rc<App>` internally and deref-coerces
    /// it to `&App` at the call site). Not a `&self` method itself — an
    /// associated function, since `RC_SELF` hands over the `Rc` wrapper,
    /// not a plain `&App`.
    fn on_settings(app: &std::rc::Rc<App>) {
        show_settings_dialog(std::rc::Rc::clone(app));
    }
}

/// Shows a "Settings" popup for editing `app`'s start page, default search
/// engine, and loaded-pages limit. Built imperatively (not via
/// `#[derive(NwgUi)]`, which is for a fixed, compile-time-known control set)
/// with a single `full_bind_event_handler` dispatching by control handle —
/// the standard NWG pattern for UIs assembled at runtime. Disables the main
/// window while shown (`window.set_enabled(false)`), same role
/// `browser-windows-win32`'s `EnableWindow(main_hwnd, false)` plays — but
/// unlike that crate, no separate nested message loop is needed: NWG's
/// `nwg::dispatch_thread_events()` already pumps every window that exists
/// on the thread, so simply showing this window is enough.
fn show_settings_dialog(app: std::rc::Rc<App>) {
    let (start_page, engine_names, current_engine, current_limit) = {
        let settings = app.settings();
        (
            settings.start_page.clone(),
            settings.search_engines.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
            settings.default_search_engine.clone(),
            settings.max_loaded_pages,
        )
    };

    let mut dialog = nwg::Window::default();
    if let Err(err) = nwg::Window::builder()
        .size((420, 230))
        .title("Settings")
        .flags(nwg::WindowFlags::WINDOW | nwg::WindowFlags::VISIBLE)
        .parent(Some(&app.window))
        .build(&mut dialog)
    {
        eprintln!("failed to create settings dialog: {err:?}");
        return;
    }

    let mut start_page_label = nwg::Label::default();
    let _ = nwg::Label::builder()
        .text("Start page")
        .position((10, 14))
        .size((90, 20))
        .parent(&dialog)
        .build(&mut start_page_label);

    let mut start_page_edit = nwg::TextInput::default();
    let _ = nwg::TextInput::builder()
        .text(&start_page)
        .position((110, 10))
        .size((290, 22))
        .parent(&dialog)
        .build(&mut start_page_edit);

    let mut engine_label = nwg::Label::default();
    let _ = nwg::Label::builder()
        .text("Search engine")
        .position((10, 46))
        .size((90, 20))
        .parent(&dialog)
        .build(&mut engine_label);

    let mut engine_combo = nwg::ComboBox::<String>::default();
    let _ = nwg::ComboBox::builder()
        .collection(engine_names.clone())
        .position((110, 42))
        .size((290, 150))
        .parent(&dialog)
        .build(&mut engine_combo);
    let current_engine_index = engine_names.iter().position(|n| *n == current_engine).unwrap_or(0);
    engine_combo.set_selection(Some(current_engine_index));

    let mut limit_label = nwg::Label::default();
    let _ = nwg::Label::builder()
        .text("Loaded pages limit")
        .position((10, 80))
        .size((150, 20))
        .parent(&dialog)
        .build(&mut limit_label);

    let mut limit_select = nwg::NumberSelect::default();
    let _ = nwg::NumberSelect::builder()
        .position((160, 76))
        .size((80, 22))
        .value_int(current_limit.unwrap_or(1) as i64)
        .min_int(1)
        .max_int(100_000)
        .parent(&dialog)
        .build(&mut limit_select);
    limit_select.set_enabled(current_limit.is_some());

    let mut unlimited_check = nwg::CheckBox::default();
    let _ = nwg::CheckBox::builder()
        .text("Unlimited")
        .check_state(if current_limit.is_none() { nwg::CheckBoxState::Checked } else { nwg::CheckBoxState::Unchecked })
        .position((250, 76))
        .size((100, 22))
        .parent(&dialog)
        .build(&mut unlimited_check);

    let mut ok_btn = nwg::Button::default();
    let _ =
        nwg::Button::builder().text("OK").position((240, 150)).size((80, 28)).parent(&dialog).build(&mut ok_btn);

    let mut cancel_btn = nwg::Button::default();
    let _ = nwg::Button::builder()
        .text("Cancel")
        .position((330, 150))
        .size((80, 28))
        .parent(&dialog)
        .build(&mut cancel_btn);

    app.window.set_enabled(false);

    let dialog_handle = dialog.handle;
    let unlimited_check_handle = unlimited_check.handle;
    let ok_handle = ok_btn.handle;
    let cancel_handle = cancel_btn.handle;

    nwg::full_bind_event_handler(&dialog_handle, move |evt, _data, handle| {
        match evt {
            nwg::Event::OnButtonClick if handle == unlimited_check_handle => {
                let unlimited = unlimited_check.check_state() == nwg::CheckBoxState::Checked;
                limit_select.set_enabled(!unlimited);
            }
            nwg::Event::OnButtonClick if handle == ok_handle => {
                let new_start_page = start_page_edit.text();
                let selected_engine = engine_combo.selection().and_then(|i| engine_names.get(i)).cloned();
                let is_unlimited = unlimited_check.check_state() == nwg::CheckBoxState::Checked;
                let limit_value = match limit_select.data() {
                    nwg::NumberSelectData::Int { value, .. } => value.max(1) as usize,
                    nwg::NumberSelectData::Float { value, .. } => (value as i64).max(1) as usize,
                };

                {
                    let mut settings = app.settings.borrow_mut();
                    settings.start_page = new_start_page.clone();
                    if let Some(name) = &selected_engine {
                        settings.default_search_engine = name.clone();
                    }
                }
                let new_limit = if is_unlimited { None } else { Some(limit_value) };
                app.set_max_loaded_pages(new_limit);
                if let Err(err) = app.settings().save() {
                    eprintln!("failed to save settings: {err:?}");
                }

                dialog.set_visible(false);
                app.window.set_enabled(true);
                dialog.close();
            }
            nwg::Event::OnButtonClick if handle == cancel_handle => {
                dialog.set_visible(false);
                app.window.set_enabled(true);
                dialog.close();
            }
            nwg::Event::OnWindowClose if handle == dialog_handle => {
                app.window.set_enabled(true);
            }
            _ => {}
        }
    });
}
