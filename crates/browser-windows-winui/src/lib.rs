//! WinUI 3 (Microsoft.UI.Xaml) native chrome — one of two active Windows
//! front ends (alongside `browser-windows-reactor`; see that crate's module
//! doc comment for why both exist). `browser-windows-win32`/
//! `browser-windows-nwg`/`browser-wx` were deleted from the repo (see
//! `ROADMAP.md`/`ARCHITECTURE.md`) — unmaintained and behind these two in
//! scope. Built out to match `browser-linux-gtk3`'s feature set (multi-page
//! browsing, a switcher grid, settings, keyboard shortcuts, profile
//! support), using the native `Microsoft.UI.Xaml.Controls.WebView2` control
//! (`render_engine::WebView2Engine`) rather than wry — see that module's doc
//! comment for why.
//!
//! Gated on `target_env = "msvc"`, not just `target_os = "windows"` — see
//! `Cargo.toml`'s comment; WinRT/COM interop needs MSVC specifically, so
//! this crate compiles to an empty no-op everywhere else.
//!
//! # A real gap in `winio-winui3`'s bindings: no working `KeyDown` or
//! `Window::Closed`
//!
//! Checked systematically (`grep -rn "pub fn new<$"` across the whole crate
//! to find every constructible delegate): only `RoutedEventHandler`,
//! `TextChangedEventHandler`, `SelectionChangedEventHandler`, and a handful of
//! others are actually implemented. `Window`'s `Closed`/`Activated`/
//! `SizeChanged`/`VisibilityChanged` and `UIElement`'s `KeyDown`/`KeyUp` all
//! need dedicated delegate types (`WindowEventHandler`, `KeyEventHandler`,
//! etc.) that don't exist anywhere in the crate — only their `Remove*`
//! accessors exist, with no way to ever `add` a handler in the first place.
//! `KeyboardAccelerator` isn't in this crate's subset either. Button `Click`,
//! `CheckBox` `Checked`/`Unchecked`, `ComboBox`/`TextBox`
//! `SelectionChanged`/`TextChanged`, and `GotFocus`/`LostFocus` all *do* work.
//!
//! Fix: subclass the top-level `HWND` directly (`SetWindowSubclass`), a
//! standard Win32 technique. See `install_hwnd_subclass` below.
#![cfg(all(target_os = "windows", target_env = "msvc"))]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use browser_core::{
    list_profile_names, resolve_address_input, Action, HistoryStore, KeyChord, Keybindings, PageManager, Profile, Session,
    SessionPage, Settings, APP_NAME,
};
use render_engine::{AssertSend, RenderEngine, WebView2Engine};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_F1, VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{WM_DESTROY, WM_KEYDOWN, WM_NCDESTROY};
use windows_core::HSTRING;
use winui3::Microsoft::UI::Xaml::Controls::{
    Button, CheckBox, ColumnDefinition, ComboBox, Grid, Orientation, RowDefinition, StackPanel, TextBlock, TextBox,
};
use winui3::Microsoft::UI::Xaml::{GridLength, GridUnitType, HorizontalAlignment, Thickness, VerticalAlignment, Visibility, Window};
use winui3::IWindowNative;

const SUBCLASS_ID: usize = 0x8b40_5757;
const CHOOSER_SUBCLASS_ID: usize = 0x8b40_5758;
const TILE_WIDTH: i32 = 150;
const TILE_HEIGHT: i32 = 110;
const TILE_COLUMNS: i32 = 5;

/// Checkpoint tracing for diagnosing this crate's first-ever real launch on
/// Windows (see `summaries/windows-github-actions-ci.md`) — writes straight
/// to a file with an explicit `sync_all()` after every line, since a
/// GitHub Actions run redirecting the process's own stdout/stderr came back
/// completely empty even on a crash, consistent with a termination abrupt
/// enough to skip normal stream flushing. Kept permanently (not stripped out
/// once the current bug is found) per explicit direction — cheap, and
/// useful again if `browser-windows-winui` ever regresses on real Windows.
pub fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("winui-trace.log") {
        let _ = writeln!(f, "{msg}");
        let _ = f.sync_all();
    }
}

pub struct AppState {
    window: Window,
    address_bar: TextBox,
    address_bar_focused: Cell<bool>,
    content_grid: Grid,
    switcher_overlay: Grid,
    search_box: TextBox,
    search_box_focused: Cell<bool>,
    tile_grid: Grid,
    settings_overlay: Grid,
    start_page_box: TextBox,
    engine_combo: ComboBox,
    unlimited_check: CheckBox,
    limit_box: TextBox,
    profile_overlay: Grid,
    /// Rebuilt from `browser_core::list_profile_names()` each time the
    /// picker opens — holds one row per existing profile.
    profile_list_panel: StackPanel,
    new_profile_box: TextBox,
    new_profile_box_focused: Cell<bool>,
    core: RefCell<PageManager<WebView2Engine>>,
    /// Per-page container, keyed by page id — `browser_core::Page` doesn't
    /// hold these since they're a WinUI3-only concept. Each is a plain
    /// `Grid` (a concrete panel type, unlike the possibly-abstract base
    /// `Panel`) hosting exactly one `WebView2Engine`, `Visibility` toggled to
    /// show only the active page — this crate's equivalent of gtk3's `Stack`
    /// (nothing called "Stack" exists in this bindings subset).
    containers: RefCell<HashMap<String, Grid>>,
    settings: RefCell<Settings>,
    history: HistoryStore,
    keybindings_overlay: Grid,
    /// Rebuilt from the current `Keybindings` each time the editor opens
    /// (and after every add/remove) — holds one row per `Action`.
    keybindings_list_panel: StackPanel,
    keybindings: RefCell<Keybindings>,
    /// `Some(action)` while the editor is waiting for the next real keydown
    /// to become that action's new binding — checked first, ahead of normal
    /// shortcut dispatch, in `subclass_proc`.
    listening_for: Cell<Option<Action>>,
    profile: Profile,
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

    /// Shows the window — called once from `main.rs` after the first page
    /// is opened, mirroring gtk3's `window.show_all()`.
    pub fn activate(&self) -> anyhow::Result<()> {
        self.window.Activate()?;
        Ok(())
    }
}

