//! WinUI 3 chrome, take two: built on Microsoft's own `windows-reactor`/
//! `windows-webview` (in-tree in `microsoft/windows-rs`, same
//! `windows-bindgen` WinMD codegen as the base `windows` crate) instead of
//! the community `winio-winui3` wrapper `browser-windows-winui` depends on.
//! See `summaries/windows-github-actions-ci.md`'s "windows-reactor
//! comparison test" section for why: a comparison app of this same general
//! shape (window + toolbar + `WebView2`) survived on a real Windows VM where
//! `browser-windows-winui` crashes with `STATUS_STOWED_EXCEPTION`.
//!
//! Being built out incrementally, one feature at a time, toward parity with
//! `browser-windows-winui`/`browser-linux-gtk3` (see `ROADMAP.md`) rather
//! than ported in one pass — `windows-reactor`'s declarative, React-like
//! model (a render function of state, re-diffed against the live tree) is a
//! genuinely different shape from `winio-winui3`'s imperative
//! widget-tree-with-handles style, so this isn't a mechanical port.
//!
//! This version wires in `browser_core::PageManager<ReactorWebViewEngine>`
//! for real (see `engine.rs`), a working switcher/settings/profile overlay
//! set (keybindings live as a section *within* settings, not their own
//! overlay — see `keybindings_section`), real global keyboard shortcuts
//! (see `shortcuts.rs`), a native `TitleBar`-hosted toolbar, and
//! `run_chooser`'s external-link launch handling — feature parity with
//! `browser-windows-winui` (see `ROADMAP.md`).
//!
//! # Why `run_chooser` spawns a new process instead of swapping windows
//!
//! `browser-windows-winui`'s equivalent (`show_external_link_chooser`)
//! builds the chooser as its own small `Window`, then on "Open" builds the
//! *real* browser window in the same process and closes the chooser one —
//! doable there since it owns the raw `HWND`s directly. `windows-reactor`
//! has no public way to close the *primary* window it opened via
//! `App::render` (`WindowHandle` — the type with a working `.close()` — is
//! only returned by `ReactorWindow::render`/`.open()` for *secondary*
//! windows; the primary window's registry key is never exposed). Rather
//! than fight that, `run_chooser` reuses a pattern this codebase already
//! has for exactly this shape of problem: `browser_core::
//! launch_new_profile_process` (used by the profile picker to open a
//! different profile) spawns a brand new process rather than swapping
//! state in place. `run_chooser`'s "Open" button does the same —
//! `exe --profile <name> <url>`, parsed back out by
//! `resolve_profile_name`/`resolve_url_argument` exactly like any other
//! relaunch — then exits this small chooser process outright.
//!
//! # Multi-page hosting in a declarative model
//!
//! `winio-winui3`'s approach (a per-page `Grid` container, `Visibility`
//! toggled to show only the active one) doesn't translate directly:
//! `windows-reactor` has no `Visibility`/display modifier at all (checked by
//! reading `crates/libs/reactor/src/element.rs`/`widget.rs` — a real gap,
//! same category as `winio-winui3`'s missing `KeyDown`, just in a different
//! place). Instead, every *loaded* page's `webview(..)` element is always
//! present in the tree, each `.with_key(id)` so the reconciler keeps that
//! specific page's underlying `WebView2` control (and its navigation
//! session) alive across renders — the same identity mechanism
//! `crates/samples/reactor/samples/examples/tab_view_add_button.rs` uses for
//! a dynamic list of tabs. All of them share one grid cell; the active
//! page's element is placed *last* in that cell's children, so it paints
//! (and receives hit-testing) on top, fully occluding the others — a real
//! technique, not a hack: WinUI 3's `Grid` has always supported multiple
//! children stacked in one cell in z-order. An *unloaded* page (evicted by
//! `max_loaded_pages`) simply isn't rendered at all — reactor's own
//! reconciler tears down its `WebView2` control when its keyed element
//! stops appearing, no manual `.close()` call needed the way
//! `WebView2Engine` requires.
//!
//! `browser_core::PageManager<ReactorWebViewEngine>` owns each page's
//! `Rc<RefCell<Option<WebView>>>`/`Rc<RefCell<Option<EventRegistration>>>`
//! (via its `engine` field — see `engine.rs`); `page_element` clones those
//! same `Rc`s out to fill in from `on_ready`, so `RenderEngine`'s methods
//! and reactor's element both read/write the identical shared cells.
//! `active_id_ref` mirrors the `active_id` reactor state into a plain
//! `HookRef<String>` so each page's long-lived navigation-completed closure
//! can check, at *event-fire* time, whether it's still the active page
//! before reflecting into the address bar — a closure capturing `active_id`
//! by value would only ever see whatever it was when that specific page
//! last mounted, not later switches.
#![cfg(all(target_os = "windows", target_env = "msvc"))]

mod engine;
mod shortcuts;

use std::cell::RefCell;
use std::rc::Rc;

use browser_core::{
    launch_new_profile_process, list_profile_names, resolve_address_input, Action, HistoryStore, KeyChord, Keybindings,
    PageManager, Profile, Settings, HOME_URL,
};
use engine::ReactorWebViewEngine;
use windows_reactor::*;
use windows_webview::{webview, WebView};

/// Same checkpoint-tracing pattern as `browser_windows_winui::trace` — cheap
/// and has already paid for itself once diagnosing a real crash on Windows.
pub fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("reactor-trace.log") {
        let _ = writeln!(f, "{msg}");
        let _ = f.sync_all();
    }
}

/// Non-reactive dependencies created once in `run()`, before entering
/// reactor's render loop, and captured by the root render closure —
/// `HistoryStore` owns a real DB connection, so it must not be recreated
/// every render the way hook state is.
struct Shared {
    history: HistoryStore,
    settings: RefCell<Settings>,
    profile: Profile,
}

