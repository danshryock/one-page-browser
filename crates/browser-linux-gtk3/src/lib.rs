//! GTK3 native chrome for the browser — Linux only. Gated on the whole
//! crate (rather than leaving it to fail on `gtk` being unresolved) so a
//! bare `cargo build` across the whole workspace succeeds everywhere: this
//! crate just compiles to an empty no-op on any other platform, symmetric
//! with how `browser-windows-winui`/`browser-windows-reactor` gate
//! themselves to `target_os = "windows"` and `browser-macos-appkit` to
//! `target_os = "macos"`.
#![cfg(target_os = "linux")]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use browser_core::{
    decide_vault_unlock_action, domain_of, list_profile_names, resolve_address_input, Action, BitwardenBackend, Bookmarks, HistoryStore,
    KeyChord, Keybindings, PageManager, PasswordStore, Profile, Session, SessionPage, Settings, Theme, VaultUnlockAction, APP_TITLE,
};
use gtk::prelude::*;
use render_engine::{NewWindowInfo, RenderEngine, WebContext, WebKitWebView, WryEngine};

/// The RPC handler registry backing the switcher/profile/passwords pages
/// (`browser_chrome_core::internal_pages`) — this file's first submodule
/// split (everything else still lives in this one file), scoped narrowly to
/// keep that JSON glue out of the GTK-widget-construction code below.
mod internal_rpc;

/// The password vault's session state — UI-level bookkeeping distinct from
/// `PasswordStore`/`PasswordBackend` (the storage/abstraction layer, in
/// `browser-core`): this enum only tracks what this specific running window
/// currently knows, not how the vault is actually backed.
enum VaultState {
    /// The profile has never had a vault passphrase set up
    /// (`!Profile::has_vault_passphrase()`).
    NotSetUp,
    /// The vault has a passphrase, but it hasn't been unlocked this run yet.
    Locked,
    Unlocked(PasswordStore),
}

pub struct AppState {
    /// The switcher grid's search/URL box, living entirely inside the
    /// switcher overlay's own layout (not the toolbar — see `title_button`).
    /// While the switcher is open, typing here filters the tile grid (open
    /// pages + history) or edits the active page's URL depending on how the
    /// switcher was opened (`open_switcher` vs. `open_switcher_editing_url`)
    /// — see `close_switcher` and the `connect_changed`/`connect_activate`
    /// wiring in `build_window_and_app`. One widget for both roles, not two,
    /// per the "unified search/URL bar" design.
    address_bar: gtk::Entry,
    /// The toolbar's clickable "title chip" label — shows the active page's
    /// title (see `refresh_title_label`). Not editable; the chip itself
    /// (the `gtk::Button` wrapping this label, styled `.title-chip`/
    /// `.title-chip:hover` in `build_window_and_app_with_history` to look
    /// like a bordered label at rest and shift toward looking like a text
    /// input on hover) isn't stored here — same convention every other
    /// toolbar button already follows, wired up once at construction time
    /// with no need to be reachable from `AppState` afterward.
    title_label: gtk::Label,
    stack: gtk::Stack,
    switcher_panel: gtk::Widget,
    /// Where `switcher_engine`'s webview gets built, once — part of
    /// `switcher_panel`'s own widget tree (see `grid_content` in
    /// `build_window_and_app_with_history`), replacing what used to be a
    /// native `gtk::FlowBox` of tiles.
    switcher_container: gtk::Box,
    /// The switcher's real webview page (`browser://switcher`'s own
    /// content) — built once, lazily, on first `open_switcher`/
    /// `open_switcher_editing_url` (`ensure_switcher_engine`), then kept
    /// alive for the rest of the process: unlike the profile-menu popover
    /// (rebuilt fresh every open, since it's small and infrequent), the
    /// switcher opens constantly (F1/Ctrl+T/every new page) and a real
    /// WebKit webview is expensive enough to construct that doing so on
    /// every single open would be a real, noticeable regression from the
    /// old native tile grid's instant toggle. `refresh_switcher_if_open`
    /// (not a full reload) is what keeps its content current instead.
    switcher_engine: RefCell<Option<WryEngine>>,
    /// Holds only the theme-dependent CSS rules (see `theme_css`'s doc
    /// comment) — reloaded by `apply_theme` whenever the theme changes,
    /// unlike the separate, never-reloaded base provider set up once in
    /// `build_window_and_app`.
    theme_provider: gtk::CssProvider,
    /// Anchored to the toolbar avatar button (`set_relative_to`, at
    /// construction) — `show_profile_menu` is what actually pops it up,
    /// after giving it a freshly built webview child (see
    /// `profile_menu_engine`). Dismiss-on-click-outside/Escape is
    /// `gtk::Popover`'s own native behavior, not something wired up here.
    profile_menu_popover: gtk::Popover,
    /// The profile-menu popover's webview, if it's currently open — built
    /// fresh on each `show_profile_menu` call and dropped (destroying the
    /// real webview widget) when the popover closes, rather than kept alive
    /// across opens: this is small and infrequent enough that reload-on-
    /// reopen is simpler than the switcher's own persistent-singleton
    /// approach, and guarantees the profile list/current profile it shows
    /// is never stale.
    profile_menu_engine: RefCell<Option<WryEngine>>,
    /// Anchored to the passwords toolbar button — `show_credential_picker_popover`
    /// pops it up with plain GTK rows (no webview, unlike the profile menu)
    /// when the active page's domain has more than one saved login to
    /// choose from. See `fill_credentials_for_active_page`.
    credential_picker_popover: gtk::Popover,
    keybindings: RefCell<Keybindings>,
    /// The toolbar star-toggle button, so its icon can be refreshed whenever
    /// the active page changes or a bookmark is added/removed for it.
    bookmark_toggle_button: gtk::Button,
    bookmarks: RefCell<Bookmarks>,
    /// The password manager overlay's root widget — now only ever shows the
    /// unlock/setup prompt below; browsing/editing saved logins lives at
    /// `browser://passwords` (see `internal_pages::PASSWORDS`) once the
    /// vault is unlocked — `open_passwords` navigates there and closes this
    /// overlay itself the moment that happens.
    passwords_panel: gtk::Widget,
    /// Text toggles "Set Up Password Vault"/"Unlock Password Vault"
    /// depending on `Profile::has_vault_passphrase()`.
    passwords_unlock_heading: gtk::Label,
    passwords_unlock_label: gtk::Label,
    passwords_unlock_entry: gtk::Entry,
    passwords_unlock_error_label: gtk::Label,
    /// Label toggles "Set Up"/"Unlock", same condition as
    /// `passwords_unlock_heading`.
    passwords_unlock_button: gtk::Button,
    /// Whether the vault has ever been set up, is set up but not opened
    /// this session, or is open and ready to use — see
    /// `decide_vault_unlock_action`'s doc comment for how a profile that
    /// already has a passphrase (for history, the vault, or both) ends up
    /// in each state.
    passwords: RefCell<VaultState>,
    /// The passphrase, if any, this run has already used to unlock *some*
    /// store (history at startup, or the vault) — reused silently instead
    /// of re-prompting when the vault turns out to need the very same
    /// passphrase. Never written to disk; cleared along with everything
    /// else when the process exits.
    session_passphrase: RefCell<Option<String>>,
    core: RefCell<PageManager<WryEngine>>,
    /// One `wry::WebContext` shared by every page this profile ever opens —
    /// what actually makes cookies/localStorage/cache persist across
    /// restarts (and be shared between tabs in the same session), instead of
    /// each page silently getting its own throwaway context. `None`
    /// directory for an `ephemeral` profile, matching every other
    /// `Profile`-scoped store's convention of never touching disk.
    web_context: RefCell<render_engine::WebContext>,
    /// GTK `Stack` children, keyed by page id — `browser_core::Page` doesn't
    /// hold these since they're a GTK-only concept.
    containers: RefCell<HashMap<String, gtk::Box>>,
    /// Every `console.log` call any page (including background/popup pages —
    /// see `add_page_related`) has made this session, in order — pushed to
    /// alongside the existing `eprintln!` at each `WryEngine::new`/
    /// `new_related` call site's `on_console_message` closure. Exists purely
    /// for `console_messages_for_test`: production code never reads this
    /// back, only appends to it (matching this file's existing `_for_test`
    /// accessor convention, e.g. `evaluate_script_on_active_page_for_test`).
    console_messages: RefCell<Vec<String>>,
    settings: RefCell<Settings>,
    history: HistoryStore,
    /// Resolved once at startup (from `--profile`, defaulting to
    /// `"default"`) — kept around so the settings overlay's Save action can
    /// re-save to the same place `Settings::load` read from, without
    /// re-parsing `std::env::args()`.
    profile: Profile,
    /// Set for the duration of `open_start_page_or_restored_session`'s
    /// restore loop — `save_session` no-ops while this is set, so restoring
    /// several saved pages (only the active one gets a real `add_page`, see
    /// that method's own doc comment; every other one is registered via
    /// `add_unloaded_page`, which never triggers a save on its own) can't
    /// have the *first* page's `add_page`/`set_active` call overwrite the
    /// just-loaded, complete session on disk with a partial one-page
    /// version before the loop even finishes registering the rest.
    restoring: Cell<bool>,
    /// Serves the compiled-in `browser-chrome-core/assets/` pages (the
    /// switcher/profile/passwords pages, plus the pre-existing embedded-
    /// assets proof-of-concept) over loopback HTTP — see
    /// `browser_chrome_core::internal_pages`. `Option` because the server
    /// can only be started once this `AppState` already exists as an `Rc`
    /// (its RPC-server sibling's handlers need `Rc::clone(&app)`), so it's
    /// backfilled right after construction, not built inline in this
    /// struct literal.
    embedded_asset_server: RefCell<Option<browser_chrome_core::EmbeddedAssetServer>>,
    /// Serves `/rpc/<method>` calls the switcher/profile/passwords pages
    /// make back into this same `AppState` — see `internal_rpc.rs`.
    webview_rpc_server: RefCell<Option<browser_chrome_core::WebviewRpcServer>>,
    /// Cached ports of the two servers above, read by `add_page`'s
    /// `browser://...` resolution (`browser_chrome_core::internal_pages::
    /// resolve`) without borrowing either `RefCell`. `0` until the servers
    /// are actually started, immediately after construction.
    asset_port: Cell<u16>,
    rpc_port: Cell<u16>,
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
        self.refresh_switcher_if_open();
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

    /// Fills the active page's login form with `entry`'s username/password
    /// (see `WryEngine::fill_login`) if `entry.domain` matches the active
    /// page's domain — filling credentials into a page whose domain doesn't
    /// match what they were saved for is a real phishing-adjacent footgun,
    /// not just a UX nicety to restrict. No native UI calls this directly
    /// right now (the old passwords overlay's Fill button moved away with
    /// the rest of that overlay — see `internal_rpc.rs`'s own doc comment on
    /// the passwords/autofill capability gap); kept alive because the real
    /// autocomplete-attribute-aware fill mechanism itself is unrelated to
    /// which UI triggers it, and a real decoupled trigger is planned
    /// follow-up work, not a dropped capability.
    fn fill_active_page_with_login(self: &Rc<Self>, entry: &browser_core::Login) {
        let Some(password) = entry.password.clone() else { return };
        let active_domain = self.core.borrow().active().map(|p| domain_of(&p.current_url()));
        if active_domain.as_deref() != Some(entry.domain.as_str()) {
            return;
        }
        let username = entry.username.clone();
        self.with_active(|engine| engine.fill_login(&username, &password));
    }

