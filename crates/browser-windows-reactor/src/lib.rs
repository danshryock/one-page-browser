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
//! `browser-windows-winui`/`browser-linux-gtk3` (see ROADMAP.md) rather than
//! ported in one pass — `windows-reactor`'s declarative, React-like model
//! (a render function of state, re-diffed against the live tree) is a
//! genuinely different shape from `winio-winui3`'s imperative
//! widget-tree-with-handles style, so this isn't a mechanical port.
//!
//! This version has multi-page hosting (see `ROADMAP.md`'s task list for
//! what's still missing: the real switcher grid, settings/profile/
//! keybindings, the custom title bar).
//!
//! # Multi-page hosting in a declarative model
//!
//! `winio-winui3`'s approach (a per-page `Grid` container, `Visibility`
//! toggled to show only the active one — see `browser-windows-winui`'s
//! `AppState::set_active`) doesn't translate directly: `windows-reactor` has
//! no `Visibility`/display modifier at all (checked by reading
//! `crates/libs/reactor/src/element.rs`/`widget.rs` — a real gap, same
//! category as `winio-winui3`'s missing `KeyDown`, just in a different
//! place). Instead, every loaded page's `webview(..)` element is always
//! present in the tree, each `.with_key(id)` so the reconciler keeps that
//! specific page's underlying `WebView2` control (and its navigation
//! session) alive across renders — the same identity mechanism
//! `crates/samples/reactor/samples/examples/tab_view_add_button.rs` uses for
//! a dynamic list of tabs. All of them share one grid cell; the active
//! page's element is placed *last* in that cell's children, so it paints
//! (and receives hit-testing) on top, fully occluding the others — a real
//! technique, not a hack: WinUI 3's `Grid` has always supported multiple
//! children stacked in one cell in z-order.
//!
//! Each page's `WebView` handle and navigation-completed `EventRegistration`
//! (which must be kept alive — see `windows-webview`'s doc comment on
//! `EventRegistration` — or the subscription is dropped immediately) live in
//! root-level `cx.use_ref` maps keyed by page id, populated by that page's
//! `on_ready` callback. `active_id_ref` mirrors the `active_id` reactor
//! state into a plain `HookRef<String>` so each page's long-lived
//! navigation-completed closure can check, at *event-fire* time, whether
//! it's still the active page before reflecting into the address bar — a
//! closure capturing `active_id` by value would only ever see whatever it
//! was when that specific page last mounted, not later switches.
#![cfg(all(target_os = "windows", target_env = "msvc"))]

use std::collections::HashMap;

use browser_core::{resolve_address_input, Settings, HOME_URL};
use windows_reactor::*;
use windows_webview::{webview, EventRegistration, WebView};

/// Same checkpoint-tracing pattern as `browser_windows_winui::trace` — cheap
/// and has already paid for itself once diagnosing a real crash on Windows.
pub fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("reactor-trace.log") {
        let _ = writeln!(f, "{msg}");
        let _ = f.sync_all();
    }
}

