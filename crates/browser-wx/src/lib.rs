//! wxDragon-based native chrome for the browser — an experiment in swapping
//! in a wholly different UI toolkit and its own embedded webview (wxWebView)
//! instead of wry, while reusing `browser_core`/`render_engine` unchanged.
//! Unlike the other three front ends, this crate is not `target_os`-gated:
//! wxWidgets is itself cross-platform, so one source tree builds natively on
//! Linux and cross-compiles to Windows unchanged.

mod engine;
mod titlebar;

pub use engine::WxEngine;
use titlebar::AddressBarValue;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use browser_core::{domain_of, resolve_address_input, PageManager, Profile, Settings};
use render_engine::RenderEngine;
use wxdragon::color::Colour;
use wxdragon::event::{EventToken, WindowEventData};
use wxdragon::prelude::*;
use wxdragon::widgets::choice::Choice;
use wxdragon::widgets::search_ctrl::SearchCtrl;
use wxdragon::widgets::simplebook::SimpleBook;
use wxdragon::widgets::spinctrl::SpinCtrl;

/// wxWidgets' `wxKeyCode` values wxdragon doesn't expose as named constants
/// (confirmed absent from the crate and its generated FFI bindings) — taken
/// directly from `wx/defs.h`'s `wxKeyCode` enum (`WXK_START = 300`, then a
/// fixed sequence through `WXK_F1`, counted by hand against the upstream
/// header rather than guessed).
mod keycode {
    pub const ESCAPE: i32 = 27;
    pub const DELETE: i32 = 127;
    pub const F1: i32 = 341;
}

/// Defers `f` to run on the next event-loop iteration via
/// `wxdragon::call_after`, rather than inline within whatever event handler
/// calls this.
///
/// Needed for any tile-grid action (closing a page, adding a page) that can
/// end up destroying the *current* tile: `rebuild_switcher_grid` tears down
/// and recreates every tile in `self.tiles`, and a tile's own click handler
/// (bound directly on that tile/its child labels) triggers exactly that —
/// so without deferring, a tile would destroy the native widget its own
/// click handler is still executing on top of, a reliably reproduced
/// segfault (confirmed via bisection: creating a second page from the
/// add-tile's own click handler crashes deep inside an unrelated widget's
/// method call a few statements later, consistent with corrupted/freed
/// memory from destroying a widget mid-callback — not reproducible when the
/// same `add_page` call is made programmatically, outside any click
/// handler). `wxdragon::call_after` requires `F: Send`; the wrapper below
/// asserts that unsafely, which only holds because this app is entirely
/// single-threaded and every deferred closure only ever runs on the same
/// wx main thread that queued it.
fn defer(f: impl FnOnce() + 'static) {
    struct SendOnMainThreadOnly<F>(F);
    unsafe impl<F> Send for SendOnMainThreadOnly<F> {}
    let wrapped = SendOnMainThreadOnly(f);
    wxdragon::call_after(Box::new(move || {
        // `let wrapped = wrapped;` forces the closure to capture the whole
        // wrapper (which we've unsafely asserted is Send), not just its `.0`
        // field directly — Rust 2021's disjoint closure captures would
        // otherwise capture the inner (non-Send) value alone, silently
        // defeating the wrapper.
        let wrapped = wrapped;
        (wrapped.0)()
    }));
}

/// Parses one of `browser_core`'s `#rrggbb` palette entries into a wx
/// `Colour` — falls back to a mid-gray if a color string is ever malformed
/// (can't happen with the fixed palette `PageManager` actually uses, but
/// `u8::from_str_radix` still needs a total fallback).
fn parse_hex_color(hex: &str) -> Colour {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(hex.get(0..2).unwrap_or(""), 16).unwrap_or(128);
    let g = u8::from_str_radix(hex.get(2..4).unwrap_or(""), 16).unwrap_or(128);
    let b = u8::from_str_radix(hex.get(4..6).unwrap_or(""), 16).unwrap_or(128);
    Colour::rgb(r, g, b)
}

