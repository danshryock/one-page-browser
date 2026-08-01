//! Native AppKit chrome — brought to feature parity with
//! `browser-windows-reactor`'s scope (multi-page via `PageManager`,
//! switcher/settings/profile overlays, global keyboard shortcuts,
//! external-link chooser): not `browser-linux-gtk3`'s superset (bookmarks,
//! light/dark theme, encrypted profiles aren't implemented on *either*
//! Windows front end either — see `ROADMAP.md`), but a real, consistent bar
//! across every native-chrome-plus-`PageManager` front end in this repo.
//!
//! # Written on Linux, cross-compiled from Linux, never *run* from Linux
//!
//! Written entirely on a Linux dev machine with no macOS hardware available
//! (a local macOS VM would violate Apple's EULA on this non-Apple hardware
//! — see `summaries/windows-github-actions-ci.md`'s "why not local VMs"
//! section). This crate *does* have a real cross-compile story from Linux —
//! `cargo zigbuild` plus an unofficial macOS SDK mirror (see README.md's
//! "browser-macos-appkit: building" section) — so every change here is at
//! least compile-and-link checked (real Mach-O binaries, real framework
//! linking) before it's pushed, not just eyeballed against `objc2-app-kit`'s
//! generated source. But there's still no way to *run* a macOS binary from
//! this Linux machine (no Wine-for-macOS equivalent), so real behavioral
//! verification still only happens on GitHub's native `macos-latest`
//! runners (see `.github/workflows/macos.yml`) — treat runtime behavior
//! here as link-checked, not yet proven correct end-to-end.
//!
//! # Layout: manual frames, not `NSStackView`/Auto Layout
//!
//! Every panel here (toolbar, switcher/settings/profile overlays) positions
//! its subviews via explicit `NSRect` frames recomputed in `relayout()`,
//! the same approach the original scaffold used for the toolbar — not
//! `NSStackView`'s Auto Layout engine, despite that being the more
//! idiomatic modern AppKit approach for a from-scratch codebase. Auto
//! Layout's constraint-conflict failures are a runtime phenomenon (logged
//! warnings, sometimes silently-wrong layout) that can't be caught at
//! compile/link time — exactly the class of bug this crate currently has no
//! way to catch before a real human runs it, unlike everything else checked
//! by cross-compiling. Manual frames are more verbose but every failure
//! mode is a compile error or an obviously-wrong number, not a runtime
//! constraint solver falling over in a way only visible on real hardware.
//!
//! # `ctrl` → ⌘, `alt` → ⌥: `browser_core::KeyChord`'s modifiers on macOS
//!
//! `KeyChord::ctrl`/`alt` are cross-platform abstractions for "the OS's
//! primary command modifier" and "the OS's secondary modifier", not
//! literally always physical Control/Alt — Windows/Linux happen to use the
//! same physical keys, but macOS's platform convention for exactly these
//! app-level shortcuts (new tab, close tab, reload, ...) is ⌘ Command, not
//! Control (which mostly does nothing useful at this level in real Mac
//! apps) — so this crate maps `ctrl` to `NSEventModifierFlags::Command` and
//! `alt` to `NSEventModifierFlags::Option` (a literal match — Option *is*
//! Alt) when building `NSMenuItem` key equivalents. See `shortcuts.rs`.
//!
//! Known gaps, to be honest about up front:
//! - No bookmarks, light/dark theme toggle, or encrypted profiles — see the
//!   module doc comment above; matches `browser-windows-winui`/
//!   `browser-windows-reactor`'s scope, not `browser-linux-gtk3`'s.
//! - The switcher overlay is a plain vertical list, not a wrapping tile
//!   grid — `NSCollectionView` (the real AppKit equivalent of GTK's
//!   `FlowBox`/reactor's `grid_view`) is a much bigger lift than this pass
//!   had time for; a list is a real, working simplification, not a stub.
//! - The address bar doesn't update when navigating via in-page links —
//!   `RenderEngine` only offers a document-title-changed callback, not a
//!   URL-changed one, and wiring that up (`WKNavigationDelegate` on the
//!   `WKWebView` `wry` hands back) is future work, same gap the original
//!   scaffold already had.
#![cfg(target_os = "macos")]

mod shortcuts;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSButton, NSButtonType, NSControlStateValueOff,
    NSControlStateValueOn, NSEventModifierFlags, NSTextField, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use browser_core::{
    domain_of, launch_new_profile_process, list_profile_names, resolve_address_input, resolve_profile_name, resolve_url_argument,
    Action, HistoryEntry, HistoryStore, Keybindings, PageManager, Profile, Settings, HOME_URL,
};
use render_engine::{RenderEngine, WryEngine};

const TOOLBAR_HEIGHT: f64 = 36.0;
const BUTTON_WIDTH: f64 = 32.0;
const BUTTON_MARGIN: f64 = 4.0;
const ROW_HEIGHT: f64 = 44.0;
const OVERLAY_MARGIN: f64 = 16.0;
const OVERLAY_WIDTH: f64 = 480.0;

/// Mutually exclusive — opening any one of these closes whichever else was
/// open, mirroring every other front end's `close_switcher`/`close_settings`/
/// `close_profile_picker` convention. No separate `Keybindings` variant: the
/// editor lives as a section within `Settings` (see `rebuild_keybindings_rows`),
/// same design `browser-windows-reactor` settled on per explicit user
/// feedback.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Overlay {
    None,
    Switcher,
    Settings,
    Profile,
}

/// One row in the switcher's list — mirrors `browser-windows-winui`'s/
/// `browser-windows-reactor`'s `Tile`: open pages matching the search query
/// first, then a trailing "add page" row, then history matches (only once
/// there's a query, and only for URLs not already open).
#[derive(Clone)]
enum SwitcherRow {
    Open { id: String, title: String, domain: String },
    Add,
    History { url: String, title: String, domain: String },
}

struct AppState {
    window: Retained<NSWindow>,
    toolbar_view: Retained<NSView>,
    /// Doubles as the switcher's search box while it's open — same unified
    /// design as `browser-linux-gtk3`'s `address_bar` (see that crate's
    /// field doc for the full reasoning): one widget for both roles, not
    /// two.
    address_bar: Retained<NSTextField>,
    switcher_button: Retained<NSButton>,
    settings_button: Retained<NSButton>,
    profile_button: Retained<NSButton>,
    /// Hosts every page's container view (see `pages`) below the toolbar —
    /// the AppKit equivalent of GTK's `Stack`/reactor's `Grid`: every loaded
    /// page's container is a sibling subview here, only the active one
    /// visible (`isHidden` toggled), an unloaded page's simply isn't
    /// created at all.
    content_view: Retained<NSView>,
    /// One container `NSView` per page, each hosting that page's `WKWebView`
    /// (via `WryEngine`) as its own child — `browser_core::Page` doesn't
    /// hold this since it's an AppKit-only concept, same reasoning as
    /// `browser-linux-gtk3`'s `containers` field.
    containers: RefCell<HashMap<String, Retained<NSView>>>,
    core: RefCell<PageManager<WryEngine>>,
    overlay: Cell<Overlay>,

