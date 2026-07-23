//! GTK3 native chrome for the browser — Linux only. Gated on the whole
//! crate (rather than leaving it to fail on `gtk` being unresolved) so a
//! bare `cargo build`/`cross build --target x86_64-pc-windows-gnu` across
//! the whole workspace succeeds everywhere: this crate just compiles to an
//! empty no-op on any other platform, symmetric with how
//! `browser-windows-win32`/`browser-windows-nwg` gate themselves to
//! `target_os = "windows"`.
#![cfg(target_os = "linux")]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use browser_core::{
    domain_of, list_profile_names, resolve_address_input, Action, Bookmarks, HistoryStore, KeyChord, Keybindings, PageManager,
    Profile, Settings,
};
use gtk::prelude::*;
use render_engine::{RenderEngine, WryEngine};

pub struct AppState {
    /// Doubles as the switcher grid's search box: while the switcher is
    /// open, typing here filters the tile grid (open pages + history)
    /// instead of doing anything to the active page, and Enter does the
    /// switcher's search-activate behavior instead of navigating — see
    /// `open_switcher`/`close_switcher` and the `connect_changed`/
    /// `connect_activate` wiring in `build_window_and_app`. One widget for
    /// both roles, not two, per the "unified search/URL bar" design.
    address_bar: gtk::Entry,
    stack: gtk::Stack,
    switcher_panel: gtk::Widget,
    flowbox: gtk::FlowBox,
    /// The settings overlay's root widget — an in-window overlay rather than
    /// a modal `gtk::Dialog` (matching `browser-windows-winui`'s settings
    /// surface, which led with this pattern since it needed to avoid
    /// `ContentDialog`'s async `ShowAsync`; gtk3 had no such constraint, this
    /// is purely for consistency between the two front ends).
    settings_panel: gtk::Widget,
    start_page_entry: gtk::Entry,
    engine_combo: gtk::ComboBoxText,
    /// Rebuilt from `Settings::search_engines` each time settings opens and
    /// after every add/remove — holds one row per engine, with a "×" to
    /// remove it.
    engines_list_box: gtk::Box,
    new_engine_name_entry: gtk::Entry,
    new_engine_url_entry: gtk::Entry,
    unlimited_check: gtk::CheckButton,
    limit_spin: gtk::SpinButton,
    /// The profile picker overlay's root widget — same in-window-overlay
    /// pattern as `settings_panel`/`switcher_panel`.
    profile_panel: gtk::Widget,
    /// Rebuilt from `browser_core::list_profile_names()` each time the
    /// picker opens — holds one row per existing profile.
    profile_list_box: gtk::Box,
    new_profile_entry: gtk::Entry,
    /// The keybindings editor's row list — lives inside the settings overlay
    /// (see `open_settings`'s doc comment for why it's not a separate
    /// overlay of its own), rebuilt from `Keybindings::bindings_for` each
    /// time settings opens (and after every add/remove).
    keybindings_list_box: gtk::Box,
    keybindings: RefCell<Keybindings>,
    /// `Some(action)` while the editor is waiting for the next real keydown
    /// to become that action's new binding — checked first, ahead of normal
    /// shortcut dispatch, by the window's `key-press-event` handler.
    listening_for: Cell<Option<Action>>,
    /// The bookmarks overlay's root widget — same in-window-overlay pattern
    /// as the other four.
    bookmarks_panel: gtk::Widget,
    /// Rebuilt from `Bookmarks::all()` each time the overlay opens and after
    /// every add/remove.
    bookmarks_list_box: gtk::Box,
    /// The toolbar star-toggle button, so its icon can be refreshed whenever
    /// the active page changes or a bookmark is added/removed for it.
    bookmark_toggle_button: gtk::Button,
    bookmarks: RefCell<Bookmarks>,
    core: RefCell<PageManager<WryEngine>>,
    /// GTK `Stack` children, keyed by page id — `browser_core::Page` doesn't
    /// hold these since they're a GTK-only concept.
    containers: RefCell<HashMap<String, gtk::Box>>,
    settings: RefCell<Settings>,
    history: HistoryStore,
    /// Resolved once at startup (from `--profile`, defaulting to
    /// `"default"`) — kept around so the settings overlay's Save action can
    /// re-save to the same place `Settings::load` read from, without
    /// re-parsing `std::env::args()`.
    profile: Profile,
}

impl AppState {
    pub fn settings(&self) -> std::cell::Ref<'_, Settings> {
        self.settings.borrow()
    }

    /// Applies a new loaded-pages limit: updates the stored setting, tells
    /// the live `PageManager` (which enforces it immediately, unlike a page
    /// reactivated via switching), and refreshes the grid so any resulting
    /// evictions show up right away. Used by the settings dialog's Save
    /// action, and reusable directly (e.g. from a test) without needing to
    /// drive that dialog's UI.
    pub fn set_max_loaded_pages(self: &Rc<Self>, limit: Option<usize>) {
        self.settings.borrow_mut().max_loaded_pages = limit;
        let evicted = self.core.borrow_mut().set_max_loaded_pages(limit);
        self.unload_engines(&evicted);
        self.rebuild_switcher_grid();
    }

    /// Whether `id` currently counts against the loaded-pages limit — test/
    /// inspection helper.
    pub fn is_page_loaded(&self, id: &str) -> bool {
        self.core.borrow().is_page_loaded(id)
    }
}

impl AppState {
    fn with_active<F: FnOnce(&WryEngine) -> anyhow::Result<()>>(&self, f: F) {
        let core = self.core.borrow();
        if let Some(page) = core.active() {
            if let Some(engine) = &page.engine {
                if let Err(err) = f(engine) {
                    eprintln!("action failed: {err}");
                }
            }
        }
    }

    pub fn add_page(self: &Rc<Self>, url: &str) -> anyhow::Result<()> {
        let id = self.core.borrow_mut().allocate_id();

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        self.stack.add_named(&container, &id);
        container.show_all();
        self.containers.borrow_mut().insert(id.clone(), container.clone());

        let title = Rc::new(RefCell::new(String::new()));
        let title_for_cb = Rc::clone(&title);
        let app_weak = Rc::downgrade(self);
        let id_for_cb = id.clone();
        let engine = WryEngine::new(&container, url, move |new_title| {
            *title_for_cb.borrow_mut() = new_title;
            if let Some(app) = app_weak.upgrade() {
                app.record_visit(&id_for_cb);
                app.rebuild_switcher_grid();
            }
        })?;

        let evicted = self.core.borrow_mut().insert(id.clone(), engine, title);
        self.unload_engines(&evicted);

        self.set_active(&id);
        self.rebuild_switcher_grid();
        Ok(())
    }

    /// Records a history visit for page `id`'s current URL/title — called
    /// from the `on_title_changed` callback (`add_page`/`ensure_engine_loaded`),
    /// the one place both are available together. Best-effort: logs rather
    /// than propagating, matching this codebase's existing error-handling
    /// style throughout (a failed history write shouldn't interrupt browsing).
    fn record_visit(&self, id: &str) {
        let core = self.core.borrow();
        let Some(page) = core.page(id) else { return };
        let url = page.current_url();
        let title = page.title.borrow().clone();
        drop(core);
        if let Err(err) = self.history.record_visit(&url, &title) {
            eprintln!("failed to record history visit: {err}");
        }
    }

    /// Actually tears down the engines for pages `PageManager` just flipped
    /// to unloaded — this is what turns the `loaded` bookkeeping flag into
    /// real resource reclamation. Dropping a `WryEngine` destroys its
    /// underlying GTK/WebKit widget (confirmed via wry's `InnerWebView` Drop
    /// impl, which calls `gtk_widget_destroy`), which also detaches it from
    /// its (now-empty, and since non-foreground, invisible) stack container.
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
    /// it back to its `last_url` into its existing (still-tracked) stack
    /// container. No-op if the page already has a live engine.
    fn ensure_engine_loaded(self: &Rc<Self>, id: &str) {
        let needs_engine = self.core.borrow().page(id).map(|p| p.engine.is_none()).unwrap_or(false);
        if !needs_engine {
            return;
        }
        let Some(container) = self.containers.borrow().get(id).cloned() else { return };
        let (url, title) = {
            let core = self.core.borrow();
            let Some(page) = core.page(id) else { return };
            (page.last_url.clone(), Rc::clone(&page.title))
        };

        let title_for_cb = Rc::clone(&title);
        let app_weak = Rc::downgrade(self);
        let id_for_cb = id.to_string();
        match WryEngine::new(&container, &url, move |new_title| {
            *title_for_cb.borrow_mut() = new_title;
            if let Some(app) = app_weak.upgrade() {
                app.record_visit(&id_for_cb);
                app.rebuild_switcher_grid();
            }
        }) {
            Ok(engine) => self.core.borrow_mut().install_engine(id, engine),
            Err(err) => eprintln!("failed to reload unloaded page: {err}"),
        }
    }

    /// Makes `id` the active/visible page, without touching the switcher
    /// panel's visibility — used wherever the active page changes as a side
    /// effect (creating a page, closing the active one) rather than as an
    /// explicit "go view this page" action from the user.
    fn set_active(self: &Rc<Self>, id: &str) {
        self.ensure_engine_loaded(id);
        self.core.borrow_mut().set_active(id);
        self.stack.set_visible_child_name(id);
        if let Some(page) = self.core.borrow().page(id) {
            self.address_bar.set_text(&page.current_url());
        }
        self.refresh_bookmark_toggle_button();
    }