pub struct AppState {
    frame: Frame,
    address_bar: titlebar::AddressBarHandle,
    page_book: SimpleBook,
    /// Order mirrors `page_book`'s internal page order exactly — every
    /// insertion/removal here is done in lockstep with a `page_book`
    /// `add_page`/`remove_page` call. Needed because, unlike GTK's `Stack`,
    /// `SimpleBook` addresses pages by integer position only, not by name.
    page_order: RefCell<Vec<String>>,
    /// Per-page container `Panel` (holding that page's `WxEngine`/`WebView`),
    /// keyed by page id — added to `page_book`, one per page, same role as
    /// `browser-linux-gtk3`'s `containers: HashMap<String, gtk::Box>`.
    containers: RefCell<HashMap<String, Panel>>,
    switcher_panel: Panel,
    search_ctrl: SearchCtrl,
    tiles_container: Panel,
    /// Tile/add-tile `Panel`s currently live in `tiles_container` — tracked
    /// explicitly (rather than queried back from `tiles_container`, which
    /// wxdragon's `Panel`/`Sizer` API has no "list my children" method for)
    /// so `rebuild_switcher_grid` knows what to destroy before rebuilding.
    tiles: RefCell<Vec<Panel>>,
    core: RefCell<PageManager<WxEngine>>,
    settings: RefCell<Settings>,
    profile: Profile,
    /// Guards against `rebuild_switcher_grid` running reentrantly (e.g. if a
    /// widget event fires synchronously partway through a rebuild already in
    /// progress) — cheap defensive check, kept alongside the `defer` fix in
    /// tile-click handlers below since that fix addresses a specific,
    /// confirmed cause of reentrancy-adjacent corruption, not reentrancy in
    /// general.
    rebuilding: std::cell::Cell<bool>,
}

impl AppState {
    pub fn settings(&self) -> std::cell::Ref<'_, Settings> {
        self.settings.borrow()
    }

    pub fn set_max_loaded_pages(self: &Rc<Self>, limit: Option<usize>) {
        self.settings.borrow_mut().max_loaded_pages = limit;
        let evicted = self.core.borrow_mut().set_max_loaded_pages(limit);
        self.unload_engines(&evicted);
        self.rebuild_switcher_grid();
    }

    pub fn is_page_loaded(&self, id: &str) -> bool {
        self.core.borrow().is_page_loaded(id)
    }
}

impl AppState {
    fn with_active<F: FnOnce(&WxEngine) -> anyhow::Result<()>>(&self, f: F) {
        let core = self.core.borrow();
        if let Some(page) = core.active() {
            if let Some(engine) = &page.engine {
                if let Err(err) = f(engine) {
                    eprintln!("action failed: {err}");
                }
            }
        }
    }

    /// Index of `id` in `page_book`'s page list — `page_order`'s position
    /// for it, since the two are always kept in lockstep.
    fn page_index(&self, id: &str) -> Option<usize> {
        self.page_order.borrow().iter().position(|pid| pid == id)
    }

    pub fn add_page(self: &Rc<Self>, url: &str) -> anyhow::Result<()> {
        let id = self.core.borrow_mut().allocate_id();

        let container = Panel::builder(&self.page_book).build();

        let title = Rc::new(RefCell::new(String::new()));
        let title_for_cb = Rc::clone(&title);
        let app_weak = Rc::downgrade(self);
        let engine = WxEngine::new(&container, url, move |new_title| {
            *title_for_cb.borrow_mut() = new_title;
            if let Some(app) = app_weak.upgrade() {
                app.rebuild_switcher_grid();
            }
        })?;

        self.page_book.add_page(&container, &id, true, None);
        self.page_order.borrow_mut().push(id.clone());
        self.containers.borrow_mut().insert(id.clone(), container);

        let evicted = self.core.borrow_mut().insert(id.clone(), engine, title);
        self.unload_engines(&evicted);

        self.set_active(&id);
        self.rebuild_switcher_grid();
        Ok(())
    }

    /// Tears down the engines for pages `PageManager` just flipped to
    /// unloaded — dropping a `WxEngine` destroys its `WebView`'s handle
    /// (wx tears down the native control when its owning window/container is
    /// destroyed; here the container `Panel` outlives it, so this alone
    /// doesn't reclaim the widget itself — that only happens if the
    /// container is later destroyed too, e.g. via `close_page`). Matches
    /// `browser-linux-gtk3`'s `unload_engines` in spirit: freezes `last_url`
    /// before dropping.
    fn unload_engines(&self, ids: &[String]) {
        let mut core = self.core.borrow_mut();
        for id in ids {
            if let Some(page) = core.page_mut(id) {
                if let Some(engine) = page.engine.take() {
                    page.last_url = engine.current_url().unwrap_or_else(|_| page.last_url.clone());
                    // engine drops here at end of scope
                }
            }
        }
    }