    /// Test helper: simulates the (currently UI-less — see
    /// `fill_active_page_with_login`'s doc comment) autofill action for the
    /// local vault's entry with the given username.
    pub fn fill_active_page_with_local_login(self: &Rc<Self>, username: &str) {
        let entry = match &*self.passwords.borrow() {
            VaultState::Unlocked(store) => store.list().unwrap_or_default().into_iter().find(|e| e.username == username),
            VaultState::Locked | VaultState::NotSetUp => None,
        };
        if let Some(entry) = entry {
            self.fill_active_page_with_login(&entry);
        }
    }

    /// Toggles reader mode on the active page — see
    /// `WryEngine::toggle_reader_mode`'s doc comment for what it actually
    /// does and its limitations. The toolbar button's action; a no-op if the
    /// active page's engine isn't currently loaded.
    pub fn toggle_reader_mode(self: &Rc<Self>) {
        self.with_active(|engine| engine.toggle_reader_mode());
    }

    pub fn add_page(self: &Rc<Self>, url: &str) -> anyhow::Result<()> {
        // `resolve_address_input` already passes a `browser://...` URL
        // through unchanged (it contains `"://"`); this is the actual
        // resolution step, turning it into the real loopback URL the
        // embedded-asset server serves it from — the one choke point every
        // navigation (address bar, switcher, history, session restore)
        // already goes through. Falls back to the literal `browser://...`
        // string (which no real `RenderEngine` can load) only if it isn't
        // one of the known internal pages, or the asset/RPC servers never
        // started.
        let resolved = browser_chrome_core::internal_pages::resolve(url, self.asset_port.get(), self.rpc_port.get());
        let url = resolved.as_deref().unwrap_or(url);

        let id = self.core.borrow_mut().allocate_id();

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        self.stack.add_named(&container, &id);
        container.show_all();
        self.containers.borrow_mut().insert(id.clone(), container.clone());

        let title = Rc::new(RefCell::new(String::new()));
        let title_for_cb = Rc::clone(&title);
        let app_weak = Rc::downgrade(self);
        let id_for_cb = id.clone();
        let app_weak_audio = Rc::downgrade(self);
        let id_for_audio_cb = id.clone();
        let app_weak_new_window = Rc::downgrade(self);
        let app_weak_console = Rc::downgrade(self);
        let engine = WryEngine::new(
            &container,
            url,
            &mut *self.web_context.borrow_mut(),
            move |new_title| {
                *title_for_cb.borrow_mut() = new_title;
                if let Some(app) = app_weak.upgrade() {
                    app.record_visit(&id_for_cb);
                    app.refresh_switcher_if_open();
                    if app.core.borrow().active_id() == id_for_cb {
                        app.refresh_title_label();
                    }
                    // A title arrives asynchronously, well after `add_page`'s
                    // own sync already ran with an empty placeholder —
                    // confirmed directly on macOS (same shape of bug):
                    // without this, a freshly restored or opened page's
                    // title stayed "" in the saved session indefinitely,
                    // until some *other* open/close/switch event happened
                    // to sync it. `save_session` itself no-ops while
                    // `restoring`, so this is safe to call unconditionally
                    // even for a title arriving mid-restore.
                    app.save_session();
                }
            },
            move |playing| {
                if let Some(app) = app_weak_audio.upgrade() {
                    app.set_page_audio_playing(&id_for_audio_cb, playing);
                }
            },
            move |info: NewWindowInfo, opener: WebKitWebView| -> Option<gtk::Widget> {
                if !info.is_user_gesture {
                    return None;
                }
                app_weak_new_window.upgrade()?.add_page_related(&opener).ok()
            },
            move |message| {
                eprintln!("console.log: {message}");
                if let Some(app) = app_weak_console.upgrade() {
                    app.console_messages.borrow_mut().push(message);
                }
            },
        )?;

        let evicted = self.core.borrow_mut().insert(id.clone(), engine, title);
        self.unload_engines(&evicted);

        self.set_active(&id);
        self.refresh_switcher_if_open();
        Ok(())
    }

    /// Opens a page related to `related_to` — used for a page opened via
    /// `window.open()`/`target="_blank"`/"open in new tab" with a real user
    /// gesture behind it (see `WryEngine::new`'s `on_new_window_requested`
    /// doc comment), preserving `window.opener`/`postMessage`/the opener's
    /// own `window.open()` return value via `WryEngine::new_related` —
    /// unlike a plain unrelated background page, this needs the constructed
    /// widget handed straight back to WebKitGTK's `create` signal (the
    /// caller of this function), not just tracked internally, so it returns
    /// the raw `gtk::Widget` rather than `()`. Otherwise mirrors `add_page`:
    /// calls `insert_background` instead of `insert` and never calls
    /// `set_active`, so the new tab doesn't steal focus.
    pub fn add_page_related(self: &Rc<Self>, related_to: &WebKitWebView) -> anyhow::Result<gtk::Widget> {
        let id = self.core.borrow_mut().allocate_id();

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        self.stack.add_named(&container, &id);
        container.show_all();
        self.containers.borrow_mut().insert(id.clone(), container.clone());

        let title = Rc::new(RefCell::new(String::new()));
        let title_for_cb = Rc::clone(&title);
        let app_weak = Rc::downgrade(self);
        let id_for_cb = id.clone();
        let app_weak_audio = Rc::downgrade(self);
        let id_for_audio_cb = id.clone();
        let app_weak_new_window = Rc::downgrade(self);
        let app_weak_console = Rc::downgrade(self);
        let engine = WryEngine::new_related(
            &container,
            related_to,
            &mut *self.web_context.borrow_mut(),
            move |new_title| {
                *title_for_cb.borrow_mut() = new_title;
                if let Some(app) = app_weak.upgrade() {
                    app.record_visit(&id_for_cb);
                    app.refresh_switcher_if_open();
                    if app.core.borrow().active_id() == id_for_cb {
                        app.refresh_title_label();
                    }
                    // A title arrives asynchronously, well after `add_page`'s
                    // own sync already ran with an empty placeholder —
                    // confirmed directly on macOS (same shape of bug):
                    // without this, a freshly restored or opened page's
                    // title stayed "" in the saved session indefinitely,
                    // until some *other* open/close/switch event happened
                    // to sync it. `save_session` itself no-ops while
                    // `restoring`, so this is safe to call unconditionally
                    // even for a title arriving mid-restore.
                    app.save_session();
                }
            },
            move |playing| {
                if let Some(app) = app_weak_audio.upgrade() {
                    app.set_page_audio_playing(&id_for_audio_cb, playing);
                }
            },
            move |info: NewWindowInfo, opener: WebKitWebView| -> Option<gtk::Widget> {
                if !info.is_user_gesture {
                    return None;
                }
                app_weak_new_window.upgrade()?.add_page_related(&opener).ok()
            },
            move |message| {
                eprintln!("console.log: {message}");
                if let Some(app) = app_weak_console.upgrade() {
                    app.console_messages.borrow_mut().push(message);
                }
            },
        )?;

        let widget = engine.widget();
        let evicted = self.core.borrow_mut().insert_background(id.clone(), engine, title);
        self.unload_engines(&evicted);
        self.refresh_switcher_if_open();
        // A background tab (`window.open()`/target="_blank") never calls
        // `set_active`, which is where every other "opened" case's sync
        // happens — needs its own call here for the same reason.
        self.save_session();
        Ok(widget)
    }

    /// Registers a restored page's URL/title without constructing a real
    /// engine for it yet — used by `open_start_page_or_restored_session` for
    /// every restored page except the one that ends up active, so restoring
    /// a session with several saved tabs doesn't eagerly spin up that many
    /// real webviews (each a real, synchronous `WryEngine::new` call) before
    /// the window has even been shown. The engine gets built lazily, the
    /// same way any other unloaded page's does, the first time the user
    /// switches to it (`ensure_engine_loaded`).
    fn add_unloaded_page(&self, url: &str, title: &str) {
        let id = self.core.borrow_mut().allocate_id();

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        self.stack.add_named(&container, &id);
        container.show_all();
        self.containers.borrow_mut().insert(id.clone(), container);

        self.core.borrow_mut().insert_unloaded(id, Rc::new(RefCell::new(title.to_string())), url.to_string());
    }

    /// Opens either the saved session's pages (if any) or `start_page` —
    /// the two real startup call sites (a plain launch, and an encrypted
    /// profile's post-unlock handoff) both funnel through this instead of
    /// each duplicating the loop. Only the page that ends up active gets a
    /// real engine constructed eagerly (via `add_page`); every other
    /// restored page is registered via `add_unloaded_page` and loads lazily
    /// the first time it's switched to — restoring a session with several
    /// saved tabs shouldn't mean constructing that many real webviews before
    /// the window is even shown. `add_page` failures are logged and skipped
    /// rather than aborting the whole restore (a URL that no longer
    /// resolves shouldn't cost the user every *other* restored tab).
    pub fn open_start_page_or_restored_session(self: &Rc<Self>) {
        let session = Session::load(&self.profile);
        let start_page = self.settings.borrow().start_page.clone();
        let plan = browser_chrome_core::resolve_restore_plan(&session, &start_page);
        let active_index = plan.active_index.unwrap_or(0);

        self.restoring.set(true);
        for (idx, url) in plan.urls.iter().enumerate() {
            if idx == active_index {
                if let Err(err) = self.add_page(url) {
                    eprintln!("failed to open restored page {url:?}: {err}");
                }
            } else {
                let title = session.pages.get(idx).map(|p| p.title.as_str()).unwrap_or_default();
                self.add_unloaded_page(url, title);
            }
        }
        self.restoring.set(false);
        // The active page's own `add_page` (above) ran while `restoring`
        // was still set, so nothing has actually synced the freshly-built
        // session yet — do that once now that every page (loaded and
        // unloaded) is registered, rather than waiting for the next real
        // open/close/switch.
        self.save_session();
    }

    /// Snapshots the currently-open pages (URL + title, in `PageManager`'s
    /// own creation order) plus which one is active, for `quit` to save.
    fn build_session(&self) -> Session {
        let core = self.core.borrow();
        let active_id = core.active_id();
        let active_index = core.pages().iter().position(|p| p.id == active_id);
        let pages = core.pages().iter().map(|p| SessionPage { url: p.current_url(), title: p.title.borrow().clone() }).collect();
        Session { pages, active_index }
    }

    /// The real "the whole app is closing" hook — saves the session, then
    /// exits. Both `Action::Quit` (Ctrl+Q) and the window's own close
    /// button (`connect_delete_event`) call this same method rather than
    /// each separately implementing save-then-quit, so there's exactly one
    /// save path to keep correct.
    fn quit(&self) {
        self.save_session();
        gtk::main_quit();
    }