// The switcher's tile list used to be a local `Tile` enum + hand-copied
// row-building here — now `browser_chrome_core::{SwitcherRow,
// build_switcher_rows, activate_row}` (see `ARCHITECTURE.md` §3.2/§4: the
// exact same decision logic was independently hand-copied in
// `browser-linux-gtk3`/`browser-macos-appkit` too, and is now unit-tested
// once, toolkit-free, instead of manually in three places). Also restores
// the per-page palette `color` this crate's `Tile` never carried (dropped
// when it was first built, relative to `browser-linux-gtk3`'s tiles) —
// `SwitcherRow::Open` carries it now, even though `tile_element` below
// doesn't render it yet (no background-color builder found on
// `windows-reactor`'s `Element`/`vstack` in the time this pass had;
// rendering it is a smaller, separate follow-up).

/// Mutually exclusive, mirroring `browser-windows-winui`'s
/// close_switcher/close_settings/close_profile_picker — opening any one of
/// these closes whichever else was open. No separate `Keybindings` variant:
/// that editor is a section within `Settings` (see `keybindings_section`),
/// not its own overlay.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Switcher,
    Settings,
    Profile,
}

fn app(cx: &mut RenderCx, shared: &Rc<Shared>) -> Element {
    trace("app: render start");
    let core = cx.use_ref(PageManager::<ReactorWebViewEngine>::new(shared.settings.borrow().max_loaded_pages));
    let (generation, set_generation) = cx.use_state(0u64);
    let (active_id, set_active_id) = cx.use_state(String::new());
    let active_id_ref = cx.use_ref(active_id.clone());
    *active_id_ref.borrow_mut() = active_id.clone();
    let (address, set_address) = cx.use_state(String::from(HOME_URL));
    let (overlay, set_overlay) = cx.use_state(Overlay::None);
    let (search_query, set_search_query) = cx.use_state(String::new());

    let (start_page_draft, set_start_page_draft) = cx.use_state(String::new());
    let (engine_index_draft, set_engine_index_draft) = cx.use_state(-1i32);
    let (unlimited_draft, set_unlimited_draft) = cx.use_state(true);
    let (limit_draft, set_limit_draft) = cx.use_state(String::new());

    let (new_profile_draft, set_new_profile_draft) = cx.use_state(String::new());

    let keybindings = cx.use_ref(Keybindings::load(&shared.profile));
    let (listening_for, set_listening_for) = cx.use_state(Option::<Action>::None);
    let (new_binding_text, set_new_binding_text) = cx.use_state(String::new());

    // Bootstrap: open the start page on the very first render (core starts
    // empty — there's no separate "startup" hook, so this just runs
    // in-line, same render pass, before anything below reads `core`).
    if core.borrow().is_empty() {
        let start_page = shared.settings.borrow().start_page.clone();
        do_add_page(&core, &start_page, &set_active_id, &active_id_ref, &set_address);
    }

    // Closures shared across multiple event handlers/`switcher_overlay` are
    // wrapped in reactor's own `Callback<T>` (an `Rc<dyn Fn(T)>` newtype) —
    // plain closures aren't `Clone` even when every captured variable is,
    // so a closure needed in more than one place has to go through this
    // (or an equivalent manual `Rc<dyn Fn>` wrapper) to be cloned at all.
    let bump: Callback<()> = Callback::new({
        let set_generation = set_generation.clone();
        move |()| set_generation.call(generation.wrapping_add(1))
    });

    let switch_to: Callback<String> = Callback::new({
        let core = core.clone();
        let set_active_id = set_active_id.clone();
        let active_id_ref = active_id_ref.clone();
        let set_address = set_address.clone();
        let set_overlay = set_overlay.clone();
        let bump = bump.clone();
        move |id: String| {
            ensure_engine_loaded(&core, &id);
            core.borrow_mut().set_active(&id);
            *active_id_ref.borrow_mut() = id.clone();
            set_active_id.call(id.clone());
            let url = core.borrow().page(&id).map(|p| p.current_url()).unwrap_or_default();
            set_address.call(if url.is_empty() { HOME_URL.to_string() } else { url });
            set_overlay.call(Overlay::None);
            bump.invoke(());
        }
    });

    let add_page_and_switch: Callback<String> = Callback::new({
        let core = core.clone();
        let set_active_id = set_active_id.clone();
        let active_id_ref = active_id_ref.clone();
        let set_address = set_address.clone();
        let set_overlay = set_overlay.clone();
        let bump = bump.clone();
        move |url: String| {
            do_add_page(&core, &url, &set_active_id, &active_id_ref, &set_address);
            set_overlay.call(Overlay::None);
            bump.invoke(());
        }
    });

    let close_page: Callback<String> = Callback::new({
        let core = core.clone();
        let shared = Rc::clone(shared);
        let switch_to = switch_to.clone();
        let add_page_and_switch = add_page_and_switch.clone();
        let bump = bump.clone();
        move |id: String| {
            let was_active = core.borrow().active_id() == id;
            core.borrow_mut().remove(&id);
            if was_active {
                let next = core.borrow().pages().first().map(|p| p.id.clone());
                match next {
                    Some(nid) => switch_to.invoke(nid),
                    None => add_page_and_switch.invoke(shared.settings.borrow().start_page.clone()),
                }
            }
            bump.invoke(());
        }
    });

    let with_active = |action: fn(&ReactorWebViewEngine) -> anyhow::Result<()>| {
        let core = core.clone();
        let active_id = active_id.clone();
        move || {
            let core = core.borrow();
            if let Some(page) = core.page(&active_id) {
                if let Some(engine) = &page.engine {
                    let _ = action(engine);
                }
            }
        }
    };

    let navigate_from_address_bar = {
        let core = core.clone();
        let active_id = active_id.clone();
        let address = address.clone();
        let settings = Rc::clone(shared);
        move || {
            trace("navigate_from_address_bar: fired");
            let core = core.borrow();
            if let Some(engine) = core.page(&active_id).and_then(|p| p.engine.as_ref()) {
                let url = resolve_address_input(&address, &settings.settings.borrow());
                trace(&format!("navigate_from_address_bar: navigating to {url}"));
                use render_engine::RenderEngine;
                let result = engine.navigate(&url);
                trace(&format!("navigate_from_address_bar: navigate() returned {result:?}"));
            } else {
                trace("navigate_from_address_bar: no active engine found");
            }
        }
    };

    let open_switcher: Callback<()> = Callback::new({
        let set_overlay = set_overlay.clone();
        let set_search_query = set_search_query.clone();
        move |()| {
            set_search_query.call(String::new());
            set_overlay.call(Overlay::Switcher);
        }
    });
    let toggle_switcher = {
        let set_overlay = set_overlay.clone();
        let open_switcher = open_switcher.clone();
        move || {
            if overlay == Overlay::Switcher {
                set_overlay.call(Overlay::None);
            } else {
                open_switcher.invoke(());
            }
        }
    };

    let close_any_overlay: Callback<()> = Callback::new({
        let set_overlay = set_overlay.clone();
        move |()| {
            trace("close_any_overlay: fired (Escape)");
            set_overlay.call(Overlay::None)
        }
    });

    let open_settings: Callback<()> = Callback::new({
        let shared = Rc::clone(shared);
        let set_overlay = set_overlay.clone();
        let set_start_page_draft = set_start_page_draft.clone();
        let set_engine_index_draft = set_engine_index_draft.clone();
        let set_unlimited_draft = set_unlimited_draft.clone();
        let set_limit_draft = set_limit_draft.clone();
        let set_listening_for = set_listening_for.clone();
        move |()| {
            set_listening_for.call(None);
            let settings = shared.settings.borrow();
            set_start_page_draft.call(settings.start_page.clone());
            let idx = settings
                .search_engines
                .iter()
                .position(|e| e.name == settings.default_search_engine)
                .map(|i| i as i32)
                .unwrap_or(-1);
            set_engine_index_draft.call(idx);
            match settings.max_loaded_pages {
                Some(n) => {
                    set_unlimited_draft.call(false);
                    set_limit_draft.call(n.to_string());
                }
                None => {
                    set_unlimited_draft.call(true);
                    set_limit_draft.call(String::new());
                }
            }
            drop(settings);
            set_overlay.call(Overlay::Settings);
        }
    });

    let save_settings: Callback<()> = Callback::new({
        let shared = Rc::clone(shared);
        let core = core.clone();
        let set_overlay = set_overlay.clone();
        let bump = bump.clone();
        let start_page_draft = start_page_draft.clone();
        let limit_draft = limit_draft.clone();
        move |()| {
            let new_limit = if unlimited_draft { None } else { limit_draft.parse::<usize>().ok().map(|n| n.max(1)) };
            {
                let mut settings = shared.settings.borrow_mut();
                settings.start_page = start_page_draft.clone();
                if let Some(engine) = settings.search_engines.get(engine_index_draft.max(0) as usize) {
                    settings.default_search_engine = engine.name.clone();
                }
                settings.max_loaded_pages = new_limit;
            }
            let evicted = core.borrow_mut().set_max_loaded_pages(new_limit);
            for id in evicted {
                core.borrow_mut().take_engine(&id);
            }
            if let Err(err) = shared.settings.borrow().save(&shared.profile) {
                eprintln!("failed to save settings: {err}");
            }
            set_overlay.call(Overlay::None);
            bump.invoke(());
        }
    });

    let open_profile: Callback<()> = Callback::new({
        let set_overlay = set_overlay.clone();
        let set_new_profile_draft = set_new_profile_draft.clone();
        move |()| {
            set_new_profile_draft.call(String::new());
            set_overlay.call(Overlay::Profile);
        }
    });

    let create_and_open_profile: Callback<()> = Callback::new({
        let set_overlay = set_overlay.clone();
        let new_profile_draft = new_profile_draft.clone();
        move |()| {
            let name = new_profile_draft.trim();
            if !name.is_empty() {
                if let Err(err) = launch_new_profile_process(name) {
                    eprintln!("failed to launch a new process for profile {name:?}: {err}");
                }
            }
            set_overlay.call(Overlay::None);
        }
    });

    // Runs whatever `action` means — the shared target of every global
    // keyboard-accelerator dispatch built below. Mirrors
    // `browser-windows-winui`'s `dispatch_action`: Bookmarks/reader mode
    // aren't implemented on this front end either yet, same as there.
    //
    // `EditUrl` (Ctrl+L) is different from those: it's not merely unbuilt,
    // it isn't *buildable* with what `windows-reactor` currently exposes.
    // Focusing the address bar programmatically needs a `Focus()`-style
    // call on the live `TextBox`, but neither `TextBox`
    // (`widgets/text_box.rs`) nor `Element`/`Widget` expose any focus
    // method, and there's no way to get a raw handle to the underlying
    // XAML element from application code either (checked directly — the
    // only place that ever touches a real `UIElement` for a mounted
    // control is `host.rs`'s own reconciler, which doesn't hand it back
    // out). So this dispatches correctly (confirmed via `trace` — Ctrl+L
    // reliably reaches this match arm) but has nothing it *can* do yet:
    // a real crate gap, not a missing feature on this end.
    let dispatch_action: Callback<Action> = Callback::new({
        let core = core.clone();
        let active_id = active_id.clone();
        let close_page = close_page.clone();
        let open_switcher = open_switcher.clone();
        let open_settings = open_settings.clone();
        let open_profile = open_profile.clone();
        let switch_to = switch_to.clone();
        move |action: Action| {
            trace(&format!("dispatch_action: fired for {action:?}"));
            use render_engine::RenderEngine;
            match action {
                Action::OpenSwitcher => open_switcher.invoke(()),
                Action::ClosePage => close_page.invoke(active_id.clone()),
                Action::Reload => {
                    if let Some(e) = core.borrow().page(&active_id).and_then(|p| p.engine.as_ref()) {
                        let _ = e.reload();
                    }
                }
                Action::GoBack => {
                    if let Some(e) = core.borrow().page(&active_id).and_then(|p| p.engine.as_ref()) {
                        let _ = e.go_back();
                    }
                }
                Action::GoForward => {
                    if let Some(e) = core.borrow().page(&active_id).and_then(|p| p.engine.as_ref()) {
                        let _ = e.go_forward();
                    }
                }
                Action::OpenSettings => open_settings.invoke(()),
                Action::OpenProfilePicker => open_profile.invoke(()),
                // EditUrl: see this closure's doc comment — not implementable
                // with the crate's current API surface, not just unbuilt.
                // OpenPasswords: no password manager overlay in this crate
                // yet — browser-core's passwords module/PasswordBackend
                // trait exist and compile against this frontend already,
                // just no UI built on top here this pass (see
                // ARCHITECTURE.md).
                // NextPage/PreviousPage (Ctrl+Tab/Ctrl+PageDown/Ctrl+Shift+
                // Tab/Ctrl+PageUp on gtk3 — this platform has no physical
                // key recognition for either yet, see ROADMAP.md, but the
                // dispatch itself is real, working code, not a stub): the
                // id is copied out of `core`'s borrow before invoking
                // `switch_to` (which needs its own borrow) rather than held
                // across it.
                Action::NextPage => {
                    let id = core.borrow().next_page_id().map(|s| s.to_string());
                    if let Some(id) = id {
                        switch_to.invoke(id);
                    }
                }
                Action::PreviousPage => {
                    let id = core.borrow().previous_page_id().map(|s| s.to_string());
                    if let Some(id) = id {
                        switch_to.invoke(id);
                    }
                }
                Action::ToggleBookmark
                | Action::OpenBookmarks
                | Action::EditUrl
                | Action::ToggleReaderMode
                | Action::OpenPasswords => {}
            }
        }
    });

    let toolbar = grid((
        Element::from(button("\u{25c0}").on_click(with_active(|e| {
            use render_engine::RenderEngine;
            e.go_back()
        })))
        .grid_column(0),
        Element::from(button("\u{25b6}").on_click(with_active(|e| {
            use render_engine::RenderEngine;
            e.go_forward()
        })))
        .grid_column(1),
        Element::from(button("\u{27f3}").on_click(with_active(|e| {
            use render_engine::RenderEngine;
            e.reload()
        })))
        .grid_column(2),
        Element::from(
            text_box(address)
                .on_text_changed(set_address.clone())
                .keyboard_accelerator(KeyboardAccelerator::new(
                    VirtualKey::Enter,
                    VirtualKeyModifiers::None,
                    navigate_from_address_bar,
                )),
        )
        .grid_column(3),
        Element::from(button("\u{229e}").on_click(toggle_switcher)).grid_column(4),
        Element::from(button("\u{2699}").on_click({
            let open_settings = open_settings.clone();
            move || open_settings.invoke(())
        }))
        .grid_column(5),
        Element::from(button("\u{1f464}").on_click({
            let open_profile = open_profile.clone();
            move || open_profile.invoke(())
        }))
        .grid_column(6),
    ))
    .columns([
        GridLength::Auto,
        GridLength::Auto,
        GridLength::Auto,
        GridLength::STAR,
        GridLength::Auto,
        GridLength::Auto,
        GridLength::Auto,
    ])
    .column_spacing(8.0)
    .margin(Thickness::uniform(8.0));

    // Real WinUI 3 `Microsoft.UI.Xaml.Controls.TitleBar`, giving a native
    // draggable custom title bar — the reactor-native equivalent of
    // `browser-windows-winui`'s manual `window.SetExtendsContentIntoTitleBar
    // (true)` + `window.SetTitleBar(...)`.
    //
    // The toolbar is deliberately *not* hosted in `TitleBar`'s `.content()`
    // slot — an earlier version of this code did exactly that, and real
    // testing in the dockur/windows VM found every click on that content
    // (the address bar, the settings gear, all of it) silently did nothing:
    // no `dispatch_action` ever fired for a button in there, confirmed via
    // this module's own `trace` log, while the same actions fired
    // correctly through `KeyboardAccelerator`s in the same session.
    // `windows-reactor`'s `host.rs` wires whatever's in this slot up via
    // `Window.SetTitleBar(element)`, which marks that element as the
    // draggable caption region; real WinUI apps that put interactive
    // controls inside a custom title bar have to separately register
    // non-client hit-test passthrough rectangles
    // (`InputNonClientPointerSource.SetRegionRects`) so clicks still reach
    // them — `windows-reactor` doesn't do that anywhere in its own source
    // (checked directly), so anything placed in `.content()` here is
    // click-dead. Keeping `TitleBar` for the native drag/window-chrome area
    // but rendering the toolbar as an ordinary row right below it sidesteps
    // the problem entirely instead of fighting non-client hit testing from
    // a crate that doesn't expose it.
    let title_bar = Element::from(TitleBar::new("claude-browser"));

    // Every *loaded* page's webview stays mounted (see this module's doc
    // comment on why); the active one is pushed last so it paints on top.
    let page_ids = core.borrow().page_ids();
    let mut page_elements: Vec<Element> = Vec::with_capacity(page_ids.len());
    for id in page_ids.iter().filter(|id| **id != active_id) {
        if core.borrow().is_page_loaded(id) {
            page_elements.push(page_element(id.clone(), &core, &shared, &active_id_ref, &set_address));
        }
    }
    if core.borrow().is_page_loaded(&active_id) {
        page_elements.push(page_element(active_id.clone(), &core, &shared, &active_id_ref, &set_address));
    }
    let content = grid(page_elements);

    let overlay_element: Option<Element> = match overlay {
        Overlay::None => None,
        Overlay::Switcher => Some(Element::from(switcher_overlay(
            &core,
            &shared,
            &search_query,
            set_search_query.clone(),
            switch_to.clone(),
            add_page_and_switch.clone(),
            close_page.clone(),
        ))),
        Overlay::Settings => Some(settings_overlay(
            &shared,
            &start_page_draft,
            set_start_page_draft.clone(),
            engine_index_draft,
            set_engine_index_draft.clone(),
            unlimited_draft,
            set_unlimited_draft.clone(),
            &limit_draft,
            set_limit_draft.clone(),
            save_settings.clone(),
            close_any_overlay.clone(),
            &keybindings,
            listening_for,
            set_listening_for.clone(),
            &new_binding_text,
            set_new_binding_text.clone(),
            &bump,
        )),
        Overlay::Profile => Some(profile_overlay(
            &shared,
            &new_profile_draft,
            set_new_profile_draft.clone(),
            create_and_open_profile.clone(),
            close_any_overlay.clone(),
        )),
    };

    // Global shortcuts: Escape always closes whichever overlay is open (a
    // fixed convention, not user-configurable — same as
    // `browser-windows-winui`'s hardcoded Escape handling), plus one
    // `KeyboardAccelerator` per currently-bound `KeyChord` across every
    // `Action`. A real, working replacement for `winio-winui3`'s raw
    // `HWND`-subclass `WM_KEYDOWN` dispatch (see this module's doc
    // comment) — `windows-reactor` has actual global-shortcut support.
    let mut accelerators: Vec<KeyboardAccelerator> = Vec::new();
    if let Some(accel) = shortcuts::chord_to_accelerator(&KeyChord::new(false, false, false, "Escape"), {
        let close_any_overlay = close_any_overlay.clone();
        move || close_any_overlay.invoke(())
    }) {
        accelerators.push(accel);
    }
    for &action in Action::ALL {
        for chord in keybindings.borrow().bindings_for(action) {
            let dispatch_action = dispatch_action.clone();
            if let Some(accel) = shortcuts::chord_to_accelerator(chord, move || dispatch_action.invoke(action)) {
                accelerators.push(accel);
            }
        }
    }

    trace("app: render end");
    let mut rows = vec![
        title_bar.grid_row(0),
        Element::from(toolbar).grid_row(1),
        Element::from(content).grid_row(2),
    ];
    if let Some(overlay_element) = overlay_element {
        rows.push(overlay_element.grid_row(2));
    }
    let mut root: Element = grid(rows).rows([GridLength::Auto, GridLength::Auto, GridLength::STAR]).into();
    for accel in accelerators {
        root = root.keyboard_accelerator(accel);
    }
    root
}

