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
//! `windows-reactor`'s own declarative `Element`/`ElementExt` has no
//! `Visibility` modifier at all (checked by reading `crates/libs/reactor/
//! src/element.rs`/`widget.rs` — a real gap, same category as
//! `winio-winui3`'s missing `KeyDown`, just in a different place).
//!
//! Two earlier versions of this module got the mounting story wrong before
//! landing here, both confirmed by direct real-VM testing (temporary
//! tracing added to the vendored `windows-webview` crate in one case):
//!
//! 1. Every *loaded* page's `webview(..)` element mounted simultaneously,
//!    each `.with_key(id)`, stacked in one grid cell with the active one
//!    placed last so it paints on top. This doesn't work, and not for a
//!    z-order/compositing reason: `ElementExt::with_key`'s match (checked
//!    directly in the vendored source) has no arm for `WebView2` (or
//!    `SwapChainPanel`), so `.with_key(id)` on one is silently a no-op —
//!    `has_keys` is always false for `page_elements`, so the reconciler
//!    falls back to *positional* reconciliation, and since `WebView2` also
//!    carries no reactive props (`bindings()` returns an empty `Vec`), a
//!    positionally-matched same-kind webview is just left alone by
//!    `update_widget`. Clicking a different switcher tile updated
//!    `active_id` (and the title chip) correctly, but the *previous* page's
//!    `WebView2` never got an `on_unmounted`/`on_mounted` cycle, so its
//!    already-loaded content just stayed on screen.
//! 2. Only mounting the *active* page's element, forcing a real
//!    unmount/mount cycle by wrapping it in a plain `Grid` on every render
//!    where the active id actually changed (defeating positional matching
//!    with a genuine kind mismatch instead of a key). This *worked*, but at
//!    a real cost this module doesn't have to pay: switching away tore the
//!    previous page's `WebView2` down entirely, so switching back re-created
//!    it and re-navigated from scratch instead of resuming an already-live
//!    session — losing scroll position, form input, and JS state on every
//!    switch.
//!
//! What actually gets both right: `Microsoft.UI.Xaml.Controls.WebView2`'s
//! own implementation (github.com/microsoft/microsoft-ui-xaml,
//! `controls/dev/WebView2/WebView2.cpp`) already listens for
//! `UIElement::VisibilityProperty()` changes and forwards them to the
//! underlying `ICoreWebView2Controller::IsVisible` for us — the same real
//! hide/show primitive `browser-linux-gtk3`'s `gtk::Stack` and
//! `browser-macos-appkit`'s hidden `NSView` already give those two front
//! ends (confirmed by reading that source directly; it explicitly does
//! *not* listen for `Opacity`, which is why the opacity approach tried
//! first — see git history — had no visible effect). `windows-reactor` just
//! doesn't expose `Visibility` through its declarative API. `xaml_interop.rs`
//! reaches past that with a small, narrowly-scoped hand-written COM shim
//! (the real `IUIElement` interface, same IID and vtable layout as the one
//! already correctly generated — but `pub(crate)`-scoped — inside
//! `windows-reactor`'s own vendored `bindings.rs`) so `page_element` can
//! call the real `SetVisibility` directly.
//!
//! So: every *loaded* page's `webview(..)` element stays mounted
//! simultaneously (`page_ids()`'s natural creation order, kept stable
//! across renders — never reordered active-last, since `WebView2` still
//! can't be meaningfully `.with_key()`d, so the reconciler still matches
//! `page_elements` by position and a reorder would read as "the page at
//! this position changed"). Each page's `on_mounted`/`on_unmounted` (from
//! `windows_webview::webview()`, wrapped rather than replaced — see
//! `page_element`) captures the raw native handle into
//! `ReactorWebViewEngine::xaml_handle`, and every render re-applies
//! `xaml_interop::set_visible` to it based on whether that page is
//! currently active — real show/hide, not mount/unmount, so in-page state
//! survives switching away and back. An *unloaded* page (evicted by
//! `max_loaded_pages`) still isn't rendered at all — reactor's own
//! reconciler tears down its `WebView2` control when its element stops
//! appearing, no manual `.close()` call needed the way `WebView2Engine`
//! requires.
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
// Field/method names below mirror the real WinRT API names (PascalCase) —
// same convention, same suppression, as `windows-reactor`'s own vendored
// `bindings.rs` (see its `lib.rs`).
#[allow(non_snake_case, non_upper_case_globals, dead_code)]
mod xaml_interop;

use std::cell::RefCell;
use std::rc::Rc;