fn app(cx: &mut RenderCx) -> Element {
    trace("app: render start");
    let (pages, set_pages) = cx.use_state(vec![String::from("0")]);
    let (active_id, set_active_id) = cx.use_state(String::from("0"));
    let (address, set_address) = cx.use_state(String::from(HOME_URL));
    let next_id = cx.use_ref::<u64>(1);
    let engines = cx.use_ref::<HashMap<String, WebView>>(HashMap::new());
    let registrations = cx.use_ref::<HashMap<String, EventRegistration>>(HashMap::new());
    let active_id_ref = cx.use_ref::<String>(active_id.clone());
    *active_id_ref.borrow_mut() = active_id.clone();

    let switch_to = {
        let engines = engines.clone();
        let active_id_ref = active_id_ref.clone();
        let set_active_id = set_active_id.clone();
        let set_address = set_address.clone();
        move |id: String| {
            *active_id_ref.borrow_mut() = id.clone();
            set_active_id.call(id.clone());
            let url = engines.borrow().get(&id).map(|w| w.source()).unwrap_or_default();
            set_address.call(if url.is_empty() { HOME_URL.to_string() } else { url });
        }
    };

    let add_page = {
        let pages = pages.clone();
        let set_pages = set_pages.clone();
        let next_id = next_id.clone();
        let switch_to = switch_to.clone();
        move || {
            let id = next_id.borrow().to_string();
            *next_id.borrow_mut() += 1;
            let mut new_pages = pages.clone();
            new_pages.push(id.clone());
            set_pages.call(new_pages);
            switch_to(id);
        }
    };

    let navigate_from_address_bar = {
        let engines = engines.clone();
        let active_id = active_id.clone();
        let address = address.clone();
        move || {
            if let Some(web) = engines.borrow().get(&active_id) {
                let url = resolve_address_input(&address, &Settings::default());
                let _ = web.navigate(&url);
            }
        }
    };

    let back = with_active(&engines, &active_id, WebView::go_back);
    let forward = with_active(&engines, &active_id, WebView::go_forward);
    let reload = with_active(&engines, &active_id, WebView::reload);

    let toolbar = grid((
        Element::from(button("\u{25c0}").on_click(back)).grid_column(0),
        Element::from(button("\u{25b6}").on_click(forward)).grid_column(1),
        Element::from(button("\u{27f3}").on_click(reload)).grid_column(2),
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
        Element::from(button("+").on_click(add_page)).grid_column(4),
    ))
    .columns([
        GridLength::Auto,
        GridLength::Auto,
        GridLength::Auto,
        GridLength::STAR,
        GridLength::Auto,
    ])
    .column_spacing(8.0)
    .margin(Thickness::uniform(8.0));

    // Minimal page-switching row — a placeholder for the real switcher grid
    // (search box + tiles, matching `winio-winui3`'s), which needs
    // `browser_core::PageManager` wired in (see ROADMAP.md's task list).
    // Just proves pages stay independently alive and switchable.
    let page_buttons: Vec<Element> = pages
        .iter()
        .map(|id| {
            let switch_to = switch_to.clone();
            let id_for_click = id.clone();
            let btn = button(format!("Page {id}")).on_click(move || switch_to(id_for_click.clone()));
            let btn = if *id == active_id { btn.accent() } else { btn };
            Element::from(btn)
        })
        .collect();
    let page_switcher_row = hstack(page_buttons).spacing(4.0).margin(Thickness::uniform(4.0));

    // Every loaded page's webview stays mounted (see this module's doc
    // comment on why); the active one is pushed last so it paints on top.
    let mut page_elements: Vec<Element> = Vec::with_capacity(pages.len());
    for id in pages.iter().filter(|id| **id != active_id) {
        page_elements.push(page_element(id.clone(), &engines, &registrations, &active_id_ref, &set_address));
    }
    page_elements.push(page_element(active_id.clone(), &engines, &registrations, &active_id_ref, &set_address));
    let content = grid(page_elements);

    trace("app: render end");
    grid((
        Element::from(toolbar).grid_row(0),
        Element::from(page_switcher_row).grid_row(1),
        Element::from(content).grid_row(2),
    ))
    .rows([GridLength::Auto, GridLength::Auto, GridLength::STAR])
    .into()
}

/// Builds a fresh closure looking up the active page's `WebView` at call
/// time (not bind time) and running `action` against it — used for the
/// back/forward/reload buttons, which must always act on whichever page is
/// currently active, not whichever was active when the button was built.
fn with_active(
    engines: &HookRef<HashMap<String, WebView>>,
    active_id: &str,
    action: fn(&WebView) -> Result<()>,
) -> impl Fn() + 'static {
    let engines = engines.clone();
    let active_id = active_id.to_string();
    move || {
        if let Some(web) = engines.borrow().get(&active_id) {
            let _ = action(web);
        }
    }
}

/// Builds one page's always-mounted `webview(..)` element. `id` is cheap to
/// clone (a small integer string) and captured by value into `on_ready`
/// along with clones of the root-level maps/refs it needs to register
/// itself into — this function itself holds no state of its own, all of it
/// lives in `app`'s hooks (see this module's doc comment).
fn page_element(
    id: String,
    engines: &HookRef<HashMap<String, WebView>>,
    registrations: &HookRef<HashMap<String, EventRegistration>>,
    active_id_ref: &HookRef<String>,
    set_address: &SetState<String>,
) -> Element {
    let engines = engines.clone();
    let registrations = registrations.clone();
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
            move |_args| {
                if *active_id_ref.borrow() == id {
                    let source = ready.source();
                    if !source.is_empty() {
                        set_address.call(source);
                    }
                }
            }
        };
        if let Ok(registration) = ready.on_navigation_completed(reflect) {
            registrations.borrow_mut().insert(id_for_ready.clone(), registration);
        }
        let _ = ready.navigate(HOME_URL);
        engines.borrow_mut().insert(id_for_ready.clone(), ready);
    };

    Element::from(webview(on_ready)).with_key(id)
}

/// Runs the app — called from `main.rs` after `bootstrap()`. Blocks until
/// the window closes (reactor's own message loop; see `App::render`'s doc
/// comment upstream).
pub fn run() -> Result<()> {
    App::new().title("claude-browser").render(app)
}