/// Allocates a fresh page id, inserts an empty `ReactorWebViewEngine` (its
/// `WebView` is filled in later by `page_element`'s `on_ready`), unloads
/// whatever `PageManager::insert` evicted to make room, and makes it active
/// — the shared core of both the first-render bootstrap and the "+"/add-tile
/// actions.
fn do_add_page(
    core: &HookRef<PageManager<ReactorWebViewEngine>>,
    url: &str,
    set_active_id: &SetState<String>,
    active_id_ref: &HookRef<String>,
    set_address: &SetState<String>,
) {
    let id = core.borrow_mut().allocate_id();
    let engine = ReactorWebViewEngine::new();
    let title = Rc::new(RefCell::new(String::new()));
    let evicted = core.borrow_mut().insert(id.clone(), engine, title);
    for evicted_id in evicted {
        core.borrow_mut().take_engine(&evicted_id);
    }
    *active_id_ref.borrow_mut() = id.clone();
    set_active_id.call(id);
    set_address.call(url.to_string());
}

/// Reconstructs a page's engine if it was unloaded (see this module's doc
/// comment: an unloaded page's `webview(..)` element simply isn't rendered,
/// so reactor already tore down its old `WebView2` control) — mirrors
/// `browser-windows-winui`'s `ensure_engine_loaded`.
fn ensure_engine_loaded(core: &HookRef<PageManager<ReactorWebViewEngine>>, id: &str) {
    let needs_engine = core.borrow().page(id).map(|p| p.engine.is_none()).unwrap_or(false);
    if needs_engine {
        core.borrow_mut().install_engine(id, ReactorWebViewEngine::new());
    }
}

