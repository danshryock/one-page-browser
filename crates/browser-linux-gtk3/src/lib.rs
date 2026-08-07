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
    decide_vault_unlock_action, domain_of, list_profile_names, resolve_address_input, Action, BitwardenBackend, BitwardenStatus,
    Bookmarks, HistoryStore, KeyChord, Keybindings, Login, LoginFields, PageManager, PasswordBackend, PasswordStore, Profile, Session,
    SessionPage, Settings, Theme, VaultUnlockAction, APP_TITLE,
};
use gtk::prelude::*;
use render_engine::{NewWindowInfo, RenderEngine, WebContext, WebKitWebView, WryEngine};

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

/// Which backend a `Login` shown in the password manager overlay actually
/// came from — `update`/`delete` must route to the same one, since there's
/// no "move a login between backends" operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginSource {
    Local,
    Bitwarden,
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
    light_theme_radio: gtk::RadioButton,
    dark_theme_radio: gtk::RadioButton,
    bitwarden_check: gtk::CheckButton,
    bitwarden_url_entry: gtk::Entry,
    /// Holds only the theme-dependent CSS rules (see `theme_css`'s doc
    /// comment) — reloaded by `apply_theme` whenever the theme changes,
    /// unlike the separate, never-reloaded base provider set up once in
    /// `build_window_and_app`.
    theme_provider: gtk::CssProvider,
    /// The profile picker overlay's root widget — same in-window-overlay
    /// pattern as `settings_panel`/`switcher_panel`.
    profile_panel: gtk::Widget,
    /// Rebuilt from `browser_core::list_profile_names()` each time the
    /// picker opens — holds one row per existing profile.
    profile_list_box: gtk::Box,
    new_profile_entry: gtk::Entry,
    new_profile_encrypted_check: gtk::CheckButton,
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
    /// The password manager overlay's root widget — same in-window-overlay
    /// pattern as the other five.
    passwords_panel: gtk::Widget,
    /// The vault locked/setup sub-group — shown instead of
    /// `passwords_content_box` while `passwords` isn't
    /// `VaultState::Unlocked`. Replaces the old separate-window
    /// `show_vault_passphrase_prompt`: the passphrase is now collected
    /// in-overlay, the same toggled-sub-group shape
    /// `browser-macos-appkit`'s `rebuild_passwords_view` already uses.
    passwords_unlock_box: gtk::Box,
    /// Text toggles "Set Up Password Vault"/"Unlock Password Vault"
    /// depending on `Profile::has_vault_passphrase()`.
    passwords_unlock_heading: gtk::Label,
    passwords_unlock_label: gtk::Label,
    passwords_unlock_entry: gtk::Entry,
    passwords_unlock_error_label: gtk::Label,
    /// Label toggles "Set Up"/"Unlock", same condition as
    /// `passwords_unlock_heading`.
    passwords_unlock_button: gtk::Button,
    /// The vault contents sub-group (saved-logins list + add/edit form) —
    /// shown once `passwords` is `VaultState::Unlocked`.
    passwords_content_box: gtk::Box,
    /// Rebuilt from `VaultState::Unlocked`'s `PasswordStore::list()` each
    /// time the overlay opens and after every add/update/delete.
    passwords_list_box: gtk::Box,
    /// Text toggles "Add Login"/"Edit Login" alongside
    /// `submit_password_button`'s label.
    passwords_form_heading: gtk::Label,
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
    /// The add/edit-credential form's fields — read/cleared by
    /// `submit_login_from_fields`, same pattern as
    /// `new_engine_name_entry`/`new_engine_url_entry` for the settings
    /// overlay's search-engine form. Doubles as the edit form too (see
    /// `start_editing_login`) rather than being a second, separate form.
    new_password_site_entry: gtk::Entry,
    new_password_username_entry: gtk::Entry,
    new_password_password_entry: gtk::Entry,
    new_password_notes_entry: gtk::Entry,
    /// Which existing login (if any) the form above is currently editing,
    /// and which backend it came from — `None` means "add new" mode.
    editing_login: RefCell<Option<(String, LoginSource)>>,
    /// Chooses which backend a brand-new login (`editing_login: None`) gets
    /// saved to — only meaningfully offered (see `rebuild_passwords_panel`)
    /// when Bitwarden integration is enabled; otherwise hidden and every add
    /// goes to the local vault, same as before this field existed.
    save_destination_combo: gtk::ComboBoxText,
    save_destination_row: gtk::Box,
    /// Submits the add/edit form — labeled "Add" or "Save" depending on
    /// `editing_login`.
    submit_password_button: gtk::Button,
    /// Only visible while `editing_login` is `Some` — abandons the edit and
    /// returns the form to "add new" mode.
    cancel_edit_button: gtk::Button,
    /// Surfaces the last add/update/delete failure against either backend
    /// (a network error, Bitwarden being locked mid-action, etc.) or a
    /// failed inline Bitwarden-unlock attempt — cleared at the top of every
    /// `rebuild_passwords_panel` call.
    passwords_error_label: gtk::Label,
    /// Snapshot taken by the last `rebuild_switcher_grid` call — the source
    /// of truth `activate_switcher_row` indexes into, so a tile's widget
    /// name can just be its position in this list rather than needing a
    /// separate id/url stashed on the widget itself.
    switcher_rows: RefCell<Vec<browser_chrome_core::SwitcherRow>>,
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

    /// Toggles reader mode on the active page — see
    /// `WryEngine::toggle_reader_mode`'s doc comment for what it actually
    /// does and its limitations. The toolbar button's action; a no-op if the
    /// active page's engine isn't currently loaded.
    pub fn toggle_reader_mode(self: &Rc<Self>) {
        self.with_active(|engine| engine.toggle_reader_mode());
    }

    /// Fills the active page's login form with `entry`'s username/password
    /// (see `WryEngine::fill_login`) and closes the password manager
    /// overlay — the "Fill" button's action. Re-checks `entry.domain`
    /// against the active page's domain itself (a no-op, silent return if
    /// it doesn't match, or if there's no password to fill) — `build_login_row`
    /// already gates whether the button is shown on the same check, but the
    /// restriction needs to be real and enforced here too, not just a UI
    /// affordance that only holds as long as this is the sole caller.
    fn fill_active_page_with_login(self: &Rc<Self>, entry: &Login) {
        let Some(password) = entry.password.clone() else { return };
        let active_domain = self.core.borrow().active().map(|p| domain_of(&p.current_url()));
        if active_domain.as_deref() != Some(entry.domain.as_str()) {
            return;
        }
        let username = entry.username.clone();
        self.with_active(|engine| engine.fill_login(&username, &password));
        self.close_passwords();
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
                    app.rebuild_switcher_grid();
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
        self.rebuild_switcher_grid();
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
                    app.rebuild_switcher_grid();
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
        self.rebuild_switcher_grid();
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
        gtk::glib::idle_add_local_once(move || app.rebuild_switcher_grid());
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
                    app.rebuild_switcher_grid();
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
        self.close_settings();
        self.close_profile_picker();
        self.close_bookmarks();
        self.close_passwords();
        self.stack.set_sensitive(false);
        // Shown *before* rebuilding: `rebuild_switcher_grid` skips its work
        // while the panel isn't visible (see its own doc comment), so this
        // order is what makes the panel actually populated when it appears.
        self.switcher_panel.show();
        self.rebuild_switcher_grid();
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

    /// Moves keyboard focus from the address bar into the switcher grid —
    /// Down arrow's role while the address bar has focus (see the
    /// `address_bar.connect_key_press_event` handler). `FlowBox` already
    /// supports arrow-key navigation among its children once focus is
    /// inside it, so this is the only push needed; `child_focus` is the
    /// standard GTK API for "a container receiving focus from outside via
    /// keyboard navigation," landing on its first (or last-focused) child.
    pub fn focus_switcher_grid(&self) {
        self.flowbox.child_focus(gtk::DirectionType::Down);
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
        self.close_passwords();
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
        match settings.theme {
            Theme::Light => self.light_theme_radio.set_active(true),
            Theme::Dark => self.dark_theme_radio.set_active(true),
        }
        match &settings.bitwarden_server_url {
            Some(url) => {
                self.bitwarden_check.set_active(true);
                self.bitwarden_url_entry.set_text(url);
            }
            None => {
                self.bitwarden_check.set_active(false);
                self.bitwarden_url_entry.set_text("");
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
            settings.theme = if self.light_theme_radio.is_active() { Theme::Light } else { Theme::Dark };
            settings.bitwarden_server_url = if self.bitwarden_check.is_active() {
                let url = self.bitwarden_url_entry.text().to_string();
                let url = url.trim();
                Some(if url.is_empty() { "http://127.0.0.1:8087".to_string() } else { url.to_string() })
            } else {
                None
            };
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
        self.apply_theme();
        self.close_settings();
    }

    /// Reloads `theme_provider` with the current `Settings::theme`'s CSS —
    /// called once at startup (right after `AppState` is constructed) and
    /// again every time `save_settings` runs, so a theme change takes
    /// effect immediately without needing a restart.
    pub fn apply_theme(&self) {
        let theme = self.settings.borrow().theme;
        let _ = self.theme_provider.load_from_data(theme_css(theme).as_bytes());
    }

    /// Whether the settings overlay is currently shown — test/inspection
    /// helper.
    pub fn is_settings_open(&self) -> bool {
        self.settings_panel.is_visible()
    }

    /// The settings toolbar button's target — closes the overlay if it's
    /// already open instead of just re-opening/re-seeding it, same
    /// open-again-to-close convention every overlay's trigger button now
    /// follows (see `toggle_switcher`, the original of this pattern).
    pub fn toggle_settings(self: &Rc<Self>) {
        if self.is_settings_open() {
            self.close_settings();
        } else {
            self.open_settings();
        }
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
        self.close_passwords();
        self.new_profile_entry.set_text("");
        self.new_profile_encrypted_check.set_active(false);
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

    /// The profile toolbar button's target — see `toggle_settings`.
    pub fn toggle_profile_picker(self: &Rc<Self>) {
        if self.is_profile_picker_open() {
            self.close_profile_picker();
        } else {
            self.open_profile_picker();
        }
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
    /// nothing needs pre-creating here. If `new_profile_encrypted_check` is
    /// checked, the new process is launched with `--setup-passphrase`
    /// instead, and will prompt for a passphrase itself rather than opening
    /// straight to the browser window — see `resolve_passphrase_setup_requested`'s
    /// doc comment for why the passphrase can't just be collected here and
    /// handed to the new process directly.
    pub fn create_and_open_profile(&self) {
        let name = self.new_profile_entry.text().to_string();
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let result = if self.new_profile_encrypted_check.is_active() {
            browser_core::launch_new_encrypted_profile_process(name)
        } else {
            browser_core::launch_new_profile_process(name)
        };
        if let Err(err) = result {
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
        self.close_passwords();
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

    /// The bookmarks toolbar button's target — see `toggle_settings`.
    pub fn toggle_bookmarks(self: &Rc<Self>) {
        if self.is_bookmarks_open() {
            self.close_bookmarks();
        } else {
            self.open_bookmarks();
        }
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

    /// Shows the password manager overlay. Unlike every other overlay, this
    /// first has to resolve the vault's unlock/setup state (see
    /// `decide_vault_unlock_action`'s doc comment) — opening it might mean
    /// silently reusing an already-known passphrase, silently establishing
    /// a brand new vault under one, or genuinely needing a prompt. Unlike
    /// this crate's earlier design, a "genuinely needing a prompt" outcome
    /// no longer opens a separate `gtk::Window` — `rebuild_passwords_panel`
    /// (called by `show_passwords_panel` below) renders the locked/setup
    /// sub-group inline instead, same as `browser-macos-appkit` already
    /// does.
    pub fn open_passwords(self: &Rc<Self>) {
        self.close_switcher();
        self.close_settings();
        self.close_profile_picker();
        self.close_bookmarks();
        // Always opens fresh, never mid-edit from a previous visit.
        self.cancel_editing_login();

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
        self.show_passwords_panel();
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
            self.rebuild_passwords_panel();
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

    /// One row for a `Login` — a label, a Copy button, an Edit button
    /// (fills the add/edit form from this entry — see `start_editing_login`),
    /// a "×" delete button (all three wired to route through `source` so
    /// they hit whichever backend this entry actually came from — identical
    /// for local and Bitwarden entries now, both are fully read/write), and
    /// a Fill button, shown only when `entry.password` is set and
    /// `entry.domain` matches `active_domain` — filling credentials into a
    /// page whose domain doesn't match what they were saved for is a real
    /// phishing-adjacent footgun, not just a UX nicety to restrict.
    fn build_login_row(self: &Rc<Self>, entry: &Login, source: LoginSource, active_domain: Option<&str>) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

        let label_text = format!("{} \u{2014} {}", entry.domain, entry.username);
        let label = gtk::Label::new(Some(&label_text));
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.pack_start(&label, true, true, 0);

        if entry.password.is_some() && active_domain == Some(entry.domain.as_str()) {
            let fill_button = gtk::Button::with_label("Fill");
            let app_clone = Rc::clone(self);
            let entry_clone = entry.clone();
            fill_button.connect_clicked(move |_| {
                app_clone.fill_active_page_with_login(&entry_clone);
            });
            row.pack_start(&fill_button, false, false, 0);
        }

        let copy_button = gtk::Button::with_label("Copy");
        let password = entry.password.clone().unwrap_or_default();
        copy_button.connect_clicked(move |_| {
            if let Some(display) = gtk::gdk::Display::default() {
                if let Some(clipboard) = gtk::Clipboard::default(&display) {
                    clipboard.set_text(&password);
                }
            }
        });
        row.pack_start(&copy_button, false, false, 0);

        let edit_button = gtk::Button::with_label("Edit");
        let app_clone = Rc::clone(self);
        let entry_clone = entry.clone();
        edit_button.connect_clicked(move |_| {
            app_clone.start_editing_login(&entry_clone, source);
        });
        row.pack_start(&edit_button, false, false, 0);

        let remove_button = gtk::Button::with_label("\u{d7}");
        let app_clone = Rc::clone(self);
        let id = entry.id.clone();
        remove_button.connect_clicked(move |_| {
            app_clone.delete_login(&id, source);
        });
        row.pack_start(&remove_button, false, false, 0);

        row
    }

    /// Rebuilds the password manager overlay: first toggles
    /// `passwords_unlock_box` vs. `passwords_content_box` based on
    /// `self.passwords`' state (mirroring
    /// `browser-macos-appkit::rebuild_passwords_view`) — if not yet
    /// unlocked, updates the unlock/setup heading/label/button text and
    /// returns, without touching the list below at all. Once unlocked,
    /// rebuilds the credential list from scratch, as two separate sections
    /// rather than one list interleaved by timestamp: "Saved" (the local
    /// vault) and, if Bitwarden integration is enabled, "Bitwarden". They're
    /// kept separate because their timestamps aren't comparable in any
    /// meaningful way (Bitwarden's own `revisionDate` isn't even fetched —
    /// see `bitwarden.rs`), so merging them into one sorted list would just
    /// be misleading. Both sections are fully read/write (see
    /// `build_login_row`) — also rebuilds `save_destination_combo`'s
    /// contents/visibility and clears any leftover error message, since
    /// this runs every time the overlay's state might have changed
    /// underneath it.
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
        self.passwords_unlock_box.set_visible(!unlocked);
        self.passwords_content_box.set_visible(unlocked);

        if !unlocked {
            self.passwords_unlock_entry.set_text("");
            self.passwords_unlock_error_label.set_text("");
            self.passwords_unlock_entry.grab_focus();
            return;
        }

        self.passwords_error_label.set_text("");
        for child in self.passwords_list_box.children() {
            self.passwords_list_box.remove(&child);
        }

        // Computed once, up front, so every row's Fill-button gating (see
        // `build_login_row`) checks against the same snapshot rather than
        // re-deriving it per row.
        let active_domain = self.core.borrow().active().map(|p| domain_of(&p.current_url()));

        let local_entries = match &*self.passwords.borrow() {
            VaultState::Unlocked(store) => store.list().unwrap_or_else(|err| {
                eprintln!("failed to list password entries: {err}");
                Vec::new()
            }),
            VaultState::Locked | VaultState::NotSetUp => Vec::new(),
        };

        let saved_title = gtk::Label::new(Some("Saved"));
        saved_title.set_halign(gtk::Align::Start);
        saved_title.style_context().add_class("tile-subtitle");
        self.passwords_list_box.pack_start(&saved_title, false, false, 0);
        if local_entries.is_empty() {
            let empty_label = gtk::Label::new(Some("No saved passwords yet"));
            empty_label.set_halign(gtk::Align::Start);
            self.passwords_list_box.pack_start(&empty_label, false, false, 0);
        }
        for entry in &local_entries {
            let row = self.build_login_row(entry, LoginSource::Local, active_domain.as_deref());
            self.passwords_list_box.pack_start(&row, false, false, 0);
        }

        let bitwarden_enabled = self.bitwarden_backend().is_some();
        self.save_destination_row.set_visible(bitwarden_enabled);
        self.refresh_save_destination_combo();

        if let Some(backend) = self.bitwarden_backend() {
            let bitwarden_title = gtk::Label::new(Some("Bitwarden"));
            bitwarden_title.set_halign(gtk::Align::Start);
            bitwarden_title.style_context().add_class("tile-subtitle");
            self.passwords_list_box.pack_start(&bitwarden_title, false, false, 0);

            match backend.status() {
                Err(err) => {
                    let label = gtk::Label::new(Some(&format!("Could not connect (is `bw serve` running?): {err}")));
                    label.set_halign(gtk::Align::Start);
                    label.set_line_wrap(true);
                    self.passwords_list_box.pack_start(&label, false, false, 0);
                }
                Ok(BitwardenStatus::Locked) => {
                    // In-overlay unlock, replacing the old
                    // `show_bitwarden_unlock_prompt` popup window — built
                    // fresh each rebuild, same as every other row in this
                    // list, rather than a persistent `AppState` field (this
                    // is a small, dynamically-shown row, unlike the vault's
                    // own locked/setup sub-group above, which is the whole
                    // overlay's alternate state and so gets named fields).
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    let label = gtk::Label::new(Some("Bitwarden is locked"));
                    label.set_halign(gtk::Align::Start);
                    row.pack_start(&label, false, false, 0);
                    let unlock_entry = gtk::Entry::new();
                    unlock_entry.set_visibility(false);
                    unlock_entry.set_placeholder_text(Some("Bitwarden master password"));
                    unlock_entry.set_hexpand(true);
                    row.pack_start(&unlock_entry, true, true, 0);
                    let unlock_button = gtk::Button::with_label("Unlock");
                    row.pack_start(&unlock_button, false, false, 0);
                    self.passwords_list_box.pack_start(&row, false, false, 0);

                    let try_unlock: Rc<dyn Fn()> = {
                        let app = Rc::clone(self);
                        let unlock_entry = unlock_entry.clone();
                        Rc::new(move || {
                            let Some(backend) = app.bitwarden_backend() else { return };
                            let password = unlock_entry.text().to_string();
                            if password.is_empty() {
                                return;
                            }
                            match backend.unlock(&password) {
                                Ok(()) => app.rebuild_passwords_panel(),
                                Err(err) => {
                                    app.passwords_error_label.set_text(&format!("Couldn't unlock Bitwarden: {err}"));
                                    unlock_entry.set_text("");
                                    unlock_entry.grab_focus();
                                }
                            }
                        })
                    };
                    {
                        let try_unlock = Rc::clone(&try_unlock);
                        unlock_button.connect_clicked(move |_| try_unlock());
                    }
                    {
                        let try_unlock = Rc::clone(&try_unlock);
                        unlock_entry.connect_activate(move |_| try_unlock());
                    }
                }
                Ok(BitwardenStatus::Unlocked) => match backend.list() {
                    Ok(entries) if entries.is_empty() => {
                        let label = gtk::Label::new(Some("No Bitwarden items"));
                        label.set_halign(gtk::Align::Start);
                        self.passwords_list_box.pack_start(&label, false, false, 0);
                    }
                    Ok(entries) => {
                        for entry in &entries {
                            let row = self.build_login_row(entry, LoginSource::Bitwarden, active_domain.as_deref());
                            self.passwords_list_box.pack_start(&row, false, false, 0);
                        }
                    }
                    Err(err) => {
                        let label = gtk::Label::new(Some(&format!("Failed to list Bitwarden items: {err}")));
                        label.set_halign(gtk::Align::Start);
                        label.set_line_wrap(true);
                        self.passwords_list_box.pack_start(&label, false, false, 0);
                    }
                },
            }
        }

        self.passwords_list_box.show_all();
    }

    /// Rebuilds `save_destination_combo`'s entries — "Local vault" always,
    /// plus "Bitwarden" when it's enabled — preserving the current
    /// selection if it's still valid, defaulting to "local" otherwise. Split
    /// out from `rebuild_passwords_panel` since `start_editing_login` also
    /// needs to leave the combo's *contents* alone while just disabling it.
    fn refresh_save_destination_combo(&self) {
        let bitwarden_enabled = self.bitwarden_backend().is_some();
        let previously_selected = self.save_destination_combo.active_id().map(|s| s.to_string());
        self.save_destination_combo.remove_all();
        self.save_destination_combo.append(Some("local"), "Local vault");
        if bitwarden_enabled {
            self.save_destination_combo.append(Some("bitwarden"), "Bitwarden");
        }
        let restore = previously_selected.filter(|id| id == "local" || (id == "bitwarden" && bitwarden_enabled));
        self.save_destination_combo.set_active_id(Some(restore.as_deref().unwrap_or("local")));
    }

    /// Fills the add/edit form from `entry` and switches it into "edit"
    /// mode — reuses the exact same form the add-new-credential flow does,
    /// rather than a second, separate edit form. `submit_login_from_fields`
    /// checks `editing_login` to decide whether to `add` or `update`.
    fn start_editing_login(self: &Rc<Self>, entry: &Login, source: LoginSource) {
        self.new_password_site_entry.set_text(&entry.site);
        self.new_password_username_entry.set_text(&entry.username);
        self.new_password_password_entry.set_text(entry.password.as_deref().unwrap_or(""));
        self.new_password_notes_entry.set_text(&entry.notes);
        *self.editing_login.borrow_mut() = Some((entry.id.clone(), source));
        // An existing login's backend can't change via `update` — there's
        // no "move a login between backends" operation.
        self.save_destination_combo.set_sensitive(false);
        self.submit_password_button.set_label("Save");
        self.passwords_form_heading.set_text("Edit Login");
        self.cancel_edit_button.set_visible(true);
    }

    /// Returns the add/edit form to "add new" mode: clears the fields,
    /// `editing_login`, and undoes everything `start_editing_login` set.
    fn cancel_editing_login(self: &Rc<Self>) {
        self.new_password_site_entry.set_text("");
        self.new_password_username_entry.set_text("");
        self.new_password_password_entry.set_text("");
        self.new_password_notes_entry.set_text("");
        *self.editing_login.borrow_mut() = None;
        self.save_destination_combo.set_sensitive(true);
        self.submit_password_button.set_label("Add");
        self.passwords_form_heading.set_text("Add Login");
        self.cancel_edit_button.set_visible(false);
    }

    /// Sets the add-new-credential form's fields and submits them — test
    /// helper, same pattern as `add_search_engine_via_fields`. Only ever
    /// used with the form in "add new" mode (`editing_login: None`), so
    /// this always creates a fresh login in whichever backend
    /// `save_destination_combo` is currently set to (the local vault, by
    /// default) — unaffected by this pass's edit-mode changes.
    pub fn add_password_via_fields(self: &Rc<Self>, site: &str, username: &str, password: &str, notes: &str) {
        self.new_password_site_entry.set_text(site);
        self.new_password_username_entry.set_text(username);
        self.new_password_password_entry.set_text(password);
        self.new_password_notes_entry.set_text(notes);
        self.submit_login_from_fields();
    }

    /// Test helper: simulates clicking "Edit" on the local vault's row with
    /// this id — looks the entry up itself, mirroring what the real Edit
    /// button's closure already captured at row-build time.
    pub fn start_editing_local_login(self: &Rc<Self>, id: &str) {
        let entry = match &*self.passwords.borrow() {
            VaultState::Unlocked(store) => store.list().unwrap_or_default().into_iter().find(|e| e.id == id),
            VaultState::Locked | VaultState::NotSetUp => None,
        };
        if let Some(entry) = entry {
            self.start_editing_login(&entry, LoginSource::Local);
        }
    }

    /// Test helper: simulates clicking "Fill" on the local vault's row for
    /// `username` — looks the entry up itself, mirroring what the real Fill
    /// button's closure already captured at row-build time. A no-op if no
    /// such entry exists, same as the real button would be if it were
    /// somehow clicked in a stale/mismatched state.
    pub fn fill_active_page_with_local_login(self: &Rc<Self>, username: &str) {
        let entry = match &*self.passwords.borrow() {
            VaultState::Unlocked(store) => store.list().unwrap_or_default().into_iter().find(|e| e.username == username),
            VaultState::Locked | VaultState::NotSetUp => None,
        };
        if let Some(entry) = entry {
            self.fill_active_page_with_login(&entry);
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

    /// Test helper: simulates clicking "Edit" on a Bitwarden row with this
    /// id — same reasoning as `start_editing_local_login`, just looking the
    /// entry up via `bitwarden_backend()` instead of the local vault.
    pub fn start_editing_bitwarden_login(self: &Rc<Self>, id: &str) {
        let entry = self.bitwarden_backend().and_then(|backend| backend.list().ok()?.into_iter().find(|e| e.id == id));
        if let Some(entry) = entry {
            self.start_editing_login(&entry, LoginSource::Bitwarden);
        }
    }

    /// Test helper: simulates clicking the "×" delete button on a Bitwarden
    /// row with this id — `delete_login`/`LoginSource` are private (this
    /// crate's own concern, not part of the public API), so this is the
    /// test-facing entry point for exercising that path.
    pub fn delete_bitwarden_login_for_test(self: &Rc<Self>, id: &str) {
        self.delete_login(id, LoginSource::Bitwarden);
    }

    /// Submits the add/edit form — adds a new login (routed to whichever
    /// backend `save_destination_combo` selects) if `editing_login` is
    /// `None`, or updates the existing one it names otherwise. A no-op if
    /// the site field is blank. On success, clears/resets the form (via
    /// `cancel_editing_login`, reused for that side effect) and rebuilds the
    /// list; on failure, surfaces the error in `passwords_error_label`
    /// instead of just logging it — this overlay had no visible error
    /// surface at all before this.
    pub fn submit_login_from_fields(self: &Rc<Self>) {
        let site = self.new_password_site_entry.text().to_string();
        let site = site.trim().to_string();
        if site.is_empty() {
            return;
        }
        let username = self.new_password_username_entry.text().to_string();
        let password_text = self.new_password_password_entry.text().to_string();
        let notes = self.new_password_notes_entry.text().to_string();
        // A blank password field means "no password" (None), not the empty
        // string as a literal secret — matches how `Login::password` reads
        // back from storage (see `passwords.rs`'s `collect_entries`).
        let password = if password_text.trim().is_empty() { None } else { Some(password_text) };
        let fields = LoginFields { site, username, password, passkey: None, notes };

        let editing = self.editing_login.borrow().clone();
        let result: anyhow::Result<()> = match editing {
            Some((id, LoginSource::Local)) => match &*self.passwords.borrow() {
                VaultState::Unlocked(store) => store.update(&id, fields),
                VaultState::Locked | VaultState::NotSetUp => return,
            },
            Some((id, LoginSource::Bitwarden)) => match self.bitwarden_backend() {
                Some(backend) => backend.update(&id, fields),
                None => return,
            },
            None => match self.save_destination_combo.active_id().as_deref() {
                Some("bitwarden") => match self.bitwarden_backend() {
                    Some(backend) => backend.add(fields).map(|_| ()),
                    None => return,
                },
                _ => match &*self.passwords.borrow() {
                    VaultState::Unlocked(store) => store.add(fields).map(|_| ()),
                    VaultState::Locked | VaultState::NotSetUp => return,
                },
            },
        };

        if let Err(err) = result {
            let action = if self.editing_login.borrow().is_some() { "save" } else { "add" };
            self.passwords_error_label.set_text(&format!("Failed to {action} login: {err}"));
            return;
        }
        self.cancel_editing_login();
        self.rebuild_passwords_panel();
    }

    /// Deletes the login identified by `id` from whichever backend `source`
    /// names, surfacing any failure the same way `submit_login_from_fields`
    /// does rather than just logging it.
    fn delete_login(self: &Rc<Self>, id: &str, source: LoginSource) {
        let result: anyhow::Result<()> = match source {
            LoginSource::Local => match &*self.passwords.borrow() {
                VaultState::Unlocked(store) => store.delete(id),
                VaultState::Locked | VaultState::NotSetUp => Ok(()),
            },
            LoginSource::Bitwarden => match self.bitwarden_backend() {
                Some(backend) => backend.delete(id),
                None => Ok(()),
            },
        };
        if let Err(err) = result {
            self.passwords_error_label.set_text(&format!("Failed to delete login: {err}"));
        }
        self.rebuild_passwords_panel();
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

    /// The local vault's id for the first entry whose username is
    /// `username` — test helper for driving `start_editing_local_login`,
    /// which needs a real id rather than the username
    /// `password_vault_usernames` already exposes.
    pub fn password_vault_id_for_username(&self, username: &str) -> Option<String> {
        match &*self.passwords.borrow() {
            VaultState::Unlocked(store) => store.list().unwrap_or_default().into_iter().find(|e| e.username == username).map(|e| e.id),
            VaultState::Locked | VaultState::NotSetUp => None,
        }
    }

    /// Whether any label anywhere in the password manager overlay's
    /// rendered list currently displays text containing `needle` — test/
    /// inspection helper. Walks the widget tree (rows/section titles/status
    /// messages are nested `Box`/`Label` combinations, not one flat widget
    /// type) rather than reading `Login`/`VaultState` data directly, since
    /// this specifically exists to check what `rebuild_passwords_panel`
    /// actually rendered (e.g. the Bitwarden section), not the underlying
    /// backend state `password_vault_usernames` already covers for the
    /// local vault.
    pub fn passwords_list_contains_text(&self, needle: &str) -> bool {
        fn walk(widget: &gtk::Widget, needle: &str) -> bool {
            if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                if label.text().contains(needle) {
                    return true;
                }
            }
            if let Some(container) = widget.downcast_ref::<gtk::Container>() {
                return container.children().iter().any(|child| walk(child, needle));
            }
            false
        }
        self.passwords_list_box.children().iter().any(|child| walk(child, needle))
    }

    /// Bookmarks a URL directly, without needing to open it as a real page
    /// first — test helper for exercising the bookmark-match path in
    /// `rebuild_switcher_grid` in isolation from the history-match path:
    /// opening a page normally also ends up recording a history visit once
    /// its title loads, which would make a search match via history
    /// instead (already covered by its own existing test).
    pub fn bookmark_url_for_test(&self, url: &str, title: &str) {
        self.bookmarks.borrow_mut().add(url, title, now_unix());
        if let Err(err) = self.bookmarks.borrow().save(&self.profile) {
            eprintln!("failed to save bookmarks: {err}");
        }
    }

    /// Records a history visit directly, without needing to open it as a
    /// real page first — test helper for exercising the similar-history-
    /// match path in `rebuild_switcher_grid` with a controlled, rich-enough
    /// title to test lexical similarity against (the fixture pages' own
    /// titles are single words, not enough vocabulary for a meaningful
    /// similarity comparison).
    pub fn record_history_visit_for_test(&self, url: &str, title: &str) -> anyhow::Result<()> {
        self.history.record_visit(url, title)
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

    /// Routes a tile activation (click or keyboard Enter/Space, both funnel
    /// through here — see `rebuild_switcher_grid` and the flowbox's
    /// `connect_child_activated` handler) through `browser_chrome_core`'s
    /// shared `activate_row`, keyed by the tile's position in
    /// `switcher_rows` (the same snapshot `rebuild_switcher_grid` just
    /// built the grid from). Covers every row kind uniformly, including
    /// history/bookmark/similar tiles — previously those only supported
    /// mouse clicks, since they never got a real `widget_name` at all.
    pub fn activate_switcher_row(self: &Rc<Self>, idx: usize) {
        let start_page = self.settings.borrow().start_page.clone();
        let activation = {
            let rows = self.switcher_rows.borrow();
            browser_chrome_core::activate_row(&rows, idx, &start_page)
        };
        match activation {
            Some(browser_chrome_core::SwitcherActivation::SwitchTo(id)) => self.switch_to(&id),
            Some(browser_chrome_core::SwitcherActivation::OpenNewPage(url)) => {
                if let Err(err) = self.add_page(&url) {
                    eprintln!("failed to open page: {err}");
                }
                self.close_switcher();
            }
            None => {}
        }
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
        // Redundant (already harmless/idempotent) when `was_active` synced
        // via `set_active`/`add_page` above — needed for the non-active
        // "closed a background tab" case, which neither of those cover.
        self.save_session();
    }

    /// Rebuilds every tile from scratch, sourcing the row list itself from
    /// `browser_chrome_core::build_switcher_rows` (open pages matching the
    /// search box's current text, the "+" add row, and — once there's a
    /// query — matching history/bookmark/lexically-similar entries not
    /// already open, all deduped against each other; see that function's
    /// doc comment). This method's job is purely turning each `SwitcherRow`
    /// into a native GTK tile; each tile's `widget_name` is just its index
    /// into `switcher_rows`, which both `connect_child_activated` and
    /// `activate_switcher_row` key off of — one shared activation path for
    /// every row kind, mouse or keyboard.
    ///
    /// No-ops while the switcher panel isn't visible: every page-lifecycle
    /// event that can trigger this (`add_page`, a title/audio-state change,
    /// closing a page, an eviction) calls it unconditionally, but destroying
    /// and recreating every tile is real, non-trivial GTK widget work with
    /// nothing to show for it while the panel is hidden — the common case
    /// during ordinary single-tab browsing and, especially, during startup
    /// (`open_start_page_or_restored_session`, which can call `add_page`
    /// before the window's event loop is even running). Skipping it there
    /// also closes off a real reentrancy risk: a title/audio-state callback
    /// can fire from *inside* `WryEngine::new`'s own post-build event-pump
    /// workaround (see that function's doc comment), i.e. while GTK is
    /// still mid-construction of that very page's widgets — rebuilding the
    /// grid from inside that nested call risked wedging the GTK main thread
    /// (see `set_page_audio_playing`'s doc comment for how that surfaced).
    /// `open_switcher_common` shows the panel *then* calls this directly
    /// (not nested inside another pending operation) so it's always
    /// up to date the moment it becomes visible.
    fn rebuild_switcher_grid(self: &Rc<Self>) {
        if !self.switcher_panel.is_visible() {
            return;
        }
        for child in self.flowbox.children() {
            self.flowbox.remove(&child);
        }

        let query = self.address_bar.text().to_string();
        let rows = browser_chrome_core::build_switcher_rows(
            &self.core.borrow(),
            &self.history,
            Some(&self.bookmarks.borrow()),
            &query,
        );

        for (idx, row) in rows.iter().enumerate() {
            let flow_child = match row {
                browser_chrome_core::SwitcherRow::Open { id, title, domain, color } => {
                    let is_playing_audio = self.core.borrow().page(id).map(|p| p.is_playing_audio).unwrap_or(false);
                    self.build_open_tile(idx, id, title, domain, color, is_playing_audio)
                }
                browser_chrome_core::SwitcherRow::Add => self.build_add_tile(idx),
                browser_chrome_core::SwitcherRow::History { title, domain, .. } => {
                    self.build_search_result_tile(idx, "history-tile", title, domain)
                }
                browser_chrome_core::SwitcherRow::Bookmark { title, domain, .. } => {
                    self.build_search_result_tile(idx, "bookmark-tile", title, domain)
                }
                browser_chrome_core::SwitcherRow::Similar { title, domain, .. } => {
                    self.build_search_result_tile(idx, "similar-tile", title, domain)
                }
                browser_chrome_core::SwitcherRow::Header(label) => self.build_header_tile(label),
            };
            flow_child.set_widget_name(&idx.to_string());
            flow_child.show_all();
            self.flowbox.insert(&flow_child, -1);
        }

        *self.switcher_rows.borrow_mut() = rows;
    }

    /// Builds one open-page tile — real per-page palette color (see the CSS
    /// comment below), a close button overlaid on top (closing a page is
    /// not part of `activate_row`'s model — see `activate_switcher_row`'s
    /// doc comment — so it stays wired directly to `close_page` here), a
    /// speaker icon overlaid in the opposite corner when `is_playing_audio`
    /// is set (see `set_page_audio_playing`), and the tile body itself wired
    /// to `activate_switcher_row(idx)`.
    fn build_open_tile(
        self: &Rc<Self>,
        idx: usize,
        id: &str,
        title: &str,
        domain: &str,
        color: &str,
        is_playing_audio: bool,
    ) -> gtk::FlowBoxChild {
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
                ".page-tile {{ background-image: none; background-color: {color}; \
                  border: none; box-shadow: none; border-radius: 10px; color: #fff; }}"
            )
            .as_bytes(),
        );
        tile.style_context()
            .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        if domain.ends_with("unloaded") {
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
        let title_label = gtk::Label::new(Some(title));
        title_label.set_halign(gtk::Align::Start);
        title_label.style_context().add_class("tile-title");
        let domain_label = gtk::Label::new(Some(domain));
        domain_label.set_halign(gtk::Align::Start);
        domain_label.style_context().add_class("tile-subtitle");
        inner.pack_start(&title_label, false, false, 0);
        inner.pack_start(&domain_label, false, false, 0);
        tile.add(&inner);

        let app_clone = Rc::clone(self);
        tile.connect_clicked(move |_| app_clone.activate_switcher_row(idx));

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
        let id_clone = id.to_string();
        close_btn.connect_clicked(move |_| {
            app_clone.close_page(&id_clone);
        });

        let audio_icon = gtk::Image::from_icon_name(Some("audio-volume-high-symbolic"), gtk::IconSize::Button);
        audio_icon.style_context().add_class("tile-audio-icon");
        audio_icon.set_halign(gtk::Align::Start);
        audio_icon.set_valign(gtk::Align::Start);
        audio_icon.set_margin_top(10);
        audio_icon.set_margin_start(10);
        audio_icon.set_visible(is_playing_audio);
        audio_icon.set_no_show_all(true);

        let tile_overlay = gtk::Overlay::new();
        tile_overlay.add(&tile);
        tile_overlay.add_overlay(&close_btn);
        tile_overlay.add_overlay(&audio_icon);

        let flow_child = gtk::FlowBoxChild::new();
        flow_child.add(&tile_overlay);
        flow_child
    }

    /// Builds a non-interactive section-heading row ("Open Pages",
    /// "History", "Bookmarks", "Similar" — see `SwitcherRow::Header`). Uses
    /// the same `FlowBoxChild` wrapper as every real tile so it still takes
    /// up a grid cell of its own (which is what forces a line break ahead
    /// of it, reading as a section divider), but with `can_focus` off so
    /// the flowbox's native arrow-key navigation skips over it — the gtk3
    /// equivalent of `next_activatable_row` skipping headers on the other
    /// front ends — and no click handler at all (`activate_switcher_row`
    /// already no-ops for header rows via `browser_chrome_core::activate_row`,
    /// but there's no `connect_clicked` here to trigger it in the first
    /// place). `FlowBox` has no notion of a child spanning the full row
    /// width, so this reads as a small labeled divider rather than a true
    /// full-width heading — the closest available with this widget.
    fn build_header_tile(&self, label: &str) -> gtk::FlowBoxChild {
        let heading = gtk::Label::new(Some(label));
        heading.style_context().add_class("switcher-heading");
        heading.set_halign(gtk::Align::Start);
        heading.set_margin_top(6);
        heading.set_margin_bottom(2);

        let flow_child = gtk::FlowBoxChild::new();
        flow_child.add(&heading);
        flow_child.set_can_focus(false);
        flow_child
    }

    fn build_add_tile(self: &Rc<Self>, idx: usize) -> gtk::FlowBoxChild {
        let add_tile = gtk::Button::new();
        add_tile.style_context().add_class("add-tile");
        add_tile.set_size_request(150, 110);
        add_tile.set_can_focus(false);
        let add_tile_label = gtk::Label::new(Some("+"));
        add_tile_label.style_context().add_class("add-tile-label");
        add_tile.add(&add_tile_label);
        let app_clone = Rc::clone(self);
        add_tile.connect_clicked(move |_| app_clone.activate_switcher_row(idx));

        let add_child = gtk::FlowBoxChild::new();
        add_child.add(&add_tile);
        add_child
    }

    /// Builds one switcher-grid tile for a history, bookmark, or
    /// lexically-similar search result — same shape as an open-page tile
    /// but without a close button, tagged with `extra_css_class`
    /// (`"history-tile"`/`"bookmark-tile"`/`"similar-tile"`) so each source
    /// reads as visually distinct. Wired to `activate_switcher_row(idx)`,
    /// same as every other tile kind.
    fn build_search_result_tile(self: &Rc<Self>, idx: usize, extra_css_class: &str, title_text: &str, domain: &str) -> gtk::FlowBoxChild {
        let tile = gtk::Button::new();
        tile.style_context().add_class("page-tile");
        tile.style_context().add_class(extra_css_class);
        tile.set_size_request(150, 110);
        tile.set_can_focus(false);

        let inner = gtk::Box::new(gtk::Orientation::Vertical, 2);
        inner.set_margin(10);
        inner.set_valign(gtk::Align::End);
        let title_label = gtk::Label::new(Some(title_text));
        title_label.set_halign(gtk::Align::Start);
        title_label.style_context().add_class("tile-title");
        let domain_label = gtk::Label::new(Some(domain));
        domain_label.set_halign(gtk::Align::Start);
        domain_label.style_context().add_class("tile-subtitle");
        inner.pack_start(&title_label, false, false, 0);
        inner.pack_start(&domain_label, false, false, 0);
        tile.add(&inner);

        let app_clone = Rc::clone(self);
        tile.connect_clicked(move |_| app_clone.activate_switcher_row(idx));

        let flow_child = gtk::FlowBoxChild::new();
        flow_child.add(&tile);
        flow_child
    }

    /// Page ids in creation order — test/inspection helper.
    pub fn page_ids(&self) -> Vec<String> {
        self.core.borrow().page_ids()
    }

    /// Number of tiles currently shown in the switcher grid (open pages +
    /// the "+" add-tile + any history/bookmark search-result tiles) — test/
    /// inspection helper.
    pub fn switcher_grid_tile_count(&self) -> usize {
        self.flowbox.children().len()
    }

    /// Whether any tile currently in the switcher grid carries
    /// `css_class` (e.g. `"bookmark-tile"`/`"history-tile"`) — test/
    /// inspection helper for confirming a specific *kind* of tile is
    /// present, since the aggregate tile count alone can't distinguish
    /// "a bookmark tile appeared" from "an open-page tile that used to
    /// match the empty query no longer matches this one and dropped out".
    pub fn switcher_grid_has_tile_with_class(&self, css_class: &str) -> bool {
        self.flowbox.children().iter().any(|child| {
            child
                .downcast_ref::<gtk::FlowBoxChild>()
                .and_then(|fbc| fbc.child())
                .map(|widget| widget.style_context().has_class(css_class))
                .unwrap_or(false)
        })
    }

    /// Whether keyboard focus is currently on one of the switcher grid's
    /// tiles — test/inspection helper for confirming `focus_switcher_grid`
    /// (Down arrow in the address bar) actually landed somewhere, not just
    /// that it didn't crash. `FlowBox`'s Browse selection mode keeps exactly
    /// the keyboard-focused child selected, so this is the same signal the
    /// Delete-key-closes-a-tile handler already reads.
    pub fn switcher_grid_has_focused_tile(&self) -> bool {
        !self.flowbox.selected_children().is_empty()
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

    /// Selects the settings overlay's "Light" theme radio button — test
    /// helper for driving a theme change before calling `save_settings`.
    pub fn select_light_theme_radio(&self) {
        self.light_theme_radio.set_active(true);
    }

    /// Sets the settings overlay's Bitwarden checkbox/URL fields — test
    /// helper for driving a Bitwarden-enable before calling `save_settings`,
    /// same pattern as `select_light_theme_radio`.
    pub fn set_bitwarden_fields(&self, enabled: bool, url: &str) {
        self.bitwarden_check.set_active(enabled);
        self.bitwarden_url_entry.set_text(url);
    }

    /// The theme-provider's currently loaded CSS — test helper for
    /// confirming `apply_theme` actually reloaded it with the right
    /// theme's rules, not just that `Settings::theme` changed.
    pub fn theme_provider_css(&self) -> String {
        self.theme_provider.to_str().to_string()
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
        // Theme-invariant rules: the switcher grid's tiles and hints always
        // sit over the scrim (a dark, translucent dimmer over the page
        // behind — see `scrim_css` below), which stays the same dark tone
        // regardless of the app's own light/dark theme, the same convention
        // most apps' modal dimmers use. The settings/profile/keybindings/
        // bookmarks/passwords overlay boxes used to have a real background
        // of their own (hence theme-dependent colors, in `theme_provider`/
        // `theme_css`) — they no longer do (`.settings-box` sits directly on
        // the scrim now, same as the switcher grid), so their text/button
        // rules moved here too: only the history/bookmark/similar
        // search-result tiles (which *do* still have their own background)
        // remain theme-dependent.
        let base_provider = gtk::CssProvider::new();
        // Each switcher tile is a `gtk::FlowBoxChild` wrapping a `gtk::Overlay`
        // of stacked buttons (the tile itself, `.tile-close-btn`, the audio
        // icon) — Adwaita's default `flowboxchild:hover` draws its own
        // rectangular prelight box around that whole wrapper, so hovering
        // *any* part of a tile (including the close icon) lit up the entire
        // card as one unit, and crossing-event delivery between the stacked
        // sibling buttons made that outer highlight flicker or stick after
        // the pointer left. Zeroing `flowboxchild:hover` here removes that
        // container-level visual entirely; each interactive piece then only
        // shows its own native, single-widget `:hover` (a `filter` brightness
        // bump, since the tiles' base colors are per-domain and per-theme —
        // see `build_open_tile`/`theme_css` — so a single flat hover color
        // can't work for all of them).
        let _ = base_provider.load_from_data(
            b"flowboxchild:hover { background-image: none; background-color: transparent; \
                box-shadow: none; border-color: transparent; outline: none; } \
              .page-tile:hover, .add-tile:hover, .history-tile:hover, .bookmark-tile:hover, \
                .similar-tile:hover, .tile-close-btn:hover, .overlay-close-btn:hover { \
                filter: brightness(1.18); } \
              .tile-title { color: #ffffff; font-weight: 600; } \
              .tile-subtitle { color: rgba(255, 255, 255, 0.75); } \
              .add-tile-label { color: #ffffff; font-size: 20px; } \
              .add-tile { background-image: none; background-color: rgba(255, 255, 255, 0.15); \
                border: none; box-shadow: none; border-radius: 10px; } \
              .tile-close-btn { background-image: none; background-color: rgba(0, 0, 0, 0.45); \
                border: none; box-shadow: none; border-radius: 9999px; padding: 0; \
                min-width: 0; min-height: 0; } \
              .tile-close-label { color: #ffffff; } \
              .overlay-close-btn { background-image: none; background-color: rgba(0, 0, 0, 0.45); \
                border: none; box-shadow: none; border-radius: 9999px; padding: 0; \
                min-width: 0; min-height: 0; } \
              .overlay-close-label { color: #ffffff; font-size: 14px; } \
              .overlay-esc-hint { color: rgba(255, 255, 255, 0.6); font-size: 12px; } \
              .tile-audio-icon { color: #ffffff; } \
              .title-chip { background-image: none; background-color: alpha(@theme_fg_color, 0.06); \
                border: 1px solid alpha(@theme_fg_color, 0.22); border-radius: 6px; box-shadow: none; \
                padding: 4px 12px; } \
              .title-chip:hover { background-color: @theme_base_color; border-color: @theme_selected_bg_color; } \
              .switcher-hint { color: rgba(255, 255, 255, 0.6); font-size: 12px; } \
              .switcher-profile-label { color: rgba(255, 255, 255, 0.6); font-size: 12px; } \
              .page-tile-unloaded { opacity: 0.5; } \
              .switcher-heading { color: rgba(255, 255, 255, 0.7); font-weight: 700; font-size: 12px; \
                letter-spacing: 0.04em; border-bottom: 1px solid rgba(255, 255, 255, 0.18); \
                padding-bottom: 4px; min-width: 150px; } \
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
    passwords_button.set_tooltip_text(Some("Password manager"));
    for button in [
        &back_button,
        &forward_button,
        &reload_button,
        &switcher_toggle,
        &settings_button,
        &profile_button,
        &bookmark_toggle_button,
        &bookmarks_button,
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
    header_bar.pack_end(&settings_button);
    header_bar.pack_end(&profile_button);
    header_bar.pack_end(&bookmarks_button);
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
    grid_content.pack_start(&address_bar, false, false, 0);
    grid_content.pack_start(&flowbox, true, true, 0);
    grid_content.pack_start(&keynav_hint, false, false, 0);

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

    // --- Settings overlay: an in-window overlay (like the switcher grid
    // above), not a modal `gtk::Dialog` — see `AppState::settings_panel`'s
    // doc comment for why. Scoped to picking the default search engine from
    // the existing seeded list, not adding/editing entries — that's a
    // fuller list-editor UI, left for later.
    let settings_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    settings_box.set_halign(gtk::Align::Fill);
    settings_box.set_valign(gtk::Align::Start);
    settings_box.style_context().add_class("settings-box");
    settings_box.set_margin(24);

    let settings_title = gtk::Label::new(Some("Settings"));
    settings_title.style_context().add_class("settings-title");
    settings_title.set_halign(gtk::Align::Start);
    settings_box.pack_start(&settings_title, false, false, 0);

    // Four tabs (General, Search Engines, Password Managers, Keybindings)
    // via a plain `gtk::Stack`/`gtk::StackSwitcher` pair — reuses the exact
    // widget classes already in this dependency (no new crate), distinct
    // from `self.stack` (the *page* stack, an unrelated concept). Each
    // tab's own heading label (redundant with the switcher's own tab
    // title) is dropped.
    let settings_stack = gtk::Stack::new();
    settings_stack.set_vexpand(true);
    let settings_stack_switcher = gtk::StackSwitcher::new();
    settings_stack_switcher.set_stack(Some(&settings_stack));
    // `StackSwitcher` applies GTK's own "linked" style by default (a fused,
    // segmented-control look — square-jointed buttons sharing edges).
    // Removed so each tab renders as its own separate, individually
    // rounded card instead, matching the switcher grid's tiles (see the
    // `.settings-box stackswitcher > button` CSS in `build_window_and_app`)
    // — the look this was actually asked to match.
    settings_stack_switcher.style_context().remove_class("linked");
    settings_box.pack_start(&settings_stack_switcher, false, false, 0);
    settings_box.pack_start(&settings_stack, true, true, 0);

    // ---- General tab ----
    let general_page = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let start_page_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    start_page_row.pack_start(&gtk::Label::new(Some("Start page")), false, false, 0);
    let start_page_entry = gtk::Entry::new();
    start_page_entry.set_hexpand(true);
    start_page_row.pack_start(&start_page_entry, true, true, 0);
    general_page.pack_start(&start_page_row, false, false, 0);

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
    general_page.pack_start(&limit_row, false, false, 0);

    let theme_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    theme_row.pack_start(&gtk::Label::new(Some("Theme")), false, false, 0);
    let dark_theme_radio = gtk::RadioButton::with_label("Dark");
    let light_theme_radio = gtk::RadioButton::with_label_from_widget(&dark_theme_radio, "Light");
    theme_row.pack_start(&dark_theme_radio, false, false, 0);
    theme_row.pack_start(&light_theme_radio, false, false, 0);
    general_page.pack_start(&theme_row, false, false, 0);

    settings_stack.add_titled(&general_page, "general", "General");

    // ---- Search Engines tab ----
    let search_engines_page = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let engine_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    engine_row.pack_start(&gtk::Label::new(Some("Search engine")), false, false, 0);
    // Populated for real (from the live per-profile Settings, not this
    // hardcoded default) by `refresh_engine_combo`, called from
    // `open_settings` every time it opens — left empty here since nothing
    // shows until the overlay is opened anyway.
    let engine_combo = gtk::ComboBoxText::new();
    engine_row.pack_start(&engine_combo, true, true, 0);
    search_engines_page.pack_start(&engine_row, false, false, 0);

    // Search engine management: add/remove entries from Settings::search_engines.
    // Unlike the fields above (staged until Save), these take effect and save
    // immediately on each add/remove — the same immediate-save convention this
    // session's bookmarks/keybindings editors already use, rather than adding a
    // separate staged/cancel-able list-editing model just for this section.
    let engines_list_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    search_engines_page.pack_start(&engines_list_box, false, false, 0);

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
    search_engines_page.pack_start(&new_engine_row, false, false, 0);

    settings_stack.add_titled(&search_engines_page, "search-engines", "Search Engines");

    // ---- Password Managers tab ----
    // Just Bitwarden for now — a generic name rather than a per-backend one
    // since other backends are a real, if not-yet-built, possibility (see
    // ROADMAP.md's Backlog: KeePassXC/secret-service, 1Password), each of
    // which would land as its own subsection here, "Bitwarden" keeping its
    // subtitle to distinguish it from whichever else eventually joins it.
    let password_managers_page = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let bitwarden_subtitle = gtk::Label::new(Some("Bitwarden"));
    bitwarden_subtitle.style_context().add_class("settings-subtitle");
    bitwarden_subtitle.set_halign(gtk::Align::Start);
    password_managers_page.pack_start(&bitwarden_subtitle, false, false, 0);

    // A checkbox plus its server URL, always shown side by side (unlike the
    // loaded-pages limit's conditionally-disabled spin button) since
    // there's only one field to fill in.
    let bitwarden_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let bitwarden_check = gtk::CheckButton::new();
    bitwarden_check.set_label("Enable Bitwarden (via bw serve)");
    let bitwarden_url_entry = gtk::Entry::new();
    bitwarden_url_entry.set_placeholder_text(Some("http://127.0.0.1:8087"));
    bitwarden_url_entry.set_hexpand(true);
    bitwarden_row.pack_start(&bitwarden_check, false, false, 0);
    bitwarden_row.pack_start(&bitwarden_url_entry, true, true, 0);
    password_managers_page.pack_start(&bitwarden_row, false, false, 0);

    settings_stack.add_titled(&password_managers_page, "password-managers", "Password Managers");

    // ---- Keybindings tab ----
    // One row per `Action::ALL`, rebuilt from the current `Keybindings` each
    // time settings opens and after every add/remove. `vexpand` on the
    // `ScrolledWindow` rather than a fixed `max_content_height` — this tab
    // now has the overlay's full height to itself, not a shared column
    // alongside every other setting.
    let keybindings_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let keybindings_list_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let keybindings_scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    keybindings_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    keybindings_scroll.set_vexpand(true);
    keybindings_scroll.add(&keybindings_list_box);
    keybindings_page.pack_start(&keybindings_scroll, true, true, 0);

    settings_stack.add_titled(&keybindings_page, "keybindings", "Keybindings");

    let settings_buttons_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    settings_buttons_row.set_halign(gtk::Align::End);
    let settings_cancel_button = gtk::Button::with_label("Cancel");
    let settings_save_button = gtk::Button::with_label("Save");
    settings_buttons_row.pack_start(&settings_cancel_button, false, false, 0);
    settings_buttons_row.pack_start(&settings_save_button, false, false, 0);
    settings_box.pack_start(&settings_buttons_row, false, false, 0);

    let (settings_overlay, settings_scrim, settings_close_button) = build_overlay_chrome(&settings_box, &scrim_css);

    // --- Profile picker overlay: same in-window-overlay pattern again.
    // Lists existing profiles (from `list_profile_names()`, rebuilt each
    // time it opens) plus a field to create a new one — picking any profile
    // other than the current one launches a new, independent process
    // scoped to it (`launch_new_profile_process`) rather than switching this
    // window in place.
    let profile_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    profile_box.set_halign(gtk::Align::Fill);
    profile_box.set_valign(gtk::Align::Start);
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

    let new_profile_encrypted_check = gtk::CheckButton::new();
    new_profile_encrypted_check.set_label("Encrypt with a passphrase");
    profile_box.pack_start(&new_profile_encrypted_check, false, false, 0);

    let profile_buttons_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    profile_buttons_row.set_halign(gtk::Align::End);
    let new_private_window_button = gtk::Button::with_label("New Private Window");
    let profile_cancel_button = gtk::Button::with_label("Cancel");
    profile_buttons_row.pack_start(&new_private_window_button, false, false, 0);
    profile_buttons_row.pack_start(&profile_cancel_button, false, false, 0);
    profile_box.pack_start(&profile_buttons_row, false, false, 0);

    let (profile_overlay, profile_scrim, profile_close_button) = build_overlay_chrome(&profile_box, &scrim_css);

    // --- Bookmarks overlay: same shape again. One row per bookmark, rebuilt
    // from `Bookmarks::all()` each time it opens and after every add/remove.
    let bookmarks_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    bookmarks_box.set_halign(gtk::Align::Fill);
    bookmarks_box.set_valign(gtk::Align::Start);
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

    let (bookmarks_overlay, bookmarks_scrim, bookmarks_close_icon) = build_overlay_chrome(&bookmarks_box, &scrim_css);

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

    // ---- vault contents sub-group ----
    // Shown instead of `passwords_unlock_box` once the vault is unlocked.
    let passwords_content_box = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let saved_logins_subtitle = gtk::Label::new(Some("Saved Logins"));
    saved_logins_subtitle.style_context().add_class("settings-subtitle");
    saved_logins_subtitle.set_halign(gtk::Align::Start);
    passwords_content_box.pack_start(&saved_logins_subtitle, false, false, 0);

    let passwords_list_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    passwords_content_box.pack_start(&passwords_list_box, false, false, 0);

    // Text toggles "Add Login"/"Edit Login" alongside
    // `submit_password_button`'s label (see `start_editing_login`/
    // `cancel_editing_login`).
    let passwords_form_heading = gtk::Label::new(Some("Add Login"));
    passwords_form_heading.style_context().add_class("settings-subtitle");
    passwords_form_heading.set_halign(gtk::Align::Start);
    passwords_content_box.pack_start(&passwords_form_heading, false, false, 0);

    let new_password_site_entry = gtk::Entry::new();
    new_password_site_entry.set_placeholder_text(Some("Site (e.g. https://example.com)"));
    passwords_content_box.pack_start(&new_password_site_entry, false, false, 0);
    let new_password_username_entry = gtk::Entry::new();
    new_password_username_entry.set_placeholder_text(Some("Username"));
    passwords_content_box.pack_start(&new_password_username_entry, false, false, 0);
    let new_password_password_entry = gtk::Entry::new();
    new_password_password_entry.set_visibility(false);
    new_password_password_entry.set_placeholder_text(Some("Password"));
    passwords_content_box.pack_start(&new_password_password_entry, false, false, 0);
    let new_password_notes_entry = gtk::Entry::new();
    new_password_notes_entry.set_placeholder_text(Some("Notes (optional)"));
    passwords_content_box.pack_start(&new_password_notes_entry, false, false, 0);

    // Hidden entirely unless Bitwarden integration is enabled (see
    // `rebuild_passwords_panel`, which rebuilds this combo's contents and
    // this row's visibility every time it runs) — a brand-new login always
    // goes to the local vault when this isn't shown, same as before this
    // field existed.
    let save_destination_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    save_destination_row.pack_start(&gtk::Label::new(Some("Save to")), false, false, 0);
    let save_destination_combo = gtk::ComboBoxText::new();
    save_destination_row.pack_start(&save_destination_combo, true, true, 0);
    passwords_content_box.pack_start(&save_destination_row, false, false, 0);

    let passwords_error_label = gtk::Label::new(None);
    passwords_error_label.set_halign(gtk::Align::Start);
    passwords_error_label.set_line_wrap(true);
    passwords_content_box.pack_start(&passwords_error_label, false, false, 0);

    let password_form_button_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let cancel_edit_button = gtk::Button::with_label("Cancel edit");
    cancel_edit_button.set_visible(false);
    let submit_password_button = gtk::Button::with_label("Add");
    password_form_button_row.pack_start(&cancel_edit_button, false, false, 0);
    password_form_button_row.pack_start(&submit_password_button, false, false, 0);
    passwords_content_box.pack_start(&password_form_button_row, false, false, 0);

    passwords_box.pack_start(&passwords_content_box, false, false, 0);

    // Close stays outside both sub-groups above — dismissing the overlay
    // shouldn't depend on the vault's unlock state.
    let passwords_close_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    passwords_close_row.set_halign(gtk::Align::End);
    let passwords_close_button = gtk::Button::with_label("Close");
    passwords_close_row.pack_start(&passwords_close_button, false, false, 0);
    passwords_box.pack_start(&passwords_close_row, false, false, 0);

    let (passwords_overlay, passwords_scrim, passwords_close_icon) = build_overlay_chrome(&passwords_box, &scrim_css);

    let root_overlay = gtk::Overlay::new();
    root_overlay.add(&stack);
    root_overlay.add_overlay(&switcher_overlay);
    root_overlay.add_overlay(&settings_overlay);
    root_overlay.add_overlay(&profile_overlay);
    root_overlay.add_overlay(&bookmarks_overlay);
    root_overlay.add_overlay(&passwords_overlay);

    window.add(&root_overlay);
    window.show_all();
    switcher_overlay.hide();
    settings_overlay.hide();
    profile_overlay.hide();
    bookmarks_overlay.hide();
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
        flowbox: flowbox.clone(),
        settings_panel: settings_overlay.clone().upcast::<gtk::Widget>(),
        start_page_entry: start_page_entry.clone(),
        engine_combo: engine_combo.clone(),
        engines_list_box: engines_list_box.clone(),
        new_engine_name_entry: new_engine_name_entry.clone(),
        new_engine_url_entry: new_engine_url_entry.clone(),
        unlimited_check: unlimited_check.clone(),
        limit_spin: limit_spin.clone(),
        light_theme_radio: light_theme_radio.clone(),
        dark_theme_radio: dark_theme_radio.clone(),
        bitwarden_check: bitwarden_check.clone(),
        bitwarden_url_entry: bitwarden_url_entry.clone(),
        theme_provider: theme_provider.clone(),
        profile_panel: profile_overlay.clone().upcast::<gtk::Widget>(),
        profile_list_box: profile_list_box.clone(),
        new_profile_entry: new_profile_entry.clone(),
        new_profile_encrypted_check: new_profile_encrypted_check.clone(),
        keybindings_list_box: keybindings_list_box.clone(),
        keybindings: RefCell::new(Keybindings::load(&profile)),
        listening_for: Cell::new(None),
        bookmarks_panel: bookmarks_overlay.clone().upcast::<gtk::Widget>(),
        bookmarks_list_box: bookmarks_list_box.clone(),
        bookmark_toggle_button: bookmark_toggle_button.clone(),
        bookmarks: RefCell::new(bookmarks),
        passwords_panel: passwords_overlay.clone().upcast::<gtk::Widget>(),
        passwords_unlock_box: passwords_unlock_box.clone(),
        passwords_unlock_heading: passwords_unlock_heading.clone(),
        passwords_unlock_label: passwords_unlock_label.clone(),
        passwords_unlock_entry: passwords_unlock_entry.clone(),
        passwords_unlock_error_label: passwords_unlock_error_label.clone(),
        passwords_unlock_button: passwords_unlock_button.clone(),
        passwords_content_box: passwords_content_box.clone(),
        passwords_list_box: passwords_list_box.clone(),
        passwords_form_heading: passwords_form_heading.clone(),
        passwords: RefCell::new(initial_vault_state),
        session_passphrase: RefCell::new(None),
        new_password_site_entry: new_password_site_entry.clone(),
        new_password_username_entry: new_password_username_entry.clone(),
        new_password_password_entry: new_password_password_entry.clone(),
        new_password_notes_entry: new_password_notes_entry.clone(),
        editing_login: RefCell::new(None),
        save_destination_combo: save_destination_combo.clone(),
        save_destination_row: save_destination_row.clone(),
        submit_password_button: submit_password_button.clone(),
        cancel_edit_button: cancel_edit_button.clone(),
        passwords_error_label: passwords_error_label.clone(),
        switcher_rows: RefCell::new(Vec::new()),
        core: RefCell::new(core),
        web_context: RefCell::new(web_context),
        containers: RefCell::new(HashMap::new()),
        console_messages: RefCell::new(Vec::new()),
        settings: RefCell::new(settings),
        history,
        profile,
        restoring: Cell::new(false),
    });
    app.apply_theme();

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
        // actually held). Down arrow moves keyboard focus into the tile grid
        // (`FlowBox` already supports arrow-key navigation among tiles once
        // focus is inside it — see the Delete-key handler below, which reads
        // back `flowbox.selected_children()`), so keyboard-only users can
        // reach the grid without a mouse.
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
        // Fires when a FlowBoxChild is activated — Enter/Space while it has
        // keyboard focus (tile/close buttons are can_focus(false), so focus
        // always lands on the FlowBoxChild itself, never its contents).
        let app = Rc::clone(&app);
        flowbox.connect_child_activated(move |_, child| {
            if let Ok(idx) = child.widget_name().parse::<usize>() {
                app.activate_switcher_row(idx);
            }
        });
    }
    {
        // Delete closes whichever tile is currently highlighted by keyboard
        // navigation (Browse selection mode keeps exactly one selected) —
        // only meaningful for open-page tiles; the add/history/bookmark/
        // similar rows have nothing to close, same as `activate_row`
        // deliberately having no close-page concept.
        let app = Rc::clone(&app);
        flowbox.connect_key_press_event(move |flowbox, event| {
            let is_delete = event.keyval() == gtk::gdk::keys::Key::from_name("Delete");
            if !is_delete {
                return gtk::glib::Propagation::Proceed;
            }
            if let Some(child) = flowbox.selected_children().into_iter().next() {
                if let Ok(idx) = child.widget_name().parse::<usize>() {
                    let open_id = match app.switcher_rows.borrow().get(idx) {
                        Some(browser_chrome_core::SwitcherRow::Open { id, .. }) => Some(id.clone()),
                        _ => None,
                    };
                    if let Some(id) = open_id {
                        app.close_page(&id);
                    }
                }
            }
            gtk::glib::Propagation::Stop
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
        settings_button.connect_clicked(move |_| {
            app.toggle_settings();
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
        settings_close_button.connect_clicked(move |_| {
            app.close_settings();
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
            app.toggle_profile_picker();
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
        profile_close_button.connect_clicked(move |_| {
            app.close_profile_picker();
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
            app.toggle_bookmarks();
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
        bookmarks_close_icon.connect_clicked(move |_| {
            app.close_bookmarks();
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
        submit_password_button.connect_clicked(move |_| {
            app.submit_login_from_fields();
        });
    }
    {
        let app = Rc::clone(&app);
        new_password_password_entry.connect_activate(move |_| {
            app.submit_login_from_fields();
        });
    }
    {
        let app = Rc::clone(&app);
        cancel_edit_button.connect_clicked(move |_| {
            app.cancel_editing_login();
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
            } else if is_escape && app.is_settings_open() {
                app.close_settings();
                return gtk::glib::Propagation::Stop;
            } else if is_escape && app.is_profile_picker_open() {
                app.close_profile_picker();
                return gtk::glib::Propagation::Stop;
            } else if is_escape && app.is_bookmarks_open() {
                app.close_bookmarks();
                return gtk::glib::Propagation::Stop;
            } else if is_escape && app.is_passwords_open() {
                app.close_passwords();
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