    /// Called continuously (`add_page`/`add_page_related`/`close_page`/
    /// `set_active`) as well as from `quit` — the session on disk is meant
    /// to always already reflect current state, not rely on a single
    /// "serialize on the way out" hook (which — see
    /// `browser-windows-reactor`'s own version of this method — is exactly
    /// the kind of thing that can quietly go unreachable on one platform's
    /// OS close button and nobody notices for a while). No-ops during
    /// `open_start_page_or_restored_session`'s restore loop (`restoring`) —
    /// see that field's own doc comment for why: the first restored page's
    /// `add_page` would otherwise save a one-page session over the real,
    /// complete one still being loaded.
    ///
    /// Split out from `quit` so tests can exercise a real save-then-restore
    /// round trip without also calling `gtk::main_quit()` — the shared GTK
    /// worker thread every test in this suite runs on (see
    /// `gtk_tests.rs`'s module doc comment) never actually calls
    /// `gtk::main()` itself (each test's job runs directly, not inside a
    /// driven main loop), so quitting it for real isn't something a test
    /// should risk relying on being harmless.
    fn save_session(&self) {
        if self.restoring.get() {
            return;
        }
        let session = self.build_session();
        if let Err(err) = session.save(&self.profile) {
            eprintln!("failed to save session: {err}");
        }
    }

    /// Test helper for `save_session` — see its doc comment for why tests
    /// don't call the real `quit()` (which also calls `gtk::main_quit()`).
    pub fn save_session_for_test(&self) {
        self.save_session();
    }

    /// Records a history visit for page `id`'s current URL/title — called
    /// from the `on_title_changed` callback (`add_page`/`ensure_engine_loaded`),
    /// the one place both are available together. Best-effort: logs rather
    /// than propagating, matching this codebase's existing error-handling
    /// style throughout (a failed history write shouldn't interrupt browsing).
    ///
    /// Skips the browser's own internal pages (`browser://switcher`, etc. —
    /// see `browser_chrome_core::internal_pages`): they're host UI a user
    /// navigated to, not a page they *browsed to*, so recording a visit for
    /// them would pollute history (and the switcher's own history column)
    /// with the browser's own chrome.
    fn record_visit(&self, id: &str) {
        let core = self.core.borrow();
        let Some(page) = core.page(id) else { return };
        let url = page.current_url();
        let title = page.title.borrow().clone();
        drop(core);
        if browser_chrome_core::internal_pages::is_internal_url(&url, self.asset_port.get()) {
            return;
        }
        if let Err(err) = self.history.record_visit(&url, &title) {
            eprintln!("failed to record history visit: {err}");
        }
    }

    /// `url`'s address-bar-facing display form: `browser://switcher` (etc.)
    /// instead of the raw loopback URL it's actually served from, for any of
    /// the browser's own internal pages — see `browser_chrome_core::
    /// internal_pages::display_url`. Passed through unchanged for an
    /// ordinary page's URL.
    fn display_url_for(&self, url: &str) -> String {
        browser_chrome_core::internal_pages::display_url(url, self.asset_port.get()).map(str::to_string).unwrap_or_else(|| url.to_string())
    }

    /// Navigates to one of the browser's own internal pages
    /// (`browser_chrome_core::internal_pages::{PROFILE,PASSWORDS}`) — or, if
    /// it's already open somewhere, switches to that existing page instead
    /// of opening a duplicate. The single navigation entry point for
    /// Settings/Passwords now that they're real pages rather than native
    /// overlays: used by the toolbar buttons, keybinding dispatch, and (via
    /// `internal_rpc.rs`'s `navigation.open_settings`/`.open_passwords`) the
    /// profile-menu popover, which has no other way to tell the host to
    /// navigate the *real* active page from inside its own small webview.
    pub fn open_or_focus_internal_page(self: &Rc<Self>, target: &'static str) {
        let existing = self
            .core
            .borrow()
            .pages()
            .iter()
            .find(|p| browser_chrome_core::internal_pages::display_url(&p.current_url(), self.asset_port.get()) == Some(target))
            .map(|p| p.id.clone());
        match existing {
            Some(id) => self.switch_to(&id),
            None => {
                if let Err(err) = self.add_page(target) {
                    eprintln!("failed to open {target}: {err}");
                }
            }
        }
    }

    /// Updates page `id`'s tracked audio-playing state and refreshes the
    /// switcher grid so its tile's speaker icon reflects it. Split out from
    /// the real `connect_is_playing_audio_notify` signal handler (wired in
    /// `add_page`/`ensure_engine_loaded`) so tests can drive it directly —
    /// this codebase's headless test compositor has no confirmed audio
    /// backend, so the real WebKitGTK signal can't be reliably exercised
    /// end-to-end here (same class of gap as `address_bar_focused`'s
    /// real-focus-event limitation).
    ///
    /// The flag is set synchronously (so `is_page_playing_audio` reflects it
    /// immediately), but the switcher-grid rebuild is deferred to the next
    /// idle main-loop iteration via `idle_add_local_once`, not called
    /// inline. `connect_is_playing_audio_notify` can fire from deep inside
    /// `WryEngine::new`'s post-build event-pump workaround (see that
    /// function's doc comment) — i.e. reentrantly, while GTK is still in
    /// the middle of processing events for a page that's still under
    /// construction. Rebuilding the whole grid (destroying and recreating
    /// every tile) from inside that nested call risks wedging the GTK main
    /// thread; WebKit's actual media pipeline runs in a separate process,
    /// so audio can keep playing audibly even while the main process never
    /// gets back around to mapping its window. Deferring runs the rebuild
    /// from the top-level main loop instead, once the nested call has
    /// unwound.
    pub fn set_page_audio_playing(self: &Rc<Self>, id: &str, playing: bool) {
        if let Some(page) = self.core.borrow_mut().page_mut(id) {
            page.is_playing_audio = playing;
        }
        let app = Rc::clone(self);
        gtk::glib::idle_add_local_once(move || app.refresh_switcher_if_open());
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
                    page.is_playing_audio = false;
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
        let app_weak_audio = Rc::downgrade(self);
        let id_for_audio_cb = id.to_string();
        let app_weak_new_window = Rc::downgrade(self);
        let app_weak_console = Rc::downgrade(self);
        match WryEngine::new(
            &container,
            &url,
            &mut *self.web_context.borrow_mut(),
            move |new_title| {
                *title_for_cb.borrow_mut() = new_title;
                if let Some(app) = app_weak.upgrade() {
                    app.record_visit(&id_for_cb);
                    app.refresh_switcher_if_open();
                    if app.core.borrow().active_id() == id_for_cb {
                        app.refresh_title_label();
                    }
                    // A title arrives asynchronously, well after `add_page`'s
                    // own sync already ran with an empty placeholder —
                    // confirmed directly on macOS (same shape of bug):
                    // without this, a freshly restored or opened page's
                    // title stayed "" in the saved session indefinitely,
                    // until some *other* open/close/switch event happened
                    // to sync it. `save_session` itself no-ops while
                    // `restoring`, so this is safe to call unconditionally
                    // even for a title arriving mid-restore.
                    app.save_session();
                }
            },
            move |playing| {
                if let Some(app) = app_weak_audio.upgrade() {
                    app.set_page_audio_playing(&id_for_audio_cb, playing);
                }
            },
            move |info: NewWindowInfo, opener: WebKitWebView| -> Option<gtk::Widget> {
                if !info.is_user_gesture {
                    return None;
                }
                app_weak_new_window.upgrade()?.add_page_related(&opener).ok()
            },
            move |message| {
                eprintln!("console.log: {message}");
                if let Some(app) = app_weak_console.upgrade() {
                    app.console_messages.borrow_mut().push(message);
                }
            },
        ) {
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
        self.refresh_title_label();
        self.refresh_bookmark_toggle_button();
        // Covers both "switched to" (direct callers) and "opened" (`add_page`
        // always calls this for the page it just created) — see
        // `save_session`'s own doc comment for why this isn't just a
        // quit-time hook anymore.
        self.save_session();
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
        self.close_passwords();
        self.stack.set_sensitive(false);
        self.ensure_switcher_engine();
        // Shown *before* refreshing: `refresh_switcher_if_open` skips its
        // work while the panel isn't visible (see its own doc comment), so
        // this order is what makes it actually populated when it appears.
        self.switcher_panel.show();
        self.refresh_switcher_if_open();
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
        self.address_bar.set_text(&self.display_url_for(&current_url));
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
            let url = page.current_url();
            self.address_bar.set_text(&self.display_url_for(&url));
        }
    }

    /// The address bar's `focus-in-event` handler's target — opens the
    /// switcher (preloaded with the active page's URL, same as Ctrl+L) the
    /// moment the address bar becomes focused, unless the switcher is
    /// already open. The toolbar (including the title chip) stays visible
    /// and clickable even while the switcher overlay is showing (the
    /// overlay only covers the content area below the header bar, not the
    /// header bar itself) — the guard matters because re-clicking the chip
    /// while the switcher is *already* open (e.g. mid-filter) must not
    /// clobber whatever the user already typed; only a *fresh* open should
    /// preload the current URL.
    pub fn title_chip_clicked(self: &Rc<Self>) {
        if !self.is_switcher_open() {
            self.open_switcher_editing_url();
        }
    }

    /// Moves keyboard focus from the address bar into the switcher's own
    /// webview — Down arrow's role while the address bar has focus (see the
    /// `address_bar.connect_key_press_event` handler). A no-op if the
    /// webview hasn't been built yet (the switcher was never opened).
    pub fn focus_switcher_grid(&self) {
        if let Some(engine) = self.switcher_engine.borrow().as_ref() {
            engine.widget().grab_focus();
        }
    }

    /// Reloads `theme_provider` with the current `Settings::theme`'s CSS —
    /// called once at startup (right after `AppState` is constructed) and
    /// again whenever `profile.settings.update_general` changes it (see
    /// `internal_rpc.rs`), so a theme change takes effect immediately
    /// without needing a restart.
    pub fn apply_theme(&self) {
        let theme = self.settings.borrow().theme;
        let _ = self.theme_provider.load_from_data(theme_css(theme).as_bytes());
    }