/// Builds one page's always-mounted `webview(..)` element, filling in the
/// same `Rc`s `core`'s `ReactorWebViewEngine` already owns for this page
/// (see this module's doc comment) — `RenderEngine`'s methods and this
/// element's `on_ready` end up sharing the identical cells.
fn page_element(
    id: String,
    core: &HookRef<PageManager<ReactorWebViewEngine>>,
    shared: &Rc<Shared>,
    active_id_ref: &HookRef<String>,
    set_address: &SetState<String>,
) -> Element {
    let Some((web_cell, registration_cell, title_cell, start_url)) = core.borrow().page(&id).map(|p| {
        let engine = p.engine.as_ref().expect("page_element only called for loaded pages");
        (engine.web.clone(), engine.registration.clone(), Rc::clone(&p.title), p.last_url.clone())
    }) else {
        return Element::from(vstack(())).with_key(id);
    };
    let start_url = if start_url.is_empty() { HOME_URL.to_string() } else { start_url };

    let shared = Rc::clone(shared);
    let active_id_ref = active_id_ref.clone();
    let set_address = set_address.clone();
    let id_for_ready = id.clone();

    let on_ready = move |ready: WebView| {
        trace(&format!("on_ready: page {id_for_ready} WebView2 ready"));
        let reflect = {
            let ready = ready.clone();
            let set_address = set_address.clone();
            let active_id_ref = active_id_ref.clone();
            let id = id_for_ready.clone();
            let shared = Rc::clone(&shared);
            let title_cell = Rc::clone(&title_cell);
            move |_args| {
                let source = ready.source();
                *title_cell.borrow_mut() = ready.document_title();
                if !source.is_empty() {
                    if let Err(err) = shared.history.record_visit(&source, &ready.document_title()) {
                        eprintln!("failed to record history visit: {err}");
                    }
                }
                if *active_id_ref.borrow() == id && !source.is_empty() {
                    set_address.call(source);
                }
            }
        };
        if let Ok(registration) = ready.on_navigation_completed(reflect) {
            *registration_cell.borrow_mut() = Some(registration);
        }
        let _ = ready.navigate(&start_url);
        *web_cell.borrow_mut() = Some(ready);
    };

    Element::from(webview(on_ready)).with_key(id)
}