    fn ensure_engine_loaded(self: &Rc<Self>, id: &str) {
        let needs_engine = self.core.borrow().page(id).map(|p| p.engine.is_none()).unwrap_or(false);
        if !needs_engine {
            return;
        }
        let Some(container) = self.containers.borrow().get(id).copied() else { return };
        let (url, title) = {
            let core = self.core.borrow();
            let Some(page) = core.page(id) else { return };
            (page.last_url.clone(), Rc::clone(&page.title))
        };

        let title_for_cb = Rc::clone(&title);
        let app_weak = Rc::downgrade(self);
        match WxEngine::new(&container, &url, move |new_title| {
            *title_for_cb.borrow_mut() = new_title;
            if let Some(app) = app_weak.upgrade() {
                app.rebuild_switcher_grid();
            }
        }) {
            Ok(engine) => self.core.borrow_mut().install_engine(id, engine),
            Err(err) => eprintln!("failed to reload unloaded page: {err}"),
        }
    }

    fn set_active(self: &Rc<Self>, id: &str) {
        self.ensure_engine_loaded(id);
        self.core.borrow_mut().set_active(id);
        if let Some(index) = self.page_index(id) {
            // `page_book.add_page(..., select=true, ...)` (in add_page)
            // already selects a brand-new page — avoid a redundant call here
            // for that same page.
            if self.page_book.selection() != index as i32 {
                self.page_book.set_selection(index);
            }
        }
        if let Some(page) = self.core.borrow().page(id) {
            self.address_bar.set_address_value(&page.current_url());
        }
    }

    pub fn open_switcher(self: &Rc<Self>) {
        self.search_ctrl.set_value("");
        self.rebuild_switcher_grid();
        self.page_book.show(false);
        self.switcher_panel.show(true);
        self.switcher_panel.raise();
        self.search_ctrl.set_focus();
    }

    pub fn close_switcher(&self) {
        self.switcher_panel.hide();
        self.page_book.show(true);
    }

    fn matching_page_ids(&self, query: &str) -> Vec<String> {
        self.core.borrow().matching_ids(query)
    }

    pub fn switch_to(self: &Rc<Self>, id: &str) {
        self.set_active(id);
        self.close_switcher();
    }

    pub fn close_page(self: &Rc<Self>, id: &str) {
        let was_active = self.core.borrow().active_id() == id;

        self.core.borrow_mut().remove(id);
        if let Some(index) = self.page_index(id) {
            self.page_book.remove_page(index);
            self.page_order.borrow_mut().remove(index);
        }
        if let Some(container) = self.containers.borrow_mut().remove(id) {
            container.destroy();
        }

        if was_active {
            let next_id = self.core.borrow().pages().first().map(|p| p.id.clone());
            match next_id {
                Some(nid) => self.set_active(&nid),
                None => {
                    let start_page = self.settings.borrow().start_page.clone();
                    if let Err(err) = self.add_page(&start_page) {
                        eprintln!("failed to open replacement page: {err}");
                    }
                }
            }
        }
        self.rebuild_switcher_grid();
    }