    /// Shared by `open_switcher`/`open_switcher_editing_url`: everything
    /// about showing the grid except how the address bar's text ends up
    /// seeded, since that differs between the two (blank vs. the active
    /// page's current URL). The page stack is made insensitive so the
    /// background webview can't steal keyboard focus (or process key/pointer
    /// input at all) while the grid is up. Closes the other overlays first if
    /// any are open — the header bar's buttons are all reachable regardless
    /// of which overlay (if any) is currently shown, so every `open_*` method
    /// defensively closes the others rather than ever showing more than one
    /// at once.
    fn open_switcher_common(self: &Rc<Self>) {
        self.close_settings();
        self.close_profile_picker();
        self.close_bookmarks();
        self.rebuild_switcher_grid();
        self.stack.set_sensitive(false);
        self.switcher_panel.show();
        self.address_bar.grab_focus();
    }

    /// Opens the switcher grid with a cleared, focused search box, ready to
    /// filter to an open page or start typing a fresh one — used by the
    /// grid-button toggle as well as the F1 / Ctrl+T shortcuts ("grid, for a
    /// new page").
    pub fn open_switcher(self: &Rc<Self>) {
        self.address_bar.set_text("");
        self.address_bar.set_placeholder_text(Some("Type to filter open pages…"));
        self.open_switcher_common();
    }

    /// Opens the switcher grid with the address bar preloaded with the
    /// active page's current URL, fully selected rather than blanked —
    /// Ctrl+L's traditional "edit the URL" role, adapted to this browser's
    /// unified address bar ("grid, to edit the URL"): the grid is still
    /// shown underneath (so clicking another open page still works exactly
    /// like `open_switcher`), but retyping/pressing Enter acts on the
    /// current URL instead of starting from a blank filter.
    pub fn open_switcher_editing_url(self: &Rc<Self>) {
        let current_url = self.core.borrow().active().map(|p| p.current_url()).unwrap_or_default();
        self.address_bar.set_text(&current_url);
        self.address_bar.set_placeholder_text(None);
        self.open_switcher_common();
        self.address_bar.select_region(0, -1);
    }

    /// Hides the switcher grid and restores the page stack's sensitivity, as
    /// well as the address bar's text/placeholder — since the address bar
    /// doubles as the switcher's search box while it's open (see the field
    /// doc on `AppState::address_bar`), closing without having made a
    /// selection (e.g. pressing Escape after typing a filter) needs to put
    /// the active page's URL back, the same way `set_active` does when a
    /// selection *is* made.
    pub fn close_switcher(&self) {
        self.switcher_panel.hide();
        self.stack.set_sensitive(true);
        self.address_bar.set_placeholder_text(None);
        if let Some(page) = self.core.borrow().active() {
            self.address_bar.set_text(&page.current_url());
        }
    }

    /// Shows the settings overlay, populated from the current `Settings`,
    /// and rebuilds the keybindings editor's rows into the same overlay
    /// (moved here rather than being its own separate overlay/toolbar
    /// button — one "app configuration" destination instead of two). See
    /// `open_switcher`'s doc comment for why it closes the other overlays
    /// first.
    pub fn open_settings(self: &Rc<Self>) {
        self.close_switcher();
        self.close_profile_picker();
        self.close_bookmarks();
        let settings = self.settings.borrow();
        self.start_page_entry.set_text(&settings.start_page);
        match settings.max_loaded_pages {
            Some(n) => {
                self.unlimited_check.set_active(false);
                self.limit_spin.set_value(n as f64);
                self.limit_spin.set_sensitive(true);
            }
            None => {
                self.unlimited_check.set_active(true);
                self.limit_spin.set_sensitive(false);
            }
        }
        drop(settings);
        self.refresh_engine_combo();
        self.rebuild_engines_list();
        self.new_engine_name_entry.set_text("");
        self.new_engine_url_entry.set_text("");
        self.listening_for.set(None);
        self.rebuild_keybindings_list();
        self.stack.set_sensitive(false);
        self.settings_panel.show();
    }

    /// Hides the settings overlay without saving — used by Cancel, the
    /// scrim, and Escape. Always use this (rather than hiding
    /// `settings_panel` directly) so the stack never gets left insensitive.
    /// Also cancels any in-progress keybinding "press keys…" capture, same
    /// as closing used to when the keybindings editor was its own overlay.
    pub fn close_settings(&self) {
        self.listening_for.set(None);
        self.settings_panel.hide();
        self.stack.set_sensitive(true);
    }

    /// Reads the overlay's fields back into `Settings`, applies the loaded-
    /// pages limit immediately (via `set_max_loaded_pages`, same as before),
    /// saves to disk, and closes the overlay — the settings overlay's Save
    /// action.
    pub fn save_settings(self: &Rc<Self>) {
        {
            let mut settings = self.settings.borrow_mut();
            settings.start_page = self.start_page_entry.text().to_string();
            if let Some(id) = self.engine_combo.active_id() {
                settings.default_search_engine = id.to_string();
            }
        }
        let new_limit = if self.unlimited_check.is_active() {
            None
        } else {
            Some(self.limit_spin.value_as_int().max(1) as usize)
        };
        self.set_max_loaded_pages(new_limit);
        if let Err(err) = self.settings().save(&self.profile) {
            eprintln!("failed to save settings: {err}");
        }
        self.close_settings();
    }

    /// Whether the settings overlay is currently shown — test/inspection
    /// helper.
    pub fn is_settings_open(&self) -> bool {
        self.settings_panel.is_visible()
    }

    /// Repopulates the default-search-engine dropdown from the live
    /// `Settings::search_engines` (rather than a fixed list) and re-selects
    /// the current default — called whenever settings opens and after every
    /// engine add/remove, so it never goes stale.
    fn refresh_engine_combo(&self) {
        self.engine_combo.remove_all();
        let settings = self.settings.borrow();
        for engine in &settings.search_engines {
            self.engine_combo.append(Some(&engine.name), &engine.name);
        }
        self.engine_combo.set_active_id(Some(&settings.default_search_engine));
    }

    /// Rebuilds the search engine management list from scratch, one row per
    /// `Settings::search_engines` entry with its query URL template shown
    /// underneath and a "×" to remove it. The "×" is omitted entirely when
    /// only one engine remains, since `Settings::remove_search_engine`
    /// refuses to remove the last one anyway — no point offering a button
    /// that would just silently do nothing.
    fn rebuild_engines_list(self: &Rc<Self>) {
        for child in self.engines_list_box.children() {
            self.engines_list_box.remove(&child);
        }

        let engines = self.settings.borrow().search_engines.clone();
        let can_remove = engines.len() > 1;
        for engine in engines {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

            let labels = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let name_label = gtk::Label::new(Some(&engine.name));
            name_label.set_halign(gtk::Align::Start);
            let url_label = gtk::Label::new(Some(&engine.query_url_template));
            url_label.set_halign(gtk::Align::Start);
            url_label.style_context().add_class("tile-subtitle");
            labels.pack_start(&name_label, false, false, 0);
            labels.pack_start(&url_label, false, false, 0);
            row.pack_start(&labels, true, true, 0);

            if can_remove {
                let remove_button = gtk::Button::with_label("\u{d7}");
                let app_clone = Rc::clone(self);
                let name = engine.name.clone();
                remove_button.connect_clicked(move |_| {
                    app_clone.remove_search_engine_by_name(&name);
                });
                row.pack_start(&remove_button, false, false, 0);
            }

            self.engines_list_box.pack_start(&row, false, false, 0);
        }
        self.engines_list_box.show_all();
    }

    /// Removes a search engine by name, saves immediately, and refreshes
    /// both the management list and the default-engine dropdown (which may
    /// have had its selection reassigned, if the removed engine was the
    /// default — see `Settings::remove_search_engine`). The management
    /// list's "×" button action.
    pub fn remove_search_engine_by_name(self: &Rc<Self>, name: &str) {
        self.settings.borrow_mut().remove_search_engine(name);
        if let Err(err) = self.settings().save(&self.profile) {
            eprintln!("failed to save settings: {err}");
        }
        self.rebuild_engines_list();
        self.refresh_engine_combo();
    }

    /// Reads the "Add engine" row's fields and adds a new search engine
    /// (or updates an existing one with the same name), saves immediately,
    /// clears the fields, and refreshes both the management list and the
    /// dropdown. Does nothing if either field is blank.
    pub fn add_search_engine_from_fields(self: &Rc<Self>) {
        let name = self.new_engine_name_entry.text().to_string();
        let name = name.trim();
        let url = self.new_engine_url_entry.text().to_string();
        let url = url.trim();
        if name.is_empty() || url.is_empty() {
            return;
        }
        self.settings.borrow_mut().add_search_engine(name, url);
        if let Err(err) = self.settings().save(&self.profile) {
            eprintln!("failed to save settings: {err}");
        }
        self.new_engine_name_entry.set_text("");
        self.new_engine_url_entry.set_text("");
        self.rebuild_engines_list();
        self.refresh_engine_combo();
    }

    /// Shows the profile picker, rebuilt from `list_profile_names()` each
    /// time (so a profile created in an earlier visit to this picker shows
    /// up) — see `open_switcher`'s doc comment for why it closes the other
    /// overlays first.
    pub fn open_profile_picker(self: &Rc<Self>) {
        self.close_switcher();
        self.close_settings();
        self.close_bookmarks();
        self.new_profile_entry.set_text("");
        self.rebuild_profile_list();
        self.stack.set_sensitive(false);
        self.profile_panel.show();
    }

    /// Hides the profile picker. Always use this (rather than hiding
    /// `profile_panel` directly) so the stack never gets left insensitive.
    pub fn close_profile_picker(&self) {
        self.profile_panel.hide();
        self.stack.set_sensitive(true);
    }

    /// Whether the profile picker is currently shown — test/inspection
    /// helper.
    pub fn is_profile_picker_open(&self) -> bool {
        self.profile_panel.is_visible()
    }