impl AppState {
    fn with_active<F: FnOnce(&WebView2Engine) -> anyhow::Result<()>>(&self, f: F) {
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

        let container = Grid::new()?;
        container.SetVisibility(Visibility::Collapsed)?;
        self.content_grid.Children()?.Append(&container)?;
        self.containers.borrow_mut().insert(id.clone(), container.clone());

        let title = Rc::new(RefCell::new(String::new()));
        let title_for_cb = Rc::clone(&title);
        let app_weak = Rc::downgrade(self);
        let id_for_cb = id.clone();
        let engine = WebView2Engine::new(&container, url, move |new_title| {
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

    /// Opens either the saved session's pages (if any) or `start_page` —
    /// this crate's single startup call site. Individual `add_page`
    /// failures are logged and skipped rather than aborting the whole
    /// restore (a URL that no longer resolves shouldn't cost the user
    /// every *other* restored tab).
    pub fn open_start_page_or_restored_session(self: &Rc<Self>) {
        let session = Session::load(&self.profile);
        let start_page = self.settings.borrow().start_page.clone();
        let plan = browser_chrome_core::resolve_restore_plan(&session, &start_page);
        for url in &plan.urls {
            if let Err(err) = self.add_page(url) {
                eprintln!("failed to open restored page {url:?}: {err}");
            }
        }
        // `add_page` makes each newly-added page active in turn, so without
        // this the *last* URL in `plan.urls` would end up active regardless
        // of which one was actually active when the session was saved. The
        // id is copied out of `core`'s borrow into its own statement before
        // calling `set_active` (which needs its own borrow) rather than
        // held across it — otherwise this would panic on the `RefCell`'s
        // already-borrowed check.
        let active_page_id = plan.active_index.and_then(|idx| self.core.borrow().pages().get(idx).map(|p| p.id.clone()));
        if let Some(id) = active_page_id {
            self.set_active(&id);
        }
    }

    /// Snapshots the currently-open pages (URL + title, in `PageManager`'s
    /// own creation order) plus which one is active — called from
    /// `subclass_proc`'s `WM_DESTROY` arm before it exits the application,
    /// the one hook both the window's own close button and `Action::Quit`
    /// (via `self.window.Close()`) route through.
    pub fn save_session(&self) {
        let core = self.core.borrow();
        let active_id = core.active_id();
        let active_index = core.pages().iter().position(|p| p.id == active_id);
        let pages = core.pages().iter().map(|p| SessionPage { url: p.current_url(), title: p.title.borrow().clone() }).collect();
        drop(core);
        let session = Session { pages, active_index };
        if let Err(err) = session.save(&self.profile) {
            eprintln!("failed to save session: {err}");
        }
    }

    /// Records a history visit for page `id`'s current URL/title — called
    /// from the `on_title_changed` callback (`add_page`/`ensure_engine_loaded`),
    /// the one place both are available together. Best-effort: logs rather
    /// than propagating.
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

    /// Tears down the engines for pages `PageManager` just flipped to
    /// unloaded. Unlike `WryEngine` (dropped, which destroys its widget via
    /// wry's own `Drop`), `WebView2Engine` needs an explicit `close()` call —
    /// see that type's doc comment.
    fn unload_engines(&self, ids: &[String]) {
        let mut core = self.core.borrow_mut();
        let containers = self.containers.borrow();
        for id in ids {
            if let Some(page) = core.page_mut(id) {
                if let Some(engine) = page.engine.take() {
                    page.last_url = engine.current_url().unwrap_or_else(|_| page.last_url.clone());
                    if let Some(container) = containers.get(id) {
                        if let Err(err) = engine.close(container) {
                            eprintln!("failed to close unloaded page's WebView2: {err}");
                        }
                    }
                    drop(engine);
                }
            }
        }
    }

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
        match WebView2Engine::new(&container, &url, move |new_title| {
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

    fn set_active(self: &Rc<Self>, id: &str) {
        self.ensure_engine_loaded(id);
        self.core.borrow_mut().set_active(id);
        for (other_id, container) in self.containers.borrow().iter() {
            let _ = container.SetVisibility(if other_id == id { Visibility::Visible } else { Visibility::Collapsed });
        }
        if let Some(page) = self.core.borrow().page(id) {
            let _ = self.address_bar.SetText(&HSTRING::from(page.current_url()));
        }
    }

    pub fn open_switcher(self: &Rc<Self>) {
        self.close_settings();
        self.close_profile_picker();
        self.close_keybindings();
        let _ = self.search_box.SetText(&HSTRING::new());
        self.rebuild_switcher_grid();
        let _ = self.content_grid.SetIsHitTestVisible(false);
        let _ = self.switcher_overlay.SetVisibility(Visibility::Visible);
    }

    pub fn close_switcher(&self) {
        let _ = self.switcher_overlay.SetVisibility(Visibility::Collapsed);
        let _ = self.content_grid.SetIsHitTestVisible(true);
    }

    pub fn open_settings(self: &Rc<Self>) {
        self.close_switcher();
        self.close_profile_picker();
        self.close_keybindings();
        let settings = self.settings.borrow();
        let _ = self.start_page_box.SetText(&HSTRING::from(&settings.start_page));
        for (index, engine) in settings.search_engines.iter().enumerate() {
            if engine.name == settings.default_search_engine {
                let _ = self.engine_combo.SetSelectedIndex(index as i32);
            }
        }
        match settings.max_loaded_pages {
            Some(n) => {
                if let Ok(v) = boxed_bool(false) {
                    let _ = self.unlimited_check.SetIsChecked(&v);
                }
                let _ = self.limit_box.SetText(&HSTRING::from(n.to_string()));
                let _ = self.limit_box.SetIsHitTestVisible(true);
            }
            None => {
                if let Ok(v) = boxed_bool(true) {
                    let _ = self.unlimited_check.SetIsChecked(&v);
                }
                let _ = self.limit_box.SetIsHitTestVisible(false);
            }
        }
        drop(settings);
        let _ = self.content_grid.SetIsHitTestVisible(false);
        let _ = self.settings_overlay.SetVisibility(Visibility::Visible);
    }

    pub fn close_settings(&self) {
        let _ = self.settings_overlay.SetVisibility(Visibility::Collapsed);
        let _ = self.content_grid.SetIsHitTestVisible(true);
    }

    pub fn save_settings(self: &Rc<Self>) {
        let unlimited = self.unlimited_check.IsChecked().ok().and_then(|r| r.Value().ok()).unwrap_or(false);
        let new_limit = if unlimited {
            None
        } else {
            self.limit_box
                .Text()
                .ok()
                .and_then(|t| t.to_string().parse::<usize>().ok())
                .map(|n| n.max(1))
        };

        {
            let mut settings = self.settings.borrow_mut();
            if let Ok(text) = self.start_page_box.Text() {
                settings.start_page = text.to_string();
            }
            if let Ok(index) = self.engine_combo.SelectedIndex() {
                if index >= 0 {
                    if let Some(engine) = settings.search_engines.get(index as usize) {
                        settings.default_search_engine = engine.name.clone();
                    }
                }
            }
        }
        self.set_max_loaded_pages(new_limit);
        if let Err(err) = self.settings().save(&self.profile) {
            eprintln!("failed to save settings: {err}");
        }
        self.close_settings();
    }

    /// Shows the profile picker, rebuilt from `list_profile_names()` each
    /// time (so a profile created in an earlier visit shows up).
    pub fn open_profile_picker(self: &Rc<Self>) {
        self.close_switcher();
        self.close_settings();
        self.close_keybindings();
        let _ = self.new_profile_box.SetText(&HSTRING::new());
        self.rebuild_profile_list();
        let _ = self.content_grid.SetIsHitTestVisible(false);
        let _ = self.profile_overlay.SetVisibility(Visibility::Visible);
    }

    pub fn close_profile_picker(&self) {
        let _ = self.profile_overlay.SetVisibility(Visibility::Collapsed);
        let _ = self.content_grid.SetIsHitTestVisible(true);
    }

    /// Rebuilds the profile picker's list of rows from scratch. The current
    /// profile is marked and, unlike every other row, clicking it just
    /// closes the picker instead of launching a duplicate process of the
    /// profile already running.
    fn rebuild_profile_list(self: &Rc<Self>) {
        let Ok(children) = self.profile_list_panel.Children() else { return };
        let _ = children.Clear();

        for name in browser_core::list_profile_names() {
            let is_current = name == self.profile.name;
            let label_text = if is_current { format!("{name} (current)") } else { name.clone() };
            let Ok(row) = Button::new() else { continue };
            let _ = row.SetHorizontalAlignment(HorizontalAlignment::Stretch);
            if let Ok(label) = TextBlock::new() {
                let _ = label.SetText(&HSTRING::from(&label_text));
                let _ = row.SetContent(&label);
            }

            let app_clone = Rc::clone(self);
            let name_clone = name.clone();
            let state = AssertSend((app_clone, name_clone, is_current));
            let _ = row.Click(&winui3::Microsoft::UI::Xaml::RoutedEventHandler::new(move |_, _| {
                let state = &state;
                let (app, name, is_current) = &state.0;
                if *is_current {
                    app.close_profile_picker();
                } else {
                    if let Err(err) = browser_core::launch_new_profile_process(name) {
                        eprintln!("failed to launch a new process for profile {name:?}: {err}");
                    }
                    app.close_profile_picker();
                }
                Ok(())
            }));

            let _ = children.Append(&row);
        }
    }

    /// Reads the new-profile field and launches a new process for it — the
    /// profile picker's "Create & Open" action. The new process creates the
    /// profile's directory lazily on first `Settings`/`HistoryStore` access;
    /// nothing needs pre-creating here.
    pub fn create_and_open_profile(&self) {
        let Ok(text) = self.new_profile_box.Text() else { return };
        let name = text.to_string();
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Err(err) = browser_core::launch_new_profile_process(name) {
            eprintln!("failed to launch a new process for profile {name:?}: {err}");
        }
        self.close_profile_picker();
    }

    /// Shows the keybindings editor, rebuilt from the current `Keybindings`
    /// each time.
    pub fn open_keybindings(self: &Rc<Self>) {
        self.close_switcher();
        self.close_settings();
        self.close_profile_picker();
        self.listening_for.set(None);
        self.rebuild_keybindings_list();
        let _ = self.content_grid.SetIsHitTestVisible(false);
        let _ = self.keybindings_overlay.SetVisibility(Visibility::Visible);
    }

    /// Hides the keybindings editor, cancelling any in-progress "press
    /// keys…" capture.
    pub fn close_keybindings(&self) {
        self.listening_for.set(None);
        let _ = self.keybindings_overlay.SetVisibility(Visibility::Collapsed);
        let _ = self.content_grid.SetIsHitTestVisible(true);
    }

    /// Called from `subclass_proc`'s `WM_KEYDOWN` handling when
    /// `listening_for` is set: assigns `chord` as a new binding for that
    /// action (in addition to any existing ones — "×" on a tag is what
    /// removes), saves, clears the listening state, and rebuilds the rows.
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
        let Ok(children) = self.keybindings_list_panel.Children() else { return };
        let _ = children.Clear();

        for &action in Action::ALL {
            let Ok(row) = StackPanel::new() else { continue };
            let _ = row.SetOrientation(Orientation::Horizontal);

            if let (Ok(label), Ok(row_children)) = (TextBlock::new(), row.Children()) {
                let _ = label.SetText(&HSTRING::from(action.label()));
                let _ = label.SetWidth(180.0);
                let _ = row_children.Append(&label);
            }

            let chords: Vec<KeyChord> = self.keybindings.borrow().bindings_for(action).to_vec();
            for chord in chords {
                if let Ok(tag) = Button::new() {
                    if let Ok(tag_label) = TextBlock::new() {
                        let _ = tag_label.SetText(&HSTRING::from(format!("{chord} \u{d7}")));
                        let _ = tag.SetContent(&tag_label);
                    }
                    let app_clone = Rc::clone(self);
                    let state = AssertSend((app_clone, action, chord));
                    let _ = tag.Click(&winui3::Microsoft::UI::Xaml::RoutedEventHandler::new(move |_, _| {
                        let state = &state;
                        let (app, action, chord) = &state.0;
                        let mut remaining = app.keybindings.borrow().bindings_for(*action).to_vec();
                        remaining.retain(|c| c != chord);
                        app.keybindings.borrow_mut().set_bindings(*action, remaining);
                        if let Err(err) = app.keybindings.borrow().save(&app.profile) {
                            eprintln!("failed to save keybindings: {err}");
                        }
                        app.rebuild_keybindings_list();
                        Ok(())
                    }));
                    if let Ok(row_children) = row.Children() {
                        let _ = row_children.Append(&tag);
                    }
                }
            }

            let listening = self.listening_for.get() == Some(action);
            if let Ok(add_button) = Button::new() {
                if let Ok(add_label) = TextBlock::new() {
                    let _ = add_label.SetText(&HSTRING::from(if listening { "Press keys\u{2026}" } else { "Add binding" }));
                    let _ = add_button.SetContent(&add_label);
                }
                let app_clone = Rc::clone(self);
                let state = AssertSend((app_clone, action));
                let _ = add_button.Click(&winui3::Microsoft::UI::Xaml::RoutedEventHandler::new(move |_, _| {
                    let state = &state;
                    let (app, action) = &state.0;
                    app.listening_for.set(Some(*action));
                    app.rebuild_keybindings_list();
                    Ok(())
                }));
                if let Ok(row_children) = row.Children() {
                    let _ = row_children.Append(&add_button);
                }
            }

            let _ = children.Append(&row);
        }
    }

    /// Runs whatever `action` means — the shared target of normal keydown
    /// dispatch in `subclass_proc`.
    fn dispatch_action(self: &Rc<Self>, action: Action) {
        match action {
            Action::OpenSwitcher => self.open_switcher(),
            Action::ClosePage => {
                let id = self.core.borrow().active_id().to_string();
                self.close_page(&id);
            }
            Action::Reload => self.with_active(|p| p.reload()),
            Action::GoBack => self.with_active(|p| p.go_back()),
            Action::GoForward => self.with_active(|p| p.go_forward()),
            Action::OpenSettings => self.open_settings(),
            Action::OpenProfilePicker => self.open_profile_picker(),
            // Bookmarks, the Ctrl+L/Ctrl+T split (EditUrl vs. OpenSwitcher),
            // reader mode, and the password manager overlay all landed in
            // browser-core + browser-linux-gtk3 only so far (see
            // ROADMAP.md's Backlog) — no winui3 UI/engine support to
            // dispatch any of these to yet.
            Action::NextPage => self.switch_to_next_page(),
            Action::PreviousPage => self.switch_to_previous_page(),
            // Triggers a real Win32 window close → WM_DESTROY → the
            // existing `subclass_proc` arm already calls
            // `Application::Current()?.Exit()` there (now saving the
            // session first too) — one save-then-quit implementation,
            // reached whether the user picks Ctrl+Q or the OS close button.
            Action::Quit => {
                if let Err(err) = self.window.Close() {
                    eprintln!("failed to close the window: {err}");
                }
            }
            Action::ToggleBookmark
            | Action::OpenBookmarks
            | Action::EditUrl
            | Action::ToggleReaderMode
            | Action::OpenPasswords => {}
        }
    }

    fn matching_page_ids(&self, query: &str) -> Vec<String> {
        self.core.borrow().matching_ids(query)
    }

    pub fn switch_to(self: &Rc<Self>, id: &str) {
        self.set_active(id);
        self.close_switcher();
    }

    /// `Action::NextPage` (Ctrl+Tab/Ctrl+PageDown on gtk3 — this platform has
    /// no physical key recognition for either yet, see `ROADMAP.md`, but the
    /// dispatch itself is real, working code, not a stub). The id is copied
    /// out of `core`'s borrow before calling `set_active` (which needs its
    /// own borrow) rather than held across it.
    fn switch_to_next_page(self: &Rc<Self>) {
        let id = self.core.borrow().next_page_id().map(|s| s.to_string());
        if let Some(id) = id {
            self.set_active(&id);
        }
    }

    /// `Action::PreviousPage` (Ctrl+Shift+Tab/Ctrl+PageUp on gtk3) — same as
    /// `switch_to_next_page`, one position earlier.
    fn switch_to_previous_page(self: &Rc<Self>) {
        let id = self.core.borrow().previous_page_id().map(|s| s.to_string());
        if let Some(id) = id {
            self.set_active(&id);
        }
    }

    pub fn close_page(self: &Rc<Self>, id: &str) {
        let was_active = self.core.borrow().active_id() == id;

        self.core.borrow_mut().remove(id);
        if let Some(container) = self.containers.borrow_mut().remove(id) {
            let _ = self.content_grid.Children().and_then(|c| {
                let mut index = 0u32;
                if c.IndexOf(&container, &mut index)? {
                    c.RemoveAt(index)?;
                }
                Ok(())
            });
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

    /// Rebuilds the switcher's tile grid from scratch, sourcing the row list
    /// itself from `browser_chrome_core::build_switcher_rows` (open pages
    /// matching the search box's current text, the "+" add row, and matching
    /// history entries not already open, once there's a query — see that
    /// function's doc comment; `None` for the bookmarks source, since this
    /// crate has no bookmarks integration). No `WrapPanel`/`GridView`
    /// binding exists in this crate's subset, so tiles are laid out by hand
    /// into a fixed `TILE_COLUMNS`-wide grid — and since
    /// `FrameworkElement::SizeChanged` has no working `add` accessor either
    /// (see this module's doc comment), the column count can't react to the
    /// actual window width the way gtk3's `FlowBox` does; it's a fixed
    /// constant instead. A known, honest simplification, not an oversight.
    fn rebuild_switcher_grid(self: &Rc<Self>) {
        let Ok(children) = self.tile_grid.Children() else { return };
        let _ = children.Clear();
        let Ok(row_defs) = self.tile_grid.RowDefinitions() else { return };
        let _ = row_defs.Clear();

        let query = self.search_box.Text().map(|t| t.to_string()).unwrap_or_default();
        let start_page = self.settings.borrow().start_page.clone();
        let rows = browser_chrome_core::build_switcher_rows(&self.core.borrow(), &self.history, None, &query);

        let row_count = ((rows.len() as i32) + TILE_COLUMNS - 1) / TILE_COLUMNS;
        for _ in 0..row_count.max(1) {
            if let Ok(row) = RowDefinition::new() {
                let _ = row.SetHeight(GridLength { Value: TILE_HEIGHT as f64, GridUnitType: GridUnitType::Pixel });
                let _ = row_defs.Append(&row);
            }
        }

        for (idx, row) in rows.iter().enumerate() {
            // Every row kind renders as the same title/domain tile shape —
            // this crate never had per-page color-coding or a close button
            // (see `build_tile`'s doc comment), so there's no visual
            // distinction to preserve between Open/History/Bookmark/Similar
            // the way gtk3's CSS classes give it. Bookmark/Similar are
            // unreachable today (bookmarks is `None` above) but matched
            // anyway so this stays exhaustive as the shared model grows.
            let built = match row {
                browser_chrome_core::SwitcherRow::Add => build_add_tile(),
                browser_chrome_core::SwitcherRow::Open { title, domain, .. }
                | browser_chrome_core::SwitcherRow::History { title, domain, .. }
                | browser_chrome_core::SwitcherRow::Bookmark { title, domain, .. }
                | browser_chrome_core::SwitcherRow::Similar { title, domain, .. } => build_tile(title, domain),
            };
            let Ok(tile) = built else { continue };

            let idx = idx as i32;
            let _ = Grid::SetRow(&tile, idx / TILE_COLUMNS);
            let _ = Grid::SetColumn(&tile, idx % TILE_COLUMNS);
            let _ = children.Append(&tile);

            let activation = browser_chrome_core::activate_row(&rows, idx as usize, &start_page);
            let app_clone = Rc::clone(self);
            let state = AssertSend((app_clone, activation));
            let _ = tile.Click(&winui3::Microsoft::UI::Xaml::RoutedEventHandler::new(move |_, _| {
                let state = &state;
                let (app, activation) = &state.0;
                match activation {
                    Some(browser_chrome_core::SwitcherActivation::SwitchTo(id)) => app.switch_to(id),
                    Some(browser_chrome_core::SwitcherActivation::OpenNewPage(url)) => {
                        if let Err(err) = app.add_page(url) {
                            eprintln!("failed to open page: {err}");
                        }
                        app.close_switcher();
                    }
                    None => {}
                }
                Ok(())
            }));
        }
    }
}

/// One switcher tile: a title/domain label pair, as a `Button` (`Click`
/// works via the implemented `RoutedEventHandler` delegate — a plain `Grid`
/// with `Tapped` doesn't, see this module's doc comment). Unlike gtk3's
/// tile, this doesn't render `page.color` as a flat background: this
/// crate's bindings have no `SolidColorBrush` at all (not feature-gated,
/// genuinely absent from `Microsoft.UI.Xaml.Media` here) and no other brush
/// type suits a flat per-page color, so per-page color-coding is dropped for
/// this pass rather than faked. Also no close ("×") button on the tile
/// itself — Ctrl+W closes the active page instead.
fn build_tile(title: &str, domain: &str) -> anyhow::Result<Button> {
    let tile = Button::new()?;
    tile.SetWidth(TILE_WIDTH as f64)?;
    tile.SetHeight(TILE_HEIGHT as f64)?;

    let inner = StackPanel::new()?;
    inner.SetOrientation(Orientation::Vertical)?;
    inner.SetVerticalAlignment(VerticalAlignment::Bottom)?;
    inner.SetMargin(Thickness { Left: 10.0, Top: 10.0, Right: 10.0, Bottom: 10.0 })?;

    let title_label = TextBlock::new()?;
    title_label.SetText(&HSTRING::from(title))?;
    inner.Children()?.Append(&title_label)?;

    let domain_label = TextBlock::new()?;
    domain_label.SetText(&HSTRING::from(domain))?;
    inner.Children()?.Append(&domain_label)?;

    tile.SetContent(&inner)?;
    Ok(tile)
}

fn build_add_tile() -> anyhow::Result<Button> {
    let button = Button::new()?;
    button.SetWidth(TILE_WIDTH as f64)?;
    button.SetHeight(TILE_HEIGHT as f64)?;
    let label = TextBlock::new()?;
    label.SetText(&HSTRING::from("+"))?;
    label.SetHorizontalTextAlignment(winui3::Microsoft::UI::Xaml::TextAlignment::Center)?;
    button.SetContent(&label)?;
    Ok(button)
}

/// `CheckBox::SetIsChecked` takes a boxed, nullable `IReference<bool>` (WinRT
/// has no plain non-nullable-bool setter for it), not a plain `bool`.
fn boxed_bool(value: bool) -> anyhow::Result<windows::Foundation::IReference<bool>> {
    let boxed = windows::Foundation::PropertyValue::CreateBoolean(value)?;
    Ok(windows_core::Interface::cast(&boxed)?)
}


fn icon_button(label: &str) -> anyhow::Result<Button> {
    let button = Button::new()?;
    let text = TextBlock::new()?;
    text.SetText(&HSTRING::from(label))?;
    button.SetContent(&text)?;
    Ok(button)
}

/// Builds the full window + chrome (custom title bar, page host, switcher and
/// settings overlays) and wires up all handlers. Does not create any page —
/// call `app.add_page(&app.settings().start_page.clone())` afterward.
pub fn build_window_and_app(profile: Profile) -> anyhow::Result<Rc<AppState>> {
    let window = Window::new()?;
    window.SetTitle(&HSTRING::from(APP_NAME))?;
    if let Ok(app_window) = window.AppWindow() {
        let _ = app_window.Resize(windows::Graphics::SizeInt32 { Width: 1024, Height: 768 });
    }

    let root_grid = Grid::new()?;
    let row_defs = root_grid.RowDefinitions()?;
    let toolbar_row = RowDefinition::new()?;
    toolbar_row.SetHeight(GridLength { Value: 0.0, GridUnitType: GridUnitType::Auto })?;
    row_defs.Append(&toolbar_row)?;
    let content_row = RowDefinition::new()?;
    content_row.SetHeight(GridLength { Value: 0.0, GridUnitType: GridUnitType::Star })?;
    row_defs.Append(&content_row)?;

    // --- Toolbar row (also the custom title bar's draggable surface) ---
    let toolbar = StackPanel::new()?;
    toolbar.SetOrientation(Orientation::Horizontal)?;
    toolbar.SetHeight(40.0)?;
    Grid::SetRow(&toolbar, 0)?;

    let back_button = icon_button("\u{25c0}")?;
    let forward_button = icon_button("\u{25b6}")?;
    let reload_button = icon_button("\u{27f3}")?;
    let switcher_toggle = icon_button("\u{229e}")?;
    let settings_button = icon_button("\u{2699}")?;
    let profile_button = icon_button("\u{1f464}")?;
    let keybindings_button = icon_button("\u{2328}")?;

    let address_bar = TextBox::new()?;
    address_bar.SetWidth(500.0)?;
    address_bar.SetHorizontalAlignment(HorizontalAlignment::Stretch)?;

    toolbar.Children()?.Append(&back_button)?;
    toolbar.Children()?.Append(&forward_button)?;
    toolbar.Children()?.Append(&reload_button)?;
    toolbar.Children()?.Append(&address_bar)?;
    toolbar.Children()?.Append(&switcher_toggle)?;
    toolbar.Children()?.Append(&settings_button)?;
    toolbar.Children()?.Append(&profile_button)?;
    toolbar.Children()?.Append(&keybindings_button)?;
    root_grid.Children()?.Append(&toolbar)?;

    // --- Content row: page host + switcher overlay + settings overlay ---
    let content_grid = Grid::new()?;
    Grid::SetRow(&content_grid, 1)?;
    root_grid.Children()?.Append(&content_grid)?;

    // No dimming scrim behind this overlay (unlike gtk3's): this crate's
    // bindings have no `SolidColorBrush`/other constructible flat-color
    // brush at all, so there's nothing to set `Background` to (see
    // `build_tile`'s doc comment for the same gap).
    let switcher_overlay = Grid::new()?;
    Grid::SetRow(&switcher_overlay, 1)?;
    switcher_overlay.SetVisibility(Visibility::Collapsed)?;
    root_grid.Children()?.Append(&switcher_overlay)?;

    let switcher_content = StackPanel::new()?;
    switcher_content.SetOrientation(Orientation::Vertical)?;
    switcher_content.SetHorizontalAlignment(HorizontalAlignment::Center)?;
    switcher_content.SetMargin(Thickness { Left: 0.0, Top: 40.0, Right: 0.0, Bottom: 0.0 })?;
    switcher_overlay.Children()?.Append(&switcher_content)?;

    let search_box = TextBox::new()?;
    search_box.SetWidth(400.0)?;
    search_box.SetPlaceholderText(&HSTRING::from("Type to filter open pages\u{2026}"))?;
    switcher_content.Children()?.Append(&search_box)?;

    let tile_grid = Grid::new()?;
    let tile_columns = tile_grid.ColumnDefinitions()?;
    for _ in 0..TILE_COLUMNS {
        let col = ColumnDefinition::new()?;
        col.SetWidth(GridLength { Value: 1.0, GridUnitType: GridUnitType::Star })?;
        tile_columns.Append(&col)?;
    }
    tile_grid.SetMargin(Thickness { Left: 24.0, Top: 16.0, Right: 24.0, Bottom: 16.0 })?;
    switcher_content.Children()?.Append(&tile_grid)?;

    let profile_label = TextBlock::new()?;
    profile_label.SetText(&HSTRING::from(&profile.name))?;
    profile_label.SetHorizontalAlignment(HorizontalAlignment::Right)?;
    Grid::SetRow(&profile_label, 1)?;
    root_grid.Children()?.Append(&profile_label)?;

    // --- Settings overlay ---
    let settings_overlay = Grid::new()?;
    Grid::SetRow(&settings_overlay, 1)?;
    settings_overlay.SetVisibility(Visibility::Collapsed)?;
    root_grid.Children()?.Append(&settings_overlay)?;

    let settings_panel = StackPanel::new()?;
    settings_panel.SetOrientation(Orientation::Vertical)?;
    settings_panel.SetHorizontalAlignment(HorizontalAlignment::Center)?;
    settings_panel.SetVerticalAlignment(VerticalAlignment::Center)?;
    settings_panel.SetWidth(360.0)?;
    settings_overlay.Children()?.Append(&settings_panel)?;

    let start_page_label = TextBlock::new()?;
    start_page_label.SetText(&HSTRING::from("Start page"))?;
    settings_panel.Children()?.Append(&start_page_label)?;
    let start_page_box = TextBox::new()?;
    settings_panel.Children()?.Append(&start_page_box)?;

    let engine_label = TextBlock::new()?;
    engine_label.SetText(&HSTRING::from("Search engine"))?;
    settings_panel.Children()?.Append(&engine_label)?;
    let engine_combo = ComboBox::new()?;
    for engine in &Settings::default().search_engines {
        let item = winui3::Microsoft::UI::Xaml::Controls::ComboBoxItem::new()?;
        let label = TextBlock::new()?;
        label.SetText(&HSTRING::from(&engine.name))?;
        item.SetContent(&label)?;
        engine_combo.Items()?.Append(&item)?;
    }
    settings_panel.Children()?.Append(&engine_combo)?;

    let unlimited_check = CheckBox::new()?;
    let unlimited_label = TextBlock::new()?;
    unlimited_label.SetText(&HSTRING::from("Unlimited loaded pages"))?;
    unlimited_check.SetContent(&unlimited_label)?;
    settings_panel.Children()?.Append(&unlimited_check)?;

    let limit_label = TextBlock::new()?;
    limit_label.SetText(&HSTRING::from("Loaded pages limit"))?;
    settings_panel.Children()?.Append(&limit_label)?;
    let limit_box = TextBox::new()?;
    settings_panel.Children()?.Append(&limit_box)?;

    let buttons_row = StackPanel::new()?;
    buttons_row.SetOrientation(Orientation::Horizontal)?;
    buttons_row.SetHorizontalAlignment(HorizontalAlignment::Right)?;
    let cancel_button = icon_button("Cancel")?;
    let save_button = icon_button("Save")?;
    buttons_row.Children()?.Append(&cancel_button)?;
    buttons_row.Children()?.Append(&save_button)?;
    settings_panel.Children()?.Append(&buttons_row)?;

    // --- Profile picker overlay: same shape as the settings overlay above.
    // Lists existing profiles (rebuilt each time it opens) plus a field to
    // create a new one — picking any profile other than the current one
    // launches a new, independent process scoped to it
    // (`launch_new_profile_process`) rather than switching this window in
    // place.
    let profile_overlay = Grid::new()?;
    Grid::SetRow(&profile_overlay, 1)?;
    profile_overlay.SetVisibility(Visibility::Collapsed)?;
    root_grid.Children()?.Append(&profile_overlay)?;

    let profile_panel = StackPanel::new()?;
    profile_panel.SetOrientation(Orientation::Vertical)?;
    profile_panel.SetHorizontalAlignment(HorizontalAlignment::Center)?;
    profile_panel.SetVerticalAlignment(VerticalAlignment::Center)?;
    profile_panel.SetWidth(360.0)?;
    profile_overlay.Children()?.Append(&profile_panel)?;

    let profile_title = TextBlock::new()?;
    profile_title.SetText(&HSTRING::from("Profiles"))?;
    profile_panel.Children()?.Append(&profile_title)?;

    let profile_list_panel = StackPanel::new()?;
    profile_list_panel.SetOrientation(Orientation::Vertical)?;
    profile_panel.Children()?.Append(&profile_list_panel)?;

    let new_profile_box = TextBox::new()?;
    new_profile_box.SetPlaceholderText(&HSTRING::from("New profile name\u{2026}"))?;
    profile_panel.Children()?.Append(&new_profile_box)?;

    let profile_buttons_row = StackPanel::new()?;
    profile_buttons_row.SetOrientation(Orientation::Horizontal)?;
    profile_buttons_row.SetHorizontalAlignment(HorizontalAlignment::Right)?;
    let profile_cancel_button = icon_button("Cancel")?;
    let create_profile_button = icon_button("Create & Open")?;
    profile_buttons_row.Children()?.Append(&profile_cancel_button)?;
    profile_buttons_row.Children()?.Append(&create_profile_button)?;
    profile_panel.Children()?.Append(&profile_buttons_row)?;

    // --- Keybindings editor overlay: same shape again. One row per
    // `Action::ALL`, rebuilt from the current `Keybindings` each time it
    // opens and after every add/remove.
    let keybindings_overlay = Grid::new()?;
    Grid::SetRow(&keybindings_overlay, 1)?;
    keybindings_overlay.SetVisibility(Visibility::Collapsed)?;
    root_grid.Children()?.Append(&keybindings_overlay)?;

    let keybindings_panel = StackPanel::new()?;
    keybindings_panel.SetOrientation(Orientation::Vertical)?;
    keybindings_panel.SetHorizontalAlignment(HorizontalAlignment::Center)?;
    keybindings_panel.SetVerticalAlignment(VerticalAlignment::Center)?;
    keybindings_panel.SetWidth(420.0)?;
    keybindings_overlay.Children()?.Append(&keybindings_panel)?;

    let keybindings_title = TextBlock::new()?;
    keybindings_title.SetText(&HSTRING::from("Keybindings"))?;
    keybindings_panel.Children()?.Append(&keybindings_title)?;

    let keybindings_list_panel = StackPanel::new()?;
    keybindings_list_panel.SetOrientation(Orientation::Vertical)?;
    keybindings_panel.Children()?.Append(&keybindings_list_panel)?;

    let keybindings_close_row = StackPanel::new()?;
    keybindings_close_row.SetOrientation(Orientation::Horizontal)?;
    keybindings_close_row.SetHorizontalAlignment(HorizontalAlignment::Right)?;
    let keybindings_close_button = icon_button("Close")?;
    keybindings_close_row.Children()?.Append(&keybindings_close_button)?;
    keybindings_panel.Children()?.Append(&keybindings_close_row)?;

    window.SetContent(&root_grid)?;
    window.SetExtendsContentIntoTitleBar(true)?;
    window.SetTitleBar(&toolbar)?;

    let settings = Settings::load(&profile);
    let history = HistoryStore::open(&profile)?;
    let core = PageManager::new(settings.max_loaded_pages);
    let app = Rc::new(AppState {
        window: window.clone(),
        address_bar: address_bar.clone(),
        address_bar_focused: Cell::new(false),
        content_grid,
        switcher_overlay,
        search_box: search_box.clone(),
        search_box_focused: Cell::new(false),
        tile_grid,
        settings_overlay,
        start_page_box,
        engine_combo,
        unlimited_check,
        limit_box,
        profile_overlay,
        profile_list_panel,
        new_profile_box: new_profile_box.clone(),
        new_profile_box_focused: Cell::new(false),
        core: RefCell::new(core),
        containers: RefCell::new(HashMap::new()),
        settings: RefCell::new(settings),
        history,
        keybindings_overlay,
        keybindings_list_panel,
        keybindings: RefCell::new(Keybindings::load(&profile)),
        listening_for: Cell::new(None),
        profile,
    });

    wire_events(&app, &back_button, &forward_button, &reload_button, &address_bar, &switcher_toggle, &settings_button, &search_box, &cancel_button, &save_button)?;
    wire_profile_picker_events(&app, &profile_button, &profile_cancel_button, &create_profile_button, &new_profile_box)?;
    wire_keybindings_events(&app, &keybindings_button, &keybindings_close_button)?;
    install_hwnd_subclass(&app, &window)?;

    Ok(app)
}

// One-off wiring call with one widget per parameter (mirrors gtk3's
// `build_window_and_app`, which wires each widget inline instead) — a
// bundling struct would only be constructed here, once, so it wouldn't
// pay for itself.
#[allow(clippy::too_many_arguments)]
fn wire_events(
    app: &Rc<AppState>,
    back_button: &Button,
    forward_button: &Button,
    reload_button: &Button,
    address_bar: &TextBox,
    switcher_toggle: &Button,
    settings_button: &Button,
    search_box: &TextBox,
    cancel_button: &Button,
    save_button: &Button,
) -> anyhow::Result<()> {
    use winui3::Microsoft::UI::Xaml::RoutedEventHandler;

    macro_rules! on_click {
        ($button:expr, $app:ident, $body:expr) => {{
            let state = AssertSend(Rc::clone(app));
            $button.Click(&RoutedEventHandler::new(move |_, _| {
                let state = &state;
                let $app = &state.0;
                $body;
                Ok(())
            }))?;
        }};
    }

    on_click!(back_button, app, app.with_active(|p| p.go_back()));
    on_click!(forward_button, app, app.with_active(|p| p.go_forward()));
    on_click!(reload_button, app, app.with_active(|p| p.reload()));
    on_click!(switcher_toggle, app, {
        if app.switcher_overlay.Visibility()? == Visibility::Visible {
            app.close_switcher();
        } else {
            app.open_switcher();
        }
    });
    on_click!(settings_button, app, app.open_settings());
    on_click!(cancel_button, app, app.close_settings());
    on_click!(save_button, app, app.save_settings());

    {
        let state = AssertSend(Rc::clone(app));
        address_bar.GotFocus(&RoutedEventHandler::new(move |_, _| {
            let state = &state;
            state.0.address_bar_focused.set(true);
            Ok(())
        }))?;
    }
    {
        let state = AssertSend(Rc::clone(app));
        address_bar.LostFocus(&RoutedEventHandler::new(move |_, _| {
            let state = &state;
            state.0.address_bar_focused.set(false);
            Ok(())
        }))?;
    }
    {
        let state = AssertSend(Rc::clone(app));
        search_box.GotFocus(&RoutedEventHandler::new(move |_, _| {
            let state = &state;
            state.0.search_box_focused.set(true);
            Ok(())
        }))?;
    }
    {
        let state = AssertSend(Rc::clone(app));
        search_box.LostFocus(&RoutedEventHandler::new(move |_, _| {
            let state = &state;
            state.0.search_box_focused.set(false);
            Ok(())
        }))?;
    }
    {
        let state = AssertSend(Rc::clone(app));
        search_box.TextChanged(&winui3::Microsoft::UI::Xaml::Controls::TextChangedEventHandler::new(move |_, _| {
            let state = &state;
            state.0.rebuild_switcher_grid();
            Ok(())
        }))?;
    }

    Ok(())
}

fn wire_profile_picker_events(
    app: &Rc<AppState>,
    profile_button: &Button,
    profile_cancel_button: &Button,
    create_profile_button: &Button,
    new_profile_box: &TextBox,
) -> anyhow::Result<()> {
    use winui3::Microsoft::UI::Xaml::RoutedEventHandler;

    macro_rules! on_click {
        ($button:expr, $app:ident, $body:expr) => {{
            let state = AssertSend(Rc::clone(app));
            $button.Click(&RoutedEventHandler::new(move |_, _| {
                let state = &state;
                let $app = &state.0;
                $body;
                Ok(())
            }))?;
        }};
    }

    on_click!(profile_button, app, app.open_profile_picker());
    on_click!(profile_cancel_button, app, app.close_profile_picker());
    on_click!(create_profile_button, app, app.create_and_open_profile());

    // No working `KeyDown` on `TextBox` in this crate (see this module's
    // doc comment) — Enter-to-submit goes through the same focus-flag +
    // HWND-subclass `WM_KEYDOWN` route as the address bar/search box
    // instead (see `new_profile_box_focused` and `subclass_proc`).
    {
        let state = AssertSend(Rc::clone(app));
        new_profile_box.GotFocus(&RoutedEventHandler::new(move |_, _| {
            let state = &state;
            state.0.new_profile_box_focused.set(true);
            Ok(())
        }))?;
    }
    {
        let state = AssertSend(Rc::clone(app));
        new_profile_box.LostFocus(&RoutedEventHandler::new(move |_, _| {
            let state = &state;
            state.0.new_profile_box_focused.set(false);
            Ok(())
        }))?;
    }

    Ok(())
}

fn wire_keybindings_events(app: &Rc<AppState>, keybindings_button: &Button, keybindings_close_button: &Button) -> anyhow::Result<()> {
    use winui3::Microsoft::UI::Xaml::RoutedEventHandler;

    macro_rules! on_click {
        ($button:expr, $app:ident, $body:expr) => {{
            let state = AssertSend(Rc::clone(app));
            $button.Click(&RoutedEventHandler::new(move |_, _| {
                let state = &state;
                let $app = &state.0;
                $body;
                Ok(())
            }))?;
        }};
    }

    on_click!(keybindings_button, app, app.open_keybindings());
    on_click!(keybindings_close_button, app, app.close_keybindings());

    Ok(())
}

/// Normalizes a real Win32 keydown (`WM_KEYDOWN`'s virtual-key code plus the
/// three modifier states) into a `KeyChord`, or `None` if the key itself is
/// a bare modifier press (Ctrl/Alt/Shift alone) or one this crate doesn't
/// know how to name — used both for normal shortcut dispatch and for the
/// keybindings editor's "press keys…" capture, so a binding can never end up
/// as just "Ctrl" with no actual key. Letters/digits map directly (Win32's
/// `VK_A`..`VK_Z`/`VK_0`..`VK_9` are already the ASCII codes); everything
/// else this app's default bindings actually use (F-keys, arrows, Escape) is
/// named explicitly.
fn winui_vk_to_chord(vk: u16, ctrl: bool, alt: bool, shift: bool) -> Option<KeyChord> {
    if vk == VK_CONTROL.0 || vk == VK_SHIFT.0 || vk == VK_MENU.0 {
        return None;
    }
    let key = if (b'A' as u16..=b'Z' as u16).contains(&vk) || (b'0' as u16..=b'9' as u16).contains(&vk) {
        (vk as u8 as char).to_string()
    } else if (VK_F1.0..=VK_F1.0 + 11).contains(&vk) {
        format!("F{}", vk - VK_F1.0 + 1)
    } else if vk == VK_ESCAPE.0 {
        "Escape".to_string()
    } else if vk == VK_LEFT.0 {
        "Left".to_string()
    } else if vk == VK_RIGHT.0 {
        "Right".to_string()
    } else if vk == VK_UP.0 {
        "Up".to_string()
    } else if vk == VK_DOWN.0 {
        "Down".to_string()
    } else {
        return None;
    };
    Some(KeyChord::new(ctrl, alt, shift, key))
}

/// See this module's doc comment: `Window` has no working `Closed` event and
/// `UIElement` has no working `KeyDown` event in this crate's bindings, so
/// both go through a raw `HWND` subclass instead. Handles:
/// - `WM_KEYDOWN`: dispatches through `Keybindings::action_for` (or, while
///   the keybindings editor is capturing a new binding, assigns it instead),
///   plus Escape closing whichever overlay is open and Enter navigating the
///   address bar/activating the switcher's search box/submitting the
///   new-profile field (via the `*_focused` flags kept up to date by
///   `GotFocus`/`LostFocus`, which *do* work) — none of those three are part
///   of the configurable action set, see this module's doc comment.
/// - `WM_DESTROY`: calls `Application::Current()?.Exit()`, replacing the
///   unusable `Window::Closed` as the app-quit trigger.
/// - `WM_NCDESTROY`: removes the subclass (standard teardown discipline).
fn install_hwnd_subclass(app: &Rc<AppState>, window: &Window) -> anyhow::Result<()> {
    let native = windows_core::Interface::cast::<IWindowNative>(window)?;
    let hwnd = unsafe { native.WindowHandle()? };

    let state = Box::new(Rc::clone(app));
    let ref_data = Box::into_raw(state) as usize;
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, ref_data);
    }
    Ok(())
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> LRESULT {
    // TEMPORARY, same diagnostic purpose as main.rs's trace() — see that
    // file's comment. Logs every single window message this subclass
    // receives, to tell whether the app's first-ever real-Windows crash
    // (which the trace in main.rs proved happens *after* a fully successful
    // startup — window built, page added, activated) happens before this
    // subclass receives any message at all (pointing at WinUI 3's own
    // Composition/rendering pipeline, independent of our message handling)
    // or during a specific one (pointing at our own handling of it).
    crate::trace(&format!("subclass_proc: msg=0x{msg:04X} wparam={} lparam={}", wparam.0, lparam.0));

    let app = unsafe { &*(ref_data as *const Rc<AppState>) };

    match msg {
        WM_KEYDOWN => {
            let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
            let alt = unsafe { GetKeyState(VK_MENU.0 as i32) } < 0;
            let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
            let vk = wparam.0 as u16;

            if vk == VK_ESCAPE.0 {
                if app.switcher_overlay.Visibility().ok() == Some(Visibility::Visible) {
                    app.close_switcher();
                } else if app.settings_overlay.Visibility().ok() == Some(Visibility::Visible) {
                    app.close_settings();
                } else if app.profile_overlay.Visibility().ok() == Some(Visibility::Visible) {
                    app.close_profile_picker();
                } else if app.keybindings_overlay.Visibility().ok() == Some(Visibility::Visible) {
                    app.close_keybindings();
                }
            } else if app.listening_for.get().is_some() {
                // The keybindings editor is waiting for the next real
                // keydown to become the new binding — takes priority over
                // every other keydown-driven behavior below.
                if let Some(chord) = winui_vk_to_chord(vk, ctrl, alt, shift) {
                    app.assign_listening_binding(chord);
                }
            } else if vk == VK_RETURN.0 && app.address_bar_focused.get() {
                if let Ok(text) = app.address_bar.Text() {
                    let url = resolve_address_input(&text.to_string(), &app.settings());
                    app.with_active(|p| p.navigate(&url));
                }
            } else if vk == VK_RETURN.0 && app.new_profile_box_focused.get() {
                app.create_and_open_profile();
            } else if vk == VK_RETURN.0 && app.search_box_focused.get() {
                if let Ok(text) = app.search_box.Text() {
                    let trimmed = text.to_string();
                    let trimmed = trimmed.trim();
                    if !trimmed.is_empty() {
                        match app.matching_page_ids(trimmed).as_slice() {
                            [] => {
                                // No open page matches — check history
                                // before treating the typed text as a
                                // fresh URL/search: exactly one history
                                // match opens that entry's real URL
                                // instead.
                                match app.history.search(trimmed, 2) {
                                    Ok(matches) if matches.len() == 1 => {
                                        if let Err(err) = app.add_page(&matches[0].url) {
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
                    }
                }
            } else if let Some(chord) = winui_vk_to_chord(vk, ctrl, alt, shift) {
                if let Some(action) = app.keybindings.borrow().action_for(&chord) {
                    app.dispatch_action(action);
                }
            }
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        WM_DESTROY => {
            // The real "the whole app is closing" hook — both the window's
            // own close button (which sends WM_DESTROY via normal Win32
            // teardown) and `Action::Quit` (via `self.window.Close()`,
            // which triggers the exact same teardown) end up here, so this
            // is the one place that needs to save the session before
            // `Application::Exit()` actually ends the process.
            app.save_session();
            let _ = winui3::Microsoft::UI::Xaml::Application::Current().and_then(|a| a.Exit());
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        WM_NCDESTROY => {
            // Reclaim the leaked `Box<Rc<AppState>>` before removing the
            // subclass, mirroring `SetWindowSubclass`'s standard teardown.
            let _ = unsafe { Box::from_raw(ref_data as *mut Rc<AppState>) };
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
    }
}

/// Shows a small standalone window for launching with a URL argument (e.g.
/// from the OS's "open with"/default-browser handoff) — lets the user
/// confirm/pick which profile to open it in before the real browser window
/// appears. `default_profile` pre-fills the field (whatever `--profile`
/// resolved to, or `"default"`).
///
/// Runs inside the same `Application::Start` callback/message pump as the
/// normal path — this only ever builds and activates a window, it doesn't
/// drive its own event loop.
pub fn show_external_link_chooser(url: String, default_profile: String) -> anyhow::Result<()> {
    let window = Window::new()?;
    window.SetTitle(&HSTRING::from("Open link"))?;
    if let Ok(app_window) = window.AppWindow() {
        let _ = app_window.Resize(windows::Graphics::SizeInt32 { Width: 480, Height: 240 });
    }

    let root = StackPanel::new()?;
    root.SetOrientation(Orientation::Vertical)?;
    root.SetMargin(Thickness { Left: 16.0, Top: 16.0, Right: 16.0, Bottom: 16.0 })?;

    let url_label = TextBlock::new()?;
    url_label.SetText(&HSTRING::from(&url))?;
    root.Children()?.Append(&url_label)?;

    let profile_label = TextBlock::new()?;
    profile_label.SetText(&HSTRING::from("Open in profile"))?;
    root.Children()?.Append(&profile_label)?;

    let profile_box = TextBox::new()?;
    profile_box.SetText(&HSTRING::from(&default_profile))?;
    root.Children()?.Append(&profile_box)?;

    // Existing profiles as one-click suggestions — not an editable ComboBox
    // (its `IsEditable`/`TextSubmitted` do check out as real, working
    // bindings here, but given how many *other* supposedly-standard events
    // in this crate have turned out to be silently broken, this sticks to
    // the plainest already-proven controls instead of leaning on one more
    // not-yet-exercised event path for a chooser this simple).
    let suggestions_row = StackPanel::new()?;
    suggestions_row.SetOrientation(Orientation::Horizontal)?;
    for name in list_profile_names() {
        let button = Button::new()?;
        let label = TextBlock::new()?;
        label.SetText(&HSTRING::from(&name))?;
        button.SetContent(&label)?;

        let profile_box = profile_box.clone();
        button.Click(&winui3::Microsoft::UI::Xaml::RoutedEventHandler::new(move |_, _| {
            let _ = profile_box.SetText(&HSTRING::from(&name));
            Ok(())
        }))?;
        suggestions_row.Children()?.Append(&button)?;
    }
    root.Children()?.Append(&suggestions_row)?;

    let button_row = StackPanel::new()?;
    button_row.SetOrientation(Orientation::Horizontal)?;
    button_row.SetHorizontalAlignment(HorizontalAlignment::Right)?;
    let cancel_button = Button::new()?;
    let cancel_label = TextBlock::new()?;
    cancel_label.SetText(&HSTRING::from("Cancel"))?;
    cancel_button.SetContent(&cancel_label)?;
    let open_button = Button::new()?;
    let open_label = TextBlock::new()?;
    open_label.SetText(&HSTRING::from("Open"))?;
    open_button.SetContent(&open_label)?;
    button_row.Children()?.Append(&cancel_button)?;
    button_row.Children()?.Append(&open_button)?;
    root.Children()?.Append(&button_row)?;

    window.SetContent(&root)?;

    // Closing this window (native close button, or Cancel) should quit the
    // app — but successfully opening the main window and closing this one
    // as part of that handoff must not also quit. `transitioning` tells the
    // WM_DESTROY handler below which case it is (same `Window::Closed`-gap
    // workaround as the main window's own `install_hwnd_subclass`).
    let transitioning = Rc::new(Cell::new(false));
    install_chooser_hwnd_subclass(&window, Rc::clone(&transitioning))?;

    {
        let window = window.clone();
        cancel_button.Click(&winui3::Microsoft::UI::Xaml::RoutedEventHandler::new(move |_, _| {
            let _ = window.Close();
            Ok(())
        }))?;
    }
    {
        let window = window.clone();
        let profile_box = profile_box.clone();
        let state = AssertSend(transitioning);
        open_button.Click(&winui3::Microsoft::UI::Xaml::RoutedEventHandler::new(move |_, _| {
            let state = &state;
            let profile_name = profile_box.Text().map(|t| t.to_string()).unwrap_or_default();
            let profile = Profile::new(profile_name);

            state.0.set(true);
            let _ = window.Close();

            match build_window_and_app(profile) {
                Ok(app) => {
                    if let Err(err) = app.add_page(&url) {
                        eprintln!("failed to open the launch URL: {err}");
                    }
                    if let Err(err) = app.activate() {
                        eprintln!("failed to activate the browser window: {err}");
                    }
                }
                Err(err) => eprintln!("failed to open the browser window: {err}"),
            }
            Ok(())
        }))?;
    }

    window.Activate()?;
    Ok(())
}

/// Same technique as `install_hwnd_subclass`, for the chooser window: only
/// handles `WM_DESTROY` (checking `transitioning` before deciding whether to
/// quit) and `WM_NCDESTROY` (teardown) — the chooser has no keyboard
/// shortcuts or address bar of its own to route through `WM_KEYDOWN`.
fn install_chooser_hwnd_subclass(window: &Window, transitioning: Rc<Cell<bool>>) -> anyhow::Result<()> {
    let native = windows_core::Interface::cast::<IWindowNative>(window)?;
    let hwnd = unsafe { native.WindowHandle()? };

    let state = Box::new(transitioning);
    let ref_data = Box::into_raw(state) as usize;
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(chooser_subclass_proc), CHOOSER_SUBCLASS_ID, ref_data);
    }
    Ok(())
}

unsafe extern "system" fn chooser_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> LRESULT {
    match msg {
        WM_DESTROY => {
            let transitioning = unsafe { &*(ref_data as *const Rc<Cell<bool>>) };
            if !transitioning.get() {
                let _ = winui3::Microsoft::UI::Xaml::Application::Current().and_then(|a| a.Exit());
            }
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        WM_NCDESTROY => {
            let _ = unsafe { Box::from_raw(ref_data as *mut Rc<Cell<bool>>) };
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(chooser_subclass_proc), CHOOSER_SUBCLASS_ID) };
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
    }
}