    /// Builds a fresh webview showing the profile-menu popover's content
    /// (see `profile_menu_engine`'s own doc comment for why fresh, not
    /// persistent) and pops it up anchored to the avatar button. No
    /// separate open/close/toggle trio the way the old in-window overlays
    /// needed — `gtk::Popover` already handles showing and click-outside/
    /// Escape dismissal itself; `profile_menu_popover.connect_closed` (see
    /// `build_window_and_app_with_history`) is what drops the engine once
    /// it's actually dismissed.
    pub fn show_profile_menu(self: &Rc<Self>) {
        for child in self.profile_menu_popover.children() {
            self.profile_menu_popover.remove(&child);
        }

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let url = format!("http://127.0.0.1:{}/profile-menu/index.html?rpc_port={}", self.asset_port.get(), self.rpc_port.get());
        let app_weak = Rc::downgrade(self);
        let engine = WryEngine::new(
            &container,
            &url,
            &mut self.web_context.borrow_mut(),
            |_title| {},
            |_playing| {},
            |_info, _opener| None,
            move |message| {
                eprintln!("console.log: {message}");
                if let Some(app) = app_weak.upgrade() {
                    app.console_messages.borrow_mut().push(message);
                }
            },
        );
        let engine = match engine {
            Ok(engine) => engine,
            Err(err) => {
                eprintln!("failed to build the profile menu popover: {err}");
                return;
            }
        };

        self.profile_menu_popover.add(&container);
        container.show_all();
        *self.profile_menu_engine.borrow_mut() = Some(engine);
        self.profile_menu_popover.popup();
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

    /// The passwords toolbar button's/`OpenPasswords` keybinding's target —
    /// resolves the vault's unlock/setup state (see
    /// `decide_vault_unlock_action`'s doc comment) first, which might mean
    /// silently reusing an already-known passphrase, silently establishing a
    /// brand new vault under one, or genuinely needing a prompt (only then
    /// does `passwords_panel` actually show, rendering just that prompt —
    /// see `rebuild_passwords_panel`). Once the vault is (or already was)
    /// unlocked, this is an **autofill** action for the *active page*, not
    /// "browse everything" — see `fill_credentials_for_active_page`.
    /// Browsing/editing every saved login lives at `browser://passwords`
    /// now, reached only via the avatar menu's Passwords link
    /// (`navigation.open_passwords`) or a bookmark, deliberately not from
    /// here: autofill needs the page you're actually trying to log into
    /// still active, which navigating away from it to a full "all passwords"
    /// page would defeat entirely.
    pub fn open_passwords(self: &Rc<Self>) {
        self.close_switcher();

        if !matches!(*self.passwords.borrow(), VaultState::Unlocked(_)) {
            match decide_vault_unlock_action(&self.profile, self.session_passphrase.borrow().as_deref()) {
                VaultUnlockAction::SilentlySetUpWith(passphrase) => {
                    self.try_open_vault_with(&passphrase, true);
                }
                VaultUnlockAction::SilentlyUnlockWith(passphrase) => {
                    self.try_open_vault_with(&passphrase, false);
                }
                // The cached passphrase (known from unlocking history)
                // somehow not working for the vault, or no passphrase known
                // yet at all — either way, `rebuild_passwords_panel` below
                // renders the locked/setup sub-group for a real prompt.
                VaultUnlockAction::PromptToSetUp | VaultUnlockAction::PromptToUnlock => {}
            }
        }

        if matches!(*self.passwords.borrow(), VaultState::Unlocked(_)) {
            self.fill_credentials_for_active_page();
            return;
        }
        self.show_passwords_panel();
    }

    /// The real autofill action once the vault is known to be unlocked:
    /// matches the active page's domain against every saved login with a
    /// password (`fill_active_page_with_login` itself re-checks the domain
    /// too — this is just what picks *which* entry to try). Exactly one
    /// match fills it directly, no UI at all; more than one shows a small
    /// native picker (`credential_picker_popover`) to choose from; zero
    /// matches is a silent no-op, same as clicking a native browser's
    /// autofill icon on a page it has nothing saved for.
    fn fill_credentials_for_active_page(self: &Rc<Self>) {
        let active_domain = self.core.borrow().active().map(|p| domain_of(&p.current_url()));
        let Some(domain) = active_domain else { return };
        let matches: Vec<browser_core::Login> = match &*self.passwords.borrow() {
            VaultState::Unlocked(store) => {
                store.list().unwrap_or_default().into_iter().filter(|entry| entry.domain == domain && entry.password.is_some()).collect()
            }
            VaultState::Locked | VaultState::NotSetUp => Vec::new(),
        };
        match matches.as_slice() {
            [] => {}
            [only] => self.fill_active_page_with_login(only),
            _ => self.show_credential_picker_popover(&matches),
        }
    }

    /// Lists `matches` (more than one saved login for the active page's
    /// domain) in a small popover anchored to the passwords toolbar button —
    /// same native `gtk::Popover` mechanism as `show_profile_menu`, just
    /// plain GTK widgets this time (no webview needed for a 2-3-row pick
    /// list). Picking one fills it and dismisses the popover.
    fn show_credential_picker_popover(self: &Rc<Self>, matches: &[browser_core::Login]) {
        for child in self.credential_picker_popover.children() {
            self.credential_picker_popover.remove(&child);
        }

        let list_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        list_box.set_margin(8);
        for entry in matches {
            let row = gtk::Button::with_label(&format!("{} \u{2014} {}", entry.domain, entry.username));
            row.style_context().add_class("flat");
            let app_clone = Rc::clone(self);
            let entry_clone = entry.clone();
            row.connect_clicked(move |_| {
                app_clone.fill_active_page_with_login(&entry_clone);
                app_clone.credential_picker_popover.popdown();
            });
            list_box.pack_start(&row, false, false, 0);
        }

        self.credential_picker_popover.add(&list_box);
        list_box.show_all();
        self.credential_picker_popover.popup();
    }

    /// Tries to open the vault with `passphrase`, updating `self.passwords`/
    /// `self.session_passphrase` on success. `is_setup` marks this as the
    /// vault's first-ever passphrase (calls `enable_vault_passphrase`) —
    /// see `PasswordStore::open_encrypted`'s doc comment for why "setup"
    /// and "unlock" are otherwise the same call. Returns whether it
    /// succeeded, so callers can fall back to rendering the in-overlay
    /// unlock prompt (see `rebuild_passwords_panel`) on failure instead of
    /// silently doing nothing.
    ///
    /// `pub` so tests can simulate completing the in-overlay unlock prompt
    /// directly (driving `AppState` rather than the real GTK widgets, same
    /// approach every other test in this suite takes) — not meant to be
    /// called from outside this crate in real use.
    pub fn try_open_vault_with(self: &Rc<Self>, passphrase: &str, is_setup: bool) -> bool {
        match PasswordStore::open_encrypted(&self.profile, passphrase) {
            Ok(store) => {
                if is_setup {
                    if let Err(err) = self.profile.enable_vault_passphrase() {
                        eprintln!("failed to mark profile as vault-passphrase-protected: {err}");
                    }
                }
                if self.session_passphrase.borrow().is_none() {
                    *self.session_passphrase.borrow_mut() = Some(passphrase.to_string());
                }
                *self.passwords.borrow_mut() = VaultState::Unlocked(store);
                true
            }
            Err(err) => {
                eprintln!("failed to open password vault: {err}");
                false
            }
        }
    }

    /// Records that `passphrase` is now known to unlock this session's
    /// encrypted stores — called once, right after successfully unlocking
    /// history with it at startup (see `show_passphrase_prompt`). If the
    /// vault already has its own passphrase set up but hasn't been opened
    /// yet this run, this also opens it immediately with the same
    /// passphrase — the concrete mechanism behind "the same passphrase
    /// unlocks both, when both are on." `pub` for the same test-facing
    /// reason as `try_open_vault_with`.
    pub fn note_unlocked_with_passphrase(self: &Rc<Self>, passphrase: &str) {
        *self.session_passphrase.borrow_mut() = Some(passphrase.to_string());
        if matches!(*self.passwords.borrow(), VaultState::Locked) {
            let _ = self.try_open_vault_with(passphrase, false);
        }
    }

    fn show_passwords_panel(self: &Rc<Self>) {
        self.rebuild_passwords_panel();
        self.stack.set_sensitive(false);
        self.passwords_panel.show();
    }

    /// The vault unlock/setup button's action (and the passphrase entry's
    /// Enter-to-submit) — replaces `show_vault_passphrase_prompt`'s
    /// `try_unlock` closure, now operating on the in-overlay fields instead
    /// of a popup window's local ones.
    fn unlock_vault_clicked(self: &Rc<Self>) {
        let passphrase = self.passwords_unlock_entry.text().to_string();
        if passphrase.is_empty() {
            self.passwords_unlock_error_label.set_text("Passphrase can't be empty.");
            return;
        }
        let is_setup = !self.profile.has_vault_passphrase();
        if self.try_open_vault_with(&passphrase, is_setup) {
            self.close_passwords();
            self.open_or_focus_internal_page(browser_chrome_core::internal_pages::PASSWORDS);
        } else {
            self.passwords_unlock_error_label.set_text("Couldn't open the vault with that passphrase. Try again.");
            self.passwords_unlock_entry.set_text("");
            self.passwords_unlock_entry.grab_focus();
        }
    }

    /// Hides the password manager overlay. Always use this (rather than
    /// hiding `passwords_panel` directly) so the stack never gets left
    /// insensitive.
    pub fn close_passwords(&self) {
        self.passwords_panel.hide();
        self.stack.set_sensitive(true);
    }

    /// Whether the password manager overlay is currently shown — test/
    /// inspection helper.
    pub fn is_passwords_open(&self) -> bool {
        self.passwords_panel.is_visible()
    }

    /// Whether the profile-menu popover is currently shown — test/
    /// inspection helper.
    pub fn is_profile_menu_open(&self) -> bool {
        self.profile_menu_popover.is_visible()
    }

    /// Whether the multi-match credential picker popover is currently
    /// shown — test/inspection helper.
    pub fn is_credential_picker_open(&self) -> bool {
        self.credential_picker_popover.is_visible()
    }

    /// The passwords toolbar button's target — see `toggle_settings`.
    pub fn toggle_passwords(self: &Rc<Self>) {
        if self.is_passwords_open() {
            self.close_passwords();
        } else {
            self.open_passwords();
        }
    }

    /// Builds a fresh `BitwardenBackend` from the current settings, if
    /// Bitwarden integration is enabled. Cheap to construct (no network I/O
    /// happens until a real call is made), so there's nothing to cache on
    /// `AppState`: `bw serve` — a separate, already-running process — is
    /// what actually owns the vault's lock state, not anything here. `pub`
    /// so tests can unlock a fake `bw serve` directly, the same
    /// test-facing-only reasoning as `try_open_vault_with`.
    pub fn bitwarden_backend(&self) -> Option<BitwardenBackend> {
        self.settings().bitwarden_server_url.clone().map(BitwardenBackend::new)
    }

    /// Reflects the vault's unlock/setup state in `passwords_unlock_box` —
    /// all `passwords_panel` shows now that browsing/editing saved logins
    /// lives at `browser://passwords` (see `internal_pages::PASSWORDS`):
    /// this overlay's only remaining job is collecting a passphrase.
    /// `open_passwords` closes the overlay and navigates there itself the
    /// moment the vault becomes unlocked — this just renders the prompt
    /// while it isn't.
    fn rebuild_passwords_panel(self: &Rc<Self>) {
        let unlocked = matches!(*self.passwords.borrow(), VaultState::Unlocked(_));
        let is_setup = !self.profile.has_vault_passphrase();

        self.passwords_unlock_heading.set_text(if is_setup { "Set Up Password Vault" } else { "Unlock Password Vault" });
        self.passwords_unlock_label.set_text(if is_setup {
            "Choose a passphrase to encrypt your password vault."
        } else {
            "Your password vault is passphrase-protected."
        });
        self.passwords_unlock_button.set_label(if is_setup { "Set Up" } else { "Unlock" });

        if !unlocked {
            self.passwords_unlock_entry.set_text("");
            self.passwords_unlock_error_label.set_text("");
            self.passwords_unlock_entry.grab_focus();
        }
    }

    /// Test helper: evaluates `script` in the active page's real webview,
    /// delivering its JSON-serialized result to `callback` — used to read
    /// back DOM state a test just filled via `fill_active_page_with_local_login`,
    /// since nothing in production code needs a script's return value (same
    /// reasoning as `WryEngine::evaluate_script_for_test`, which this calls).
    pub fn evaluate_script_on_active_page_for_test(&self, script: &str, callback: impl Fn(String) + Send + 'static) {
        let core = self.core.borrow();
        if let Some(page) = core.active() {
            if let Some(engine) = &page.engine {
                if let Err(err) = engine.evaluate_script_for_test(script, callback) {
                    eprintln!("evaluate_script_for_test failed: {err}");
                }
            }
        }
    }

    /// Test helper: every `console.log` call any page (including
    /// background/popup pages) has relayed this session, in order — see
    /// `console_messages`'s own doc comment for why this exists (real,
    /// production console-log capture, not something bolted on just for
    /// tests) and `web-standards-tests/`'s fixtures for what reads this back
    /// (`__test_target__ <name> <rect>` lines for resolving where to click,
    /// plus the fixture's own real assertion output).
    pub fn console_messages_for_test(&self) -> Vec<String> {
        self.console_messages.borrow().clone()
    }

    /// Test helper: the active page's real webview widget — lets a test
    /// compute where a `data-test-target` element actually is on screen
    /// (widget allocation + toplevel origin) before sending a genuine OS
    /// click there, the same way `evaluate_script_on_active_page_for_test`
    /// reaches the active page's engine for script evaluation.
    pub fn active_page_widget_for_test(&self) -> Option<gtk::Widget> {
        let core = self.core.borrow();
        core.active().and_then(|page| page.engine.as_ref()).map(|engine| engine.widget())
    }

    /// Shows a native "Save Screenshot" dialog (suggesting a filename built
    /// from the active page's domain and the current time, starting in this
    /// profile's `screenshots_dir()`) and, if confirmed, hands the chosen
    /// path to `save_screenshot_to`. Split out from the actual capture logic
    /// since the dialog blocks on real user input (`.run()`) — nothing that
    /// can run inside an automated test.
    pub fn take_screenshot(self: &Rc<Self>) {
        let default_name = {
            let core = self.core.borrow();
            let domain = core.active().map(|p| domain_of(&p.current_url())).unwrap_or_else(|| "page".to_string());
            format!("{domain}-{}.png", now_unix())
        };

        let dialog = gtk::FileChooserNative::new(
            Some("Save Screenshot"),
            gtk::Window::NONE,
            gtk::FileChooserAction::Save,
            Some("Save"),
            Some("Cancel"),
        );
        dialog.set_current_name(&default_name);
        if let Some(dir) = self.profile.screenshots_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = dialog.set_current_folder(&dir);
        }

        if dialog.run() == gtk::ResponseType::Accept {
            if let Some(path) = dialog.filename() {
                self.save_screenshot_to(path);
            }
        }
    }