    switcher_view: Retained<NSView>,
    switcher_rows_container: Retained<NSView>,
    /// Rebuilt every time the switcher opens or its query changes — row
    /// buttons are tagged with their index into this so a click can look up
    /// which row it was (AppKit's `target`/`action` dispatch has no
    /// built-in "which item" beyond the sender itself).
    switcher_rows: RefCell<Vec<SwitcherRow>>,

    settings_view: Retained<NSView>,
    start_page_field: Retained<NSTextField>,
    unlimited_checkbox: Retained<NSButton>,
    limit_field: Retained<NSTextField>,
    keybindings_rows_container: Retained<NSView>,
    keybindings: RefCell<Keybindings>,
    /// `Some(action)` while the "Add binding" flow is waiting for text in
    /// `new_binding_field` to be committed as that action's new binding —
    /// mirrors `browser-windows-reactor`'s `listening_for` state.
    listening_for: Cell<Option<Action>>,
    new_binding_field: Retained<NSTextField>,

    profile_view: Retained<NSView>,
    profile_rows_container: Retained<NSView>,
    new_profile_field: Retained<NSTextField>,

    settings: RefCell<Settings>,
    history: HistoryStore,
    /// Resolved once at startup (from `--profile`, defaulting to
    /// `"default"`) — kept so the settings overlay's Save action re-saves to
    /// the same place `Settings::load`/`Keybindings::load` read from.
    profile: Profile,
}

impl AppState {
    /// Recomputes every frame from the window's current content size —
    /// AppKit has no layout manager doing this automatically for views added
    /// without an autoresizing mask (see this module's doc comment on why
    /// that's a deliberate choice, not an oversight).
    fn relayout(&self) {
        let content_size = self.window.contentView().map(|view| view.frame().size).unwrap_or(NSSize::new(0.0, 0.0));
        self.toolbar_view.setFrame(NSRect::new(
            NSPoint::new(0.0, content_size.height - TOOLBAR_HEIGHT),
            NSSize::new(content_size.width, TOOLBAR_HEIGHT),
        ));

        let button_count = 6.0; // back, forward, reload, switcher, settings, profile
        let address_bar_x = 3.0 * BUTTON_WIDTH + 4.0 * BUTTON_MARGIN;
        let address_bar_end = content_size.width - (button_count - 3.0) * (BUTTON_WIDTH + BUTTON_MARGIN) - BUTTON_MARGIN;
        self.address_bar.setFrame(NSRect::new(
            NSPoint::new(address_bar_x, BUTTON_MARGIN),
            NSSize::new((address_bar_end - address_bar_x).max(0.0), TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN),
        ));
        let mut x = address_bar_end + BUTTON_MARGIN;
        for button in [&self.switcher_button, &self.settings_button, &self.profile_button] {
            button.setFrame(NSRect::new(NSPoint::new(x, BUTTON_MARGIN), NSSize::new(BUTTON_WIDTH, TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN)));
            x += BUTTON_WIDTH + BUTTON_MARGIN;
        }

        let content_height_below_toolbar = (content_size.height - TOOLBAR_HEIGHT).max(0.0);
        let content_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(content_size.width, content_height_below_toolbar));
        for container in self.containers.borrow().values() {
            container.setFrame(content_frame);
        }
        if let Some(page) = self.core.borrow().active() {
            if let Some(engine) = &page.engine {
                if let Err(err) = engine.set_bounds(0, 0, content_frame.size.width.max(0.0) as u32, content_frame.size.height.max(0.0) as u32) {
                    eprintln!("failed to resize webview: {err}");
                }
            }
        }