    /// Rebuilds every tile in the switcher grid from scratch — used both for
    /// structural changes (page added/closed/renamed) and, unlike
    /// `browser-linux-gtk3`'s incremental `FlowBox` filter function, for
    /// every search-box keystroke too: wxdragon's `Sizer` has no
    /// remove-all-children method (nor a per-child visibility filter hook
    /// like `gtk::FlowBox::set_filter_func`), so the simplest correct
    /// approach is destroying and recreating the tiles container each time.
    /// Cheap enough at the scale of a handful of open pages.
    fn rebuild_switcher_grid(self: &Rc<Self>) {
        if self.rebuilding.get() {
            return;
        }
        self.rebuilding.set(true);
        for tile in self.tiles.borrow_mut().drain(..) {
            tile.destroy();
        }
        // Without overriding the default flags, WrapSizer stretches the
        // last item on each line (`ExtendLastOnEachLine`) to fill the row —
        // fine for text-flow layout, but it doesn't suit fixed-size tiles:
        // whichever tile lands last on a line would balloon to fill the
        // remaining width.
        let tiles_sizer = WrapSizer::builder(Orientation::Horizontal).with_flags(WrapSizerFlag::RemoveLeadingSpaces).build();

        let query = self.search_ctrl.get_value();
        let matches: std::collections::HashSet<String> = self.matching_page_ids(&query).into_iter().collect();

        {
            let core = self.core.borrow();
            for page in core.pages() {
                if !matches.contains(&page.id) {
                    continue;
                }
                let id = page.id.clone();
                let title_text = {
                    let t = page.title.borrow();
                    if t.is_empty() { "New Page".to_string() } else { t.clone() }
                };
                let url = page.current_url();
                let domain = domain_of(&url);
                let subtitle = if page.loaded { domain } else { format!("{domain} \u{b7} unloaded") };

                let tile = Panel::builder(&self.tiles_container).with_size(Size::new(150, 110)).build();
                tile.set_background_color(parse_hex_color(page.color));
                let tile_sizer = BoxSizer::builder(Orientation::Vertical).build();

                let close_row = BoxSizer::builder(Orientation::Horizontal).build();
                close_row.add_stretch_spacer(1);
                let close_btn = Button::builder(&tile).with_label("\u{d7}").with_size(Size::new(22, 22)).build();
                close_row.add(&close_btn, 0, SizerFlag::Top | SizerFlag::Right, 4);
                tile_sizer.add_sizer(&close_row, 0, SizerFlag::Expand, 0);

                tile_sizer.add_stretch_spacer(1);
                let title_label = StaticText::builder(&tile).with_label(&title_text).build();
                let subtitle_label = StaticText::builder(&tile).with_label(&subtitle).build();
                title_label.set_foreground_color(Colour::rgb(255, 255, 255));
                subtitle_label.set_foreground_color(Colour::rgb(230, 230, 230));
                tile_sizer.add(&title_label, 0, SizerFlag::Left | SizerFlag::All, 8);
                tile_sizer.add(&subtitle_label, 0, SizerFlag::Left, 8);
                tile.set_sizer(tile_sizer, true);

                let app_clone = Rc::clone(self);
                let id_clone = id.clone();
                close_btn.on_click(move |_| {
                    let app_clone = Rc::clone(&app_clone);
                    let id_clone = id_clone.clone();
                    defer(move || app_clone.close_page(&id_clone));
                });

                // Bind the click-to-switch handler on the tile panel *and*
                // both text labels: they're child windows drawn on top of
                // the panel, so a click landing on the text itself wouldn't
                // otherwise reach the panel's own handler.
                let app_clone = Rc::clone(self);
                let id_clone = id.clone();
                tile.on_mouse_left_down(move |_| {
                    app_clone.switch_to(&id_clone);
                });
                let app_clone = Rc::clone(self);
                let id_clone = id.clone();
                title_label.on_mouse_left_down(move |_| {
                    app_clone.switch_to(&id_clone);
                });
                let app_clone = Rc::clone(self);
                let id_clone = id.clone();
                subtitle_label.on_mouse_left_down(move |_| {
                    app_clone.switch_to(&id_clone);
                });

                let app_clone = Rc::clone(self);
                let id_clone = id.clone();
                tile.on_key_down(move |event| {
                    if let WindowEventData::Keyboard(kb) = event {
                        if kb.get_key_code() == Some(keycode::DELETE) {
                            let app_clone = Rc::clone(&app_clone);
                            let id_clone = id_clone.clone();
                            defer(move || app_clone.close_page(&id_clone));
                        } else {
                            kb.event.skip(true);
                        }
                    }
                });

                self.tiles.borrow_mut().push(tile);
                tiles_sizer.add(&tile, 0, SizerFlag::All, 8);
            }
        }

        let add_tile = Panel::builder(&self.tiles_container).with_size(Size::new(150, 110)).build();
        add_tile.set_background_color(Colour::rgb(225, 225, 225));
        let add_sizer = BoxSizer::builder(Orientation::Vertical).build();
        let add_label = StaticText::builder(&add_tile).with_label("+").build();
        add_sizer.add_stretch_spacer(1);
        add_sizer.add(&add_label, 0, SizerFlag::AlignCenterHorizontal, 0);
        add_sizer.add_stretch_spacer(1);
        add_tile.set_sizer(add_sizer, true);
        let app_clone = Rc::clone(self);
        let open_new_page = move || {
            let start_page = app_clone.settings.borrow().start_page.clone();
            if let Err(err) = app_clone.add_page(&start_page) {
                eprintln!("failed to open new page: {err}");
            }
            app_clone.close_switcher();
        };
        let open_new_page = Rc::new(open_new_page);
        {
            let open_new_page = Rc::clone(&open_new_page);
            add_tile.on_mouse_left_down(move |_| {
                let open_new_page = Rc::clone(&open_new_page);
                defer(move || open_new_page());
            });
        }
        {
            let open_new_page = Rc::clone(&open_new_page);
            add_label.on_mouse_left_down(move |_| {
                let open_new_page = Rc::clone(&open_new_page);
                defer(move || open_new_page());
            });
        }
        self.tiles.borrow_mut().push(add_tile);
        tiles_sizer.add(&add_tile, 0, SizerFlag::All, 8);

        self.tiles_container.set_sizer(tiles_sizer, true);
        self.tiles_container.layout();
        self.rebuilding.set(false);
    }