/// The search-box-plus-tile-grid overlay, matching `browser-windows-winui`'s
/// `rebuild_switcher_grid`: open pages first (filtered by the search query,
/// via `PageManager::matching_ids`), a trailing add-page tile, then history
/// matches (only once there's a query, and only for URLs not already open).
/// Uses reactor's native `grid_view` (wrapping tile layout, handled by the
/// control itself) rather than `winio-winui3`'s fixed-column-count
/// workaround (that crate has no working `SizeChanged` event to react to
/// the real window width with — see `browser-windows-winui`'s doc comment).
#[allow(clippy::too_many_arguments)]
fn switcher_overlay(
    core: &HookRef<PageManager<ReactorWebViewEngine>>,
    shared: &Rc<Shared>,
    search_query: &str,
    set_search_query: SetState<String>,
    switch_to: Callback<String>,
    add_page_and_switch: Callback<String>,
    close_page: Callback<String>,
) -> Grid {
    // No bookmarks integration on this platform (see `ARCHITECTURE.md`
    // §5/Backlog) — `None` means `build_switcher_rows` simply skips that
    // source.
    let tiles = browser_chrome_core::build_switcher_rows(&core.borrow(), &shared.history, None, search_query);

    let start_page = shared.settings.borrow().start_page.clone();
    let tiles_for_select = tiles.clone();
    let add_page_and_switch_for_select = add_page_and_switch.clone();
    let grid_of_tiles = grid_view(tiles, |tile, _idx| tile_element(tile))
        .with_key_selector(tile_key)
        .selected_index(-1)
        .on_selection_changed(move |idx: i32| {
            let Some(activation) = browser_chrome_core::activate_row(&tiles_for_select, idx.max(0) as usize, &start_page) else { return };
            match activation {
                browser_chrome_core::SwitcherActivation::SwitchTo(id) => switch_to.invoke(id),
                browser_chrome_core::SwitcherActivation::OpenNewPage(url) => add_page_and_switch_for_select.invoke(url),
            }
        });
    let _ = close_page; // reserved for a future close-tile control; not wired yet

    // Ctrl+Enter always opens a brand-new page from the typed text, even
    // when it matches an open page or history entry (which selecting a
    // tile would instead switch to/open) — the escape hatch for
    // deliberately wanting a second page at the same URL. Mirrors
    // `browser-linux-gtk3`'s `force_new_page_from_search`; dropped
    // (silently, not as a deliberate scope cut) when this crate was first
    // built out — see `ARCHITECTURE.md` §3.3. Note this search box has no
    // plain-Enter behavior at all (only click/selection on a tile) — a
    // real, separate gap from `browser-linux-gtk3`'s unified address-bar/
    // search-box design that ARCHITECTURE.md understated; Ctrl+Enter is
    // the one specific behavior being restored here.
    let force_new_page_from_search = {
        let query = search_query.to_string();
        let shared = Rc::clone(shared);
        let add_page_and_switch = add_page_and_switch.clone();
        move || {
            let trimmed = query.trim();
            if trimmed.is_empty() {
                return;
            }
            let url = resolve_address_input(trimmed, &shared.settings.borrow());
            add_page_and_switch.invoke(url);
        }
    };

    let search_box = text_box(search_query.to_string())
        .placeholder_text("Type to filter open pages\u{2026}")
        .on_text_changed(set_search_query)
        .width(400.0)
        .keyboard_accelerator(KeyboardAccelerator::new(VirtualKey::Enter, VirtualKeyModifiers::Control, force_new_page_from_search));

    grid((
        Element::from(search_box).grid_row(0),
        Element::from(grid_of_tiles).grid_row(1),
    ))
    .rows([GridLength::Auto, GridLength::STAR])
    .margin(Thickness::uniform(16.0))
}