        for overlay_view in [&self.switcher_view, &self.settings_view, &self.profile_view] {
            overlay_view.setFrame(content_frame);
        }
    }

    fn with_active(&self, action: impl FnOnce(&WryEngine) -> anyhow::Result<()>) {
        let core = self.core.borrow();
        if let Some(page) = core.active() {
            if let Some(engine) = &page.engine {
                if let Err(err) = action(engine) {
                    eprintln!("action on active page failed: {err}");
                }
            }
        }
    }

    fn is_switcher_open(&self) -> bool {
        self.overlay.get() == Overlay::Switcher
    }

    // ---- overlay open/close -------------------------------------------

    fn close_all_overlays(&self) {
        self.overlay.set(Overlay::None);
        self.switcher_view.setHidden(true);
        self.settings_view.setHidden(true);
        self.profile_view.setHidden(true);
        if let Some(page) = self.core.borrow().active() {
            self.address_bar.setStringValue(&NSString::from_str(&page.current_url()));
        }
        self.address_bar.setPlaceholderString(None);
    }

    fn open_switcher(self: &Rc<Self>) {
        self.close_all_overlays();
        self.address_bar.setStringValue(&NSString::from_str(""));
        self.address_bar.setPlaceholderString(Some(&NSString::from_str("Type to filter open pages\u{2026}")));
        self.overlay.set(Overlay::Switcher);
        self.rebuild_switcher_rows();
        self.switcher_view.setHidden(false);
        self.window.makeFirstResponder(Some(&self.address_bar));
    }

    /// `EditUrl` (⌘L): opens the switcher preloaded with the active page's
    /// current URL, fully selected — a real "edit the URL" affordance,
    /// unlike `browser-windows-reactor`'s `EditUrl`, which dispatches
    /// correctly but can't actually focus anything (`windows-reactor`
    /// exposes no `Focus()`-style API at all — see that crate's
    /// `dispatch_action` doc comment). AppKit *does* expose real,
    /// unrestricted programmatic focus (`NSWindow::makeFirstResponder`),
    /// so this is implemented for real here.
    fn open_switcher_editing_url(self: &Rc<Self>) {
        self.close_all_overlays();
        let current_url = self.core.borrow().active().map(|p| p.current_url()).unwrap_or_default();
        self.address_bar.setStringValue(&NSString::from_str(&current_url));
        self.address_bar.setPlaceholderString(None);
        self.overlay.set(Overlay::Switcher);
        self.rebuild_switcher_rows();
        self.switcher_view.setHidden(false);
        self.window.makeFirstResponder(Some(&self.address_bar));
        if let Some(editor) = self.address_bar.currentEditor() {
            unsafe { editor.selectAll(None) };
        }
    }

    fn toggle_switcher(self: &Rc<Self>) {
        if self.is_switcher_open() {
            self.close_all_overlays();
        } else {
            self.open_switcher();
        }
    }

    fn open_settings(self: &Rc<Self>) {
        self.close_all_overlays();
        self.listening_for.set(None);
        let settings = self.settings.borrow();
        self.start_page_field.setStringValue(&NSString::from_str(&settings.start_page));
        match settings.max_loaded_pages {
            Some(n) => {
                self.unlimited_checkbox.setState(NSControlStateValueOff);
                self.limit_field.setStringValue(&NSString::from_str(&n.to_string()));
                self.limit_field.setEnabled(true);
            }
            None => {
                self.unlimited_checkbox.setState(NSControlStateValueOn);
                self.limit_field.setStringValue(&NSString::from_str(""));
                self.limit_field.setEnabled(false);
            }
        }
        drop(settings);
        self.overlay.set(Overlay::Settings);
        self.rebuild_keybindings_rows();
        self.settings_view.setHidden(false);
    }

    /// Live toggle: clicking "Unlimited loaded pages" enables/disables
    /// `limit_field` immediately, matching `browser-linux-gtk3`'s
    /// `unlimited_check`'s toggle handler — without this, the field would
    /// only ever reflect the setting as of when the overlay was last
    /// opened, a real, worth-fixing UX gap, not just a cosmetic one.
    fn toggle_unlimited(&self) {
        let unlimited = self.unlimited_checkbox.state() == NSControlStateValueOn;
        self.limit_field.setEnabled(!unlimited);
    }

    fn save_settings(self: &Rc<Self>) {
        let unlimited = self.unlimited_checkbox.state() == NSControlStateValueOn;
        let limit_text = self.limit_field.stringValue().to_string();
        let new_limit = if unlimited { None } else { limit_text.trim().parse::<usize>().ok().map(|n| n.max(1)) };
        {
            let mut settings = self.settings.borrow_mut();
            settings.start_page = self.start_page_field.stringValue().to_string();
            settings.max_loaded_pages = new_limit;
        }
        let evicted = self.core.borrow_mut().set_max_loaded_pages(new_limit);
        self.unload_engines(&evicted);
        if let Err(err) = self.settings.borrow().save(&self.profile) {
            eprintln!("failed to save settings: {err}");
        }
        self.close_all_overlays();
    }

    fn open_profile_picker(self: &Rc<Self>) {
        self.close_all_overlays();
        self.new_profile_field.setStringValue(&NSString::from_str(""));
        self.overlay.set(Overlay::Profile);
        self.rebuild_profile_rows();
        self.profile_view.setHidden(false);
    }

    fn create_and_open_profile(&self) {
        let name = self.new_profile_field.stringValue().to_string();
        let name = name.trim();
        if !name.is_empty() {
            if let Err(err) = launch_new_profile_process(name) {
                eprintln!("failed to launch a new process for profile {name:?}: {err}");
            }
        }
        self.close_all_overlays();
    }

    // ---- pages ----------------------------------------------------------

    /// Allocates a fresh page id, builds its container view + `WryEngine`,
    /// unloads whatever `PageManager::insert` evicted to make room, and
    /// makes it active — the shared core of both the first-page bootstrap
    /// and the switcher's "+" tile.
    fn add_page(self: &Rc<Self>, url: &str) -> anyhow::Result<String> {
        let mtm = self.mtm();
        let mut core = self.core.borrow_mut();
        let id = core.allocate_id();
        drop(core);

        let container = NSView::initWithFrame(NSView::alloc(mtm), self.content_view.frame());
        self.content_view.addSubview(&container);
        // Weak, not `Rc::clone(self)` — this closure is stored inside the
        // `wry::WebView` that ends up owned (via `PageManager`) by this same
        // `AppState`, so a strong reference here would be a genuine `Rc`
        // cycle (`AppState -> core -> PageManager -> Page.engine ->
        // wry::WebView -> this closure -> Rc<AppState>`), keeping `AppState`
        // alive forever. Matches `browser-linux-gtk3`'s
        // `Rc::downgrade(self)` in the same spot.
        let self_for_title = Rc::downgrade(self);
        let id_for_title = id.clone();
        let engine = WryEngine::new(&container, url, move |title| {
            let Some(app) = self_for_title.upgrade() else { return };
            if let Some(page) = app.core.borrow_mut().page_mut(&id_for_title) {
                *page.title.borrow_mut() = title;
            }
            app.record_visit(&id_for_title);
        })?;

        let title = Rc::new(RefCell::new(String::new()));
        let evicted = self.core.borrow_mut().insert(id.clone(), engine, title);
        self.unload_engines(&evicted);
        self.containers.borrow_mut().insert(id.clone(), container);
        self.set_active(&id);
        Ok(id)
    }

    /// Records a history visit for `id`'s current URL/title — called from
    /// every page's title-changed callback (see `add_page`/
    /// `ensure_engine_loaded`). Previously missing entirely on this
    /// platform (a real, silent gap: browsing history never accumulated on
    /// macOS at all — see `ARCHITECTURE.md` §3.8); mirrors
    /// `browser-linux-gtk3`'s `AppState::record_visit`.
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

    /// Drops the live engine/container for every id `PageManager` evicted —
    /// the actual resource reclamation `enforce_loaded_limit`'s bookkeeping
    /// alone doesn't perform (mirrors every other front end's
    /// `unload_engines`/equivalent).
    fn unload_engines(&self, ids: &[String]) {
        for id in ids {
            self.core.borrow_mut().take_engine(id);
            if let Some(container) = self.containers.borrow_mut().remove(id) {
                container.removeFromSuperview();
            }
        }
    }

    /// Rebuilds a page's container/engine if it was unloaded — mirrors
    /// `browser-windows-reactor`'s `ensure_engine_loaded`.
    fn ensure_engine_loaded(self: &Rc<Self>, id: &str) {
        let needs_engine = self.core.borrow().page(id).map(|p| p.engine.is_none()).unwrap_or(false);
        if !needs_engine {
            return;
        }
        let mtm = self.mtm();
        let last_url = self.core.borrow().page(id).map(|p| p.current_url()).unwrap_or_default();
        let url = if last_url.is_empty() { HOME_URL.to_string() } else { last_url };
        let container = NSView::initWithFrame(NSView::alloc(mtm), self.content_view.frame());
        self.content_view.addSubview(&container);
        // Weak — see the identical comment in `add_page`.
        let self_for_title = Rc::downgrade(self);
        let id_for_title = id.to_string();
        match WryEngine::new(&container, &url, move |title| {
            let Some(app) = self_for_title.upgrade() else { return };
            if let Some(page) = app.core.borrow_mut().page_mut(&id_for_title) {
                *page.title.borrow_mut() = title;
            }
            app.record_visit(&id_for_title);
        }) {
            Ok(engine) => {
                self.core.borrow_mut().install_engine(id, engine);
                self.containers.borrow_mut().insert(id.to_string(), container);
            }
            Err(err) => eprintln!("failed to reload page {id}: {err}"),
        }
    }

    fn switch_to(self: &Rc<Self>, id: &str) {
        self.ensure_engine_loaded(id);
        self.set_active(id);
        self.close_all_overlays();
    }

    fn set_active(&self, id: &str) {
        self.core.borrow_mut().set_active(id);
        for (page_id, container) in self.containers.borrow().iter() {
            container.setHidden(page_id != id);
        }
        if let Some(page) = self.core.borrow().active() {
            self.address_bar.setStringValue(&NSString::from_str(&page.current_url()));
        }
        self.relayout();
    }

    fn close_page(self: &Rc<Self>, id: &str) {
        let was_active = self.core.borrow().active_id() == id;
        self.core.borrow_mut().remove(id);
        if let Some(container) = self.containers.borrow_mut().remove(id) {
            container.removeFromSuperview();
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
        self.rebuild_switcher_rows();
    }

    /// Enter in the address bar: navigates the active page, unless the
    /// switcher is open, in which case the same widget is acting as its
    /// search box — same unified design as `browser-linux-gtk3`'s
    /// `connect_activate` handler (see that crate's comment for the full
    /// reasoning): an exactly-one open-page match switches to it, else an
    /// exactly-one history match opens that entry, else the typed text is
    /// resolved (URL or search) into a brand-new page.
    /// ⌘Enter (the platform mapping of `browser-linux-gtk3`'s Ctrl+Enter —
    /// see this module's doc comment on `ctrl` → ⌘) while the switcher is
    /// open always opens a brand-new page from the typed text, even when it
    /// matches an open page or history entry (which plain Enter would
    /// instead switch to/open) — the escape hatch for deliberately wanting a
    /// second page at the same URL. Mirrors
    /// `browser-linux-gtk3`'s `force_new_page_from_search`; dropped
    /// (silently, not as a deliberate scope cut) when this crate was first
    /// ported from that one — see `ARCHITECTURE.md` §3.3.
    fn force_new_page_from_search(self: &Rc<Self>, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let url = resolve_address_input(trimmed, &self.settings.borrow());
        if let Err(err) = self.add_page(&url) {
            eprintln!("failed to open new page: {err}");
        }
        self.close_all_overlays();
    }

    /// Whether ⌘ was held for the key event that triggered the control
    /// action currently being handled — `NSApplication.currentEvent` is the
    /// standard AppKit way to recover this from inside an action method
    /// (there's no argument carrying it directly, unlike a raw `keyDown:`
    /// override).
    fn command_key_held(&self) -> bool {
        NSApplication::sharedApplication(self.mtm())
            .currentEvent()
            .is_some_and(|event| event.modifierFlags().contains(NSEventModifierFlags::Command))
    }

    fn address_bar_activated(self: &Rc<Self>) {
        let text = self.address_bar.stringValue().to_string();
        if self.is_switcher_open() && self.command_key_held() {
            self.force_new_page_from_search(&text);
            return;
        }
        if self.is_switcher_open() {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return;
            }
            let matches = self.core.borrow().matching_ids(trimmed);
            match matches.as_slice() {
                [only] => self.switch_to(&only.clone()),
                _ => {
                    let history_matches = self.history.search(trimmed, 2).unwrap_or_default();
                    if let [only] = history_matches.as_slice() {
                        let url = only.url.clone();
                        if let Err(err) = self.add_page(&url) {
                            eprintln!("failed to open history entry: {err}");
                        }
                        self.close_all_overlays();
                    } else {
                        let url = resolve_address_input(trimmed, &self.settings.borrow());
                        if let Err(err) = self.add_page(&url) {
                            eprintln!("failed to open new page: {err}");
                        }
                        self.close_all_overlays();
                    }
                }
            }
        } else {
            let resolved = resolve_address_input(&text, &self.settings.borrow());
            self.with_active(|engine| engine.navigate(&resolved));
        }
    }

    fn mtm(&self) -> MainThreadMarker {
        MainThreadMarker::new().expect("AppState is only ever touched from the main thread")
    }

    // ---- rebuilding overlay row lists ------------------------------------

    /// Rebuilds every switcher row from scratch — open pages matching the
    /// search box's current text (or all of them, if empty —
    /// `PageManager::matching_ids` already handles that), a trailing
    /// "+ New Page" row, then, only once there's a query, matching history
    /// entries not already open. Mirrors `browser-windows-reactor`'s
    /// `switcher_overlay` closely; see this module's doc comment for why
    /// it's a plain list here instead of a wrapping tile grid.
    fn rebuild_switcher_rows(&self) {
        let mtm = self.mtm();
        let query = self.address_bar.stringValue().to_string();
        let open_matches = self.core.borrow().matching_ids(&query);
        let mut rows: Vec<SwitcherRow> = Vec::new();
        {
            let core = self.core.borrow();
            for page in core.pages() {
                if !open_matches.contains(&page.id) {
                    continue;
                }
                let title = page.title.borrow().clone();
                let title = if title.is_empty() { "New Page".to_string() } else { title };
                let url = page.current_url();
                let domain = domain_of(&url);
                let domain = if page.loaded { domain } else { format!("{domain} \u{b7} unloaded") };
                rows.push(SwitcherRow::Open { id: page.id.clone(), title, domain });
            }
        }
        rows.push(SwitcherRow::Add);
        if !query.trim().is_empty() {
            let open_urls: Vec<String> = self.core.borrow().pages().iter().map(|p| p.current_url()).collect();
            let history_matches: Vec<HistoryEntry> = self
                .history
                .search(&query, 8)
                .unwrap_or_else(|err| {
                    eprintln!("history search failed: {err}");
                    Vec::new()
                })
                .into_iter()
                .filter(|entry| !open_urls.contains(&entry.url))
                .collect();
            for entry in history_matches {
                let title = if entry.title.is_empty() { "New Page".to_string() } else { entry.title };
                rows.push(SwitcherRow::History { url: entry.url, title, domain: format!("{} \u{b7} history", entry.domain) });
            }
        }

        clear_subviews(&self.switcher_rows_container);
        let width = self.switcher_rows_container.frame().size.width;
        for (idx, row) in rows.iter().enumerate() {
            let (label, sub) = match row {
                SwitcherRow::Open { title, domain, .. } => (title.clone(), domain.clone()),
                SwitcherRow::Add => ("+ New Page".to_string(), String::new()),
                SwitcherRow::History { title, domain, .. } => (title.clone(), domain.clone()),
            };
            let text = if sub.is_empty() { label } else { format!("{label}\n{sub}") };
            let button = unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str(&text), None, None, mtm) };
            button.setTag(idx as isize);
            button.setFrame(NSRect::new(
                NSPoint::new(0.0, (rows.len() - 1 - idx) as f64 * ROW_HEIGHT),
                NSSize::new(width, ROW_HEIGHT - BUTTON_MARGIN),
            ));
            self.switcher_rows_container.addSubview(&button);
        }
        *self.switcher_rows.borrow_mut() = rows;
    }

    fn switcher_row_clicked(self: &Rc<Self>, idx: usize) {
        let rows = self.switcher_rows.borrow();
        let Some(row) = rows.get(idx).cloned() else { return };
        drop(rows);
        let start_page = self.settings.borrow().start_page.clone();
        match row {
            SwitcherRow::Open { id, .. } => self.switch_to(&id),
            SwitcherRow::Add => {
                if let Err(err) = self.add_page(&start_page) {
                    eprintln!("failed to open new page: {err}");
                }
                self.close_all_overlays();
            }
            SwitcherRow::History { url, .. } => {
                if let Err(err) = self.add_page(&url) {
                    eprintln!("failed to open history entry: {err}");
                }
                self.close_all_overlays();
            }
        }
    }

    /// Rebuilds the keybindings editor's rows — one per `Action::ALL`,
    /// showing its label, current chords as removable "×" buttons, and
    /// either an "Add binding" button or (while `listening_for ==
    /// Some(action)`) a text field to type the new chord in
    /// `"Cmd+Shift+P"` format. See `shortcuts::parse_chord`'s doc comment
    /// for why text entry rather than live key capture.
    fn rebuild_keybindings_rows(&self) {
        let mtm = self.mtm();
        clear_subviews(&self.keybindings_rows_container);
        let listening_for = self.listening_for.get();
        let actions = Action::ALL;
        for (row_idx, &action) in actions.iter().enumerate() {
            let y = (actions.len() - 1 - row_idx) as f64 * ROW_HEIGHT;
            let label = unsafe {
                NSButton::buttonWithTitle_target_action(&NSString::from_str(action.label()), None, None, mtm)
            };
            label.setButtonType(NSButtonType::MomentaryLight);
            label.setEnabled(false);
            label.setFrame(NSRect::new(NSPoint::new(0.0, y), NSSize::new(200.0, ROW_HEIGHT - BUTTON_MARGIN)));
            self.keybindings_rows_container.addSubview(&label);

            let mut x = 210.0;
            let chords = self.keybindings.borrow().bindings_for(action).to_vec();
            for chord in &chords {
                let remove = unsafe {
                    NSButton::buttonWithTitle_target_action(&NSString::from_str(&format!("{chord} \u{d7}")), None, None, mtm)
                };
                remove.setTag(row_idx as isize);
                remove.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(90.0, ROW_HEIGHT - BUTTON_MARGIN)));
                self.keybindings_rows_container.addSubview(&remove);
                x += 94.0;
            }

            if listening_for == Some(action) {
                self.new_binding_field.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(140.0, ROW_HEIGHT - BUTTON_MARGIN)));
                self.new_binding_field.setHidden(false);
                self.keybindings_rows_container.addSubview(&self.new_binding_field);
                x += 144.0;
                let ok = unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str("OK"), None, None, mtm) };
                ok.setTag(row_idx as isize);
                ok.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(40.0, ROW_HEIGHT - BUTTON_MARGIN)));
                self.keybindings_rows_container.addSubview(&ok);
            } else {
                let add = unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str("Add binding"), None, None, mtm) };
                add.setTag(row_idx as isize);
                add.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(100.0, ROW_HEIGHT - BUTTON_MARGIN)));
                self.keybindings_rows_container.addSubview(&add);
            }
        }
        if listening_for.is_none() {
            self.new_binding_field.setHidden(true);
            self.new_binding_field.setStringValue(&NSString::from_str(""));
        }
    }

    fn keybinding_add_clicked(&self, action_idx: usize) {
        let Some(&action) = Action::ALL.get(action_idx) else { return };
        self.new_binding_field.setStringValue(&NSString::from_str(""));
        self.listening_for.set(Some(action));
        self.rebuild_keybindings_rows();
        self.window.makeFirstResponder(Some(&self.new_binding_field));
    }

    fn keybinding_remove_clicked(&self, action_idx: usize) {
        // Tag only identifies the row/action here — with possibly several
        // chords per action, the leftmost chord button removed first is an
        // acceptable simplification (real removal-by-exact-chord would need
        // per-chord tags, not just per-row).
        let Some(&action) = Action::ALL.get(action_idx) else { return };
        let mut chords = self.keybindings.borrow().bindings_for(action).to_vec();
        if !chords.is_empty() {
            chords.remove(0);
        }
        self.keybindings.borrow_mut().set_bindings(action, chords);
        if let Err(err) = self.keybindings.borrow().save(&self.profile) {
            eprintln!("failed to save keybindings: {err}");
        }
        self.rebuild_keybindings_rows();
        self.rebuild_menu_key_equivalents();
    }

    fn keybinding_commit(&self, action_idx: usize) {
        let Some(&action) = Action::ALL.get(action_idx) else { return };
        let text = self.new_binding_field.stringValue().to_string();
        if let Some(chord) = shortcuts::parse_chord(&text) {
            let mut chords = self.keybindings.borrow().bindings_for(action).to_vec();
            if !chords.contains(&chord) {
                chords.push(chord);
            }
            self.keybindings.borrow_mut().set_bindings(action, chords);
            if let Err(err) = self.keybindings.borrow().save(&self.profile) {
                eprintln!("failed to save keybindings: {err}");
            }
        }
        self.listening_for.set(None);
        self.rebuild_keybindings_rows();
        self.rebuild_menu_key_equivalents();
    }

    /// Rebuilds the profile list — existing profiles (from
    /// `list_profile_names()`, fresh every time this overlay opens), the
    /// current one marked and closing the picker instead of launching a
    /// duplicate process of itself.
    fn rebuild_profile_rows(&self) {
        let mtm = self.mtm();
        clear_subviews(&self.profile_rows_container);
        let width = self.profile_rows_container.frame().size.width;
        let current_profile = self.profile.name.clone();
        let names = list_profile_names();
        for (idx, name) in names.iter().enumerate() {
            let is_current = *name == current_profile;
            let label = if is_current { format!("{name} (current)") } else { name.clone() };
            let button = unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str(&label), None, None, mtm) };
            button.setTag(idx as isize);
            button.setFrame(NSRect::new(
                NSPoint::new(0.0, (names.len() - 1 - idx) as f64 * ROW_HEIGHT),
                NSSize::new(width, ROW_HEIGHT - BUTTON_MARGIN),
            ));
            self.profile_rows_container.addSubview(&button);
        }
    }

    fn profile_row_clicked(self: &Rc<Self>, idx: usize) {
        let names = list_profile_names();
        let Some(name) = names.get(idx) else { return };
        if *name == self.profile.name {
            self.close_all_overlays();
            return;
        }
        if let Err(err) = launch_new_profile_process(name) {
            eprintln!("failed to launch a new process for profile {name:?}: {err}");
        } else {
            self.close_all_overlays();
        }
    }

    /// Rebuilds the app's main menu bar's key equivalents from the current
    /// `Keybindings` — called after every keybinding change, since
    /// `NSMenuItem`'s key equivalent (unlike reactor's per-render
    /// `KeyboardAccelerator` rebuild) is imperative, persistent state that
    /// has to be explicitly refreshed rather than being recomputed for free.
    fn rebuild_menu_key_equivalents(&self) {
        let Some(menu) = self.window.menu().or_else(|| NSApplication::sharedApplication(self.mtm()).mainMenu()) else { return };
        shortcuts::apply_key_equivalents(&menu, &self.keybindings.borrow());
    }
}