    /// Captures the active page and writes it to `path` — the actual
    /// screenshot logic, independent of `take_screenshot`'s save dialog, so
    /// it can be driven directly from a test. Screenshotting is inherently
    /// async on every platform (see `RenderEngine::screenshot`), so the
    /// write happens in the callback, after this method has already
    /// returned — errors are logged rather than propagated, consistent with
    /// this codebase's other fire-and-forget UI actions.
    pub fn save_screenshot_to(self: &Rc<Self>, path: std::path::PathBuf) {
        self.with_active(|engine| {
            engine.screenshot(Box::new(move |result| match result {
                Ok(bytes) => {
                    if let Err(err) = std::fs::write(&path, &bytes) {
                        eprintln!("failed to write screenshot to {path:?}: {err}");
                    }
                }
                Err(err) => eprintln!("failed to capture screenshot: {err}"),
            }));
            Ok(())
        });
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

    /// Updates the toolbar's title chip to reflect the active page's current
    /// title — called whenever the active page changes or its title changes
    /// (`WryEngine::new`'s title-changed callback). Falls back to "New Page"
    /// for an empty title, matching `browser_chrome_core::switcher`'s
    /// existing convention for the same case.
    fn refresh_title_label(&self) {
        let title = self.core.borrow().active().map(|p| p.title.borrow().clone()).unwrap_or_default();
        self.title_label.set_text(if title.is_empty() { "New Page" } else { &title });
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

    /// Usernames of every credential currently in the vault, most-recently-
    /// added first — empty if the vault isn't unlocked. Test/inspection
    /// helper.
    pub fn password_vault_usernames(&self) -> Vec<String> {
        match &*self.passwords.borrow() {
            VaultState::Unlocked(store) => store.list().unwrap_or_default().into_iter().map(|e| e.username).collect(),
            VaultState::Locked | VaultState::NotSetUp => Vec::new(),
        }
    }

    /// Adds a login straight to the unlocked local vault — test helper.
    /// Real adds go through `internal_rpc.rs`'s `passwords.add` now (driven
    /// by `browser://passwords`, not a native form); a no-op if the vault
    /// isn't unlocked.
    pub fn add_password_for_test(&self, site: &str, username: &str, password: &str, notes: &str) {
        if let VaultState::Unlocked(store) = &*self.passwords.borrow() {
            if let Err(err) = store.add(browser_core::LoginFields::password_only(site, username, password, notes)) {
                eprintln!("failed to add password: {err}");
            }
        }
    }

    /// Bookmarks a URL directly, without needing to open it as a real page
    /// first — test helper for exercising the switcher webview's bookmark
    /// match (`internal_rpc.rs`'s `switcher.bookmarks`) in isolation from
    /// the history match: opening a page normally also ends up recording a
    /// history visit once its title loads, which would make a search match
    /// via history instead (already covered by its own existing test).
    pub fn bookmark_url_for_test(&self, url: &str, title: &str) {
        self.bookmarks.borrow_mut().add(url, title, now_unix());
        if let Err(err) = self.bookmarks.borrow().save(&self.profile) {
            eprintln!("failed to save bookmarks: {err}");
        }
    }

    /// Records a history visit directly, without needing to open it as a
    /// real page first — test helper for exercising the similar-history
    /// match (`internal_rpc.rs`'s `switcher_history`, merging in
    /// `HistoryStore::search_similar`) with a controlled, rich-enough title
    /// to test lexical similarity against (the fixture pages' own titles
    /// are single words, not enough vocabulary for a meaningful similarity
    /// comparison).
    pub fn record_history_visit_for_test(&self, url: &str, title: &str) -> anyhow::Result<()> {
        self.history.record_visit(url, title)
    }

    /// Sets and saves `Settings::start_page` directly — test helper. Real
    /// changes to `Settings` go through `internal_rpc.rs`'s
    /// `profile.settings.update_general` now (driven by `browser://profile`,
    /// not a native form), so this is the test-facing equivalent of what
    /// used to be `set_settings_start_page` + `save_settings`.
    pub fn set_start_page_for_test(&self, url: &str) {
        self.settings.borrow_mut().start_page = url.to_string();
        if let Err(err) = self.settings().save(&self.profile) {
            eprintln!("failed to save settings: {err}");
        }
    }

    /// Sets `Settings::theme` and reapplies it — test helper. Real theme
    /// changes go through `internal_rpc.rs`'s `profile.settings.
    /// update_general` now (driven by `browser://profile`), same reasoning
    /// as `set_start_page_for_test`.
    pub fn set_theme_for_test(self: &Rc<Self>, theme: Theme) {
        self.settings.borrow_mut().theme = theme;
        if let Err(err) = self.settings().save(&self.profile) {
            eprintln!("failed to save settings: {err}");
        }
        self.apply_theme();
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
            Action::OpenSettings => self.open_or_focus_internal_page(browser_chrome_core::internal_pages::PROFILE),
            Action::OpenProfilePicker => self.show_profile_menu(),
            Action::ToggleBookmark => self.toggle_bookmark_for_active(),
            // The dedicated bookmarks overlay is retired — browsing
            // bookmarks lives in the switcher's own Bookmarks tab now.
            Action::OpenBookmarks => self.open_switcher(),
            Action::ToggleReaderMode => self.toggle_reader_mode(),
            Action::OpenPasswords => self.open_passwords(),
            Action::NextPage => self.switch_to_next_page(),
            Action::PreviousPage => self.switch_to_previous_page(),
            Action::Quit => self.quit(),
        }
    }

    /// Ctrl+Tab/Ctrl+PageDown — switches to the next open page in creation
    /// order, wrapping around. A no-op with zero or one page (`next_page_id`
    /// returns the active page's own id, so this is harmless either way).
    /// The id is copied out of `core`'s borrow before calling `set_active`
    /// (which needs its own mutable borrow) rather than held across it —
    /// otherwise this would panic on the `RefCell`'s already-borrowed check.
    pub fn switch_to_next_page(self: &Rc<Self>) {
        let id = self.core.borrow().next_page_id().map(|s| s.to_string());
        if let Some(id) = id {
            self.set_active(&id);
        }
    }

    /// Ctrl+Shift+Tab/Ctrl+PageUp — same as `switch_to_next_page`, one
    /// position earlier.
    pub fn switch_to_previous_page(self: &Rc<Self>) {
        let id = self.core.borrow().previous_page_id().map(|s| s.to_string());
        if let Some(id) = id {
            self.set_active(&id);
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
        self.refresh_switcher_if_open();
        // Redundant (already harmless/idempotent) when `was_active` synced
        // via `set_active`/`add_page` above — needed for the non-active
        // "closed a background tab" case, which neither of those cover.
        self.save_session();
    }

    /// Builds the switcher's webview into `switcher_container` if it
    /// doesn't exist yet — see `switcher_engine`'s own doc comment for why
    /// this is a one-time, persistent build rather than something
    /// `refresh_switcher_if_open` redoes on every call.
    fn ensure_switcher_engine(self: &Rc<Self>) {
        if self.switcher_engine.borrow().is_some() {
            return;
        }
        let url = browser_chrome_core::internal_pages::resolve(browser_chrome_core::internal_pages::SWITCHER, self.asset_port.get(), self.rpc_port.get())
            .expect("SWITCHER is always a known internal page");
        let app_weak = Rc::downgrade(self);
        let engine = WryEngine::new(
            &self.switcher_container,
            &url,
            &mut self.web_context.borrow_mut(),
            |_title| {},
            |_playing| {},
            |_info, _opener| None,
            move |message| {
                eprintln!("console.log: {message}");
                if let Some(app) = app_weak.upgrade() {
                    app.console_messages.borrow_mut().push(message);
                }
            },
        );
        match engine {
            Ok(engine) => *self.switcher_engine.borrow_mut() = Some(engine),
            Err(err) => eprintln!("failed to build the switcher webview: {err}"),
        }
    }

    /// Tells the switcher's already-built webview to re-fetch and re-render
    /// itself (via its own `assets/switcher/app.js`'s `loadAll`), passing
    /// the address bar's current text through as the filter query — the
    /// replacement for what used to be a full native tile-grid rebuild.
    /// No-ops while the switcher panel isn't visible or the webview hasn't
    /// been built yet, same "nothing to show for it" reasoning the old
    /// tile-rebuild had (see every `add_page`/title-change/close-page call
    /// site this runs from unconditionally). `evaluate_script_for_test`'s
    /// name is a holdover from when it was only ever used by tests — the
    /// mechanism itself (WebKitGTK script evaluation) is exactly what a
    /// production-code refresh needs too, not something test-only.
    fn refresh_switcher_if_open(&self) {
        if !self.switcher_panel.is_visible() {
            return;
        }
        let binding = self.switcher_engine.borrow();
        let Some(engine) = binding.as_ref() else { return };
        let query = self.address_bar.text().to_string();
        let query_literal = serde_json::to_string(&query).unwrap_or_else(|_| "\"\"".to_string());
        let script = format!("if (typeof loadAll === 'function') {{ loadAll({query_literal}); }}");
        if let Err(err) = engine.evaluate_script_for_test(&script, |_| {}) {
            eprintln!("failed to refresh the switcher webview: {err}");
        }
    }

    /// Page ids in creation order — test/inspection helper.
    pub fn page_ids(&self) -> Vec<String> {
        self.core.borrow().page_ids()
    }

    /// Whether the switcher's webview currently has keyboard focus — test/
    /// inspection helper for confirming `focus_switcher_grid` (Down arrow
    /// in the address bar) actually landed somewhere, not just that it
    /// didn't crash. Real tile/row content is checked via
    /// `console_messages_for_test` instead, the same real-webview pattern
    /// every other internal page's tests already use — there's no native
    /// widget tree left to introspect for that.
    pub fn switcher_webview_has_focus(&self) -> bool {
        self.switcher_engine.borrow().as_ref().map(|engine| engine.widget().is_focus()).unwrap_or(false)
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

    /// A page's tracked audio-playing state (see `set_page_audio_playing`)
    /// — test/inspection helper.
    pub fn is_page_playing_audio(&self, id: &str) -> bool {
        self.core.borrow().page(id).map(|p| p.is_playing_audio).unwrap_or(false)
    }

    /// Whether the switcher grid is currently shown — test/inspection helper.
    pub fn is_switcher_open(&self) -> bool {
        self.switcher_panel.is_visible()
    }

    /// The switcher toolbar button's target — closes the grid if it's
    /// already open instead of just re-opening it. Every other overlay's
    /// trigger button now follows this same open-again-to-close convention
    /// (`toggle_settings`/`toggle_profile_picker`/`toggle_bookmarks`/
    /// `toggle_passwords`), this was just the original one.
    pub fn toggle_switcher(self: &Rc<Self>) {
        if self.is_switcher_open() {
            self.close_switcher();
        } else {
            self.open_switcher();
        }
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

    /// The toolbar's title chip's current text — test/inspection helper.
    pub fn title_label_text(&self) -> String {
        self.title_label.text().to_string()
    }

    /// Whether the address bar's entire current text is selected — test
    /// helper for confirming `open_switcher_editing_url` selects rather than
    /// blanks the current URL.
    pub fn address_bar_is_fully_selected(&self) -> bool {
        let len = self.address_bar.text().len() as i32;
        len > 0 && self.address_bar.selection_bounds() == Some((0, len))
    }

    /// The theme-provider's currently loaded CSS — test helper for
    /// confirming `apply_theme` actually reloaded it with the right
    /// theme's rules, not just that `Settings::theme` changed.
    pub fn theme_provider_css(&self) -> String {
        self.theme_provider.to_str().to_string()
    }

    /// Names of every search engine currently in `Settings::search_engines`
    /// — test helper for confirming add/remove actually changed the
    /// underlying data.
    pub fn settings_engine_names(&self) -> Vec<String> {
        self.settings.borrow().search_engines.iter().map(|e| e.name.clone()).collect()
    }
}

/// Current time as Unix seconds — used as a bookmark's `created_at` when
/// added, same precision `HistoryStore` uses internally for its own
/// timestamps.
fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// CSS for the theme-dependent rules only — the switcher grid's history/
/// bookmark/similar search-result tiles, the only surfaces left with a real
/// background of their own now that the settings/profile/keybindings/
/// bookmarks/passwords overlay boxes sit directly on the scrim (see
/// `base_provider`'s doc comment in `build_window_and_app`). Nothing else
/// needs to vary by theme.
fn theme_css(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => {
            ".history-tile { background-image: none; background-color: rgba(255, 255, 255, 0.12); \
               border: 1px dashed rgba(255, 255, 255, 0.3); box-shadow: none; border-radius: 10px; \
               color: #fff; opacity: 0.75; } \
             .bookmark-tile { background-image: none; background-color: rgba(212, 175, 55, 0.18); \
               border: 1px dashed rgba(212, 175, 55, 0.5); box-shadow: none; border-radius: 10px; \
               color: #fff; opacity: 0.85; } \
             .similar-tile { background-image: none; background-color: rgba(90, 200, 180, 0.16); \
               border: 1px dashed rgba(90, 200, 180, 0.5); box-shadow: none; border-radius: 10px; \
               color: #fff; opacity: 0.8; }"
        }
        Theme::Light => {
            ".history-tile { background-image: none; background-color: rgba(0, 0, 0, 0.06); \
               border: 1px dashed rgba(0, 0, 0, 0.25); box-shadow: none; border-radius: 10px; \
               color: #1a1a1a; opacity: 0.85; } \
             .bookmark-tile { background-image: none; background-color: rgba(180, 140, 20, 0.14); \
               border: 1px dashed rgba(180, 140, 20, 0.45); box-shadow: none; border-radius: 10px; \
               color: #1a1a1a; opacity: 0.9; } \
             .similar-tile { background-image: none; background-color: rgba(20, 140, 120, 0.12); \
               border: 1px dashed rgba(20, 140, 120, 0.4); box-shadow: none; border-radius: 10px; \
               color: #1a1a1a; opacity: 0.9; }"
        }
    }
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
        // GDK's own keysym names use underscores ("Page_Up"/"Page_Down"),
        // unlike every other named key this codebase already recognizes
        // ("Tab", "Left", "Right", "F1", ...) — translated here to match
        // `Keybindings::default()`'s "PageUp"/"PageDown" convention.
        None => match keyval.name()?.as_str() {
            "Page_Up" => "PageUp".to_string(),
            "Page_Down" => "PageDown".to_string(),
            name => name.to_string(),
        },
    };
    Some(KeyChord::new(ctrl, alt, shift, key))
}

/// Shared chrome for every full-screen overlay (switcher/settings/profile/
/// bookmarks/passwords): a dimming scrim behind `content`, a close (×)
/// button pinned to the upper-right, and a small "Press Esc to close" hint
/// next to it. Construction and signal-wiring are already two separate
/// passes in `build_window_and_app_with_history` — this only builds
/// widgets, so it takes no `Rc<AppState>` and the caller wires the returned
/// `scrim`/`close_button` to whichever `close_*` method is right for that
/// overlay, the same way scrim-click wiring already works today.
fn build_overlay_chrome(
    content: &impl gtk::glib::IsA<gtk::Widget>,
    scrim_css: &gtk::CssProvider,
) -> (gtk::Overlay, gtk::EventBox, gtk::Button) {
    let scrim = gtk::EventBox::new();
    scrim.style_context().add_class("switcher-scrim");
    scrim.style_context().add_provider(scrim_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    // Same "× glyph in a circular translucent button" idiom already used
    // for switcher tiles' `tile-close-btn`/`tile-close-label`, just larger
    // and pinned to the overlay's own corner instead of a tile's.
    let close_button = gtk::Button::new();
    close_button.style_context().add_class("overlay-close-btn");
    let close_label = gtk::Label::new(Some("\u{d7}"));
    close_label.style_context().add_class("overlay-close-label");
    close_button.add(&close_label);
    close_button.set_halign(gtk::Align::End);
    close_button.set_valign(gtk::Align::Start);
    close_button.set_margin_top(16);
    close_button.set_margin_end(16);
    close_button.set_size_request(28, 28);

    let esc_hint = gtk::Label::new(Some("Press Esc to close"));
    esc_hint.style_context().add_class("overlay-esc-hint");
    esc_hint.set_halign(gtk::Align::End);
    esc_hint.set_valign(gtk::Align::Start);
    esc_hint.set_margin_top(20);
    esc_hint.set_margin_end(52); // clears the close button

    let root_overlay = gtk::Overlay::new();
    root_overlay.add(&scrim);
    root_overlay.add_overlay(content);
    root_overlay.add_overlay(&close_button);
    root_overlay.add_overlay(&esc_hint);
    (root_overlay, scrim, close_button)
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
    let history = if profile.ephemeral { HistoryStore::open_in_memory()? } else { HistoryStore::open(&profile)? };
    build_window_and_app_with_history(profile, history)
}

/// Same as `build_window_and_app`, but takes an already-opened
/// `HistoryStore` instead of opening one itself — the passphrase-unlock
/// flow needs this, since collecting (and verifying) a passphrase and
/// opening an *encrypted* `HistoryStore` has to happen before this function
/// runs, not inside it (see `show_passphrase_prompt`). Every other caller
/// should keep using the plain `build_window_and_app` above.
pub fn build_window_and_app_with_history(profile: Profile, history: HistoryStore) -> anyhow::Result<(gtk::Window, Rc<AppState>)> {
    let theme_provider = gtk::CssProvider::new();
    if let Some(screen) = gtk::gdk::Screen::default() {
        // Theme-invariant rules: the overlay chrome (close button/Esc hint),
        // the title chip, and the passwords unlock overlay's own text/button
        // styling always sit over the scrim (a dark, translucent dimmer —
        // see `scrim_css` below) or otherwise don't vary with the app's own
        // light/dark theme, the same convention most apps' modal dimmers
        // use. The switcher itself is a real webview now (see
        // `ensure_switcher_engine`) with its own CSS, not native GTK
        // widgets styled from here.
        let base_provider = gtk::CssProvider::new();
        let _ = base_provider.load_from_data(
            b".overlay-close-btn:hover { filter: brightness(1.18); } \
              .overlay-close-btn { background-image: none; background-color: rgba(0, 0, 0, 0.45); \
                border: none; box-shadow: none; border-radius: 9999px; padding: 0; \
                min-width: 0; min-height: 0; } \
              .overlay-close-label { color: #ffffff; font-size: 14px; } \
              .overlay-esc-hint { color: rgba(255, 255, 255, 0.6); font-size: 12px; } \
              .title-chip { background-image: none; background-color: alpha(@theme_fg_color, 0.06); \
                border: 1px solid alpha(@theme_fg_color, 0.22); border-radius: 6px; box-shadow: none; \
                padding: 4px 12px; } \
              .title-chip:hover { background-color: @theme_base_color; border-color: @theme_selected_bg_color; } \
              .switcher-profile-label { color: rgba(255, 255, 255, 0.6); font-size: 12px; } \
              .settings-box { padding: 16px; } \
              .settings-title { color: #ffffff; font-weight: 600; font-size: 14px; } \
              .settings-box label:not(.settings-title) { color: rgba(255, 255, 255, 0.92); } \
              .settings-box label.settings-subtitle { color: rgba(255, 255, 255, 0.6); font-weight: 600; \
                font-size: 11px; margin-top: 10px; } \
              .settings-box button, .settings-box button:hover { background-image: none; \
                background-color: transparent; border: none; box-shadow: none; } \
              .settings-box button label { color: rgba(255, 255, 255, 0.92); } \
              .settings-box stackswitcher > button { background-image: none; \
                background-color: rgba(255, 255, 255, 0.15); border: none; box-shadow: none; \
                border-radius: 10px; margin: 0 4px 0 0; padding: 8px 14px; } \
              .settings-box stackswitcher > button:checked { background-color: #3b6fd4; }",
        );
        gtk::StyleContext::add_provider_for_screen(&screen, &base_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        gtk::StyleContext::add_provider_for_screen(&screen, &theme_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(&if profile.ephemeral { format!("{APP_TITLE} (Private)") } else { APP_TITLE.to_string() });
    window.set_default_size(1024, 768);
    // `connect_delete_event` is wired further down, once `app` exists —
    // see the comment there (search "the window's own close button") for
    // why it needs to call `AppState::quit` rather than `gtk::main_quit()`
    // directly.

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
    let profile_button = gtk::Button::new();
    profile_button.set_image(Some(&gtk::Image::from_icon_name(
        Some("avatar-default-symbolic"),
        gtk::IconSize::Button,
    )));
    // Anchored here once; `AppState::show_profile_menu` gives it a fresh
    // webview child on each open (see that method's own doc comment).
    let profile_menu_popover = gtk::Popover::new(Some(&profile_button));
    // Starts unbookmarked/non-starred — `refresh_bookmark_toggle_button`
    // (called once below, after `app` exists, and on every active-page
    // change afterward) corrects this immediately if the start page already
    // happens to be bookmarked.
    let bookmark_toggle_button = gtk::Button::new();
    bookmark_toggle_button.set_image(Some(&gtk::Image::from_icon_name(
        Some("non-starred-symbolic"),
        gtk::IconSize::Button,
    )));
    let screenshot_button = gtk::Button::new();
    screenshot_button.set_image(Some(&gtk::Image::from_icon_name(
        Some("camera-photo-symbolic"),
        gtk::IconSize::Button,
    )));
    screenshot_button.set_tooltip_text(Some("Save screenshot"));
    let reader_mode_button = gtk::Button::new();
    reader_mode_button.set_image(Some(&gtk::Image::from_icon_name(
        Some("view-reader-symbolic"),
        gtk::IconSize::Button,
    )));
    reader_mode_button.set_tooltip_text(Some("Toggle reader mode"));
    let passwords_button = gtk::Button::new();
    passwords_button.set_image(Some(&gtk::Image::from_icon_name(
        Some("dialog-password-symbolic"),
        gtk::IconSize::Button,
    )));
    passwords_button.set_tooltip_text(Some("Fill saved password"));
    // Anchored here once; `show_credential_picker_popover` gives it fresh
    // rows on each open when there's more than one match to choose from.
    let credential_picker_popover = gtk::Popover::new(Some(&passwords_button));
    for button in [
        &back_button,
        &forward_button,
        &reload_button,
        &switcher_toggle,
        &profile_button,
        &bookmark_toggle_button,
        &screenshot_button,
        &reader_mode_button,
        &passwords_button,
    ] {
        button.style_context().add_class("flat");
    }

    header_bar.pack_start(&back_button);
    header_bar.pack_start(&forward_button);

    // A clickable "title chip", not a text field — shows the active page's
    // title (see `refresh_title_label`), borders itself subtly, and shifts
    // toward looking like a text input on hover (both via the
    // `.title-chip`/`.title-chip:hover` CSS below) as a discoverability
    // hint that clicking it opens the switcher in URL-editing mode
    // (`open_switcher_editing_url`). The real editable text entry now lives
    // entirely inside the switcher overlay (see `grid_content` below) — a
    // `gtk::Button` (not a plain `Label`/`EventBox`) specifically because
    // buttons get native `:hover`/pseudo-class CSS support for free, and
    // it's the same click primitive every other toolbar control already
    // uses.
    let title_button = gtk::Button::new();
    title_button.style_context().add_class("title-chip");
    title_button.style_context().add_class("flat");
    let title_label = gtk::Label::new(Some("New Page"));
    title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title_button.add(&title_label);
    title_button.set_hexpand(true);

    // Group the reload button with the title chip itself (rather than
    // packing it into the header bar's separate end-region) so it's centered
    // as part of the same unit as the title chip, sitting flush against it.
    // A spacer before the chip and one after the reload button (each about
    // one toolbar button wide) doubles as draggable header-bar space for
    // moving the window.
    const TOOLBAR_BUTTON_WIDTH: i32 = 36;
    let spacer_before_address_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer_before_address_bar.set_size_request(TOOLBAR_BUTTON_WIDTH, -1);
    let spacer_after_reload = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer_after_reload.set_size_request(TOOLBAR_BUTTON_WIDTH, -1);

    let address_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    address_group.pack_start(&spacer_before_address_bar, false, false, 0);
    address_group.pack_start(&title_button, true, true, 0);
    address_group.pack_start(&bookmark_toggle_button, false, false, 0);
    address_group.pack_start(&reload_button, false, false, 0);
    address_group.pack_start(&spacer_after_reload, false, false, 0);
    header_bar.set_custom_title(Some(&address_group));

    header_bar.pack_end(&switcher_toggle);
    header_bar.pack_end(&profile_button);
    header_bar.pack_end(&screenshot_button);
    header_bar.pack_end(&reader_mode_button);
    header_bar.pack_end(&passwords_button);

    window.set_titlebar(Some(&header_bar));

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.set_hexpand(true);

    let scrim_css = gtk::CssProvider::new();
    let _ = scrim_css.load_from_data(b".switcher-scrim { background-color: rgba(20,20,18,0.88); }");

    // The real, editable text entry — lives entirely inside the switcher now
    // (see the toolbar's `title_button` above). Doubles as the switcher's
    // search box (filter open pages/history) and, when opened via the title
    // chip's click, the URL editor for the active page — one widget for
    // both roles, same as before, just relocated out of the toolbar.
    let address_bar = gtk::Entry::new();
    address_bar.set_hexpand(true);

    // Where the switcher's real webview gets built (see `ensure_switcher_engine`)
    // — replaces what used to be a native `gtk::FlowBox` of tiles.
    let switcher_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    switcher_container.set_vexpand(true);
    switcher_container.set_hexpand(true);

    let grid_content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    grid_content.set_halign(gtk::Align::Fill);
    grid_content.set_valign(gtk::Align::Fill);
    grid_content.set_vexpand(true);
    grid_content.set_margin_top(12);
    grid_content.pack_start(&address_bar, false, false, 0);
    grid_content.pack_start(&switcher_container, true, true, 0);

    // Top-left, not top-right — the overlay chrome's close icon/Esc hint
    // now own that corner for every overlay.
    let profile_label = gtk::Label::new(Some(&profile.name));
    profile_label.style_context().add_class("switcher-profile-label");
    profile_label.set_halign(gtk::Align::Start);
    profile_label.set_valign(gtk::Align::Start);
    profile_label.set_margin_top(12);
    profile_label.set_margin_start(16);

    let (switcher_overlay, scrim, switcher_close_button) = build_overlay_chrome(&grid_content, &scrim_css);
    switcher_overlay.add_overlay(&profile_label);

    // --- Password manager overlay: same shape again. One row per
    // credential, rebuilt from `PasswordStore::list()` each time the
    // overlay opens and after every add/update/delete — plus an inline
    // add-new-credential form, since unlike bookmarks a password isn't
    // captured from the active page, it has to be typed in.
    let passwords_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    passwords_box.set_halign(gtk::Align::Fill);
    passwords_box.set_valign(gtk::Align::Start);
    passwords_box.style_context().add_class("settings-box");
    passwords_box.set_margin(24);

    let passwords_title = gtk::Label::new(Some("Password Manager"));
    passwords_title.style_context().add_class("settings-title");
    passwords_title.set_halign(gtk::Align::Start);
    passwords_box.pack_start(&passwords_title, false, false, 0);

    // ---- vault locked/setup sub-group ----
    // Shown instead of `passwords_content_box` below while the vault isn't
    // unlocked — replaces the old separate `show_vault_passphrase_prompt`
    // window (see `rebuild_passwords_panel`, which toggles between the two
    // sub-groups and fills in this group's text).
    let passwords_unlock_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let passwords_unlock_heading = gtk::Label::new(None);
    passwords_unlock_heading.style_context().add_class("settings-subtitle");
    passwords_unlock_heading.set_halign(gtk::Align::Start);
    passwords_unlock_box.pack_start(&passwords_unlock_heading, false, false, 0);
    let passwords_unlock_label = gtk::Label::new(None);
    passwords_unlock_label.set_line_wrap(true);
    passwords_unlock_label.set_halign(gtk::Align::Start);
    passwords_unlock_box.pack_start(&passwords_unlock_label, false, false, 0);
    let passwords_unlock_entry = gtk::Entry::new();
    passwords_unlock_entry.set_visibility(false);
    passwords_unlock_entry.set_placeholder_text(Some("Passphrase"));
    passwords_unlock_box.pack_start(&passwords_unlock_entry, false, false, 0);
    let passwords_unlock_error_label = gtk::Label::new(None);
    passwords_unlock_error_label.set_halign(gtk::Align::Start);
    passwords_unlock_box.pack_start(&passwords_unlock_error_label, false, false, 0);
    let passwords_unlock_button = gtk::Button::with_label("Unlock");
    passwords_unlock_box.pack_start(&passwords_unlock_button, false, false, 0);
    passwords_box.pack_start(&passwords_unlock_box, false, false, 0);

    // Close stays outside the unlock sub-group above — dismissing the
    // overlay shouldn't depend on the vault's unlock state.
    let passwords_close_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    passwords_close_row.set_halign(gtk::Align::End);
    let passwords_close_button = gtk::Button::with_label("Close");
    passwords_close_row.pack_start(&passwords_close_button, false, false, 0);
    passwords_box.pack_start(&passwords_close_row, false, false, 0);

    let (passwords_overlay, passwords_scrim, passwords_close_icon) = build_overlay_chrome(&passwords_box, &scrim_css);

    let root_overlay = gtk::Overlay::new();
    root_overlay.add(&stack);
    root_overlay.add_overlay(&switcher_overlay);
    root_overlay.add_overlay(&passwords_overlay);

    window.add(&root_overlay);
    window.show_all();
    switcher_overlay.hide();
    passwords_overlay.hide();

    let settings = Settings::load(&profile);
    let bookmarks = Bookmarks::load(&profile);
    let core = PageManager::new(settings.max_loaded_pages);
    let initial_vault_state = if profile.has_vault_passphrase() { VaultState::Locked } else { VaultState::NotSetUp };
    // One context for every page this profile ever opens (see
    // `WryEngine::new`'s doc comment). For an `ephemeral` profile this is
    // *not* simply `WebContext::new(None)` — confirmed by a real, initially-
    // failing test that two separate `None`-directory contexts still saw
    // each other's data: passing `None` doesn't mean "no persistence", it
    // means "whatever webkit2gtk's own built-in default `WebsiteDataManager`
    // is", which turned out to be a single shared, deterministic location,
    // not a fresh one per context. `WebContext`'s own dedicated
    // `new_ephemeral()` constructor exists for exactly this but is
    // `pub(crate)`-only inside `wry`, unreachable from here — so instead
    // each ephemeral profile gets its own uniquely-named temp directory,
    // guaranteeing it never shares data with any other session (this one
    // included, across separate `--incognito` launches) even though,
    // unlike every other `Profile`-scoped store, it does mean some bytes
    // briefly touch disk during the session rather than staying purely
    // in-memory.
    let web_context = if profile.ephemeral {
        static EPHEMERAL_WEBVIEW_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = EPHEMERAL_WEBVIEW_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("claude-browser-ephemeral-{}-{n}", std::process::id()));
        WebContext::new(Some(dir))
    } else {
        WebContext::new(profile.webview_data_dir())
    };
    let app = Rc::new(AppState {
        address_bar: address_bar.clone(),
        title_label: title_label.clone(),
        stack,
        switcher_panel: switcher_overlay.clone().upcast::<gtk::Widget>(),
        switcher_container: switcher_container.clone(),
        switcher_engine: RefCell::new(None),
        theme_provider: theme_provider.clone(),
        profile_menu_popover: profile_menu_popover.clone(),
        profile_menu_engine: RefCell::new(None),
        credential_picker_popover: credential_picker_popover.clone(),
        keybindings: RefCell::new(Keybindings::load(&profile)),
        bookmark_toggle_button: bookmark_toggle_button.clone(),
        bookmarks: RefCell::new(bookmarks),
        passwords_panel: passwords_overlay.clone().upcast::<gtk::Widget>(),
        passwords_unlock_heading: passwords_unlock_heading.clone(),
        passwords_unlock_label: passwords_unlock_label.clone(),
        passwords_unlock_entry: passwords_unlock_entry.clone(),
        passwords_unlock_error_label: passwords_unlock_error_label.clone(),
        passwords_unlock_button: passwords_unlock_button.clone(),
        passwords: RefCell::new(initial_vault_state),
        session_passphrase: RefCell::new(None),
        core: RefCell::new(core),
        web_context: RefCell::new(web_context),
        containers: RefCell::new(HashMap::new()),
        console_messages: RefCell::new(Vec::new()),
        settings: RefCell::new(settings),
        history,
        profile,
        restoring: Cell::new(false),
        embedded_asset_server: RefCell::new(None),
        webview_rpc_server: RefCell::new(None),
        asset_port: Cell::new(0),
        rpc_port: Cell::new(0),
    });
    app.apply_theme();

    // Started here (rather than inline in the struct literal above) since
    // the RPC server's handlers need `Rc::clone(&app)`, same as every GTK
    // signal handler wired below — `app` has to exist as an `Rc` first. See
    // `internal_pages::resolve` (called from `add_page`) for how a
    // `browser://...` URL turns into a real request to these two servers.
    match browser_chrome_core::EmbeddedAssetServer::start(browser_chrome_core::embedded_assets(), "index.html") {
        Ok(server) => {
            app.asset_port.set(server.port());
            *app.embedded_asset_server.borrow_mut() = Some(server);
        }
        Err(err) => eprintln!("failed to start the embedded asset server: {err}"),
    }
    match browser_chrome_core::WebviewRpcServer::start(internal_rpc::build_handlers(&app)) {
        Ok(server) => {
            app.rpc_port.set(server.port());
            *app.webview_rpc_server.borrow_mut() = Some(server);
        }
        Err(err) => eprintln!("failed to start the internal webview RPC server: {err}"),
    }

    {
        // Only filters the grid while the switcher is actually open — the
        // address bar keeps getting `connect_changed` events from ordinary
        // navigation (typing a URL) the rest of the time, which must not
        // touch the switcher's (hidden, but still live) tile list.
        let app = Rc::clone(&app);
        address_bar.connect_changed(move |_| {
            if app.is_switcher_open() {
                app.refresh_switcher_if_open();
            }
        });
    }
    {
        // The toolbar's title chip — see `AppState::title_chip_clicked`'s
        // doc comment for why it's guarded on `!is_switcher_open()`.
        let app = Rc::clone(&app);
        title_button.connect_clicked(move |_| {
            app.title_chip_clicked();
        });
    }
    {
        // Ctrl+Enter always opens a fresh page, even when the typed text
        // matches an open page/history entry — checked ahead of the plain
        // `connect_activate` handler below (which GtkEntry still emits
        // afterward for a bare Enter, since we only `Stop` when Ctrl is
        // actually held). Down arrow moves keyboard focus into the switcher's
        // own webview (see `focus_switcher_grid`) — closing/switching/
        // filtering inside it is a real page now, with its own click/
        // keyboard handling, not native FlowBox navigation.
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
            if event.keyval() == gtk::gdk::keys::Key::from_name("Down") {
                app.focus_switcher_grid();
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
    }
    {
        // The window's own close button (Ctrl+Q's `Action::Quit` arm calls
        // the same `AppState::quit` method — see its doc comment for why
        // both routes converge on one save-then-quit implementation rather
        // than each separately calling `gtk::main_quit()`).
        let app = Rc::clone(&app);
        window.connect_delete_event(move |_, _| {
            app.quit();
            gtk::glib::Propagation::Proceed
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
        // The address bar's Enter handler — filters/edits are its only
        // roles now that it lives entirely inside the switcher panel (see
        // the field doc on `AppState::address_bar`), so this no longer
        // needs to branch on whether the switcher is open: it always is,
        // by construction, whenever this widget is reachable at all.
        let app = Rc::clone(&app);
        address_bar.connect_activate(move |entry| {
            let text = entry.text().to_string();
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
        });
    }
    {
        let app = Rc::clone(&app);
        switcher_toggle.connect_clicked(move |_| {
            app.toggle_switcher();
        });
    }
    {
        let app = Rc::clone(&app);
        switcher_close_button.connect_clicked(move |_| {
            app.close_switcher();
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
        profile_button.connect_clicked(move |_| {
            app.show_profile_menu();
        });
    }
    {
        let app = Rc::clone(&app);
        profile_menu_popover.connect_closed(move |_| {
            *app.profile_menu_engine.borrow_mut() = None;
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
        screenshot_button.connect_clicked(move |_| {
            app.take_screenshot();
        });
    }
    {
        let app = Rc::clone(&app);
        reader_mode_button.connect_clicked(move |_| {
            app.toggle_reader_mode();
        });
    }
    {
        let app = Rc::clone(&app);
        passwords_button.connect_clicked(move |_| {
            app.toggle_passwords();
        });
    }
    {
        let app = Rc::clone(&app);
        passwords_scrim.connect_button_press_event(move |_, _| {
            app.close_passwords();
            gtk::glib::Propagation::Stop
        });
    }
    {
        let app = Rc::clone(&app);
        passwords_close_button.connect_clicked(move |_| {
            app.close_passwords();
        });
    }
    {
        let app = Rc::clone(&app);
        passwords_close_icon.connect_clicked(move |_| {
            app.close_passwords();
        });
    }
    {
        let app = Rc::clone(&app);
        passwords_unlock_button.connect_clicked(move |_| {
            app.unlock_vault_clicked();
        });
    }
    {
        let app = Rc::clone(&app);
        passwords_unlock_entry.connect_activate(move |_| {
            app.unlock_vault_clicked();
        });
    }
    {
        let app = Rc::clone(&app);
        window.connect_key_press_event(move |_, event| {
            let is_escape = event.keyval() == gtk::gdk::keys::Key::from_name("Escape");
            if is_escape && app.is_switcher_open() {
                app.close_switcher();
                return gtk::glib::Propagation::Stop;
            } else if is_escape && app.is_passwords_open() {
                app.close_passwords();
                return gtk::glib::Propagation::Stop;
            }

            let Some(chord) = gtk_key_to_chord(event) else {
                return gtk::glib::Propagation::Proceed;
            };

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

/// Shows a small standalone window collecting a passphrase for `profile`,
/// either to set up *new* encryption (`setup: true`, when launched with
/// `--setup-passphrase`, on a profile with no history database yet) or to
/// *unlock* an already-encrypted one (`setup: false`, when
/// `profile.has_passphrase()` — retries in place on a wrong passphrase
/// rather than closing). On success, builds the real browser window the
/// same way `show_external_link_chooser` does.
///
/// Assumes `gtk::init()` has already been called, and that the caller will
/// still call `gtk::main()` afterward regardless of which path (this or
/// `build_window_and_app`) ends up running — this only ever shows a window,
/// never drives its own event loop.
pub fn show_passphrase_prompt(profile: Profile, setup: bool) -> anyhow::Result<()> {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(if setup { "Set a passphrase" } else { "Enter passphrase" });
    window.set_default_size(420, 160);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin(16);

    let prompt_text = if setup {
        format!("Choose a passphrase to encrypt \u{201c}{}\u{201d}'s history.", profile.name)
    } else {
        format!("\u{201c}{}\u{201d}'s history is passphrase-protected.", profile.name)
    };
    let prompt_label = gtk::Label::new(Some(&prompt_text));
    prompt_label.set_line_wrap(true);
    prompt_label.set_halign(gtk::Align::Start);
    content.pack_start(&prompt_label, false, false, 0);

    let passphrase_entry = gtk::Entry::new();
    passphrase_entry.set_visibility(false);
    passphrase_entry.set_placeholder_text(Some("Passphrase"));
    content.pack_start(&passphrase_entry, false, false, 0);

    let error_label = gtk::Label::new(None);
    error_label.set_halign(gtk::Align::Start);
    content.pack_start(&error_label, false, false, 0);

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    button_row.set_halign(gtk::Align::End);
    let cancel_button = gtk::Button::with_label("Cancel");
    let confirm_button = gtk::Button::with_label(if setup { "Set Passphrase" } else { "Unlock" });
    button_row.pack_start(&cancel_button, false, false, 0);
    button_row.pack_start(&confirm_button, false, false, 0);
    content.pack_start(&button_row, false, false, 0);

    window.add(&content);
    window.show_all();

    // Same "was this close a real quit, or a handoff to the main window"
    // distinction `show_external_link_chooser` makes.
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

    // Shared between the button click and the entry's Enter-to-submit —
    // wrapped in `Rc<dyn Fn()>` rather than relying on the closure itself
    // being `Clone` (it would be here, since every capture is `Clone`, but
    // this doesn't depend on that holding for whatever this closure grows
    // to capture later).
    let try_unlock: Rc<dyn Fn()> = {
        let window = window.clone();
        let passphrase_entry = passphrase_entry.clone();
        let error_label = error_label.clone();
        let transitioning = Rc::clone(&transitioning);
        Rc::new(move || {
            let passphrase = passphrase_entry.text().to_string();
            if passphrase.is_empty() {
                error_label.set_text("Passphrase can't be empty.");
                return;
            }

            match HistoryStore::open_encrypted(&profile, &passphrase) {
                Ok(history) => {
                    // See `HistoryStore::open_encrypted`'s doc comment: the
                    // very first successful open with a passphrase is what
                    // establishes a fresh database's encryption, so "setup"
                    // and "unlock" both just open_encrypted the same way —
                    // this call *is* the setup step when `setup` is true.
                    if setup {
                        if let Err(err) = profile.enable_passphrase() {
                            eprintln!("failed to mark profile as passphrase-protected: {err}");
                        }
                    }
                    transitioning.set(true);
                    window.close();
                    match build_window_and_app_with_history(profile.clone(), history) {
                        Ok((_main_window, app)) => {
                            // The vault, if it has its own passphrase set up
                            // already, shares this same one (see
                            // `note_unlocked_with_passphrase`'s doc comment)
                            // — do this before opening the start page so a
                            // vault-open failure is logged early, not
                            // silently deferred.
                            app.note_unlocked_with_passphrase(&passphrase);
                            app.open_start_page_or_restored_session();
                        }
                        Err(err) => eprintln!("failed to open the browser window: {err}"),
                    }
                }
                Err(err) => {
                    // A wrong passphrase surfaces here as a generic error
                    // from the schema query (see `HistoryStore::open_encrypted`'s
                    // doc comment for why) — indistinguishable in practice
                    // from other open failures, so the message stays
                    // generic too rather than claiming more certainty than
                    // is actually known.
                    error_label.set_text("Couldn't open this profile with that passphrase. Try again.");
                    eprintln!("failed to open encrypted history store: {err}");
                    passphrase_entry.set_text("");
                    passphrase_entry.grab_focus();
                }
            }
        })
    };
    {
        let try_unlock = Rc::clone(&try_unlock);
        confirm_button.connect_clicked(move |_| try_unlock());
    }
    {
        let try_unlock = Rc::clone(&try_unlock);
        passphrase_entry.connect_activate(move |_| try_unlock());
    }

    Ok(())
}