fn tile_key(tile: &browser_chrome_core::SwitcherRow) -> String {
    use browser_chrome_core::SwitcherRow;
    match tile {
        SwitcherRow::Open { id, .. } => format!("open:{id}"),
        SwitcherRow::Add => "add".to_string(),
        SwitcherRow::History { url, .. } => format!("history:{url}"),
        // Not reachable today (this platform never passes `Some(bookmarks)`
        // to `build_switcher_rows` — see `switcher_overlay` — so these two
        // variants are never actually produced), but handled anyway so this
        // match stays exhaustive if that changes.
        SwitcherRow::Bookmark { url, .. } => format!("bookmark:{url}"),
        SwitcherRow::Similar { url, .. } => format!("similar:{url}"),
    }
}

fn tile_element(tile: &browser_chrome_core::SwitcherRow) -> Element {
    use browser_chrome_core::SwitcherRow;
    let (title, domain) = match tile {
        SwitcherRow::Open { title, domain, .. } => (title.clone(), domain.clone()),
        SwitcherRow::Add => ("+".to_string(), String::new()),
        SwitcherRow::History { title, domain, .. }
        | SwitcherRow::Bookmark { title, domain, .. }
        | SwitcherRow::Similar { title, domain, .. } => (title.clone(), domain.clone()),
    };
    vstack((
        Element::from(text_block(title).bold()),
        Element::from(text_block(domain)),
    ))
    .width(150.0)
    .height(110.0)
    .padding(Thickness::uniform(10.0))
    .into()
}