fn clear_subviews(view: &NSView) {
    for subview in view.subviews().iter() {
        subview.removeFromSuperview();
    }
}

struct AppDelegateIvars {
    state: RefCell<Option<Rc<AppState>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements we're violating —
    // AppDelegate doesn't override `dealloc`/`init` in a way that skips
    // superclass behavior.
    #[unsafe(super(NSObject))]
    // Every ivar here is only safe to touch from the main thread anyway, so
    // this object is main-thread-only rather than trying to make it
    // `Send`/`Sync`.
    #[thread_kind = MainThreadOnly]
    #[ivars = AppDelegateIvars]
    struct AppDelegate;

    impl AppDelegate {
        #[unsafe(method(goBack:))]
        fn go_back(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.with_active(|e| e.go_back());
            }
        }

        #[unsafe(method(goForward:))]
        fn go_forward(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.with_active(|e| e.go_forward());
            }
        }

        #[unsafe(method(reloadPage:))]
        fn reload_page(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.with_active(|e| e.reload());
            }
        }

        #[unsafe(method(addressBarActivated:))]
        fn address_bar_activated(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.address_bar_activated();
            }
        }

        #[unsafe(method(toggleSwitcher:))]
        fn toggle_switcher(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.toggle_switcher();
            }
        }

        #[unsafe(method(openSwitcherAction:))]
        fn open_switcher_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.open_switcher();
            }
        }

        #[unsafe(method(openSwitcherEditingUrl:))]
        fn open_switcher_editing_url(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.open_switcher_editing_url();
            }
        }

        #[unsafe(method(closePageAction:))]
        fn close_page_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                let id = state.core.borrow().active_id().to_string();
                state.close_page(&id);
            }
        }

        #[unsafe(method(closeAnyOverlay:))]
        fn close_any_overlay(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.close_all_overlays();
            }
        }

        #[unsafe(method(openSettingsAction:))]
        fn open_settings_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.open_settings();
            }
        }

        #[unsafe(method(saveSettingsAction:))]
        fn save_settings_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.save_settings();
            }
        }

        #[unsafe(method(toggleUnlimitedAction:))]
        fn toggle_unlimited_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.toggle_unlimited();
            }
        }

        #[unsafe(method(openProfilePickerAction:))]
        fn open_profile_picker_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.open_profile_picker();
            }
        }

        #[unsafe(method(createAndOpenProfileAction:))]
        fn create_and_open_profile_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.create_and_open_profile();
            }
        }

        #[unsafe(method(switcherRowClicked:))]
        fn switcher_row_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.switcher_row_clicked(idx);
                }
            }
        }

        #[unsafe(method(keybindingAddClicked:))]
        fn keybinding_add_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.keybinding_add_clicked(idx);
                }
            }
        }

        #[unsafe(method(keybindingRemoveClicked:))]
        fn keybinding_remove_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.keybinding_remove_clicked(idx);
                }
            }
        }

        #[unsafe(method(keybindingCommit:))]
        fn keybinding_commit(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.keybinding_commit(idx);
                }
            }
        }

        #[unsafe(method(profileRowClicked:))]
        fn profile_row_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.profile_row_clicked(idx);
                }
            }
        }

        #[unsafe(method(dispatchAction:))]
        fn dispatch_action(&self, sender: Option<&AnyObject>) {
            let Some(state) = self.ivars().state.borrow().clone() else { return };
            let Some(idx) = sender_tag(sender) else { return };
            let Some(&action) = Action::ALL.get(idx) else { return };
            match action {
                Action::OpenSwitcher => state.open_switcher(),
                Action::EditUrl => state.open_switcher_editing_url(),
                Action::ClosePage => {
                    let id = state.core.borrow().active_id().to_string();
                    state.close_page(&id);
                }
                Action::Reload => state.with_active(|e| e.reload()),
                Action::GoBack => state.with_active(|e| e.go_back()),
                Action::GoForward => state.with_active(|e| e.go_forward()),
                Action::OpenSettings => state.open_settings(),
                Action::OpenProfilePicker => state.open_profile_picker(),
                // Bookmarks/reader mode aren't implemented on this front end
                // either yet — matches browser-windows-winui/reactor's scope.
                Action::ToggleBookmark | Action::OpenBookmarks | Action::ToggleReaderMode => {}
            }
        }
    }

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSWindowDelegate for AppDelegate {
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.relayout();
            }
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        }
    }
);

