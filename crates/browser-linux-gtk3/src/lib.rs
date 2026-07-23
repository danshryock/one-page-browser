//! GTK3 native chrome for the browser — Linux only. Gated on the whole
//! crate (rather than leaving it to fail on `gtk` being unresolved) so a
//! bare `cargo build`/`cross build --target x86_64-pc-windows-gnu` across
//! the whole workspace succeeds everywhere: this crate just compiles to an
//! empty no-op on any other platform, symmetric with how
//! `browser-windows-win32`/`browser-windows-nwg` gate themselves to
//! `target_os = "windows"`.
#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use browser_core::{domain_of, resolve_address_input, HistoryStore, PageManager, Profile, Settings};
use gtk::prelude::*;
use render_engine::{RenderEngine, WryEngine};

pub struct AppState {
    address_bar: gtk::Entry,
    stack: gtk::Stack,
    switcher_panel: gtk::Widget,
    search_entry: gtk::SearchEntry,
    flowbox: gtk::FlowBox,
    /// The settings overlay's root widget — an in-window overlay rather than
    /// a modal `gtk::Dialog` (matching `browser-windows-winui`'s settings
    /// surface, which led with this pattern since it needed to avoid
    /// `ContentDialog`'s async `ShowAsync`; gtk3 had no such constraint, this
    /// is purely for consistency between the two front ends).
    settings_panel: gtk::Widget,
    start_page_entry: gtk::Entry,
    engine_combo: gtk::ComboBoxText,
    unlimited_check: gtk::CheckButton,
    limit_spin: gtk::SpinButton,
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
    }

    /// Opens the switcher grid with a cleared, focused search box — used by
    /// the grid-button toggle as well as the F1 / Ctrl+T / Ctrl+L shortcuts.
    /// The page stack is made insensitive so the background webview can't
    /// steal keyboard focus (or process key/pointer input at all) while the
    /// grid is up. Closes the settings overlay first if it's open — the
    /// header bar's switcher/settings buttons are both reachable regardless
    /// of which overlay (if any) is currently shown, so both `open_*`
    /// methods defensively close the other rather than ever showing both at
    /// once.
    pub fn open_switcher(self: &Rc<Self>) {
        self.close_settings();
        self.search_entry.set_text("");
        self.rebuild_switcher_grid();
        self.stack.set_sensitive(false);
        self.switcher_panel.show();
        self.search_entry.grab_focus();
    }

    /// Hides the switcher grid and restores the page stack's sensitivity.
    /// Always use this (rather than hiding `switcher_panel` directly) so the
    /// stack never gets left insensitive.
    pub fn close_switcher(&self) {
        self.switcher_panel.hide();
        self.stack.set_sensitive(true);
    }

    /// Shows the settings overlay, populated from the current `Settings`.
    /// See `open_switcher`'s doc comment for why it closes the switcher grid
    /// first.
    pub fn open_settings(self: &Rc<Self>) {
        self.close_switcher();
        let settings = self.settings.borrow();
        self.start_page_entry.set_text(&settings.start_page);
        self.engine_combo.set_active_id(Some(&settings.default_search_engine));
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
        self.stack.set_sensitive(false);
        self.settings_panel.show();
    }

    /// Hides the settings overlay without saving — used by Cancel, the
    /// scrim, and Escape. Always use this (rather than hiding
    /// `settings_panel` directly) so the stack never gets left insensitive.
    pub fn close_settings(&self) {
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

        let query = self.search_entry.text().to_string();
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

    /// Whether the page stack (and so the background webview) can currently
    /// take input/focus — test/inspection helper.
    pub fn is_background_page_interactive(&self) -> bool {
        self.stack.is_sensitive()
    }

    /// Types `text` into the switcher's search box and simulates pressing
    /// Enter — test helper for the "open a new page if nothing matches"
    /// behavior, exercising the same `connect_activate` handler a real
    /// keypress would trigger.
    pub fn search_activate(&self, text: &str) {
        self.search_entry.set_text(text);
        self.search_entry.emit_activate();
    }

    /// Types `text` into the toolbar address bar and simulates pressing
    /// Enter — test helper exercising the real `connect_activate` handler
    /// (and so the real `resolve_address_input` integration), the same way
    /// `search_activate` does for the switcher's search box.
    pub fn address_bar_activate(&self, text: &str) {
        self.address_bar.set_text(text);
        self.address_bar.emit_activate();
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
                color: #fff; opacity: 0.75; }",
        );
        gtk::StyleContext::add_provider_for_screen(&screen, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("claude-browser");
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
    for button in [&back_button, &forward_button, &reload_button, &switcher_toggle, &settings_button] {
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
    address_group.pack_start(&reload_button, false, false, 0);
    address_group.pack_start(&spacer_after_reload, false, false, 0);
    header_bar.set_custom_title(Some(&address_group));

    header_bar.pack_end(&switcher_toggle);
    header_bar.pack_end(&settings_button);

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

    let search_entry = gtk::SearchEntry::new();
    search_entry.set_placeholder_text(Some("Type to filter open pages\u{2026}"));
    search_entry.set_halign(gtk::Align::Center);
    search_entry.set_width_chars(40);

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
    grid_content.pack_start(&search_entry, false, false, 0);
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
    let engine_combo = gtk::ComboBoxText::new();
    for engine in &Settings::default().search_engines {
        engine_combo.append(Some(&engine.name), &engine.name);
    }
    engine_row.pack_start(&engine_combo, true, true, 0);
    settings_box.pack_start(&engine_row, false, false, 0);

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

    let root_overlay = gtk::Overlay::new();
    root_overlay.add(&stack);
    root_overlay.add_overlay(&switcher_overlay);
    root_overlay.add_overlay(&settings_overlay);

    window.add(&root_overlay);
    window.show_all();
    switcher_overlay.hide();
    settings_overlay.hide();
    window.set_title("claude-browser");

    let settings = Settings::load(&profile);
    let history = HistoryStore::open(&profile)?;
    let core = PageManager::new(settings.max_loaded_pages);
    let app = Rc::new(AppState {
        address_bar: address_bar.clone(),
        stack,
        switcher_panel: switcher_overlay.clone().upcast::<gtk::Widget>(),
        search_entry: search_entry.clone(),
        flowbox: flowbox.clone(),
        settings_panel: settings_overlay.clone().upcast::<gtk::Widget>(),
        start_page_entry: start_page_entry.clone(),
        engine_combo: engine_combo.clone(),
        unlimited_check: unlimited_check.clone(),
        limit_spin: limit_spin.clone(),
        core: RefCell::new(core),
        containers: RefCell::new(HashMap::new()),
        settings: RefCell::new(settings),
        history,
        profile,
    });

    {
        let app = Rc::clone(&app);
        search_entry.connect_changed(move |_| {
            app.rebuild_switcher_grid();
        });
    }
    {
        let app = Rc::clone(&app);
        search_entry.connect_activate(move |entry| {
            let text = entry.text().to_string();
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return;
            }
            match app.matching_page_ids(trimmed).as_slice() {
                [] => {
                    // No open page matches — check history before treating
                    // the typed text as a fresh URL/search: exactly one
                    // history match opens that entry's real URL instead.
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
        });
    }
    {
        // GtkSearchEntry emits "stop-search" (rather than a plain key-press)
        // when Escape is pressed while it has focus, which is the common
        // case since open_switcher() focuses it — handle it here so Escape
        // reliably closes the switcher regardless of the window-level
        // key-press-event handler below.
        let app = Rc::clone(&app);
        search_entry.connect_stop_search(move |_| {
            app.close_switcher();
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
        let app = Rc::clone(&app);
        address_bar.connect_activate(move |entry| {
            let text = entry.text().to_string();
            let url = resolve_address_input(&text, &app.settings());
            app.with_active(|p| p.navigate(&url));
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
        window.connect_key_press_event(move |_, event| {
            let ctrl = event.state().contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let keyval = event.keyval();
            let is_f1 = keyval == gtk::gdk::keys::Key::from_name("F1");
            let unicode = keyval.to_unicode();
            let is_t = unicode.map(|c| c.eq_ignore_ascii_case(&'t')).unwrap_or(false);
            let is_l = unicode.map(|c| c.eq_ignore_ascii_case(&'l')).unwrap_or(false);
            let is_w = unicode.map(|c| c.eq_ignore_ascii_case(&'w')).unwrap_or(false);
            let is_escape = keyval == gtk::gdk::keys::Key::from_name("Escape");
            if is_f1 || (ctrl && is_t) || (ctrl && is_l) {
                app.open_switcher();
                gtk::glib::Propagation::Stop
            } else if is_escape && app.is_switcher_open() {
                app.close_switcher();
                gtk::glib::Propagation::Stop
            } else if is_escape && app.is_settings_open() {
                app.close_settings();
                gtk::glib::Propagation::Stop
            } else if ctrl && is_w {
                app.close_page(&app.active_id());
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
    }

    Ok((window, app))
}