/// Mirrors `browser-windows-winui`'s settings overlay: start page, search
/// engine, loaded-page limit. Draft values (the panel's own local reactor
/// state) rather than editing `shared.settings` directly, so Cancel really
/// discards — the same reasoning as `winio-winui3`'s copy-into-textboxes
/// approach in `open_settings`, just via state instead of imperative
/// `SetText` calls.
#[allow(clippy::too_many_arguments)]
fn settings_overlay(
    shared: &Rc<Shared>,
    start_page: &str,
    set_start_page: SetState<String>,
    engine_index: i32,
    set_engine_index: SetState<i32>,
    unlimited: bool,
    set_unlimited: SetState<bool>,
    limit_text: &str,
    set_limit_text: SetState<String>,
    on_save: Callback<()>,
    on_cancel: Callback<()>,
    keybindings: &HookRef<Keybindings>,
    listening_for: Option<Action>,
    set_listening_for: SetState<Option<Action>>,
    new_binding_text: &str,
    set_new_binding_text: SetState<String>,
    bump: &Callback<()>,
) -> Element {
    let engine_names: Vec<String> = shared.settings.borrow().search_engines.iter().map(|e| e.name.clone()).collect();
    let keybindings_section = keybindings_section(
        shared,
        keybindings,
        listening_for,
        set_listening_for,
        new_binding_text,
        set_new_binding_text,
        bump,
    );
    vstack((
        Element::from(text_block("Start page")),
        Element::from(text_box(start_page.to_string()).on_text_changed(set_start_page)),
        Element::from(text_block("Search engine")),
        Element::from(ComboBox::new(engine_names).selected_index(engine_index).on_selection_changed(set_engine_index)),
        Element::from(check_box(unlimited).content("Unlimited loaded pages").on_checked(set_unlimited)),
        Element::from(text_block("Loaded pages limit")),
        Element::from(text_box(limit_text.to_string()).enabled(!unlimited).on_text_changed(set_limit_text)),
        keybindings_section,
        Element::from(
            hstack((
                Element::from(button("Cancel").on_click(move || on_cancel.invoke(()))),
                Element::from(button("Save").on_click(move || on_save.invoke(()))),
            ))
            .spacing(8.0),
        ),
    ))
    .spacing(8.0)
    .width(480.0)
    .margin(Thickness::uniform(16.0))
    .into()
}

/// Mirrors `browser-windows-winui`'s profile picker: existing profiles
/// (rebuilt from `list_profile_names()` fresh every time this overlay
/// renders — cheap, and picks up a profile created in an earlier visit),
/// the current one marked and closing the picker instead of launching a
/// duplicate process of itself, plus a field to create a new one.
fn profile_overlay(
    shared: &Rc<Shared>,
    new_profile_text: &str,
    set_new_profile_text: SetState<String>,
    on_create: Callback<()>,
    on_cancel: Callback<()>,
) -> Element {
    let current_profile = shared.profile.name.clone();
    let rows: Vec<Element> = list_profile_names()
        .into_iter()
        .map(|name| {
            let is_current = name == current_profile;
            let label = if is_current { format!("{name} (current)") } else { name.clone() };
            let on_cancel = on_cancel.clone();
            Element::from(button(label).on_click(move || {
                if is_current {
                    on_cancel.invoke(());
                } else if let Err(err) = launch_new_profile_process(&name) {
                    eprintln!("failed to launch a new process for profile {name:?}: {err}");
                } else {
                    on_cancel.invoke(());
                }
            }))
        })
        .collect();

    vstack((
        Element::from(text_block("Profiles").bold()),
        Element::from(vstack(rows).spacing(4.0)),
        Element::from(text_box(new_profile_text.to_string()).placeholder_text("New profile name\u{2026}").on_text_changed(set_new_profile_text)),
        Element::from(
            hstack((
                Element::from(button("Cancel").on_click({
                    let on_cancel = on_cancel.clone();
                    move || on_cancel.invoke(())
                })),
                Element::from(button("Create & Open").on_click(move || on_create.invoke(()))),
            ))
            .spacing(8.0),
        ),
    ))
    .spacing(8.0)
    .width(360.0)
    .margin(Thickness::uniform(16.0))
    .into()
}