/// Reads an `NSButton`'s `tag` (set via `setTag` when the row/button was
/// created) back out as a `usize` index — AppKit's `target`/`action`
/// dispatch hands back only the sender itself, so this is how a shared
/// handler method learns *which* row fired it.
fn sender_tag(sender: Option<&AnyObject>) -> Option<usize> {
    let sender = sender?;
    let button: &NSButton = sender.downcast_ref()?;
    usize::try_from(button.tag()).ok()
}

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppDelegateIvars { state: RefCell::new(None) });
        unsafe { msg_send![super(this), init] }
    }
}

/// Owns the delegate (and, through its ivars, the whole window/webview
/// tree) for as long as the app is meant to run — dropping this would tear
/// the app down, so `main.rs` is expected to hold it until `run()` returns.
pub struct App {
    mtm: MainThreadMarker,
    _delegate: Retained<AppDelegate>,
}

impl App {
    /// Hands control to `NSApplication`'s run loop — same role as GTK's
    /// `gtk::main()`, Win32's message loop, or WinUI 3's
    /// `Application::Start`. Doesn't return until the app quits (see
    /// `AppDelegate::window_will_close`, the only quit path this wires up).
    pub fn run(&self) {
        NSApplication::sharedApplication(self.mtm).run();
    }
}