    /// Rebuilds the profile picker's list of rows from scratch. The current
    /// profile is marked and, unlike every other row, clicking it just
    /// closes the picker instead of launching a duplicate process of the
    /// profile already running.
    fn rebuild_profile_list(self: &Rc<Self>) {
        for child in self.profile_list_box.children() {
            self.profile_list_box.remove(&child);
        }

        for name in list_profile_names() {
            let is_current = name == self.profile.name;
            let label_text = if is_current { format!("{name} (current)") } else { name.clone() };
            let row = gtk::Button::with_label(&label_text);
            row.style_context().add_class("flat");
            if is_current {
                row.style_context().add_class("current-profile-row");
            }

            let app_clone = Rc::clone(self);
            let name_clone = name.clone();
            row.connect_clicked(move |_| {
                if is_current {
                    app_clone.close_profile_picker();
                    return;
                }
                if let Err(err) = browser_core::launch_new_profile_process(&name_clone) {
                    eprintln!("failed to launch a new process for profile {name_clone:?}: {err}");
                }
                app_clone.close_profile_picker();
            });

            self.profile_list_box.pack_start(&row, false, false, 0);
        }
        self.profile_list_box.show_all();
    }

    /// Reads the new-profile field and launches a new process for it — the
    /// profile picker's "Create & Open" action. The new process creates the
    /// profile's directory lazily on first `Settings`/`HistoryStore` access;
    /// nothing needs pre-creating here.
    pub fn create_and_open_profile(&self) {
        let name = self.new_profile_entry.text().to_string();
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Err(err) = browser_core::launch_new_profile_process(name) {
            eprintln!("failed to launch a new process for profile {name:?}: {err}");
        }
        self.close_profile_picker();
    }

    /// Launches a new, independent private/incognito/guest window — the
    /// profile picker's "New Private Window" action. See
    /// `Profile::ephemeral`'s doc comment for exactly what "private" means.
    pub fn open_new_private_window(&self) {
        if let Err(err) = browser_core::launch_new_ephemeral_process() {
            eprintln!("failed to launch a new private window: {err}");
        }
        self.close_profile_picker();
    }

    /// Shows the bookmarks overlay, rebuilt from the current `Bookmarks`
    /// each time — see `open_switcher`'s doc comment for why it closes the
    /// other overlays first.
    pub fn open_bookmarks(self: &Rc<Self>) {
        self.close_switcher();
        self.close_settings();
        self.close_profile_picker();
        self.rebuild_bookmarks_list();
        self.stack.set_sensitive(false);
        self.bookmarks_panel.show();
    }

    /// Hides the bookmarks overlay. Always use this (rather than hiding
    /// `bookmarks_panel` directly) so the stack never gets left insensitive.
    pub fn close_bookmarks(&self) {
        self.bookmarks_panel.hide();
        self.stack.set_sensitive(true);
    }

    /// Whether the bookmarks overlay is currently shown — test/inspection
    /// helper.
    pub fn is_bookmarks_open(&self) -> bool {
        self.bookmarks_panel.is_visible()
    }

    /// Rebuilds the bookmarks overlay's list of rows from scratch, most-
    /// recently-added first (`Bookmarks::all()`'s order). Each row opens the
    /// bookmark as a new page when clicked; the "×" removes it without
    /// opening anything.
    fn rebuild_bookmarks_list(self: &Rc<Self>) {
        for child in self.bookmarks_list_box.children() {
            self.bookmarks_list_box.remove(&child);
        }

        let bookmarks = self.bookmarks.borrow();
        let all = bookmarks.all();
        if all.is_empty() {
            let empty_label = gtk::Label::new(Some("No bookmarks yet"));
            empty_label.set_halign(gtk::Align::Start);
            self.bookmarks_list_box.pack_start(&empty_label, false, false, 0);
        }
        for bookmark in all {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

            let open_button = gtk::Button::new();
            open_button.set_hexpand(true);
            open_button.style_context().add_class("flat");
            let label_text = if bookmark.title.is_empty() { bookmark.url.clone() } else { format!("{} — {}", bookmark.title, bookmark.domain) };
            let label = gtk::Label::new(Some(&label_text));
            label.set_halign(gtk::Align::Start);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            open_button.add(&label);

            let app_clone = Rc::clone(self);
            let url = bookmark.url.clone();
            open_button.connect_clicked(move |_| {
                if let Err(err) = app_clone.add_page(&url) {
                    eprintln!("failed to open bookmark: {err}");
                }
                app_clone.close_bookmarks();
            });
            row.pack_start(&open_button, true, true, 0);

            let remove_button = gtk::Button::with_label("\u{d7}");
            let app_clone = Rc::clone(self);
            let url = bookmark.url.clone();
            remove_button.connect_clicked(move |_| {
                app_clone.bookmarks.borrow_mut().remove(&url);
                if let Err(err) = app_clone.bookmarks.borrow().save(&app_clone.profile) {
                    eprintln!("failed to save bookmarks: {err}");
                }
                app_clone.rebuild_bookmarks_list();
                app_clone.refresh_bookmark_toggle_button();
            });
            row.pack_start(&remove_button, false, false, 0);

            self.bookmarks_list_box.pack_start(&row, false, false, 0);
        }
        self.bookmarks_list_box.show_all();
    }

    /// Adds or removes a bookmark for the active page — the toolbar star
    /// button's toggle action, and the `ToggleBookmark` keybinding.
    pub fn toggle_bookmark_for_active(self: &Rc<Self>) {
        let (url, title) = {
            let core = self.core.borrow();
            let Some(page) = core.active() else { return };
            let title = page.title.borrow().clone();
            (page.current_url(), title)
        };
        self.bookmarks.borrow_mut().toggle(&url, &title, now_unix());
        if let Err(err) = self.bookmarks.borrow().save(&self.profile) {
            eprintln!("failed to save bookmarks: {err}");
        }
        self.refresh_bookmark_toggle_button();
    }

    /// Updates the toolbar star button's icon/tooltip to reflect whether the
    /// active page is currently bookmarked — called whenever the active page
    /// changes or a bookmark is toggled, so it never shows stale state.
    fn refresh_bookmark_toggle_button(&self) {
        let is_bookmarked = self
            .core
            .borrow()
            .active()
            .map(|p| self.bookmarks.borrow().is_bookmarked(&p.current_url()))
            .unwrap_or(false);
        let icon_name = if is_bookmarked { "starred-symbolic" } else { "non-starred-symbolic" };
        self.bookmark_toggle_button
            .set_image(Some(&gtk::Image::from_icon_name(Some(icon_name), gtk::IconSize::Button)));
        self.bookmark_toggle_button
            .set_tooltip_text(Some(if is_bookmarked { "Remove bookmark" } else { "Bookmark this page" }));
    }

    /// Whether the active page is currently bookmarked — test/inspection
    /// helper.
    pub fn is_active_bookmarked(&self) -> bool {
        self.core
            .borrow()
            .active()
            .map(|p| self.bookmarks.borrow().is_bookmarked(&p.current_url()))
            .unwrap_or(false)
    }

    /// Bookmarked URLs, most-recently-added first — test/inspection helper.
    pub fn bookmarked_urls(&self) -> Vec<String> {
        self.bookmarks.borrow().all().iter().map(|b| b.url.clone()).collect()
    }

    /// Number of rows currently shown in the keybindings editor (folded into
    /// the settings overlay — see `open_settings`'s doc comment) — test/
    /// inspection helper confirming it's actually populated when settings
    /// opens, one row per `Action::ALL`.
    pub fn keybindings_row_count(&self) -> usize {
        self.keybindings_list_box.children().len()
    }

    /// Called from the window's `key-press-event` handler when
    /// `listening_for` is set: assigns `chord` as a new binding for that
    /// action (in addition to any existing ones — the editor's "Add
    /// binding" always adds, "×" on a tag is what removes), saves, clears
    /// the listening state, and rebuilds the rows.
    fn assign_listening_binding(self: &Rc<Self>, chord: KeyChord) {
        let Some(action) = self.listening_for.take() else { return };
        let mut chords = self.keybindings.borrow().bindings_for(action).to_vec();
        if !chords.contains(&chord) {
            chords.push(chord);
        }
        self.keybindings.borrow_mut().set_bindings(action, chords);
        if let Err(err) = self.keybindings.borrow().save(&self.profile) {
            eprintln!("failed to save keybindings: {err}");
        }
        self.rebuild_keybindings_list();
    }

    /// Rebuilds the keybindings editor's rows from scratch — one per
    /// `Action::ALL`, each showing its label, its current chords as
    /// removable tags, and an "Add binding" button (which shows "Press
    /// keys…" instead while listening for that specific action).
    fn rebuild_keybindings_list(self: &Rc<Self>) {
        for child in self.keybindings_list_box.children() {
            self.keybindings_list_box.remove(&child);
        }

        for &action in Action::ALL {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

            let label = gtk::Label::new(Some(action.label()));
            label.set_width_chars(18);
            label.set_halign(gtk::Align::Start);
            row.pack_start(&label, false, false, 0);

            let chords_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            let chords: Vec<KeyChord> = self.keybindings.borrow().bindings_for(action).to_vec();
            for chord in chords {
                let tag = gtk::Button::with_label(&format!("{chord} \u{d7}"));
                tag.style_context().add_class("flat");
                let app_clone = Rc::clone(self);
                tag.connect_clicked(move |_| {
                    let mut remaining = app_clone.keybindings.borrow().bindings_for(action).to_vec();
                    remaining.retain(|c| *c != chord);
                    app_clone.keybindings.borrow_mut().set_bindings(action, remaining);
                    if let Err(err) = app_clone.keybindings.borrow().save(&app_clone.profile) {
                        eprintln!("failed to save keybindings: {err}");
                    }
                    app_clone.rebuild_keybindings_list();
                });
                chords_box.pack_start(&tag, false, false, 0);
            }
            row.pack_start(&chords_box, true, true, 0);

            let listening = self.listening_for.get() == Some(action);
            let add_button = gtk::Button::with_label(if listening { "Press keys\u{2026}" } else { "Add binding" });
            let app_clone = Rc::clone(self);
            add_button.connect_clicked(move |_| {
                app_clone.listening_for.set(Some(action));
                app_clone.rebuild_keybindings_list();
            });
            row.pack_start(&add_button, false, false, 0);

            self.keybindings_list_box.pack_start(&row, false, false, 0);
        }
        self.keybindings_list_box.show_all();
    }