/// Mirrors `browser-windows-winui`'s keybindings editor, folded into the
/// settings overlay as its own section (per explicit user feedback — this
/// front end originally had it behind a separate toolbar button/overlay,
/// matching `browser-windows-winui`'s layout, but that's a worse fit here):
/// one row per `Action::ALL`, each showing its label, current chords as
/// removable tags, and either an "Add binding" button or (while
/// `listening_for == Some(action)`) a text entry to type the new chord in
/// `"Ctrl+Shift+P"` format — see `shortcuts::parse_chord`'s doc comment for
/// why text entry rather than live key capture. Returns just the rows
/// (no title/wrapper — `settings_overlay` supplies those as part of its own
/// single panel).
fn keybindings_section(
    shared: &Rc<Shared>,
    keybindings: &HookRef<Keybindings>,
    listening_for: Option<Action>,
    set_listening_for: SetState<Option<Action>>,
    new_binding_text: &str,
    set_new_binding_text: SetState<String>,
    bump: &Callback<()>,
) -> Element {
    let mut rows: Vec<Element> = Vec::new();
    for &action in Action::ALL {
        let chords = keybindings.borrow().bindings_for(action).to_vec();
        let mut row: Vec<Element> = vec![Element::from(text_block(action.label()).width(200.0))];
        for chord in chords {
            let keybindings = keybindings.clone();
            let shared = Rc::clone(shared);
            let bump = bump.clone();
            let chord_for_click = chord.clone();
            row.push(Element::from(button(format!("{chord} \u{d7}")).on_click(move || {
                let mut remaining = keybindings.borrow().bindings_for(action).to_vec();
                remaining.retain(|c| c != &chord_for_click);
                keybindings.borrow_mut().set_bindings(action, remaining);
                if let Err(err) = keybindings.borrow().save(&shared.profile) {
                    eprintln!("failed to save keybindings: {err}");
                }
                bump.invoke(());
            })));
        }
        if listening_for == Some(action) {
            let commit: Callback<()> = Callback::new({
                let keybindings = keybindings.clone();
                let shared = Rc::clone(shared);
                let bump = bump.clone();
                let set_listening_for = set_listening_for.clone();
                let set_new_binding_text = set_new_binding_text.clone();
                let new_binding_text = new_binding_text.to_string();
                move |()| {
                    if let Some(chord) = shortcuts::parse_chord(&new_binding_text) {
                        let mut chords = keybindings.borrow().bindings_for(action).to_vec();
                        if !chords.contains(&chord) {
                            chords.push(chord);
                        }
                        keybindings.borrow_mut().set_bindings(action, chords);
                        if let Err(err) = keybindings.borrow().save(&shared.profile) {
                            eprintln!("failed to save keybindings: {err}");
                        }
                    }
                    set_listening_for.call(None);
                    set_new_binding_text.call(String::new());
                    bump.invoke(());
                }
            });
            row.push(Element::from(
                text_box(new_binding_text.to_string())
                    .placeholder_text("e.g. Ctrl+Shift+P")
                    .on_text_changed(set_new_binding_text.clone())
                    .keyboard_accelerator(KeyboardAccelerator::new(VirtualKey::Enter, VirtualKeyModifiers::None, {
                        let commit = commit.clone();
                        move || commit.invoke(())
                    })),
            ));
            row.push(Element::from(button("OK").on_click(move || commit.invoke(()))));
        } else {
            let set_listening_for = set_listening_for.clone();
            let set_new_binding_text = set_new_binding_text.clone();
            row.push(Element::from(button("Add binding").on_click(move || {
                set_new_binding_text.call(String::new());
                set_listening_for.call(Some(action));
            })));
        }
        rows.push(Element::from(hstack(row).spacing(4.0)));
    }

    vstack((
        Element::from(text_block("Keybindings").bold()),
        Element::from(vstack(rows).spacing(4.0)),
    ))
    .spacing(8.0)
    .into()
}

/// Runs the app — called from `main.rs` after `bootstrap()`. Blocks until
/// the window closes (reactor's own message loop; see `App::render`'s doc
/// comment upstream).
pub fn run(profile: Profile) -> anyhow::Result<()> {
    let settings = Settings::load(&profile);
    let history = HistoryStore::open(&profile)?;
    let shared = Rc::new(Shared { history, settings: RefCell::new(settings), profile });
    // `App::render` requires `Send`, even though this whole app is
    // single-threaded (one STA UI thread — see reactor's own "Threading"
    // docs) — same situation `render_engine::AssertSend` already exists for
    // (used throughout `browser-windows-winui` for winio-winui3's WinRT
    // delegate constructors), so reused here rather than duplicated.
    let shared = render_engine::AssertSend(shared);
    App::new().title("claude-browser").render(move |cx| {
        let shared = &shared;
        app(cx, &shared.0)
    })?;
    Ok(())
}

/// Shows a small standalone window for launching with a URL argument (e.g.
/// from the OS's "open with"/default-browser handoff) — lets the user
/// confirm/pick which profile to open it in before the real browser window
/// appears. `default_profile` pre-fills the field (whatever `--profile`
/// resolved to, or `"default"`). See this module's doc comment for why
/// "Open" spawns a new process rather than swapping in the real browser
/// window in place.
pub fn run_chooser(url: String, default_profile: String) -> anyhow::Result<()> {
    App::new()
        .title("Open link")
        .inner_size(480.0, 240.0)
        .render(move |cx| chooser_app(cx, &url, &default_profile))?;
    Ok(())
}

fn chooser_app(cx: &mut RenderCx, url: &str, default_profile: &str) -> Element {
    let (profile_name, set_profile_name) = cx.use_state(default_profile.to_string());

    let open: Callback<()> = Callback::new({
        let profile_name = profile_name.clone();
        let url = url.to_string();
        move |()| {
            if let Ok(exe) = std::env::current_exe() {
                if let Err(err) = std::process::Command::new(exe).arg("--profile").arg(&profile_name).arg(&url).spawn() {
                    eprintln!("failed to launch the browser process: {err}");
                }
            }
            std::process::exit(0);
        }
    });
    let cancel: Callback<()> = Callback::new(|()| std::process::exit(0));

    let suggestions: Vec<Element> = list_profile_names()
        .into_iter()
        .map(|name| {
            let set_profile_name = set_profile_name.clone();
            Element::from(button(name.clone()).on_click(move || set_profile_name.call(name.clone())))
        })
        .collect();

    vstack((
        Element::from(text_block(url.to_string())),
        Element::from(text_block("Open in profile")),
        Element::from(text_box(profile_name).on_text_changed(set_profile_name)),
        Element::from(hstack(suggestions).spacing(4.0)),
        Element::from(
            hstack((
                Element::from(button("Cancel").on_click(move || cancel.invoke(()))),
                Element::from(button("Open").on_click(move || open.invoke(()))),
            ))
            .spacing(8.0),
        ),
    ))
    .spacing(12.0)
    .margin(Thickness::uniform(16.0))
    .into()
}