/// A borderless-style toolbar/overlay button — small helper so every call
/// site doesn't repeat the same three-call dance.
fn make_button(title: &str, target: &AnyObject, action: objc2::runtime::Sel, mtm: MainThreadMarker) -> Retained<NSButton> {
    unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str(title), Some(target), Some(action), mtm) }
}

fn make_overlay_container(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSView> {
    let view = NSView::initWithFrame(NSView::alloc(mtm), frame);
    view.setHidden(true);
    view
}

fn make_text_field(mtm: MainThreadMarker, initial: &str) -> Retained<NSTextField> {
    NSTextField::textFieldWithString(&NSString::from_str(initial), mtm)
}

/// Builds the window, toolbar, overlay panels, and first page (loaded to
/// `settings.start_page`), wires the app's `NSMenu`, and returns an [`App`]
/// ready to `run()`.
pub fn build_window_and_app(profile: Profile) -> anyhow::Result<App> {
    let mtm = MainThreadMarker::new().ok_or_else(|| anyhow::anyhow!("build_window_and_app must be called from the main thread"))?;
    let settings = Settings::load(&profile);
    let history = HistoryStore::open(&profile)?;
    let keybindings = Keybindings::load(&profile);
    let initial_url = settings.start_page.clone();

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let delegate = AppDelegate::new(mtm);

    let window_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1000.0, 700.0));
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable | NSWindowStyleMask::Resizable;
    let window = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), window_rect, style, NSBackingStoreType::Buffered, false) };
    window.setTitle(&NSString::from_str("Claude Browser"));
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let content_root = window.contentView().ok_or_else(|| anyhow::anyhow!("NSWindow has no content view"))?;

    let toolbar_view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, window_rect.size.height - TOOLBAR_HEIGHT), NSSize::new(window_rect.size.width, TOOLBAR_HEIGHT)));
    content_root.addSubview(&toolbar_view);

    let back_button = make_button("\u{2190}", &delegate, sel!(goBack:), mtm);
    back_button.setFrame(NSRect::new(NSPoint::new(BUTTON_MARGIN, BUTTON_MARGIN), NSSize::new(BUTTON_WIDTH, TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN)));
    toolbar_view.addSubview(&back_button);

    let forward_button = make_button("\u{2192}", &delegate, sel!(goForward:), mtm);
    forward_button.setFrame(NSRect::new(NSPoint::new(2.0 * BUTTON_MARGIN + BUTTON_WIDTH, BUTTON_MARGIN), NSSize::new(BUTTON_WIDTH, TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN)));
    toolbar_view.addSubview(&forward_button);

    let reload_button = make_button("\u{21BB}", &delegate, sel!(reloadPage:), mtm);
    reload_button.setFrame(NSRect::new(NSPoint::new(3.0 * BUTTON_MARGIN + 2.0 * BUTTON_WIDTH, BUTTON_MARGIN), NSSize::new(BUTTON_WIDTH, TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN)));
    toolbar_view.addSubview(&reload_button);

    let address_bar = make_text_field(mtm, &initial_url);
    unsafe {
        address_bar.setTarget(Some(&*delegate));
        address_bar.setAction(Some(sel!(addressBarActivated:)));
    }
    toolbar_view.addSubview(&address_bar);

    let switcher_button = make_button("\u{229e}", &delegate, sel!(toggleSwitcher:), mtm);
    toolbar_view.addSubview(&switcher_button);
    let settings_button = make_button("\u{2699}", &delegate, sel!(openSettingsAction:), mtm);
    toolbar_view.addSubview(&settings_button);
    let profile_button = make_button("\u{1f464}", &delegate, sel!(openProfilePickerAction:), mtm);
    toolbar_view.addSubview(&profile_button);

    let content_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(window_rect.size.width, window_rect.size.height - TOOLBAR_HEIGHT));
    let content_view = NSView::initWithFrame(NSView::alloc(mtm), content_frame);
    content_root.addSubview(&content_view);

    // ---- switcher overlay ----
    let switcher_view = make_overlay_container(mtm, content_frame);
    content_view.addSubview(&switcher_view);
    let switcher_rows_container = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN), NSSize::new(content_frame.size.width - 2.0 * OVERLAY_MARGIN, content_frame.size.height - 2.0 * OVERLAY_MARGIN)));
    switcher_view.addSubview(&switcher_rows_container);
    // Row clicks route through the shared `switcherRowClicked:` selector —
    // set on each row button individually in `rebuild_switcher_rows`, not
    // here (there's nothing to attach it to yet).

    // ---- settings overlay ----
    let settings_view = make_overlay_container(mtm, content_frame);
    content_view.addSubview(&settings_view);
    let start_page_field = make_text_field(mtm, &settings.start_page);
    start_page_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, content_frame.size.height - OVERLAY_MARGIN - ROW_HEIGHT), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&start_page_field);

    let unlimited_checkbox = make_button("Unlimited loaded pages", &delegate, sel!(toggleUnlimitedAction:), mtm);
    unlimited_checkbox.setButtonType(NSButtonType::Switch);
    unlimited_checkbox.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, content_frame.size.height - OVERLAY_MARGIN - 2.0 * ROW_HEIGHT), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&unlimited_checkbox);

    let limit_field = make_text_field(mtm, "");
    limit_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, content_frame.size.height - OVERLAY_MARGIN - 3.0 * ROW_HEIGHT), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&limit_field);

    let keybindings_rows_container = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN + ROW_HEIGHT), NSSize::new(content_frame.size.width - 2.0 * OVERLAY_MARGIN, content_frame.size.height - 4.0 * ROW_HEIGHT - 2.0 * OVERLAY_MARGIN)),
    );
    settings_view.addSubview(&keybindings_rows_container);
    let new_binding_field = make_text_field(mtm, "");
    new_binding_field.setPlaceholderString(Some(&NSString::from_str("e.g. Cmd+Shift+P")));
    unsafe {
        new_binding_field.setTarget(Some(&*delegate));
        new_binding_field.setAction(Some(sel!(keybindingCommit:)));
    }
    new_binding_field.setHidden(true);

    let cancel_button = make_button("Cancel", &delegate, sel!(closeAnyOverlay:), mtm);
    cancel_button.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN), NSSize::new(90.0, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&cancel_button);
    let save_button = make_button("Save", &delegate, sel!(saveSettingsAction:), mtm);
    save_button.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN + 94.0, OVERLAY_MARGIN), NSSize::new(90.0, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&save_button);

    // ---- profile overlay ----
    let profile_view = make_overlay_container(mtm, content_frame);
    content_view.addSubview(&profile_view);
    let profile_rows_container = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN + 2.0 * ROW_HEIGHT), NSSize::new(content_frame.size.width - 2.0 * OVERLAY_MARGIN, content_frame.size.height - 4.0 * ROW_HEIGHT)),
    );
    profile_view.addSubview(&profile_rows_container);
    let new_profile_field = make_text_field(mtm, "");
    new_profile_field.setPlaceholderString(Some(&NSString::from_str("New profile name\u{2026}")));
    new_profile_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN + ROW_HEIGHT), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    profile_view.addSubview(&new_profile_field);
    let profile_cancel = make_button("Cancel", &delegate, sel!(closeAnyOverlay:), mtm);
    profile_cancel.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN), NSSize::new(90.0, ROW_HEIGHT - BUTTON_MARGIN)));
    profile_view.addSubview(&profile_cancel);
    let profile_create = make_button("Create & Open", &delegate, sel!(createAndOpenProfileAction:), mtm);
    profile_create.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN + 94.0, OVERLAY_MARGIN), NSSize::new(140.0, ROW_HEIGHT - BUTTON_MARGIN)));
    profile_view.addSubview(&profile_create);

    let state = Rc::new(AppState {
        window: window.clone(),
        toolbar_view,
        address_bar,
        switcher_button,
        settings_button,
        profile_button,
        content_view,
        containers: RefCell::new(HashMap::new()),
        core: RefCell::new(PageManager::new(settings.max_loaded_pages)),
        overlay: Cell::new(Overlay::None),
        switcher_view,
        switcher_rows_container,
        switcher_rows: RefCell::new(Vec::new()),
        settings_view,
        start_page_field,
        unlimited_checkbox,
        limit_field,
        keybindings_rows_container,
        keybindings: RefCell::new(keybindings),
        listening_for: Cell::new(None),
        new_binding_field,
        profile_view,
        profile_rows_container,
        new_profile_field,
        settings: RefCell::new(settings),
        history,
        profile,
    });
    delegate.ivars().state.replace(Some(Rc::clone(&state)));

    if let Err(err) = state.add_page(&initial_url) {
        eprintln!("failed to open the start page: {err}");
    }
    state.relayout();

    let menu = shortcuts::build_menu(&delegate, &state.keybindings.borrow(), mtm);
    app.setMainMenu(Some(&menu));

    window.makeKeyAndOrderFront(None);
    app.activate();

    Ok(App { mtm, _delegate: delegate })
}

