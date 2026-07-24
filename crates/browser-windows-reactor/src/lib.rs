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
//! for real (see `engine.rs`) and a working switcher overlay (search box +
//! tile grid, open pages plus history matches, matching
//! `browser-windows-winui`'s `rebuild_switcher_grid`). Still missing (see
//! `ROADMAP.md`'s task list): settings/profile/keybindings overlays, the
//! custom title bar.
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

use std::cell::RefCell;
use std::rc::Rc;

use browser_core::{domain_of, resolve_address_input, HistoryEntry, HistoryStore, PageManager, Profile, Settings, HOME_URL};
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
    // Not read yet — needed once the settings/profile-picker overlays land
    // (see ROADMAP.md's task list): Settings::save and
    // launch_new_profile_process both take a &Profile.
    #[allow(dead_code)]
    profile: Profile,
}

/// One entry in the switcher's tile grid — mirrors
/// `browser-windows-winui`'s `rebuild_switcher_grid`: open pages matching
/// the search query, a trailing "add page" tile, then history matches (only
/// once there's a query, and only for URLs not already open).
#[derive(Clone)]
enum Tile {
    Open { id: String, title: String, domain: String },
    Add,
    History { url: String, title: String, domain: String },
}

fn app(cx: &mut RenderCx, shared: &Rc<Shared>) -> Element {
    trace("app: render start");
    let core = cx.use_ref(PageManager::<ReactorWebViewEngine>::new(shared.settings.borrow().max_loaded_pages));
    let (generation, set_generation) = cx.use_state(0u64);
    let (active_id, set_active_id) = cx.use_state(String::new());
    let active_id_ref = cx.use_ref(active_id.clone());
    *active_id_ref.borrow_mut() = active_id.clone();
    let (address, set_address) = cx.use_state(String::from(HOME_URL));
    let (switcher_open, set_switcher_open) = cx.use_state(false);
    let (search_query, set_search_query) = cx.use_state(String::new());

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
        let set_switcher_open = set_switcher_open.clone();
        let bump = bump.clone();
        move |id: String| {
            ensure_engine_loaded(&core, &id);
            core.borrow_mut().set_active(&id);
            *active_id_ref.borrow_mut() = id.clone();
            set_active_id.call(id.clone());
            let url = core.borrow().page(&id).map(|p| p.current_url()).unwrap_or_default();
            set_address.call(if url.is_empty() { HOME_URL.to_string() } else { url });
            set_switcher_open.call(false);
            bump.invoke(());
        }
    });

    let add_page_and_switch: Callback<String> = Callback::new({
        let core = core.clone();
        let set_active_id = set_active_id.clone();
        let active_id_ref = active_id_ref.clone();
        let set_address = set_address.clone();
        let set_switcher_open = set_switcher_open.clone();
        let bump = bump.clone();
        move |url: String| {
            do_add_page(&core, &url, &set_active_id, &active_id_ref, &set_address);
            set_switcher_open.call(false);
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
            let core = core.borrow();
            if let Some(engine) = core.page(&active_id).and_then(|p| p.engine.as_ref()) {
                let url = resolve_address_input(&address, &settings.settings.borrow());
                use render_engine::RenderEngine;
                let _ = engine.navigate(&url);
            }
        }
    };

    let toggle_switcher = {
        let set_switcher_open = set_switcher_open.clone();
        let set_search_query = set_search_query.clone();
        move || {
            set_search_query.call(String::new());
            set_switcher_open.call(!switcher_open);
        }
    };

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

    let switcher = if switcher_open {
        Some(switcher_overlay(
            &core,
            &shared,
            &search_query,
            set_search_query.clone(),
            switch_to.clone(),
            add_page_and_switch.clone(),
            close_page.clone(),
        ))
    } else {
        None
    };

    trace("app: render end");
    let mut rows = vec![Element::from(toolbar).grid_row(0), Element::from(content).grid_row(1)];
    if let Some(switcher) = switcher {
        rows.push(Element::from(switcher).grid_row(1));
    }
    grid(rows).rows([GridLength::Auto, GridLength::STAR]).into()
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
    let open_matches = core.borrow().matching_ids(search_query);
    let mut tiles: Vec<Tile> = Vec::new();
    {
        let core = core.borrow();
        for page in core.pages() {
            if !open_matches.contains(&page.id) {
                continue;
            }
            let title = page.title.borrow().clone();
            let title = if title.is_empty() { "New Page".to_string() } else { title };
            let url = page.current_url();
            let domain = domain_of(&url);
            let domain = if page.loaded { domain } else { format!("{domain} \u{b7} unloaded") };
            tiles.push(Tile::Open { id: page.id.clone(), title, domain });
        }
    }
    tiles.push(Tile::Add);

    if !search_query.trim().is_empty() {
        let open_urls: Vec<String> = core.borrow().pages().iter().map(|p| p.current_url()).collect();
        let history_matches: Vec<HistoryEntry> = shared
            .history
            .search(search_query, 8)
            .unwrap_or_else(|err| {
                eprintln!("history search failed: {err}");
                Vec::new()
            })
            .into_iter()
            .filter(|entry| !open_urls.contains(&entry.url))
            .collect();
        for entry in history_matches {
            let title = if entry.title.is_empty() { "New Page".to_string() } else { entry.title };
            tiles.push(Tile::History { url: entry.url, title, domain: format!("{} \u{b7} history", entry.domain) });
        }
    }

    let start_page = shared.settings.borrow().start_page.clone();
    let tiles_for_select = tiles.clone();
    let grid_of_tiles = grid_view(tiles, |tile, _idx| tile_element(tile))
        .with_key_selector(tile_key)
        .selected_index(-1)
        .on_selection_changed(move |idx: i32| {
            let Some(tile) = tiles_for_select.get(idx.max(0) as usize) else { return };
            match tile {
                Tile::Open { id, .. } => switch_to.invoke(id.clone()),
                Tile::Add => add_page_and_switch.invoke(start_page.clone()),
                Tile::History { url, .. } => add_page_and_switch.invoke(url.clone()),
            }
        });
    let _ = close_page; // reserved for a future close-tile control; not wired yet

    let search_box = text_box(search_query.to_string())
        .placeholder_text("Type to filter open pages\u{2026}")
        .on_text_changed(set_search_query)
        .width(400.0);

    grid((
        Element::from(search_box).grid_row(0),
        Element::from(grid_of_tiles).grid_row(1),
    ))
    .rows([GridLength::Auto, GridLength::STAR])
    .margin(Thickness::uniform(16.0))
}

fn tile_key(tile: &Tile) -> String {
    match tile {
        Tile::Open { id, .. } => format!("open:{id}"),
        Tile::Add => "add".to_string(),
        Tile::History { url, .. } => format!("history:{url}"),
    }
}

fn tile_element(tile: &Tile) -> Element {
    let (title, domain) = match tile {
        Tile::Open { title, domain, .. } => (title.clone(), domain.clone()),
        Tile::Add => ("+".to_string(), String::new()),
        Tile::History { title, domain, .. } => (title.clone(), domain.clone()),
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