    pub fn page_ids(&self) -> Vec<String> {
        self.core.borrow().page_ids()
    }

    pub fn active_id(&self) -> String {
        self.core.borrow().active_id().to_string()
    }

    pub fn active_url(&self) -> Option<String> {
        self.core.borrow().active().map(|p| p.current_url())
    }

    pub fn page_title(&self, id: &str) -> Option<String> {
        self.core.borrow().page(id).map(|p| p.title.borrow().clone())
    }

    pub fn page_url(&self, id: &str) -> Option<String> {
        self.core.borrow().page(id).map(|p| p.current_url())
    }

    pub fn is_switcher_open(&self) -> bool {
        self.switcher_panel.is_shown()
    }
}

/// Shows a modal "Settings" dialog for editing `AppState.settings` — direct
/// analogue of `browser-linux-gtk3::show_settings_dialog`.
fn show_settings_dialog(app: &Rc<AppState>) {
    let dialog = Dialog::builder(&app.frame, "Settings").with_size(360, 220).build();
    let root_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let (current_start_page, current_engine, current_limit) = {
        let settings = app.settings();
        (settings.start_page.clone(), settings.default_search_engine.clone(), settings.max_loaded_pages)
    };

    let start_page_row = BoxSizer::builder(Orientation::Horizontal).build();
    start_page_row.add(&StaticText::builder(&dialog).with_label("Start page").build(), 0, SizerFlag::AlignCenterVertical | SizerFlag::All, 4);
    let start_page_entry = TextCtrl::builder(&dialog).with_value(&current_start_page).build();
    start_page_row.add(&start_page_entry, 1, SizerFlag::Expand | SizerFlag::All, 4);
    root_sizer.add_sizer(&start_page_row, 0, SizerFlag::Expand, 0);

    let engine_row = BoxSizer::builder(Orientation::Horizontal).build();
    engine_row.add(&StaticText::builder(&dialog).with_label("Search engine").build(), 0, SizerFlag::AlignCenterVertical | SizerFlag::All, 4);
    let engine_choice = Choice::builder(&dialog).build();
    let engines = app.settings().search_engines.clone();
    let mut selected_index = 0u32;
    for (i, engine) in engines.iter().enumerate() {
        engine_choice.append(&engine.name);
        if engine.name == current_engine {
            selected_index = i as u32;
        }
    }
    engine_choice.set_selection(selected_index);
    engine_row.add(&engine_choice, 1, SizerFlag::Expand | SizerFlag::All, 4);
    root_sizer.add_sizer(&engine_row, 0, SizerFlag::Expand, 0);

    let limit_row = BoxSizer::builder(Orientation::Horizontal).build();
    let unlimited_check = CheckBox::builder(&dialog).with_label("Unlimited").build();
    let limit_spin = SpinCtrl::builder(&dialog).with_range(1, 100).build();
    match current_limit {
        Some(n) => {
            unlimited_check.set_value(false);
            limit_spin.set_value(n as i32);
        }
        None => {
            unlimited_check.set_value(true);
            limit_spin.enable(false);
        }
    }
    unlimited_check.on_toggled(move |event| {
        limit_spin.enable(!event.is_checked());
    });
    limit_row.add(&unlimited_check, 0, SizerFlag::AlignCenterVertical | SizerFlag::All, 4);
    limit_row.add(&limit_spin, 0, SizerFlag::All, 4);
    root_sizer.add_sizer(&limit_row, 0, SizerFlag::Expand, 0);

    let button_row = BoxSizer::builder(Orientation::Horizontal).build();
    let cancel_btn = Button::builder(&dialog).with_label("Cancel").build();
    let save_btn = Button::builder(&dialog).with_label("Save").build();
    button_row.add_stretch_spacer(1);
    button_row.add(&cancel_btn, 0, SizerFlag::All, 4);
    button_row.add(&save_btn, 0, SizerFlag::All, 4);
    root_sizer.add_sizer(&button_row, 0, SizerFlag::Expand, 0);

    dialog.set_sizer(root_sizer, true);

    cancel_btn.on_click(move |_| {
        dialog.end_modal(ID_CANCEL);
    });
    {
        let app = Rc::clone(app);
        save_btn.on_click(move |_| {
            {
                let mut settings = app.settings.borrow_mut();
                settings.start_page = start_page_entry.get_value();
                if let Some(name) = engine_choice.get_string_selection() {
                    settings.default_search_engine = name;
                }
            }
            let new_limit = if unlimited_check.is_checked() { None } else { Some(limit_spin.value().max(1) as usize) };
            app.set_max_loaded_pages(new_limit);
            if let Err(err) = app.settings().save(&app.profile) {
                eprintln!("failed to save settings: {err}");
            }
            dialog.end_modal(ID_OK);
        });
    }

    dialog.show_modal();
    dialog.destroy();
}