/// Shows a small standalone window for launching with a URL argument (e.g.
/// from the OS's "open with"/default-browser handoff) — lets the user
/// confirm/pick which profile to open it in before the real browser window
/// appears. Mirrors `browser-windows-reactor`'s `run_chooser`: spawns a new
/// process rather than swapping in the real browser window in place (this
/// crate has no more of a way to hand off between two `NSApplication`
/// instances in one process than reactor does between two `windows-reactor`
/// windows).
pub fn run_chooser(url: String, default_profile: String) -> anyhow::Result<()> {
    let mtm = MainThreadMarker::new().ok_or_else(|| anyhow::anyhow!("run_chooser must be called from the main thread"))?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let delegate = ChooserDelegate::new(mtm, url, default_profile);

    let window_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(480.0, 240.0));
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
    let window = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), window_rect, style, NSBackingStoreType::Buffered, false) };
    window.setTitle(&NSString::from_str("Open link"));
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let content = window.contentView().ok_or_else(|| anyhow::anyhow!("NSWindow has no content view"))?;

    let url_label = make_text_field(mtm, delegate.ivars().url.as_str());
    url_label.setEditable(false);
    url_label.setBordered(false);
    url_label.setFrame(NSRect::new(NSPoint::new(16.0, 190.0), NSSize::new(448.0, 24.0)));
    content.addSubview(&url_label);

    let profile_field = make_text_field(mtm, &delegate.ivars().profile_name.borrow());
    profile_field.setFrame(NSRect::new(NSPoint::new(16.0, 150.0), NSSize::new(448.0, 28.0)));
    content.addSubview(&profile_field);
    *delegate.ivars().profile_field.borrow_mut() = Some(profile_field);

    let mut x = 16.0;
    for name in list_profile_names() {
        let button = make_button(&name, &delegate, sel!(pickSuggestion:), mtm);
        button.setFrame(NSRect::new(NSPoint::new(x, 110.0), NSSize::new(100.0, 28.0)));
        content.addSubview(&button);
        x += 104.0;
    }

    let cancel = make_button("Cancel", &delegate, sel!(cancelChooser:), mtm);
    cancel.setFrame(NSRect::new(NSPoint::new(16.0, 16.0), NSSize::new(100.0, 28.0)));
    content.addSubview(&cancel);
    let open = make_button("Open", &delegate, sel!(openChooser:), mtm);
    open.setFrame(NSRect::new(NSPoint::new(120.0, 16.0), NSSize::new(100.0, 28.0)));
    content.addSubview(&open);

    window.makeKeyAndOrderFront(None);
    app.activate();
    app.run();
    Ok(())
}