use browser_core::{
    launch_new_profile_process, list_profile_names, resolve_address_input, Action, HistoryStore, KeyChord, Keybindings,
    PageManager, Profile, Session, SessionPage, Settings, APP_TITLE, HOME_URL,
};
use engine::ReactorWebViewEngine;
use windows_reactor::*;
use windows_webview::{webview, NewWindowRequestedArgs, WebView};

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
    // Backs the real fix for the switcher search box's plain-Enter gap (see
    // `switcher_overlay`'s `activate_search` and `xaml_interop`'s module
    // doc comment): `enter_subscription` holds the one `PreviewKeyDown`
    // subscription for the app's lifetime (sets itself up lazily below,
    // since the native root content element isn't available on the very
    // first render); `enter_action` holds whatever that subscription
    // should currently do with a plain Enter, refreshed every render —
    // `Some` only while the switcher overlay's search box actually has
    // something to act on, `None` the rest of the time so Enter elsewhere
    // in the app (Settings, Profile) is left alone. The actual subscribe
    // call is further down, once `bump` exists (see there for why).
    let enter_action: HookRef<Option<Rc<dyn Fn() -> bool>>> = cx.use_ref(None);
    let enter_subscription: HookRef<Option<windows_core::EventRevoker>> = cx.use_ref(None);
    *enter_action.borrow_mut() = None;
    let (overlay, set_overlay) = cx.use_state(Overlay::None);
    let (search_query, set_search_query) = cx.use_state(String::new());
    // WinUI/XAML has no CSS `:hover` — this is the reactor-idiomatic
    // equivalent for the toolbar's title chip's hover-looks-like-an-input
    // state (see the `toolbar` grid below), same reactive-state pattern as
    // every other piece of UI state in this file.
    let (hovering, set_hovering) = cx.use_state(false);

    let (start_page_draft, set_start_page_draft) = cx.use_state(String::new());
    let (engine_index_draft, set_engine_index_draft) = cx.use_state(-1i32);
    let (unlimited_draft, set_unlimited_draft) = cx.use_state(true);
    let (limit_draft, set_limit_draft) = cx.use_state(String::new());

    let (new_profile_draft, set_new_profile_draft) = cx.use_state(String::new());

    let keybindings = cx.use_ref(Keybindings::load(&shared.profile));
    let (listening_for, set_listening_for) = cx.use_state(Option::<Action>::None);
    let (new_binding_text, set_new_binding_text) = cx.use_state(String::new());

    // Closures shared across multiple event handlers/`switcher_overlay` are
    // wrapped in reactor's own `Callback<T>` (an `Rc<dyn Fn(T)>` newtype) —
    // plain closures aren't `Clone` even when every captured variable is,
    // so a closure needed in more than one place has to go through this
    // (or an equivalent manual `Rc<dyn Fn>` wrapper) to be cloned at all.
    let bump: Callback<()> = Callback::new({
        let set_generation = set_generation.clone();
        move |()| set_generation.call(generation.wrapping_add(1))
    });

    // `PreviewKeyDown` is a real native XAML event, not one of reactor's own
    // wired-up `Element` events (see `xaml_interop`'s module doc comment) —
    // it fires on the UI thread same as any other input event (confirmed:
    // interleaves correctly with reactor's own renders in `trace()` output),
    // but calling reactor's `SetState`/`Callback` setters from inside it
    // doesn't on its own get picked up: reactor's normal event handlers all
    // go through some shared internal wrapper that checks for dirty state
    // and schedules a render afterward, and this callback — subscribed via
    // raw interop, entirely outside reactor's own `Element`/event system —
    // never passes through that wrapper. Confirmed directly: `trace()`
    // showed `activate_search` correctly running and returning `true` on a
    // real plain-Enter press, but no render followed until some unrelated,
    // genuinely reactor-dispatched event came along. `bump.invoke(())`
    // (already the established fix for "state changed outside reactor's
    // own dispatch" elsewhere in this file) closes that gap here too.
    if enter_subscription.borrow().is_none() {
        match xaml_interop::root_content() {
            Some(root) => {
                let enter_action = enter_action.clone();
                let bump = bump.clone();
                let on_plain_enter = move || {
                    let handled = enter_action.borrow().as_ref().is_some_and(|action| action());
                    if handled {
                        bump.invoke(());
                    }
                    handled
                };
                if let Ok(revoker) = xaml_interop::intercept_plain_enter(&root, on_plain_enter) {
                    *enter_subscription.borrow_mut() = Some(revoker);
                }
            }
            None => {}
        }
    }

    // One-time setup for the toolbar-in-the-title-bar passthrough region
    // (see `title_bar`'s doc comment below and `xaml_interop::
    // setup_titlebar_passthrough`) — same lazy-retry-until-it-succeeds
    // shape as `enter_subscription` above, since the native root content
    // element isn't available on the very first render either. Unlike
    // `enter_action`, there's nothing to refresh every render here: the
    // subscription itself reapplies the region on every `SizeChanged`.
    let titlebar_passthrough_subscription: HookRef<Option<windows_core::EventRevoker>> = cx.use_ref(None);
    if titlebar_passthrough_subscription.borrow().is_none()
        && let Some(revoker) = xaml_interop::setup_titlebar_passthrough()
    {
        *titlebar_passthrough_subscription.borrow_mut() = Some(revoker);
    }

    let switch_to: Callback<String> = Callback::new({
        let core = core.clone();
        let set_active_id = set_active_id.clone();
        let active_id_ref = active_id_ref.clone();
        let set_overlay = set_overlay.clone();
        let bump = bump.clone();
        move |id: String| {
            ensure_engine_loaded(&core, &id);
            core.borrow_mut().set_active(&id);
            *active_id_ref.borrow_mut() = id.clone();
            set_active_id.call(id.clone());
            set_overlay.call(Overlay::None);
            bump.invoke(());
        }
    });

    // Bootstrap: open either the saved session's pages or the start page,
    // on the very first render (core starts empty — there's no separate
    // "startup" hook, so this just runs in-line, same render pass; placed
    // after `switch_to`'s own definition above, not before it like the
    // single-URL version this replaced, since restoring which page was
    // active needs to call it). Individual `do_add_page` calls have no
    // failure mode to report (unlike the other three frontends' `add_page`,
    // this one returns nothing — see its own doc comment), so there's
    // nothing to skip-and-log here.
    if core.borrow().is_empty() {
        let session = Session::load(&shared.profile);
        let start_page = shared.settings.borrow().start_page.clone();
        let plan = browser_chrome_core::resolve_restore_plan(&session, &start_page);
        for url in &plan.urls {
            do_add_page(&core, url, &set_active_id, &active_id_ref);
        }
        // `do_add_page` makes each newly-added page active in turn, so
        // without this the *last* URL in `plan.urls` would end up active
        // regardless of which one was actually active when the session was
        // saved. The id is copied out of `core`'s borrow into its own
        // statement before calling `switch_to` (which needs its own
        // borrow) rather than held across it.
        let active_page_id = plan.active_index.and_then(|idx| core.borrow().pages().get(idx).map(|p| p.id.clone()));
        if let Some(id) = active_page_id {
            switch_to.invoke(id);
        }
    }

    let add_page_and_switch: Callback<String> = Callback::new({
        let core = core.clone();
        let set_active_id = set_active_id.clone();
        let active_id_ref = active_id_ref.clone();
        let set_overlay = set_overlay.clone();
        let bump = bump.clone();
        move |url: String| {
            do_add_page(&core, &url, &set_active_id, &active_id_ref);
            set_overlay.call(Overlay::None);
            bump.invoke(());
        }
    });

    let add_page_background: Callback<String> = Callback::new({
        let core = core.clone();
        let bump = bump.clone();
        move |url: String| {
            do_add_page_background(&core, &url, &bump);
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

    // Opens the switcher preloaded with the active page's current URL —
    // the toolbar title chip's click target, "grid, to edit the URL",
    // mirroring `browser-linux-gtk3`'s `open_switcher_editing_url`. Real,
    // honest gap versus gtk3/macOS: it can't also focus+select-all the
    // text (this crate's existing, already-documented limitation — no
    // `Focus()`-style API on `TextBox`/`Element`, see `dispatch_action`'s
    // own doc comment on `EditUrl`) — presetting the content is real and
    // works, just not the focus/select part.
    let open_switcher_editing_url: Callback<()> = Callback::new({
        let core = core.clone();
        let active_id = active_id.clone();
        let set_overlay = set_overlay.clone();
        let set_search_query = set_search_query.clone();
        move |()| {
            let url = core.borrow().page(&active_id).map(|p| p.current_url()).unwrap_or_default();
            set_search_query.call(url);
            set_overlay.call(Overlay::Switcher);
        }
    });
    // The title chip's click handler — see `browser-linux-gtk3`'s
    // `title_chip_clicked` for why it's guarded on `overlay != Overlay::
    // Switcher`: the toolbar stays clickable even while the switcher is
    // open (it only covers the content area below the toolbar), and
    // re-clicking while it's already open must not clobber whatever the
    // user already typed.
    let title_chip_clicked = {
        let open_switcher_editing_url = open_switcher_editing_url.clone();
        move || {
            if overlay != Overlay::Switcher {
                open_switcher_editing_url.invoke(());
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
    let toggle_settings = {
        let set_overlay = set_overlay.clone();
        let open_settings = open_settings.clone();
        move || {
            if overlay == Overlay::Settings {
                set_overlay.call(Overlay::None);
            } else {
                open_settings.invoke(());
            }
        }
    };

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
    let toggle_profile = {
        let set_overlay = set_overlay.clone();
        let open_profile = open_profile.clone();
        move || {
            if overlay == Overlay::Profile {
                set_overlay.call(Overlay::None);
            } else {
                open_profile.invoke(());
            }
        }
    };

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
        let shared = Rc::clone(shared);
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
                // `windows-reactor`'s `Window::Closed`/`Close` are
                // `pub(crate)` — confirmed by direct compile error, not
                // assumed — so unlike the other three front ends, this one
                // has no way to trigger a real window close (or intercept
                // one) from outside the crate at all. Saves synchronously
                // and exits directly instead — mirrors `run_chooser`'s own
                // `std::process::exit(0)` elsewhere in this crate for the
                // same "no other way to end this process" reason. One real,
                // honest gap versus the other three front ends: the *OS*
                // close button (the window chrome's own X) has no save hook
                // reachable from this crate either, so only this Ctrl+Q
                // path actually saves a session on this front end — see
                // ROADMAP.md.
                Action::Quit => {
                    save_session(&core, &shared.profile);
                    std::process::exit(0);
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
        {
            // A clickable "title chip", not a text box — shows the active
            // page's title, not editable; clicking it opens the switcher in
            // URL-editing mode (`open_switcher_editing_url`). The real
            // editable text entry now lives entirely inside the switcher
            // overlay — see `switcher_overlay`'s `search_box`.
            let active_title = core
                .borrow()
                .page(&active_id)
                .map(|p| p.title.borrow().clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "New Page".to_string());
            Element::from(
                border(Element::from(text_block(active_title).semibold()))
                    .corner_radius(6.0)
                    .border_thickness(Thickness::uniform(1.0))
                    .border_brush(ThemeRef::ControlStroke)
                    .background(if hovering { ThemeRef::ControlFillInputActive } else { ThemeRef::ControlFillTertiary })
                    .padding(Thickness { left: 12.0, right: 12.0, top: 6.0, bottom: 6.0 }),
            )
            .horizontal_alignment(HorizontalAlignment::Stretch)
            .on_tapped(title_chip_clicked)
            .on_pointer_entered({
                let set_hovering = set_hovering.clone();
                move |_| set_hovering.call(true)
            })
            .on_pointer_exited({
                let set_hovering = set_hovering.clone();
                move || set_hovering.call(false)
            })
        }
        .grid_column(3),
        Element::from(button("\u{229e}").on_click(toggle_switcher)).grid_column(4),
        Element::from(button("\u{2699}").on_click(toggle_settings)).grid_column(5),
        Element::from(button("\u{1f464}").on_click(toggle_profile)).grid_column(6),
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
    // The toolbar now lives in `TitleBar`'s `.content()` slot, merging it
    // into the draggable title bar area like a real browser (tabs/controls
    // sitting in the same strip as the native minimize/maximize/close
    // buttons) instead of as an ordinary row below a mostly-empty title bar.
    // An earlier version of this code tried this and reverted it: real
    // testing found every click on that content silently did nothing,
    // because `windows-reactor`'s `host.rs` wires whatever's in `.content()`
    // up via `Window.SetTitleBar(element)`, which marks that whole element
    // as the draggable caption region — and putting interactive controls
    // inside a custom title bar needs separately registering non-client
    // hit-test passthrough rectangles (`InputNonClientPointerSource.
    // SetRegionRects`) so clicks still reach them, which `windows-reactor`
    // doesn't do anywhere in its own source (checked directly). That's now
    // handled directly: `xaml_interop::setup_titlebar_passthrough` (raw
    // interop, same category of fix as `intercept_plain_enter`) marks the
    // toolbar's row `Passthrough`, reapplied on every resize — with left
    // and right margins reserved so the window stays draggable and the
    // system's own caption buttons keep working (both found broken by real
    // testing when the passthrough region was too generous — see that
    // function's doc comment for the details and why it still doesn't need
    // to track the toolbar's exact bounds).
    let title_bar = Element::from(TitleBar::new(APP_TITLE).content(toolbar));

    // Every *loaded* page's webview stays mounted (see this module's doc
    // comment on why) — `page_ids()`'s natural (creation) order is kept
    // stable across renders, never reordered active-last: `WebView2` can't
    // be `.with_key()`d (see the doc comment), so the reconciler matches
    // `page_elements` by position, and reordering would make it treat an
    // ordinary switch as "the page at this position changed" instead.
    // Visibility (see `page_element`'s use of `xaml_interop::set_visible`),
    // not mounting/unmounting, is what actually shows the active one.
    let page_ids = core.borrow().page_ids();
    let mut page_elements: Vec<Element> = Vec::with_capacity(page_ids.len());
    for id in &page_ids {
        if core.borrow().is_page_loaded(id) {
            let is_active = *id == active_id;
            page_elements.push(page_element(id.clone(), &core, &shared, &active_id_ref, &bump, &add_page_background, is_active));
        }
    }
    let content = grid(page_elements);

    let overlay_element: Option<Element> = match overlay {
        Overlay::None => None,
        Overlay::Switcher => Some(overlay_chrome(
            switcher_overlay(
                &core,
                &shared,
                &search_query,
                set_search_query.clone(),
                switch_to.clone(),
                add_page_and_switch.clone(),
                close_page.clone(),
                &enter_action,
            ),
            close_any_overlay.clone(),
        )),
        Overlay::Settings => Some(overlay_chrome(
            settings_overlay(
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
            ),
            close_any_overlay.clone(),
        )),
        Overlay::Profile => Some(overlay_chrome(
            profile_overlay(
                &shared,
                &new_profile_draft,
                set_new_profile_draft.clone(),
                create_and_open_profile.clone(),
                close_any_overlay.clone(),
            ),
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
    let mut rows = vec![title_bar.grid_row(0), Element::from(content).grid_row(1)];
    if let Some(overlay_element) = overlay_element {
        rows.push(overlay_element.grid_row(1));
    }
    let mut root: Element = grid(rows).rows([GridLength::Auto, GridLength::STAR]).into();
    for accel in accelerators {
        root = root.keyboard_accelerator(accel);
    }
    root
}

/// Snapshots the currently-open pages (URL + title, in `PageManager`'s own
/// creation order) plus which one is active, and saves it — called from
/// `Action::Quit`'s dispatch arm (see its own comment for why that's the
/// *only* save point on this front end, unlike the other three).
fn save_session(core: &HookRef<PageManager<ReactorWebViewEngine>>, profile: &Profile) {
    let core = core.borrow();
    let active_id = core.active_id();
    let active_index = core.pages().iter().position(|p| p.id == active_id);
    let pages = core.pages().iter().map(|p| SessionPage { url: p.current_url(), title: p.title.borrow().clone() }).collect();
    drop(core);
    let session = Session { pages, active_index };
    if let Err(err) = session.save(profile) {
        eprintln!("failed to save session: {err}");
    }
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
) {
    let id = core.borrow_mut().allocate_id();
    let engine = ReactorWebViewEngine::new();
    let title = Rc::new(RefCell::new(String::new()));
    let evicted = core.borrow_mut().insert(id.clone(), engine, title);
    for evicted_id in evicted {
        core.borrow_mut().take_engine(&evicted_id);
    }
    // `insert` always starts a fresh page with an empty `last_url` — without
    // this, `page_element`'s `on_ready` (which reads `last_url`, not this
    // function's own `url` parameter — see its doc comment) navigates every
    // new page to `HOME_URL` regardless of what was actually requested here.
    // A real, pre-existing gap independent of session restore (both this
    // bootstrap and the "+" add-tile already pass a real `url`), just never
    // exercised until something — restore — actually needed `url` honored.
    if let Some(page) = core.borrow_mut().page_mut(&id) {
        page.last_url = url.to_string();
    }
    *active_id_ref.borrow_mut() = id.clone();
    set_active_id.call(id);
}

/// Same as `do_add_page`, but doesn't make the new page active — used for a
/// page opened via `window.open()`/`target="_blank"`/"open in new tab" (see
/// `page_element`'s `on_new_window_requested` registration), which shouldn't
/// steal focus from whatever the user's currently looking at. Still seeds
/// `last_url` for the same reason `do_add_page` does, and still calls `bump`
/// so the switcher grid picks up the new tile on next render.
fn do_add_page_background(core: &HookRef<PageManager<ReactorWebViewEngine>>, url: &str, bump: &Callback<()>) {
    let id = core.borrow_mut().allocate_id();
    let engine = ReactorWebViewEngine::new();
    let title = Rc::new(RefCell::new(String::new()));
    let evicted = core.borrow_mut().insert_background(id.clone(), engine, title);
    for evicted_id in evicted {
        core.borrow_mut().take_engine(&evicted_id);
    }
    if let Some(page) = core.borrow_mut().page_mut(&id) {
        page.last_url = url.to_string();
    }
    bump.invoke(());
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
    bump: &Callback<()>,
    add_page_background: &Callback<String>,
    is_active: bool,
) -> Element {
    let Some((web_cell, registration_cell, title_registration_cell, title_cell, xaml_handle_cell, new_window_registration_cell, start_url)) =
        core.borrow().page(&id).map(|p| {
            let engine = p.engine.as_ref().expect("page_element only called for loaded pages");
            (
                engine.web.clone(),
                engine.registration.clone(),
                engine.title_registration.clone(),
                Rc::clone(&p.title),
                engine.xaml_handle.clone(),
                engine.new_window_registration.clone(),
                p.last_url.clone(),
            )
        })
    else {
        return Element::from(vstack(())).with_key(id);
    };
    let start_url = if start_url.is_empty() { HOME_URL.to_string() } else { start_url };

    // The widget itself has no per-render update hook (`bindings()` is
    // empty — see this module's doc comment), so a page that's already
    // mounted from an earlier render needs its visibility re-applied here,
    // directly, every time `is_active` might have changed — `on_mounted`
    // below only fires once, at first mount.
    if let Some(handle) = xaml_handle_cell.borrow().as_ref() {
        xaml_interop::set_visible(handle, is_active);
    }

    let core = core.clone();
    let shared = Rc::clone(shared);
    let active_id_ref = active_id_ref.clone();
    let bump = bump.clone();
    let add_page_background = add_page_background.clone();
    let id_for_ready = id.clone();

    let on_ready = move |ready: WebView| {
        trace(&format!("on_ready: page {id_for_ready} WebView2 ready"));
        let reflect = {
            let ready = ready.clone();
            let bump = bump.clone();
            let active_id_ref = active_id_ref.clone();
            let id = id_for_ready.clone();
            let core = core.clone();
            let shared = Rc::clone(&shared);
            let title_cell = Rc::clone(&title_cell);
            move |_args| {
                let source = ready.source();
                *title_cell.borrow_mut() = ready.document_title();
                if !source.is_empty() {
                    if let Err(err) = shared.history.record_visit(&source, &ready.document_title()) {
                        eprintln!("failed to record history visit: {err}");
                    }
                    // Kept live (not just frozen at unload/eviction time, the
                    // only other place this is written — see `do_add_page`'s
                    // comment) because `page_element` now only mounts the
                    // *active* page's `WebView2` (see `app`'s render
                    // function): switching away tears this control down, so
                    // switching back rebuilds a fresh one from `last_url` —
                    // without updating it here, that would reload whatever
                    // URL the page was created with, not the one the user
                    // actually last navigated to.
                    if let Some(page) = core.borrow_mut().page_mut(&id) {
                        page.last_url = source.clone();
                    }
                }
                // Forces a re-render so the toolbar's title chip (read
                // directly from `core` each render, see `app`'s render
                // function) picks up this page's real title once it's
                // known — only when this is the active page, matching what
                // this used to gate `set_address.call(source)` on before
                // the toolbar showed the URL instead of the title. This
                // `bump.invoke(())` likely has the same not-always-a-real-
                // render gap documented on `title_changed`'s below — kept
                // anyway since it's still correct/harmless when it *does*
                // take effect, and this closure's other job (recording the
                // visit) is unaffected either way.
                if *active_id_ref.borrow() == id && !source.is_empty() {
                    bump.invoke(());
                }
            }
        };
        if let Ok(registration) = ready.on_navigation_completed(reflect) {
            *registration_cell.borrow_mut() = Some(registration);
        }
        // Separate from `reflect`/`on_navigation_completed` above — this is
        // the semantically correct event for a title change (the same one
        // gtk3/macOS's `wry` engines already use), not just a duplicate.
        //
        // Real, pre-existing gap surfaced (not introduced) by this code,
        // confirmed by direct testing in the real VM with `trace()` logging:
        // `title_cell` gets updated correctly and this handler *does* fire
        // with the right title, but the `bump.invoke(())` below doesn't
        // reliably produce a new render on its own — the toolbar's title
        // chip stayed on "New Page" until some unrelated, genuinely
        // UI-thread-originated event (a button click, a keyboard
        // accelerator) forced the next render, which then picked up the
        // already-correct `title_cell` value. `reflect`'s own `bump.invoke`
        // above likely has the exact same gap — it was never visible before
        // because the toolbar used to show the URL, which `do_add_page`
        // already seeds correctly before the page ever finishes loading, so
        // there was nothing to visibly go stale. Root cause is very likely
        // that `WebView2`'s native event callbacks (`on_document_title_changed`/
        // `on_navigation_completed`) don't run on whatever thread/message-loop
        // tick `windows-reactor`'s own dispatch relies on to notice a state
        // update — properly fixing that means marshaling through a
        // `DispatcherQueue` (real APIs for this exist in `windows-reactor`'s
        // own `host.rs`, e.g. `DispatcherQueue::GetForCurrentThread()` +
        // `TryEnqueueWithPriority`), which needs the UI thread's queue
        // captured at startup and threaded all the way down here — a
        // bigger, riskier change than this pass had scope for. Not fixed
        // here; flagging clearly instead of silently shipping it.
        let title_changed = {
            let bump = bump.clone();
            let active_id_ref = active_id_ref.clone();
            let id = id_for_ready.clone();
            let title_cell = Rc::clone(&title_cell);
            move |new_title: String| {
                *title_cell.borrow_mut() = new_title;
                if *active_id_ref.borrow() == id {
                    bump.invoke(());
                }
            }
        };
        if let Ok(registration) = ready.on_document_title_changed(title_changed) {
            *title_registration_cell.borrow_mut() = Some(registration);
        }
        // Fires for a `target="_blank"` link click, a `window.open()` call,
        // or WebView2's own default right-click "Open link in new tab"
        // context-menu item — all three route through the same
        // `NewWindowRequested` event, so this one handler covers all of
        // them (see `ROADMAP.md`/this function's own history for why no
        // separate context-menu code exists anywhere in this codebase).
        // Always marks the request handled (never lets WebView2 create a
        // real second top-level window — there's no concept of one
        // anywhere in this app), and only actually opens a background tab
        // for `is_user_initiated()` requests: a script calling
        // `window.open()` with no real click behind it is silently
        // suppressed, matching what most real browsers do out of the box.
        let new_window_requested = {
            let add_page_background = add_page_background.clone();
            move |args: NewWindowRequestedArgs| {
                let _ = args.set_handled(true);
                if args.is_user_initiated() {
                    add_page_background.invoke(args.uri());
                }
            }
        };
        if let Ok(registration) = ready.on_new_window_requested(new_window_requested) {
            *new_window_registration_cell.borrow_mut() = Some(registration);
        }
        let _ = ready.navigate(&start_url);
        *web_cell.borrow_mut() = Some(ready);
    };

    // `windows_webview::webview()` already wires its own `.on_mounted`/
    // `.on_unmounted` (the `EnsureCoreWebView2Async`/bridging dance above,
    // via `on_ready`) — both fields are `pub`, so rather than reimplement
    // that here, this wraps the callbacks it already set: call through to
    // the original first, then additionally capture the raw XAML handle and
    // apply this page's current visibility via `xaml_interop::set_visible`.
    // See this module's doc comment for why that's the one thing reaching
    // past `windows-reactor`'s own declarative API for.
    let mut widget = webview(on_ready);
    let original_mounted = widget.mounted.take();
    let xaml_handle_for_mount = xaml_handle_cell.clone();
    widget.mounted = Some(Callback::new(move |handle: Option<windows_core::IInspectable>| {
        if let Some(cb) = &original_mounted {
            cb.invoke(handle.clone());
        }
        if let Some(h) = &handle {
            xaml_interop::set_visible(h, is_active);
        }
        *xaml_handle_for_mount.borrow_mut() = handle;
    }));
    let original_unmounted = widget.unmounted.take();
    let xaml_handle_for_unmount = xaml_handle_cell.clone();
    widget.unmounted = Some(Callback::new(move |handle: Option<windows_core::IInspectable>| {
        if let Some(cb) = &original_unmounted {
            cb.invoke(handle);
        }
        *xaml_handle_for_unmount.borrow_mut() = None;
    }));

    Element::from(widget).with_key(id)
}

/// Shared chrome for every full-screen overlay (switcher/settings/profile):
/// a dim backdrop filling the overlay's grid cell, `content` on top, and a
/// close (×) button + "Press Esc to close" hint pinned to the top-right —
/// all stacked in one implicit grid cell (no `.rows()/.columns()` on the
/// outer `grid`, the same z-by-array-position trick `app`'s own render
/// function already uses for `overlay_element.grid_row(2)` layering over
/// `content`). Matches `browser-linux-gtk3`'s `build_overlay_chrome`'s
/// rgba(20,20,18,0.88) backdrop color — a free visual-consistency touch,
/// not shared code (different UI frameworks). Caller still builds its own
/// content and passes the same `Callback<()>` it already threads through as
/// `on_cancel`/`on_close` (see `settings_overlay`/`profile_overlay`) — this
/// doesn't invent a second close path.
fn overlay_chrome(content: impl Into<Element>, on_close: Callback<()>) -> Element {
    let backdrop = Element::from(Shape::rectangle().fill(Color { a: 224, r: 20, g: 20, b: 18 }))
        .horizontal_alignment(HorizontalAlignment::Stretch)
        .vertical_alignment(VerticalAlignment::Stretch);

    let close_button = Element::from(button("\u{2715}").on_click(move || on_close.invoke(())))
        .horizontal_alignment(HorizontalAlignment::Right)
        .vertical_alignment(VerticalAlignment::Top)
        .margin(Thickness::uniform(12.0));

    let hint = Element::from(caption("Press Esc to close"))
        .horizontal_alignment(HorizontalAlignment::Right)
        .vertical_alignment(VerticalAlignment::Top)
        .margin(Thickness { top: 12.0, right: 44.0, left: 0.0, bottom: 0.0 });

    grid((backdrop, content.into(), close_button, hint)).into()
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
    enter_action: &HookRef<Option<Rc<dyn Fn() -> bool>>>,
) -> Grid {
    // No bookmarks integration on this platform (see `ARCHITECTURE.md`
    // §5/Backlog) — `None` means `build_switcher_rows` simply skips that
    // source.
    let tiles = browser_chrome_core::build_switcher_rows(&core.borrow(), &shared.history, None, search_query);

    let start_page = shared.settings.borrow().start_page.clone();
    let tiles_for_select = tiles.clone();
    let switch_to_for_select = switch_to.clone();
    let add_page_and_switch_for_select = add_page_and_switch.clone();
    let grid_of_tiles = grid_view(tiles, |tile, _idx| tile_element(tile))
        .with_key_selector(tile_key)
        .selected_index(-1)
        .on_selection_changed(move |idx: i32| {
            let Some(activation) = browser_chrome_core::activate_row(&tiles_for_select, idx.max(0) as usize, &start_page) else { return };
            match activation {
                browser_chrome_core::SwitcherActivation::SwitchTo(id) => switch_to_for_select.invoke(id),
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

    // Plain Enter — ported one-for-one from `browser-linux-gtk3`'s unified
    // `connect_activate` handler (see that crate's `lib.rs`): exactly one
    // open-page match switches to it; otherwise exactly one history match
    // opens that entry's real URL, and anything else resolves the typed
    // text as a fresh address/search. Makes typing a URL in this box
    // (preloaded via the toolbar title chip's click, in URL-editing mode)
    // and pressing Enter actually navigate, not just filter tiles.
    //
    // Not wired through a `KeyboardAccelerator` (unlike
    // `force_new_page_from_search` above) — confirmed by direct testing
    // that a plain, unmodified `VirtualKeyModifiers::None` `Enter`
    // accelerator never fires while this `TextBox` has keyboard focus
    // (the *identical* mechanism with a Ctrl modifier fires correctly from
    // the same focused box, isolating this to bare Enter specifically): a
    // focused `TextBox` consumes it as part of its own default input
    // handling before accelerators ever see it. Wired into `enter_action`
    // instead (see `app`'s `xaml_interop::intercept_plain_enter`
    // subscription, set up once on the window's root content and reading
    // this cell fresh on every Enter press) — a `PreviewKeyDown` tunnels
    // down *before* the `TextBox`'s own handling runs, which is the actual
    // fix, not a workaround. Returns whether it did anything, so that
    // shared subscription knows whether to mark the key handled.
    let activate_search = {
        let core = core.clone();
        let shared = Rc::clone(shared);
        let query = search_query.to_string();
        let switch_to = switch_to.clone();
        let add_page_and_switch = add_page_and_switch.clone();
        move || -> bool {
            let trimmed = query.trim();
            if trimmed.is_empty() {
                return false;
            }
            // `matching_ids(..)` is collected into an owned `Vec` *before*
            // the `match` on purpose, not just inline in the scrutinee: a
            // `match`'s scrutinee temporaries live for the whole match
            // expression (a real, easy-to-miss Rust rule), so
            // `core.borrow()` done inline here would still be held live
            // while the `_` arm below calls `add_page_and_switch` →
            // `do_add_page` → `core.borrow_mut()` — a `BorrowMutError`
            // panic, confirmed by direct testing (silently caught and
            // logged by reactor's own fault boundary, `fault::catch`,
            // rather than crashing, which is exactly why this had to be
            // chased down with temporary `trace()` calls rather than a
            // visible failure).
            let matching = core.borrow().matching_ids(trimmed);
            match matching.as_slice() {
                [only] => switch_to.invoke(only.clone()),
                _ => {
                    let history_matches = shared.history.search(trimmed, 2).unwrap_or_default();
                    if let [only] = history_matches.as_slice() {
                        add_page_and_switch.invoke(only.url.clone());
                    } else {
                        let url = resolve_address_input(trimmed, &shared.settings.borrow());
                        add_page_and_switch.invoke(url);
                    }
                }
            }
            true
        }
    };
    *enter_action.borrow_mut() = Some(Rc::new(activate_search) as Rc<dyn Fn() -> bool>);

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
    App::new().title(APP_TITLE).render(move |cx| {
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