    /// Runs whatever `action` means — the shared target of both normal
    /// keydown dispatch and (indirectly, since it's just `AppState` methods)
    /// the toolbar buttons.
    fn dispatch_action(self: &Rc<Self>, action: Action) {
        match action {
            Action::OpenSwitcher => self.open_switcher(),
            Action::EditUrl => self.open_switcher_editing_url(),
            Action::ClosePage => self.close_page(&self.active_id()),
            Action::Reload => self.with_active(|p| p.reload()),
            Action::GoBack => self.with_active(|p| p.go_back()),
            Action::GoForward => self.with_active(|p| p.go_forward()),
            Action::OpenSettings => self.open_settings(),
            Action::OpenProfilePicker => self.open_profile_picker(),
            Action::ToggleBookmark => self.toggle_bookmark_for_active(),
            Action::OpenBookmarks => self.open_bookmarks(),
        }
    }

    /// Ids of pages matching `query` (case-insensitive substring of title or
    /// URL), in creation order — same predicate the switcher grid's filter
    /// uses.
    fn matching_page_ids(&self, query: &str) -> Vec<String> {
        self.core.borrow().matching_ids(query)
    }

    /// User explicitly picked a page to view (clicked a tile, or a single
    /// search match) — updates the active page and closes the switcher.
    pub fn switch_to(self: &Rc<Self>, id: &str) {
        self.set_active(id);
        self.close_switcher();
    }

    /// Ctrl+Enter in the switcher's search box: always opens a brand-new page
    /// from the typed text, even when it matches an open page or history
    /// entry (which plain Enter, via `address_bar`'s `connect_activate`,
    /// would instead switch to) — the escape hatch for deliberately wanting a
    /// second page at the same URL. Caller must have already called
    /// `open_switcher()`; does nothing if the text is blank.
    pub fn force_new_page_from_search(self: &Rc<Self>, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let url = resolve_address_input(trimmed, &self.settings());
        if let Err(err) = self.add_page(&url) {
            eprintln!("failed to open new page: {err}");
        }
        self.close_switcher();
    }