/// Builds the full window + chrome and wires up all handlers. Does not
/// create any page — call `app.add_page(&app.settings().start_page.clone())`
/// afterward to open the first one. Direct analogue of
/// `browser-linux-gtk3::build_window_and_app`.
pub fn build_frame_and_app(profile: Profile) -> Rc<AppState> {
    let frame = Frame::builder().with_title("claude-browser").with_size(Size::new(1024, 768)).build();

    // Custom title bar, phase 1 (widget-building) — see crates/browser-wx/src/titlebar/.
    // Linux replaces the native title bar wholesale with a GTK header bar
    // (built here, before any other wx widget, mirroring browser-linux-gtk3's
    // own ordering); everywhere else keeps the native title bar and the wx
    // toolbar row below it exactly as before.
    #[cfg(target_os = "linux")]
    let linux_titlebar = titlebar::linux::build(&frame);

    let root_sizer = BoxSizer::builder(Orientation::Vertical).build();

    // --- Toolbar --- (not built on Linux: the GTK header bar above serves
    // this role instead, entirely outside wx's own sizer tree)
    #[cfg(not(target_os = "linux"))]
    let (toolbar_panel, back_button, forward_button, reload_button, address_bar, switcher_toggle, settings_button) = {
        let toolbar_panel = Panel::builder(&frame).build();
        let toolbar_sizer = BoxSizer::builder(Orientation::Horizontal).build();
        let back_button = Button::builder(&toolbar_panel).with_label("\u{25c0}").build();
        let forward_button = Button::builder(&toolbar_panel).with_label("\u{25b6}").build();
        let reload_button = Button::builder(&toolbar_panel).with_label("\u{27f3}").build();
        let address_bar = TextCtrl::builder(&toolbar_panel).with_style(TextCtrlStyle::ProcessEnter).build();
        let switcher_toggle = Button::builder(&toolbar_panel).with_label("\u{25a6}").build();
        let settings_button = Button::builder(&toolbar_panel).with_label("\u{2699}").build();
        toolbar_sizer.add(&back_button, 0, SizerFlag::All, 4);
        toolbar_sizer.add(&forward_button, 0, SizerFlag::All, 4);
        toolbar_sizer.add(&reload_button, 0, SizerFlag::All, 4);
        toolbar_sizer.add(&address_bar, 1, SizerFlag::Expand | SizerFlag::All, 4);
        toolbar_sizer.add(&switcher_toggle, 0, SizerFlag::All, 4);
        toolbar_sizer.add(&settings_button, 0, SizerFlag::All, 4);

        // Windows only: the custom title bar (installed below, once
        // toolbar_panel exists) strips the native min/max/close buttons
        // along with the caption, so add our own to the same row. These
        // don't need `Rc<AppState>` at all, so they're wired immediately.
        #[cfg(target_os = "windows")]
        {
            let minimize_button = Button::builder(&toolbar_panel).with_label("\u{2015}").with_size(Size::new(40, -1)).build();
            let maximize_button = Button::builder(&toolbar_panel).with_label("\u{25a1}").with_size(Size::new(40, -1)).build();
            let close_button = Button::builder(&toolbar_panel).with_label("\u{2715}").with_size(Size::new(40, -1)).build();
            toolbar_sizer.add(&minimize_button, 0, SizerFlag::All, 4);
            toolbar_sizer.add(&maximize_button, 0, SizerFlag::All, 4);
            toolbar_sizer.add(&close_button, 0, SizerFlag::All, 4);
            minimize_button.on_click(move |_| frame.iconize(true));
            maximize_button.on_click(move |_| frame.maximize(!frame.is_maximized()));
            close_button.on_click(move |_| frame.close(false));
        }

        toolbar_panel.set_sizer(toolbar_sizer, true);
        (toolbar_panel, back_button, forward_button, reload_button, address_bar, switcher_toggle, settings_button)
    };

    // Custom title bar, phase 2 (Windows): strip the native caption and
    // install the WM_NCHITTEST subclass now that toolbar_panel (whose row
    // acts as the caption) exists.
    #[cfg(target_os = "windows")]
    titlebar::windows::install(&frame, &toolbar_panel);

    // --- Content area: page_book and switcher_panel are unmanaged siblings
    // of the same content_panel parent, both manually sized to fill it (wx
    // has no direct analogue of GTK's `Overlay`), with the switcher raised
    // and shown/hidden on top when open.
    let content_panel = Panel::builder(&frame).build();
    let page_book = SimpleBook::builder(&content_panel).build();

    let switcher_panel = Panel::builder(&content_panel).build();
    let switcher_sizer = BoxSizer::builder(Orientation::Vertical).build();
    switcher_sizer.add_spacer(40);
    let search_ctrl = SearchCtrl::builder(&switcher_panel).with_style(SearchCtrlStyle::ProcessEnter).build();
    switcher_sizer.add(&search_ctrl, 0, SizerFlag::AlignCenterHorizontal | SizerFlag::All, 8);
    let tiles_container = Panel::builder(&switcher_panel).build();
    switcher_sizer.add(&tiles_container, 1, SizerFlag::Expand | SizerFlag::All, 16);
    let hint_label =
        StaticText::builder(&switcher_panel).with_label("\u{21b5} Switch to page   \u{2326} Close page").build();
    switcher_sizer.add(&hint_label, 0, SizerFlag::AlignCenterHorizontal | SizerFlag::All, 8);
    switcher_panel.set_sizer(switcher_sizer, true);

    let profile_label = StaticText::builder(&switcher_panel).with_label(&profile.name).build();

    content_panel.on_size(move |_evt| {
        let size = content_panel.get_client_size();
        page_book.set_size(size);
        switcher_panel.set_size(size);
        profile_label.set_size_with_pos(size.width - 140, 12, 120, 20);
    });

    switcher_panel.hide();

    #[cfg(not(target_os = "linux"))]
    root_sizer.add(&toolbar_panel, 0, SizerFlag::Expand, 0);
    root_sizer.add(&content_panel, 1, SizerFlag::Expand, 0);
    frame.set_sizer(root_sizer, true);

    let settings = Settings::load(&profile);
    let core = PageManager::new(settings.max_loaded_pages);
    let app = Rc::new(AppState {
        frame,
        #[cfg(target_os = "linux")]
        address_bar: linux_titlebar.address_bar.clone(),
        #[cfg(not(target_os = "linux"))]
        address_bar,
        page_book,
        page_order: RefCell::new(Vec::new()),
        containers: RefCell::new(HashMap::new()),
        switcher_panel,
        search_ctrl,
        tiles_container,
        tiles: RefCell::new(Vec::new()),
        core: RefCell::new(core),
        settings: RefCell::new(settings),
        profile,
        rebuilding: std::cell::Cell::new(false),
    });

    {
        let app = Rc::clone(&app);
        search_ctrl.on_text_updated(move |_| {
            app.rebuild_switcher_grid();
        });
    }
    {
        let app = Rc::clone(&app);
        search_ctrl.on_enter_pressed(move |_| {
            let text = app.search_ctrl.get_value();
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return;
            }
            match app.matching_page_ids(trimmed).as_slice() {
                [] => {
                    let url = resolve_address_input(trimmed, &app.settings());
                    if let Err(err) = app.add_page(&url) {
                        eprintln!("failed to open new page: {err}");
                    }
                    app.close_switcher();
                }
                [only] => app.switch_to(only),
                _ => {}
            }
        });
    }
    {
        let app = Rc::clone(&app);
        search_ctrl.on_key_down(move |event| {
            if let WindowEventData::Keyboard(kb) = event {
                if kb.get_key_code() == Some(keycode::ESCAPE) && app.is_switcher_open() {
                    app.close_switcher();
                } else {
                    kb.event.skip(true);
                }
            }
        });
    }

    // Toolbar button/address-bar/shortcut wiring — Linux's real GTK header
    // bar wires its own equivalent buttons (built as part of
    // titlebar::linux::build) entirely independently, since none of them are
    // wx widgets.
    #[cfg(target_os = "linux")]
    titlebar::linux::wire(&app, &linux_titlebar);

    #[cfg(not(target_os = "linux"))]
    {
        {
            let app = Rc::clone(&app);
            back_button.on_click(move |_| app.with_active(|p| p.go_back()));
        }
        {
            let app = Rc::clone(&app);
            forward_button.on_click(move |_| app.with_active(|p| p.go_forward()));
        }
        {
            let app = Rc::clone(&app);
            reload_button.on_click(move |_| app.with_active(|p| p.reload()));
        }
        {
            let app = Rc::clone(&app);
            address_bar.on_enter_pressed(move |_| {
                let text = app.address_bar.get_value();
                let url = resolve_address_input(&text, &app.settings());
                app.with_active(|p| p.navigate(&url));
            });
        }
        {
            let app = Rc::clone(&app);
            switcher_toggle.on_click(move |_| {
                if app.is_switcher_open() {
                    app.close_switcher();
                } else {
                    app.open_switcher();
                }
            });
        }
        {
            let app = Rc::clone(&app);
            settings_button.on_click(move |_| {
                show_settings_dialog(&app);
            });
        }

        // wx's raw key events don't bubble from a focused child control up to
        // the Frame (unlike GTK's, which do propagate to the toplevel unless a
        // child stops them) — and wxdragon doesn't wrap `wxAcceleratorTable`, the
        // usual fix for exactly this. Binding the same shortcut handler on the
        // Frame *and* the address bar covers the common cases (browsing, typing
        // a URL) without chasing every possible focus target.
        let _ = bind_shortcut_handler(&app, &app.frame);
        let _ = bind_shortcut_handler(&app, &app.address_bar);
    }

    app.frame.on_close(move |event| {
        if let WindowEventData::General(raw_event) = event {
            raw_event.skip(true);
        }
    });

    app.frame.show(true);
    app.frame.centre();
    // Sizer layout only takes effect once the frame is shown — force one
    // resize pass now so page_book/switcher_panel/profile_label get their
    // initial bounds instead of staying at their zero-size construction
    // default until the user manually resizes the window.
    app.frame.layout();
    let initial_size = content_panel.get_client_size();
    app.page_book.set_size(initial_size);
    app.switcher_panel.set_size(initial_size);

    app
}

/// Binds the F1 / Ctrl+T / Ctrl+L / Escape / Ctrl+W shortcut set (same as
/// `browser-linux-gtk3`) to `widget`'s `KeyDown` event.
fn bind_shortcut_handler<W: WindowEvents>(app: &Rc<AppState>, widget: &W) -> EventToken {
    let app = Rc::clone(app);
    widget.on_key_down(move |event| {
        let WindowEventData::Keyboard(kb) = event else { return };
        let ctrl = kb.control_down() || kb.cmd_down();
        let code = kb.get_key_code();
        let is_f1 = code == Some(keycode::F1);
        let is_t = ctrl && code == Some('T' as i32);
        let is_l = ctrl && code == Some('L' as i32);
        let is_w = ctrl && code == Some('W' as i32);
        let is_escape = code == Some(keycode::ESCAPE);

        if is_f1 || is_t || is_l {
            app.open_switcher();
        } else if is_escape && app.is_switcher_open() {
            app.close_switcher();
        } else if is_w {
            app.close_page(&app.active_id());
        } else {
            kb.event.skip(true);
        }
    })
}