struct ChooserIvars {
    url: String,
    profile_name: RefCell<String>,
    profile_field: RefCell<Option<Retained<NSTextField>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ChooserIvars]
    struct ChooserDelegate;

    impl ChooserDelegate {
        #[unsafe(method(pickSuggestion:))]
        fn pick_suggestion(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            let Some(button): Option<&NSButton> = sender.downcast_ref() else { return };
            let title = button.title().to_string();
            if let Some(field) = self.ivars().profile_field.borrow().as_ref() {
                field.setStringValue(&NSString::from_str(&title));
            }
        }

        #[unsafe(method(cancelChooser:))]
        fn cancel_chooser(&self, _sender: Option<&AnyObject>) {
            std::process::exit(0);
        }

        #[unsafe(method(openChooser:))]
        fn open_chooser(&self, _sender: Option<&AnyObject>) {
            let profile_name = self
                .ivars()
                .profile_field
                .borrow()
                .as_ref()
                .map(|f| f.stringValue().to_string())
                .unwrap_or_default();
            if let Ok(exe) = std::env::current_exe() {
                if let Err(err) = std::process::Command::new(exe).arg("--profile").arg(&profile_name).arg(&self.ivars().url).spawn() {
                    eprintln!("failed to launch the browser process: {err}");
                }
            }
            std::process::exit(0);
        }
    }

    unsafe impl NSObjectProtocol for ChooserDelegate {}

    unsafe impl NSWindowDelegate for ChooserDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            std::process::exit(0);
        }
    }
);

impl ChooserDelegate {
    fn new(mtm: MainThreadMarker, url: String, default_profile: String) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ChooserIvars { url, profile_name: RefCell::new(default_profile), profile_field: RefCell::new(None) });
        unsafe { msg_send![super(this), init] }
    }
}

/// Re-exported so `main.rs` can parse `--profile`/a bare URL argument
/// exactly like every other front end's CLI handling.
pub fn resolve_args(args: Vec<String>) -> (Option<String>, String) {
    let url = resolve_url_argument(args.clone());
    let profile = resolve_profile_name(args);
    (url, profile)
}