    pub fn close_page(self: &Rc<Self>, id: &str) {
        let was_active = self.core.borrow().active_id() == id;

        self.core.borrow_mut().remove(id);
        if let Some(container) = self.containers.borrow_mut().remove(id) {
            self.stack.remove(&container);
        }

        if was_active {
            // Reassign the active page without touching the switcher's
            // visibility: closing a page from the grid should switch to the
            // nearest remaining one but leave the grid open, not dismiss it.
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

    /// Rebuilds every tile from scratch — open pages matching the search
    /// box's current text (or all of them, if empty — `matching_ids` already
    /// handles that), plus, when there's a query, matching history entries
    /// not already open. Does its own filtering here (rather than GTK's
    /// separate `set_filter_func`/`invalidate_filter` mechanism, used until
    /// this history integration) since folding in a second, differently-
    /// sourced set of results (history, queried fresh each time) doesn't fit
    /// a filter predicate over already-built children.
    fn rebuild_switcher_grid(self: &Rc<Self>) {
        for child in self.flowbox.children() {
            self.flowbox.remove(&child);
        }

        let query = self.address_bar.text().to_string();
        let open_matches = self.core.borrow().matching_ids(&query);

        {
            let core = self.core.borrow();
            for page in core.pages() {
                if !open_matches.contains(&page.id) {
                    continue;
                }
                let id = page.id.clone();
                let title_text = {
                    let t = page.title.borrow();
                    if t.is_empty() { "New Page".to_string() } else { t.clone() }
                };
                let url = page.current_url();
                let domain = domain_of(&url);

                let tile = gtk::Button::new();
                tile.style_context().add_class("page-tile");
                let css = gtk::CssProvider::new();
                // Adwaita's button theme draws its own background-image (a
                // gradient) on top of any background-color, which is why the
                // tile's real color was only visible on hover (when the theme's
                // hover state happens to thin that gradient). Explicitly zeroing
                // background-image/border/box-shadow here removes the theme's
                // button chrome so the flat color always shows.
                let _ = css.load_from_data(
                    format!(
                        ".page-tile {{ background-image: none; background-color: {}; \
                          border: none; box-shadow: none; border-radius: 10px; color: #fff; }}",
                        page.color
                    )
                    .as_bytes(),
                );
                tile.style_context()
                    .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
                if !page.loaded {
                    tile.style_context().add_class("page-tile-unloaded");
                }
                tile.set_size_request(150, 110);
                // Keyboard focus should land on the FlowBoxChild wrapper, not
                // descend into this button, so arrow keys move between tiles
                // instead of highlighting sub-widgets. Mouse clicks still work —
                // can_focus only affects keyboard focus/tab order.
                tile.set_can_focus(false);

                let inner = gtk::Box::new(gtk::Orientation::Vertical, 2);
                inner.set_margin(10);
                inner.set_valign(gtk::Align::End);
                let title_label = gtk::Label::new(Some(&title_text));
                title_label.set_halign(gtk::Align::Start);
                title_label.style_context().add_class("tile-title");
                let domain_text = if page.loaded { domain } else { format!("{domain} \u{b7} unloaded") };
                let domain_label = gtk::Label::new(Some(&domain_text));
                domain_label.set_halign(gtk::Align::Start);
                domain_label.style_context().add_class("tile-subtitle");
                inner.pack_start(&title_label, false, false, 0);
                inner.pack_start(&domain_label, false, false, 0);
                tile.add(&inner);

                let app_clone = Rc::clone(self);
                let id_clone = id.clone();
                tile.connect_clicked(move |_| {
                    app_clone.switch_to(&id_clone);
                });

                let close_btn = gtk::Button::new();
                close_btn.style_context().add_class("tile-close-btn");
                let close_label = gtk::Label::new(Some("\u{d7}"));
                close_label.style_context().add_class("tile-close-label");
                close_btn.add(&close_label);
                close_btn.set_halign(gtk::Align::End);
                close_btn.set_valign(gtk::Align::Start);
                close_btn.set_margin_top(10);
                close_btn.set_margin_end(10);
                close_btn.set_size_request(18, 18);
                close_btn.set_can_focus(false);
                let app_clone = Rc::clone(self);
                let id_clone = id.clone();
                close_btn.connect_clicked(move |_| {
                    app_clone.close_page(&id_clone);
                });

                let tile_overlay = gtk::Overlay::new();
                tile_overlay.add(&tile);
                tile_overlay.add_overlay(&close_btn);

                let flow_child = gtk::FlowBoxChild::new();
                flow_child.set_widget_name(&id);
                flow_child.add(&tile_overlay);
                flow_child.show_all();
                self.flowbox.insert(&flow_child, -1);
            }
        }

        let add_tile = gtk::Button::new();
        add_tile.style_context().add_class("add-tile");
        add_tile.set_size_request(150, 110);
        add_tile.set_can_focus(false);
        let add_tile_label = gtk::Label::new(Some("+"));
        add_tile_label.style_context().add_class("add-tile-label");
        add_tile.add(&add_tile_label);
        let app_clone = Rc::clone(self);
        add_tile.connect_clicked(move |_| {
            let start_page = app_clone.settings.borrow().start_page.clone();
            if let Err(err) = app_clone.add_page(&start_page) {
                eprintln!("failed to open new page: {err}");
            }
            app_clone.close_switcher();
        });

        let add_child = gtk::FlowBoxChild::new();
        add_child.set_widget_name("__add__");
        add_child.add(&add_tile);
        add_child.show_all();
        self.flowbox.insert(&add_child, -1);

        // History matches — only once there's a query to narrow by (an empty
        // query would otherwise dump the entire history into the grid).
        // Skips any entry whose URL is already an open page's, so nothing
        // shows up twice.
        if !query.trim().is_empty() {
            let open_urls: Vec<String> = self.core.borrow().pages().iter().map(|p| p.current_url()).collect();
            let history_matches = self.history.search(&query, 8).unwrap_or_else(|err| {
                eprintln!("history search failed: {err}");
                Vec::new()
            });
            for entry in history_matches {
                if open_urls.contains(&entry.url) {
                    continue;
                }

                let tile = gtk::Button::new();
                tile.style_context().add_class("page-tile");
                tile.style_context().add_class("history-tile");
                tile.set_size_request(150, 110);
                tile.set_can_focus(false);

                let inner = gtk::Box::new(gtk::Orientation::Vertical, 2);
                inner.set_margin(10);
                inner.set_valign(gtk::Align::End);
                let title_text = if entry.title.is_empty() { "New Page".to_string() } else { entry.title.clone() };
                let title_label = gtk::Label::new(Some(&title_text));
                title_label.set_halign(gtk::Align::Start);
                title_label.style_context().add_class("tile-title");
                let domain_label = gtk::Label::new(Some(&entry.domain));
                domain_label.set_halign(gtk::Align::Start);
                domain_label.style_context().add_class("tile-subtitle");
                inner.pack_start(&title_label, false, false, 0);
                inner.pack_start(&domain_label, false, false, 0);
                tile.add(&inner);

                let app_clone = Rc::clone(self);
                let url = entry.url.clone();
                tile.connect_clicked(move |_| {
                    if let Err(err) = app_clone.add_page(&url) {
                        eprintln!("failed to open history entry: {err}");
                    }
                    app_clone.close_switcher();
                });

                let flow_child = gtk::FlowBoxChild::new();
                flow_child.add(&tile);
                flow_child.show_all();
                self.flowbox.insert(&flow_child, -1);
            }
        }
    }

    /// Page ids in creation order — test/inspection helper.
    pub fn page_ids(&self) -> Vec<String> {
        self.core.borrow().page_ids()
    }

    /// Currently active page id — test/inspection helper.
    pub fn active_id(&self) -> String {
        self.core.borrow().active_id().to_string()
    }

    /// The `Stack`'s visible child name, so tests can confirm the UI (not just
    /// internal state) actually switched — test/inspection helper.
    pub fn stack_visible_child_name(&self) -> Option<String> {
        self.stack.visible_child_name().map(|s| s.to_string())
    }

    /// The active page's current URL — test/inspection helper.
    pub fn active_url(&self) -> Option<String> {
        self.core.borrow().active().map(|p| p.current_url())
    }

    /// Number of GTK children in a page's stack container — 0 means its
    /// webview has actually been torn down (real reclamation), 1 means a
    /// live webview is present. Distinguishes real teardown from the
    /// `loaded` bool alone (already covered by `is_page_loaded`) —
    /// test/inspection helper.
    pub fn page_container_child_count(&self, id: &str) -> usize {
        self.containers.borrow().get(id).map(|c| c.children().len()).unwrap_or(0)
    }

    /// A page's tracked title (updated via wry's document-title-changed
    /// handler) — test/inspection helper.
    pub fn page_title(&self, id: &str) -> Option<String> {
        self.core.borrow().page(id).map(|p| p.title.borrow().clone())
    }

    /// A page's current URL regardless of whether it's loaded or active —
    /// the live engine's URL if it has one, else the frozen URL from before
    /// it was unloaded. Unlike `active_url`, works for any page — test/
    /// inspection helper.
    pub fn page_url(&self, id: &str) -> Option<String> {
        self.core.borrow().page(id).map(|p| p.current_url())
    }

    /// Whether the switcher grid is currently shown — test/inspection helper.
    pub fn is_switcher_open(&self) -> bool {
        self.switcher_panel.is_visible()
    }

    /// Whether this window belongs to an ephemeral (private/incognito/guest)
    /// profile — test/inspection helper. More reliable than checking
    /// `window.title()` directly: with a custom `GtkHeaderBar` set as the
    /// window's titlebar (as this app always does), `gtk_window_get_title()`
    /// doesn't reliably reflect what was passed to `set_title` under every
    /// compositor — confirmed empirically while testing this feature, not
    /// merely suspected — so tests (and any other code) should check this
    /// instead of the window's own title property.
    pub fn is_ephemeral(&self) -> bool {
        self.profile.ephemeral
    }

    /// Whether the page stack (and so the background webview) can currently
    /// take input/focus — test/inspection helper.
    pub fn is_background_page_interactive(&self) -> bool {
        self.stack.is_sensitive()
    }

    /// Types `text` into the address bar (which doubles as the switcher's
    /// search box while the switcher is open — see the field doc on
    /// `AppState::address_bar`) and simulates pressing Enter — test helper
    /// for the "open a new page if nothing matches" behavior, exercising the
    /// same `connect_activate` handler a real keypress would trigger. Caller
    /// must have already called `open_switcher()` for this to hit the
    /// switcher-search branch rather than plain navigation.
    pub fn search_activate(&self, text: &str) {
        self.address_bar.set_text(text);
        self.address_bar.emit_activate();
    }

    /// Types `text` into the toolbar address bar and simulates pressing
    /// Enter — test helper exercising the real `connect_activate` handler
    /// (and so the real `resolve_address_input` integration). Caller must
    /// ensure the switcher is closed for this to hit the plain-navigation
    /// branch rather than switcher-search.
    pub fn address_bar_activate(&self, text: &str) {
        self.address_bar.set_text(text);
        self.address_bar.emit_activate();
    }

    /// The address bar's current text, without simulating Enter — test
    /// helper for confirming what `open_switcher`/`close_switcher` leave it
    /// showing.
    pub fn address_bar_text(&self) -> String {
        self.address_bar.text().to_string()
    }

    /// Types into the address bar without simulating Enter — test helper for
    /// setting up the "typed a filter, then closed without selecting" case.
    pub fn set_address_bar_text(&self, text: &str) {
        self.address_bar.set_text(text);
    }

    /// Whether the address bar's entire current text is selected — test
    /// helper for confirming `open_switcher_editing_url` selects rather than
    /// blanks the current URL.
    pub fn address_bar_is_fully_selected(&self) -> bool {
        let len = self.address_bar.text().len() as i32;
        len > 0 && self.address_bar.selection_bounds() == Some((0, len))
    }

    /// The settings overlay's start-page field, as currently shown — test
    /// helper for confirming `open_settings` pre-populates it from the
    /// current `Settings` rather than leaving it stale from a previous open.
    pub fn settings_start_page_entry_text(&self) -> String {
        self.start_page_entry.text().to_string()
    }

    /// Types into the settings overlay's start-page field — test helper for
    /// driving an edit before calling `save_settings`, the same way
    /// `address_bar_activate`/`search_activate` drive their own widgets.
    pub fn set_settings_start_page(&self, text: &str) {
        self.start_page_entry.set_text(text);
    }

    /// Names of every search engine currently in `Settings::search_engines`
    /// — test helper for confirming add/remove actually changed the
    /// underlying data (not just the management list widget).
    pub fn settings_engine_names(&self) -> Vec<String> {
        self.settings.borrow().search_engines.iter().map(|e| e.name.clone()).collect()
    }

    /// The default-search-engine dropdown's currently active id (which is
    /// also the engine's name) — test helper for confirming
    /// `refresh_engine_combo` actually re-selects the current default after
    /// being repopulated.
    pub fn engine_combo_active_id(&self) -> Option<String> {
        self.engine_combo.active_id().map(|s| s.to_string())
    }

    /// Types into the "Add engine" row's fields and clicks "Add engine" —
    /// test helper exercising the real `add_search_engine_from_fields`
    /// handler a real click would trigger.
    pub fn add_search_engine_via_fields(self: &Rc<Self>, name: &str, url_template: &str) {
        self.new_engine_name_entry.set_text(name);
        self.new_engine_url_entry.set_text(url_template);
        self.add_search_engine_from_fields();
    }

    /// Number of rows currently shown in the search engine management list
    /// — test/inspection helper.
    pub fn engines_row_count(&self) -> usize {
        self.engines_list_box.children().len()
    }
}

/// Current time as Unix seconds — used as a bookmark's `created_at` when
/// added, same precision `HistoryStore` uses internally for its own
/// timestamps.
fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Normalizes a real GTK keydown event into a `KeyChord`, or `None` if the
/// key itself is a bare modifier press (Ctrl/Alt/Shift/Super alone, with
/// nothing else) — used both for normal shortcut dispatch and for the
/// keybindings editor's "press keys…" capture, so a binding can never end up
/// as just "Ctrl" with no actual key.
fn gtk_key_to_chord(event: &gtk::gdk::EventKey) -> Option<KeyChord> {
    let keyval = event.keyval();
    let is_bare_modifier = matches!(
        keyval.name().as_deref(),
        Some("Control_L")
            | Some("Control_R")
            | Some("Shift_L")
            | Some("Shift_R")
            | Some("Alt_L")
            | Some("Alt_R")
            | Some("Super_L")
            | Some("Super_R")
            | Some("Meta_L")
            | Some("Meta_R")
    );
    if is_bare_modifier {
        return None;
    }

    let state = event.state();
    let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let alt = state.contains(gtk::gdk::ModifierType::MOD1_MASK);
    let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
    let key = match keyval.to_unicode().filter(|c| c.is_ascii_alphanumeric()) {
        Some(c) => c.to_ascii_uppercase().to_string(),
        None => keyval.name()?.to_string(),
    };
    Some(KeyChord::new(ctrl, alt, shift, key))
}

/// Builds the full window + chrome (header bar, page stack, switcher overlay)
/// and wires up all signal handlers. Does not create any page — call
/// `app.add_page(&app.settings().start_page.clone())` (or any other URL)
/// afterward to open the first one.
///
/// `profile` scopes where `Settings` is loaded from/saved to (see
/// `browser_core::Profile`) — pass `Profile::default()` for the implicit
/// `"default"` profile, or a profile resolved from `--profile` via
/// `browser_core::resolve_profile_name`.
///
/// Assumes `gtk::init()` has already been called.
pub fn build_window_and_app(profile: Profile) -> anyhow::Result<(gtk::Window, Rc<AppState>)> {
    if let Some(screen) = gtk::gdk::Screen::default() {
        let provider = gtk::CssProvider::new();
        let _ = provider.load_from_data(
            b".tile-title { color: #ffffff; font-weight: 600; } \
              .tile-subtitle { color: rgba(255, 255, 255, 0.75); } \
              .add-tile-label { color: #ffffff; font-size: 20px; } \
              .add-tile { background-image: none; background-color: rgba(255, 255, 255, 0.15); \
                border: none; box-shadow: none; border-radius: 10px; } \
              .tile-close-btn { background-image: none; background-color: rgba(0, 0, 0, 0.45); \
                border: none; box-shadow: none; border-radius: 9999px; padding: 0; \
                min-width: 0; min-height: 0; } \
              .tile-close-label { color: #ffffff; } \
              .switcher-hint { color: rgba(255, 255, 255, 0.6); font-size: 12px; } \
              .switcher-profile-label { color: rgba(255, 255, 255, 0.6); font-size: 12px; } \
              .page-tile-unloaded { opacity: 0.5; } \
              .settings-box { background-color: #2e2e2c; border-radius: 10px; padding: 16px; } \
              .settings-title { color: #ffffff; font-weight: 600; font-size: 14px; } \
              .history-tile { background-image: none; background-color: rgba(255, 255, 255, 0.12); \
                border: 1px dashed rgba(255, 255, 255, 0.3); box-shadow: none; border-radius: 10px; \
                color: #fff; opacity: 0.75; } \
              .settings-box label:not(.settings-title) { color: rgba(255, 255, 255, 0.92); } \
              .settings-box button.flat, .settings-box button.flat:hover { \
                background-image: none; background-color: transparent; } \
              .settings-box button.flat label { color: rgba(255, 255, 255, 0.92); }",
        );
        gtk::StyleContext::add_provider_for_screen(&screen, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(if profile.ephemeral { "claude-browser (Private)" } else { "claude-browser" });
    window.set_default_size(1024, 768);
    window.connect_delete_event(|_, _| {
        gtk::main_quit();
        gtk::glib::Propagation::Proceed
    });

    let header_bar = gtk::HeaderBar::new();
    header_bar.set_show_close_button(true);
    header_bar.set_decoration_layout(Some(":minimize,maximize,close"));

    let back_button = gtk::Button::new();
    back_button.set_image(Some(&gtk::Image::from_icon_name(
        Some("pan-start-symbolic"),
        gtk::IconSize::Button,
    )));
    let forward_button = gtk::Button::new();
    forward_button.set_image(Some(&gtk::Image::from_icon_name(
        Some("pan-end-symbolic"),
        gtk::IconSize::Button,
    )));
    let reload_button = gtk::Button::new();
    reload_button.set_image(Some(&gtk::Image::from_icon_name(
        Some("view-refresh-symbolic"),
        gtk::IconSize::Button,
    )));
    let switcher_toggle = gtk::Button::new();
    switcher_toggle.set_image(Some(&gtk::Image::from_icon_name(
        Some("view-grid-symbolic"),
        gtk::IconSize::Button,
    )));
    let settings_button = gtk::Button::new();
    settings_button.set_image(Some(&gtk::Image::from_icon_name(
        Some("preferences-system-symbolic"),
        gtk::IconSize::Button,
    )));
    let profile_button = gtk::Button::new();
    profile_button.set_image(Some(&gtk::Image::from_icon_name(
        Some("avatar-default-symbolic"),
        gtk::IconSize::Button,
    )));
    // Starts unbookmarked/non-starred — `refresh_bookmark_toggle_button`
    // (called once below, after `app` exists, and on every active-page
    // change afterward) corrects this immediately if the start page already
    // happens to be bookmarked.
    let bookmark_toggle_button = gtk::Button::new();
    bookmark_toggle_button.set_image(Some(&gtk::Image::from_icon_name(
        Some("non-starred-symbolic"),
        gtk::IconSize::Button,
    )));
    let bookmarks_button = gtk::Button::new();
    bookmarks_button.set_image(Some(&gtk::Image::from_icon_name(
        Some("user-bookmarks-symbolic"),
        gtk::IconSize::Button,
    )));
    for button in [
        &back_button,
        &forward_button,
        &reload_button,
        &switcher_toggle,
        &settings_button,
        &profile_button,
        &bookmark_toggle_button,
        &bookmarks_button,
    ] {
        button.style_context().add_class("flat");
    }

    header_bar.pack_start(&back_button);
    header_bar.pack_start(&forward_button);

    let address_bar = gtk::Entry::new();
    address_bar.set_width_chars(50);
    address_bar.set_hexpand(true);

    // Group the reload button with the address bar itself (rather than
    // packing it into the header bar's separate end-region) so it's centered
    // as part of the same unit as the address bar, sitting flush against it.
    // A spacer before the address bar and one after the reload button (each
    // about one toolbar button wide) doubles as draggable header-bar space
    // for moving the window.
    const TOOLBAR_BUTTON_WIDTH: i32 = 36;
    let spacer_before_address_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer_before_address_bar.set_size_request(TOOLBAR_BUTTON_WIDTH, -1);
    let spacer_after_reload = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer_after_reload.set_size_request(TOOLBAR_BUTTON_WIDTH, -1);

    let address_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    address_group.pack_start(&spacer_before_address_bar, false, false, 0);
    address_group.pack_start(&address_bar, true, true, 0);
    address_group.pack_start(&bookmark_toggle_button, false, false, 0);
    address_group.pack_start(&reload_button, false, false, 0);
    address_group.pack_start(&spacer_after_reload, false, false, 0);
    header_bar.set_custom_title(Some(&address_group));

    header_bar.pack_end(&switcher_toggle);
    header_bar.pack_end(&settings_button);
    header_bar.pack_end(&profile_button);
    header_bar.pack_end(&bookmarks_button);

    window.set_titlebar(Some(&header_bar));

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.set_hexpand(true);

    let scrim = gtk::EventBox::new();
    scrim.style_context().add_class("switcher-scrim");
    let scrim_css = gtk::CssProvider::new();
    let _ = scrim_css.load_from_data(b".switcher-scrim { background-color: rgba(20,20,18,0.55); }");
    scrim
        .style_context()
        .add_provider(&scrim_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let flowbox = gtk::FlowBox::new();
    flowbox.set_valign(gtk::Align::Start);
    // Browse keeps exactly one child highlighted/selected at all times as
    // arrow keys move over the grid, which is what lets Delete know which
    // page to close (via selected_children()) and gives keyboard users a
    // visible "current" tile.
    flowbox.set_selection_mode(gtk::SelectionMode::Browse);
    flowbox.set_homogeneous(true);
    flowbox.set_margin(24);
    flowbox.set_row_spacing(16);
    flowbox.set_column_spacing(16);

    let keynav_hint = gtk::Label::new(Some("\u{21b5} Switch to page   \u{2326} Close page"));
    keynav_hint.style_context().add_class("switcher-hint");
    keynav_hint.set_halign(gtk::Align::Center);

    let grid_content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    grid_content.set_halign(gtk::Align::Fill);
    grid_content.set_valign(gtk::Align::Start);
    grid_content.set_margin_top(40);
    grid_content.pack_start(&flowbox, true, true, 0);
    grid_content.pack_start(&keynav_hint, false, false, 0);

    let profile_label = gtk::Label::new(Some(&profile.name));
    profile_label.style_context().add_class("switcher-profile-label");
    profile_label.set_halign(gtk::Align::End);
    profile_label.set_valign(gtk::Align::Start);
    profile_label.set_margin_top(12);
    profile_label.set_margin_end(16);

    let switcher_overlay = gtk::Overlay::new();
    switcher_overlay.add(&scrim);
    switcher_overlay.add_overlay(&grid_content);
    switcher_overlay.add_overlay(&profile_label);

    // --- Settings overlay: an in-window overlay (like the switcher grid
    // above), not a modal `gtk::Dialog` — see `AppState::settings_panel`'s
    // doc comment for why. Scoped to picking the default search engine from
    // the existing seeded list, not adding/editing entries — that's a
    // fuller list-editor UI, left for later.
    let settings_scrim = gtk::EventBox::new();
    settings_scrim.style_context().add_class("switcher-scrim");
    settings_scrim
        .style_context()
        .add_provider(&scrim_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let settings_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    settings_box.set_halign(gtk::Align::Center);
    settings_box.set_valign(gtk::Align::Center);
    settings_box.style_context().add_class("settings-box");
    settings_box.set_margin(24);

    let settings_title = gtk::Label::new(Some("Settings"));
    settings_title.style_context().add_class("settings-title");
    settings_title.set_halign(gtk::Align::Start);
    settings_box.pack_start(&settings_title, false, false, 0);

    let start_page_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    start_page_row.pack_start(&gtk::Label::new(Some("Start page")), false, false, 0);
    let start_page_entry = gtk::Entry::new();
    start_page_entry.set_hexpand(true);
    start_page_row.pack_start(&start_page_entry, true, true, 0);
    settings_box.pack_start(&start_page_row, false, false, 0);

    let engine_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    engine_row.pack_start(&gtk::Label::new(Some("Search engine")), false, false, 0);
    // Populated for real (from the live per-profile Settings, not this
    // hardcoded default) by `refresh_engine_combo`, called from
    // `open_settings` every time it opens — left empty here since nothing
    // shows until the overlay is opened anyway.
    let engine_combo = gtk::ComboBoxText::new();
    engine_row.pack_start(&engine_combo, true, true, 0);
    settings_box.pack_start(&engine_row, false, false, 0);

    // Search engine management: add/remove entries from Settings::search_engines.
    // Unlike the fields above (staged until Save), these take effect and save
    // immediately on each add/remove — the same immediate-save convention this
    // session's bookmarks/keybindings editors already use, rather than adding a
    // separate staged/cancel-able list-editing model just for this section.
    let engines_list_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    settings_box.pack_start(&engines_list_box, false, false, 0);

    let new_engine_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let new_engine_name_entry = gtk::Entry::new();
    new_engine_name_entry.set_placeholder_text(Some("Name"));
    new_engine_name_entry.set_hexpand(true);
    let new_engine_url_entry = gtk::Entry::new();
    new_engine_url_entry.set_placeholder_text(Some("https://example.com/search?q={query}"));
    new_engine_url_entry.set_hexpand(true);
    let add_engine_button = gtk::Button::with_label("Add engine");
    new_engine_row.pack_start(&new_engine_name_entry, true, true, 0);
    new_engine_row.pack_start(&new_engine_url_entry, true, true, 0);
    new_engine_row.pack_start(&add_engine_button, false, false, 0);
    settings_box.pack_start(&new_engine_row, false, false, 0);

    let limit_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    limit_row.pack_start(&gtk::Label::new(Some("Loaded pages limit")), false, false, 0);
    let unlimited_check = gtk::CheckButton::new();
    unlimited_check.set_label("Unlimited");
    let limit_spin = gtk::SpinButton::with_range(1.0, 100.0, 1.0);
    {
        let limit_spin = limit_spin.clone();
        unlimited_check.connect_toggled(move |check| {
            limit_spin.set_sensitive(!check.is_active());
        });
    }
    limit_row.pack_start(&unlimited_check, false, false, 0);
    limit_row.pack_start(&limit_spin, false, false, 0);
    settings_box.pack_start(&limit_row, false, false, 0);

    // Keybindings editor, folded into the settings overlay rather than
    // being its own separate destination — one row per `Action::ALL`,
    // rebuilt from the current `Keybindings` each time settings opens and
    // after every add/remove. Wrapped in a `ScrolledWindow` (rather than
    // just packed straight into `settings_box`) since ~10 actions' worth of
    // rows alongside the settings fields above would otherwise make the
    // overlay taller than comfortably fits on screen.
    settings_box.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 4);

    let keybindings_title = gtk::Label::new(Some("Keybindings"));
    keybindings_title.style_context().add_class("settings-title");
    keybindings_title.set_halign(gtk::Align::Start);
    settings_box.pack_start(&keybindings_title, false, false, 0);

    let keybindings_list_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let keybindings_scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    keybindings_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    keybindings_scroll.set_propagate_natural_height(true);
    keybindings_scroll.set_max_content_height(260);
    keybindings_scroll.add(&keybindings_list_box);
    settings_box.pack_start(&keybindings_scroll, true, true, 0);

    let settings_buttons_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    settings_buttons_row.set_halign(gtk::Align::End);
    let settings_cancel_button = gtk::Button::with_label("Cancel");
    let settings_save_button = gtk::Button::with_label("Save");
    settings_buttons_row.pack_start(&settings_cancel_button, false, false, 0);
    settings_buttons_row.pack_start(&settings_save_button, false, false, 0);
    settings_box.pack_start(&settings_buttons_row, false, false, 0);

    let settings_overlay = gtk::Overlay::new();
    settings_overlay.add(&settings_scrim);
    settings_overlay.add_overlay(&settings_box);

    // --- Profile picker overlay: same in-window-overlay pattern again.
    // Lists existing profiles (from `list_profile_names()`, rebuilt each
    // time it opens) plus a field to create a new one — picking any profile
    // other than the current one launches a new, independent process
    // scoped to it (`launch_new_profile_process`) rather than switching this
    // window in place.
    let profile_scrim = gtk::EventBox::new();
    profile_scrim.style_context().add_class("switcher-scrim");
    profile_scrim
        .style_context()
        .add_provider(&scrim_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let profile_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    profile_box.set_halign(gtk::Align::Center);
    profile_box.set_valign(gtk::Align::Center);
    profile_box.style_context().add_class("settings-box");
    profile_box.set_margin(24);

    let profile_title = gtk::Label::new(Some("Profiles"));
    profile_title.style_context().add_class("settings-title");
    profile_title.set_halign(gtk::Align::Start);
    profile_box.pack_start(&profile_title, false, false, 0);

    let profile_list_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    profile_box.pack_start(&profile_list_box, false, false, 0);

    let new_profile_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let new_profile_entry = gtk::Entry::new();
    new_profile_entry.set_placeholder_text(Some("New profile name\u{2026}"));
    new_profile_entry.set_hexpand(true);
    let create_profile_button = gtk::Button::with_label("Create & Open");
    new_profile_row.pack_start(&new_profile_entry, true, true, 0);
    new_profile_row.pack_start(&create_profile_button, false, false, 0);
    profile_box.pack_start(&new_profile_row, false, false, 0);

    let profile_buttons_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    profile_buttons_row.set_halign(gtk::Align::End);
    let new_private_window_button = gtk::Button::with_label("New Private Window");
    let profile_cancel_button = gtk::Button::with_label("Cancel");
    profile_buttons_row.pack_start(&new_private_window_button, false, false, 0);
    profile_buttons_row.pack_start(&profile_cancel_button, false, false, 0);
    profile_box.pack_start(&profile_buttons_row, false, false, 0);

    let profile_overlay = gtk::Overlay::new();
    profile_overlay.add(&profile_scrim);
    profile_overlay.add_overlay(&profile_box);

    // --- Bookmarks overlay: same shape again. One row per bookmark, rebuilt
    // from `Bookmarks::all()` each time it opens and after every add/remove.
    let bookmarks_scrim = gtk::EventBox::new();
    bookmarks_scrim.style_context().add_class("switcher-scrim");
    bookmarks_scrim
        .style_context()
        .add_provider(&scrim_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let bookmarks_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    bookmarks_box.set_halign(gtk::Align::Center);
    bookmarks_box.set_valign(gtk::Align::Center);
    bookmarks_box.style_context().add_class("settings-box");
    bookmarks_box.set_margin(24);

    let bookmarks_title = gtk::Label::new(Some("Bookmarks"));
    bookmarks_title.style_context().add_class("settings-title");
    bookmarks_title.set_halign(gtk::Align::Start);
    bookmarks_box.pack_start(&bookmarks_title, false, false, 0);

    let bookmarks_list_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    bookmarks_box.pack_start(&bookmarks_list_box, false, false, 0);

    let bookmarks_close_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bookmarks_close_row.set_halign(gtk::Align::End);
    let bookmarks_close_button = gtk::Button::with_label("Close");
    bookmarks_close_row.pack_start(&bookmarks_close_button, false, false, 0);
    bookmarks_box.pack_start(&bookmarks_close_row, false, false, 0);

    let bookmarks_overlay = gtk::Overlay::new();
    bookmarks_overlay.add(&bookmarks_scrim);
    bookmarks_overlay.add_overlay(&bookmarks_box);

    let root_overlay = gtk::Overlay::new();
    root_overlay.add(&stack);
    root_overlay.add_overlay(&switcher_overlay);
    root_overlay.add_overlay(&settings_overlay);
    root_overlay.add_overlay(&profile_overlay);
    root_overlay.add_overlay(&bookmarks_overlay);

    window.add(&root_overlay);
    window.show_all();
    switcher_overlay.hide();
    settings_overlay.hide();
    profile_overlay.hide();
    bookmarks_overlay.hide();

    let settings = Settings::load(&profile);
    // An ephemeral (private/incognito/guest) profile's history should never
    // touch disk at all — see `Profile::ephemeral`'s doc comment.
    let history = if profile.ephemeral { HistoryStore::open_in_memory()? } else { HistoryStore::open(&profile)? };
    let bookmarks = Bookmarks::load(&profile);
    let core = PageManager::new(settings.max_loaded_pages);
    let app = Rc::new(AppState {
        address_bar: address_bar.clone(),
        stack,
        switcher_panel: switcher_overlay.clone().upcast::<gtk::Widget>(),
        flowbox: flowbox.clone(),
        settings_panel: settings_overlay.clone().upcast::<gtk::Widget>(),
        start_page_entry: start_page_entry.clone(),
        engine_combo: engine_combo.clone(),
        engines_list_box: engines_list_box.clone(),
        new_engine_name_entry: new_engine_name_entry.clone(),
        new_engine_url_entry: new_engine_url_entry.clone(),
        unlimited_check: unlimited_check.clone(),
        limit_spin: limit_spin.clone(),
        profile_panel: profile_overlay.clone().upcast::<gtk::Widget>(),
        profile_list_box: profile_list_box.clone(),
        new_profile_entry: new_profile_entry.clone(),
        keybindings_list_box: keybindings_list_box.clone(),
        keybindings: RefCell::new(Keybindings::load(&profile)),
        listening_for: Cell::new(None),
        bookmarks_panel: bookmarks_overlay.clone().upcast::<gtk::Widget>(),
        bookmarks_list_box: bookmarks_list_box.clone(),
        bookmark_toggle_button: bookmark_toggle_button.clone(),
        bookmarks: RefCell::new(bookmarks),
        core: RefCell::new(core),
        containers: RefCell::new(HashMap::new()),
        settings: RefCell::new(settings),
        history,
        profile,
    });

    {
        // Only filters the grid while the switcher is actually open — the
        // address bar keeps getting `connect_changed` events from ordinary
        // navigation (typing a URL) the rest of the time, which must not
        // touch the switcher's (hidden, but still live) tile list.
        let app = Rc::clone(&app);
        address_bar.connect_changed(move |_| {
            if app.is_switcher_open() {
                app.rebuild_switcher_grid();
            }
        });
    }
    {
        // Ctrl+Enter always opens a fresh page, even when the typed text
        // matches an open page/history entry — checked ahead of the plain
        // `connect_activate` handler below (which GtkEntry still emits
        // afterward for a bare Enter, since we only `Stop` when Ctrl is
        // actually held).
        let app = Rc::clone(&app);
        address_bar.connect_key_press_event(move |entry, event| {
            if !app.is_switcher_open() {
                return gtk::glib::Propagation::Proceed;
            }
            let is_enter = matches!(event.keyval().name().as_deref(), Some("Return") | Some("KP_Enter"));
            let ctrl_held = event.state().contains(gtk::gdk::ModifierType::CONTROL_MASK);
            if is_enter && ctrl_held {
                app.force_new_page_from_search(&entry.text());
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
    }
    {
        // Fires when a FlowBoxChild is activated — Enter/Space while it has
        // keyboard focus (tile/close buttons are can_focus(false), so focus
        // always lands on the FlowBoxChild itself, never its contents).
        let app = Rc::clone(&app);
        flowbox.connect_child_activated(move |_, child| {
            let name = child.widget_name();
            if name.as_str() == "__add__" {
                let start_page = app.settings.borrow().start_page.clone();
                if let Err(err) = app.add_page(&start_page) {
                    eprintln!("failed to open new page: {err}");
                }
                app.close_switcher();
            } else {
                app.switch_to(name.as_str());
            }
        });
    }
    {
        // Delete closes whichever tile is currently highlighted by keyboard
        // navigation (Browse selection mode keeps exactly one selected).
        let app = Rc::clone(&app);
        flowbox.connect_key_press_event(move |flowbox, event| {
            let is_delete = event.keyval() == gtk::gdk::keys::Key::from_name("Delete");
            if !is_delete {
                return gtk::glib::Propagation::Proceed;
            }
            if let Some(child) = flowbox.selected_children().into_iter().next() {
                let name = child.widget_name();
                if name.as_str() != "__add__" {
                    app.close_page(name.as_str());
                }
            }
            gtk::glib::Propagation::Stop
        });
    }

    {
        let app = Rc::clone(&app);
        back_button.connect_clicked(move |_| app.with_active(|p| p.go_back()));
    }
    {
        let app = Rc::clone(&app);
        forward_button.connect_clicked(move |_| app.with_active(|p| p.go_forward()));
    }
    {
        let app = Rc::clone(&app);
        reload_button.connect_clicked(move |_| app.with_active(|p| p.reload()));
    }
    {
        // Contextual: while the switcher is open, the address bar is acting
        // as its search box (filter open pages/history, Enter opens the sole
        // match or a fresh page) instead of navigating the active page — see
        // the field doc on `AppState::address_bar`.
        let app = Rc::clone(&app);
        address_bar.connect_activate(move |entry| {
            let text = entry.text().to_string();
            if app.is_switcher_open() {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return;
                }
                match app.matching_page_ids(trimmed).as_slice() {
                    [] => {
                        // No open page matches — check history before
                        // treating the typed text as a fresh URL/search:
                        // exactly one history match opens that entry's real
                        // URL instead.
                        match app.history.search(trimmed, 2) {
                            Ok(matches) if matches.len() == 1 => {
                                let url = matches[0].url.clone();
                                if let Err(err) = app.add_page(&url) {
                                    eprintln!("failed to open history entry: {err}");
                                }
                                app.close_switcher();
                            }
                            _ => {
                                let url = resolve_address_input(trimmed, &app.settings());
                                if let Err(err) = app.add_page(&url) {
                                    eprintln!("failed to open new page: {err}");
                                }
                                app.close_switcher();
                            }
                        }
                    }
                    [only] => app.switch_to(only),
                    _ => {}
                }
            } else {
                let url = resolve_address_input(&text, &app.settings());
                app.with_active(|p| p.navigate(&url));
            }
        });
    }
    {
        let app = Rc::clone(&app);
        switcher_toggle.connect_clicked(move |_| {
            if app.is_switcher_open() {
                app.close_switcher();
            } else {
                app.open_switcher();
            }
        });
    }
    {
        let app = Rc::clone(&app);
        settings_button.connect_clicked(move |_| {
            app.open_settings();
        });
    }
    {
        let app = Rc::clone(&app);
        scrim.connect_button_press_event(move |_, _| {
            app.close_switcher();
            gtk::glib::Propagation::Stop
        });
    }
    {
        let app = Rc::clone(&app);
        settings_scrim.connect_button_press_event(move |_, _| {
            app.close_settings();
            gtk::glib::Propagation::Stop
        });
    }
    {
        let app = Rc::clone(&app);
        settings_cancel_button.connect_clicked(move |_| {
            app.close_settings();
        });
    }
    {
        let app = Rc::clone(&app);
        settings_save_button.connect_clicked(move |_| {
            app.save_settings();
        });
    }
    {
        let app = Rc::clone(&app);
        add_engine_button.connect_clicked(move |_| {
            app.add_search_engine_from_fields();
        });
    }
    {
        let app = Rc::clone(&app);
        new_engine_url_entry.connect_activate(move |_| {
            app.add_search_engine_from_fields();
        });
    }
    {
        let app = Rc::clone(&app);
        profile_button.connect_clicked(move |_| {
            app.open_profile_picker();
        });
    }
    {
        let app = Rc::clone(&app);
        profile_scrim.connect_button_press_event(move |_, _| {
            app.close_profile_picker();
            gtk::glib::Propagation::Stop
        });
    }
    {
        let app = Rc::clone(&app);
        profile_cancel_button.connect_clicked(move |_| {
            app.close_profile_picker();
        });
    }
    {
        let app = Rc::clone(&app);
        create_profile_button.connect_clicked(move |_| {
            app.create_and_open_profile();
        });
    }
    {
        let app = Rc::clone(&app);
        new_private_window_button.connect_clicked(move |_| {
            app.open_new_private_window();
        });
    }
    {
        let app = Rc::clone(&app);
        new_profile_entry.connect_activate(move |_| {
            app.create_and_open_profile();
        });
    }
    {
        let app = Rc::clone(&app);
        bookmark_toggle_button.connect_clicked(move |_| {
            app.toggle_bookmark_for_active();
        });
    }
    {
        let app = Rc::clone(&app);
        bookmarks_button.connect_clicked(move |_| {
            app.open_bookmarks();
        });
    }
    {
        let app = Rc::clone(&app);
        bookmarks_scrim.connect_button_press_event(move |_, _| {
            app.close_bookmarks();
            gtk::glib::Propagation::Stop
        });
    }
    {
        let app = Rc::clone(&app);
        bookmarks_close_button.connect_clicked(move |_| {
            app.close_bookmarks();
        });
    }
    {
        let app = Rc::clone(&app);
        window.connect_key_press_event(move |_, event| {
            let is_escape = event.keyval() == gtk::gdk::keys::Key::from_name("Escape");
            if is_escape && app.is_switcher_open() {
                app.close_switcher();
                return gtk::glib::Propagation::Stop;
            } else if is_escape && app.is_settings_open() {
                app.close_settings();
                return gtk::glib::Propagation::Stop;
            } else if is_escape && app.is_profile_picker_open() {
                app.close_profile_picker();
                return gtk::glib::Propagation::Stop;
            } else if is_escape && app.is_bookmarks_open() {
                app.close_bookmarks();
                return gtk::glib::Propagation::Stop;
            }

            let Some(chord) = gtk_key_to_chord(event) else {
                return gtk::glib::Propagation::Proceed;
            };

            // While the keybindings editor is waiting for a new binding,
            // this keypress becomes that binding instead of triggering
            // whatever it's currently bound to (if anything).
            if app.listening_for.get().is_some() {
                app.assign_listening_binding(chord);
                return gtk::glib::Propagation::Stop;
            }

            match app.keybindings.borrow().action_for(&chord) {
                Some(action) => {
                    app.dispatch_action(action);
                    gtk::glib::Propagation::Stop
                }
                None => gtk::glib::Propagation::Proceed,
            }
        });
    }

    Ok((window, app))
}

/// Shows a small standalone window for launching with a URL argument (e.g.
/// from the OS's "open with"/default-browser handoff) — lets the user
/// confirm/pick which profile to open it in before the real browser window
/// appears. `default_profile` pre-fills the field (whatever `--profile`
/// resolved to, or `"default"`).
///
/// Assumes `gtk::init()` has already been called, and that the caller will
/// still call `gtk::main()` afterward regardless of which path (this or
/// `build_window_and_app`) ends up running — this only ever shows a window,
/// never drives its own event loop.
pub fn show_external_link_chooser(url: String, default_profile: String) -> anyhow::Result<()> {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Open link");
    window.set_default_size(480, 200);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin(16);

    let url_label = gtk::Label::new(Some(&url));
    url_label.set_line_wrap(true);
    url_label.set_halign(gtk::Align::Start);
    url_label.set_selectable(true);
    content.pack_start(&url_label, false, false, 0);

    let profile_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    profile_row.pack_start(&gtk::Label::new(Some("Open in profile")), false, false, 0);
    let profile_combo = gtk::ComboBoxText::with_entry();
    for name in list_profile_names() {
        profile_combo.append_text(&name);
    }
    if let Some(entry) = profile_combo.child().and_then(|w| w.downcast::<gtk::Entry>().ok()) {
        entry.set_text(&default_profile);
    }
    profile_row.pack_start(&profile_combo, true, true, 0);
    content.pack_start(&profile_row, false, false, 0);

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    button_row.set_halign(gtk::Align::End);
    let cancel_button = gtk::Button::with_label("Cancel");
    let open_button = gtk::Button::with_label("Open");
    button_row.pack_start(&cancel_button, false, false, 0);
    button_row.pack_start(&open_button, false, false, 0);
    content.pack_start(&button_row, false, false, 0);

    window.add(&content);
    window.show_all();

    // Closing this window (native close button, or Cancel) should quit the
    // app — but successfully opening the main window and closing this one
    // as part of that handoff must not also quit. `transitioning` tells
    // `delete-event` which case it is.
    let transitioning = Rc::new(Cell::new(false));

    {
        let transitioning = Rc::clone(&transitioning);
        window.connect_delete_event(move |_, _| {
            if !transitioning.get() {
                gtk::main_quit();
            }
            gtk::glib::Propagation::Proceed
        });
    }
    {
        let window = window.clone();
        cancel_button.connect_clicked(move |_| {
            window.close();
        });
    }
    {
        let transitioning = Rc::clone(&transitioning);
        let window = window.clone();
        open_button.connect_clicked(move |_| {
            let profile_name = profile_combo
                .child()
                .and_then(|w| w.downcast::<gtk::Entry>().ok())
                .map(|entry| entry.text().to_string())
                .unwrap_or_default();
            let profile = Profile::new(profile_name);

            transitioning.set(true);
            window.close();

            match build_window_and_app(profile) {
                Ok((_main_window, app)) => {
                    if let Err(err) = app.add_page(&url) {
                        eprintln!("failed to open the launch URL: {err}");
                    }
                }
                Err(err) => eprintln!("failed to open the browser window: {err}"),
            }
        });
    }

    Ok(())
}
