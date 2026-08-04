//! Real `cargo test`-integrated GTK unit tests, using `gtk-test` (see
//! `Cargo.toml`) as the dev-dependency backing this — though the actual
//! value taken from it here is running as ordinary `#[test]` functions and
//! its `run_loop`-style polling approach, not its `enigo`-backed synthetic
//! mouse/keyboard functions (`click`/`key_press`/etc.): those drive the UI
//! via real OS-level input against the on-screen window and explicitly
//! require it to have real focus/stacking (documented in `gtk-test`'s own
//! source), which this app's own `AppState` methods
//! (`add_page`/`switch_to`/`search_activate`/`address_bar_activate`/etc.)
//! don't need at all — they drive behavior directly, and are more robust
//! for this than synthetic input would be.
//!
//! Needs a real display (`DISPLAY` pointed at a working X11/Xwayland
//! server) to run at all — GTK itself, not just this file, requires one.
//! `xwayland-run` (a headless Wayland compositor + Xwayland, genuinely
//! isolated from any real desktop) is the recommended way to get one in a
//! terminal/CI with no physical display; see `README.md`'s Testing section.
//!
//! GTK doesn't just require staying on one thread at a time — `gtk::init()`
//! (see `gtk-0.18.2/src/rt.rs`) permanently remembers *which* thread called
//! it first via a `thread_local!` flag, and panics ("Attempted to initialize
//! GTK from two different threads") if any *other* thread ever calls it
//! again, even long after the first thread has finished. Rust's test harness
//! spawns a fresh OS thread per `#[test]` function even under
//! `--test-threads=1` (confirmed by actually running this suite: the first
//! test passed, every subsequent one panicked with exactly that message) —
//! so a per-test `Mutex` guarding sequential access isn't enough; GTK must
//! be initialized and driven from the *same* thread for the whole process.
//!
//! Fixed with a single persistent worker thread (`gtk_thread`, spawned once,
//! lazily), which is the only thread that ever touches GTK. Each test sends
//! its body to it via `run_on_gtk_thread` and blocks for the result,
//! `catch_unwind`-ing on the worker side and re-panicking on the calling
//! (real test) thread so `cargo test`'s reporting still attributes failures
//! to the right test name. This also fully serializes the suite as a side
//! effect (one job runs at a time on the worker), so no separate lock is
//! needed on top.

use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use browser_core::{Action, HistoryStore, PasswordBackend, Profile};
use browser_linux_gtk3::{build_window_and_app, build_window_and_app_with_history, AppState};
use gtk::prelude::*;
use render_engine::{RenderEngine, WryEngine};

type Job = Box<dyn FnOnce() + Send>;

/// The single, persistent GTK-owning thread's job queue — spawned on first
/// use, lives for the rest of the process.
fn gtk_thread() -> &'static Sender<Job> {
    static SENDER: OnceLock<Sender<Job>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            gtk::init().expect("gtk::init should succeed against a real display");
            for job in rx {
                job();
            }
        });
        tx
    })
}

/// Runs `body` on the single GTK-owning thread and blocks until it's done,
/// re-panicking on the calling thread if `body` panicked there — see this
/// module's doc comment for why this exists at all. `body` must not capture
/// anything non-`Send` (none of the tests here need to: each builds its own
/// profile/window/app entirely inside the closure).
fn run_on_gtk_thread<F: FnOnce() + Send + 'static>(body: F) {
    let (done_tx, done_rx) = mpsc::channel::<Option<String>>();
    let job: Job = Box::new(move || {
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(body));
        let failure = outcome.err().map(|payload| {
            payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "GTK thread panicked with a non-string payload".to_string())
        });
        let _ = done_tx.send(failure);
    });
    gtk_thread().send(job).expect("GTK worker thread should still be alive");
    if let Some(message) = done_rx.recv().expect("GTK worker thread should reply") {
        panic!("{message}");
    }
}

/// Generous ceiling for how long a webview-derived value (current URL,
/// document title) may take to settle after a navigation. Wide on purpose:
/// this only slows a run down when a check genuinely never becomes true (a
/// real failure) — `wait_until` returns the moment the condition is met, so
/// under normal load this costs milliseconds, not seconds.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Minimum extra time to keep pumping after `condition` first becomes true,
/// before trusting it. E.g. `active_url()` can report the destination URL
/// before WebKitGTK has actually finished registering that navigation in its
/// joint history stack — returning the instant a condition matches leaves
/// too little real time elapsed for whatever runs next to be reliable.
/// Confirmed empirically (carried over from this suite's original
/// example-binary form): without this settle window, polling that returns
/// as soon as the condition matches reproduces a real flake; with it, ten
/// runs straight pass.
const SETTLE: Duration = Duration::from_millis(200);

/// Polls `condition` (pumping the GTK loop between attempts) until it's true
/// or `TIMEOUT` elapses. Use this for anything that depends on the embedded
/// webview's async state (URL after a navigation, title after a document
/// load) — plain app state (page list, active id, overlay visibility)
/// updates synchronously in Rust and never needs this.
fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        if condition() {
            let settle_until = Instant::now() + SETTLE;
            while Instant::now() < settle_until {
                while gtk::events_pending() {
                    gtk::main_iteration_do(false);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fixture_url(name: &str) -> String {
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/fixtures");
    format!("file://{fixtures}/{name}")
}

/// A profile scoped to this test alone (unique per test name + process),
/// so tests never share or pollute each other's — or the real user's —
/// `Settings`/`HistoryStore`/`Keybindings` files on disk. Removes its
/// on-disk directory up front (in case a previous run of this same test
/// left one) and returns it ready to use; callers should also remove it
/// afterward via `cleanup_test_profile`.
fn test_profile(name: &str) -> Profile {
    let profile = Profile::new(format!("gtk-test-{name}-{}", std::process::id()));
    cleanup_test_profile(&profile);
    profile
}

/// Best-effort removal of a test profile's on-disk directories. Ignores
/// errors — this is cleanup, not something a test should fail over.
/// `settings_path`/`keybindings_path` live under the config dir,
/// `history_db_path` under the data dir (a genuinely different path on
/// Linux — `~/.config/claude-browser/...` vs. `~/.local/share/claude-browser/...`)
/// — both need removing, or `HistoryStore::open`'s `history.db` is left
/// behind under the data dir even after the config-dir side is cleaned up.
fn cleanup_test_profile(profile: &Profile) {
    for path in [profile.settings_path(), profile.history_db_path(), profile.bookmarks_path()].into_iter().flatten() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

/// Exercises `render_engine::WryEngine` directly (the same object the app's
/// back/forward/reload toolbar buttons drive through `AppState::with_active`,
/// a private method with no public test hook) rather than through
/// `AppState`/`build_window_and_app` like the other tests here — this one is
/// about `WryEngine`'s own navigation/history behavior specifically.
#[test]
fn navigation_back_forward_reload() {
    run_on_gtk_thread(|| {
        let url_a = fixture_url("page_a.html");
        let url_b = fixture_url("page_b.html");

        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&content);
        window.show_all();

        let engine = WryEngine::new(&content, &url_a, |_| {}).expect("WryEngine::new should succeed");
        assert!(
            wait_until(|| engine.current_url().ok().as_deref() == Some(url_a.as_str())),
            "initial load should reach page A"
        );

        engine.navigate(&url_b).expect("navigate should succeed");
        assert!(
            wait_until(|| engine.current_url().ok().as_deref() == Some(url_b.as_str())),
            "navigating should reach page B"
        );

        engine.go_back().expect("go_back should succeed");
        assert!(
            wait_until(|| engine.current_url().ok().as_deref() == Some(url_a.as_str())),
            "go_back should return to page A"
        );

        engine.go_forward().expect("go_forward should succeed");
        assert!(
            wait_until(|| engine.current_url().ok().as_deref() == Some(url_b.as_str())),
            "go_forward should return to page B"
        );

        engine.reload().expect("reload should succeed");
        assert!(
            wait_until(|| engine.current_url().ok().as_deref() == Some(url_b.as_str())),
            "reload should stay on page B"
        );
    });
}

#[test]
fn reader_mode_toggles_on_and_off() {
    run_on_gtk_thread(|| {
        let url_a = fixture_url("page_a.html");

        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&content);
        window.show_all();

        let title = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let title_for_cb = std::rc::Rc::clone(&title);
        let engine = WryEngine::new(&content, &url_a, move |new_title| {
            *title_for_cb.borrow_mut() = new_title;
        })
        .expect("WryEngine::new should succeed");
        assert!(wait_until(|| *title.borrow() == "Page A"), "initial load should reach page A");

        engine.toggle_reader_mode().expect("toggle_reader_mode should succeed");
        assert!(
            wait_until(|| *title.borrow() == "Reader: Page A"),
            "turning reader mode on should retitle the page to make it visually obvious"
        );

        engine.toggle_reader_mode().expect("toggle_reader_mode should succeed a second time");
        assert!(wait_until(|| *title.borrow() == "Page A"), "turning reader mode off again should restore the original title");
    });
}

#[test]
fn page_lifecycle_add_switch_close() {
    run_on_gtk_thread(|| {
        let profile = test_profile("page-lifecycle");
        let url_a = fixture_url("page_a.html");
        let url_b = fixture_url("page_b.html");
        let url_c = fixture_url("page_c.html");

        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        app.add_page(&url_a).expect("add_page should succeed");
        assert_eq!(app.page_ids().len(), 1, "adding the first page should open exactly one page");
        let id_a = app.page_ids()[0].clone();
        assert_eq!(app.active_id(), id_a, "the first page should become active");
        assert_eq!(app.stack_visible_child_name().as_deref(), Some(id_a.as_str()));
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));
        assert!(wait_until(|| app.page_title(&id_a).as_deref() == Some("Page A")));

        app.add_page(&url_b).expect("add_page should succeed");
        assert_eq!(app.page_ids().len(), 2, "adding a second page should grow the list to 2");
        let id_b = app.page_ids()[1].clone();
        assert_eq!(app.active_id(), id_b, "the new page should become active");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_b.as_str())));

        app.add_page(&url_c).expect("add_page should succeed");
        assert_eq!(app.page_ids().len(), 3, "adding a third page should grow the list to 3");
        let id_c = app.page_ids()[2].clone();
        assert_eq!(app.active_id(), id_c, "the third page should become active");

        app.switch_to(&id_a);
        assert_eq!(app.active_id(), id_a, "switching back to A should update the active id");
        assert_eq!(app.stack_visible_child_name().as_deref(), Some(id_a.as_str()));
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));
        assert_eq!(app.page_ids().len(), 3, "switching shouldn't drop other pages");

        // A is active; closing it should fall back to a remaining page, not vanish.
        app.close_page(&id_a);
        assert_eq!(app.page_ids().len(), 2, "closing the active page should remove it from the list");
        assert_ne!(app.active_id(), id_a, "closing the active page should pick a new active page");
        assert!(
            app.active_id() == id_b || app.active_id() == id_c,
            "the new active page should be one of the remaining ones"
        );

        cleanup_test_profile(&profile);
    });
}

#[test]
fn next_and_previous_page_shortcuts_cycle_through_open_pages() {
    run_on_gtk_thread(|| {
        let profile = test_profile("next-previous-page");
        let url_a = fixture_url("page_a.html");
        let url_b = fixture_url("page_b.html");
        let url_c = fixture_url("page_c.html");

        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        app.add_page(&url_a).expect("add_page should succeed");
        let id_a = app.page_ids()[0].clone();
        app.add_page(&url_b).expect("add_page should succeed");
        let id_b = app.page_ids()[1].clone();
        app.add_page(&url_c).expect("add_page should succeed");
        let id_c = app.page_ids()[2].clone();
        assert_eq!(app.active_id(), id_c, "sanity check: the third page should be active after adding it");

        // Ctrl+Tab/Ctrl+PageDown — Action::NextPage's dispatch target.
        app.switch_to_next_page();
        assert_eq!(app.active_id(), id_a, "next page should wrap from the last page back to the first");
        app.switch_to_next_page();
        assert_eq!(app.active_id(), id_b);
        app.switch_to_next_page();
        assert_eq!(app.active_id(), id_c);

        // Ctrl+Shift+Tab/Ctrl+PageUp — Action::PreviousPage's dispatch target.
        app.switch_to_previous_page();
        assert_eq!(app.active_id(), id_b);
        app.switch_to_previous_page();
        assert_eq!(app.active_id(), id_a);
        app.switch_to_previous_page();
        assert_eq!(app.active_id(), id_c, "previous page should wrap from the first page back to the last");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn switcher_search_and_grid() {
    run_on_gtk_thread(|| {
        let profile = test_profile("switcher-search");
        let url_a = fixture_url("page_a.html");
        let url_b = fixture_url("page_b.html");
        let url_c = fixture_url("page_c.html");

        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        app.add_page(&url_b).expect("add_page should succeed");
        let id_b = app.active_id();
        // The later "page b" search matches on title (the URL has an underscore,
        // not a space, so it can't match "page b"), so B's title has to have
        // settled before that point.
        assert!(wait_until(|| app.page_title(&id_b).as_deref() == Some("Page B")));
        app.add_page(&url_c).expect("add_page should succeed");

        // open_switcher shows the panel with a cleared, focused search box (this
        // is what F1/Ctrl+T/Ctrl+L and the grid button all trigger now).
        app.open_switcher();
        assert!(app.is_switcher_open(), "open_switcher should show the switcher panel");
        assert!(!app.is_background_page_interactive(), "open_switcher should make the background page stack insensitive");

        // Typing a query that matches no open page and pressing Enter should
        // open a new page from it instead of doing nothing.
        let before_count = app.page_ids().len();
        app.search_activate("some-nonexistent-domain-example");
        assert_eq!(app.page_ids().len(), before_count + 1, "search should open a new page when nothing matches");
        let new_id = app.page_ids().last().cloned().unwrap_or_default();
        assert_eq!(app.active_id(), new_id, "the new page from search should become active");
        // WebKitGTK normalizes a bare-domain URL by adding a trailing slash.
        assert!(wait_until(|| app.active_url().as_deref() == Some("https://some-nonexistent-domain-example/")));
        assert!(!app.is_switcher_open(), "opening a page from search should close the switcher");
        assert!(app.is_background_page_interactive(), "closing the switcher should restore background page interactivity");

        // Filtering down to exactly one matching page and pressing Enter should
        // switch to it (not create a duplicate).
        app.open_switcher();
        let existing_count = app.page_ids().len();
        app.search_activate("page b");
        assert_eq!(app.page_ids().len(), existing_count, "search shouldn't open a new page when a single match exists");
        assert_eq!(app.active_id(), id_b, "search should switch to the single matching page");
        assert!(!app.is_switcher_open(), "switching via search should close the switcher");

        // Filtering to a query matching MORE than one page shouldn't switch
        // anywhere (ambiguous) or create a duplicate.
        app.open_switcher();
        let active_before = app.active_id();
        app.search_activate("page");
        assert_eq!(app.page_ids().len(), existing_count, "search shouldn't open a new page when multiple pages match");
        assert_eq!(app.active_id(), active_before, "search shouldn't switch when multiple pages match");

        // Closing the ACTIVE page from an open grid should keep the grid open
        // and switch to the nearest remaining page, not dismiss the grid.
        let active_before_close = app.active_id();
        let count_before_close = app.page_ids().len();
        app.close_page(&active_before_close);
        assert!(app.is_switcher_open(), "closing the active page from the grid should keep the grid open");
        assert_eq!(app.page_ids().len(), count_before_close - 1);
        assert_ne!(app.active_id(), active_before_close, "closing the active page from the grid should switch to a remaining page");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn loaded_page_limit_evicts_and_reclaims() {
    run_on_gtk_thread(|| {
        let profile = test_profile("loaded-limit");
        let url_a = fixture_url("page_a.html");
        let url_b = fixture_url("page_b.html");
        let url_c = fixture_url("page_c.html");

        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        app.add_page(&url_b).expect("add_page should succeed");
        app.add_page(&url_c).expect("add_page should succeed");

        // Real end-to-end check that loaded/unloaded tracking works through the
        // actual WryEngine-backed PageManager and AppState::set_max_loaded_pages
        // — browser-core's own unit tests for this only exercise a mock engine.
        let count_before_limit = app.page_ids().len();
        app.set_max_loaded_pages(Some(2));
        let loaded_after_limit = app.page_ids().iter().filter(|id| app.is_page_loaded(id)).count();
        assert_eq!(loaded_after_limit, count_before_limit.min(2), "tightening the limit should evict down to it immediately");

        app.add_page(&url_a).expect("add_page should succeed");
        let loaded_after_new_page = app.page_ids().iter().filter(|id| app.is_page_loaded(id)).count();
        assert_eq!(loaded_after_new_page, 2, "loading a new page past the limit should evict the oldest again");
        let newest_id = app.page_ids().last().cloned().unwrap_or_default();
        assert!(app.is_page_loaded(&newest_id), "the newly loaded page itself should be loaded");

        app.set_max_loaded_pages(None);
        let loaded_after_unlimited = app.page_ids().iter().filter(|id| app.is_page_loaded(id)).count();
        assert_eq!(loaded_after_unlimited, 2, "removing the limit shouldn't retroactively reload anything unloaded");

        // The `loaded` flag alone only proves bookkeeping, not real resource
        // reclamation. Tighten the limit to 1 to force an eviction, confirm the
        // evicted page's webview widget was actually torn down (its stack
        // container has zero children), then confirm switching back to it
        // rebuilds a live widget reloaded at its original URL.
        app.set_max_loaded_pages(Some(1));
        let reclaimed_id = app
            .page_ids()
            .into_iter()
            .find(|id| !app.is_page_loaded(id))
            .expect("tightening to limit 1 with more than one open page should evict at least one");
        let reclaimed_url = app.page_url(&reclaimed_id).expect("an evicted page should still remember its last URL");
        assert_eq!(
            app.page_container_child_count(&reclaimed_id),
            0,
            "the evicted page's webview widget should be actually torn down, not just flagged"
        );

        app.switch_to(&reclaimed_id);
        assert_eq!(app.page_container_child_count(&reclaimed_id), 1, "switching to an unloaded page should rebuild a live webview widget");
        assert!(app.is_page_loaded(&reclaimed_id), "switching to an unloaded page should mark it loaded again");
        assert!(
            wait_until(|| app.active_url().as_deref() == Some(reclaimed_url.as_str())),
            "the rebuilt webview should reload the page's original URL"
        );

        cleanup_test_profile(&profile);
    });
}

#[test]
fn toolbar_address_bar_resolves_search_queries() {
    run_on_gtk_thread(|| {
        let profile = test_profile("address-bar-search");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&fixture_url("page_a.html")).expect("add_page should succeed");

        // Real end-to-end check that the toolbar address bar (not just the
        // switcher's search box) resolves non-URL input via the preferred
        // search engine.
        app.address_bar_activate("how to cook rice");
        assert!(
            wait_until(|| app.active_url().as_deref() == Some("https://www.google.com/search?q=how%20to%20cook%20rice")),
            "the toolbar address bar should resolve multi-word input via the search engine"
        );

        cleanup_test_profile(&profile);
    });
}

#[test]
fn settings_overlay_mutual_exclusion_and_save() {
    run_on_gtk_thread(|| {
        let profile = test_profile("settings-overlay");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&fixture_url("page_a.html")).expect("add_page should succeed");

        // Settings overlay: shows/hides in place of a modal dialog, is mutually
        // exclusive with the switcher grid (both are reachable from the header
        // bar regardless of which, if either, is currently open), pre-populates
        // from the current Settings, and Save actually persists an edit.
        app.open_settings();
        assert!(app.is_settings_open(), "open_settings should show the settings overlay");
        assert_eq!(
            app.settings_start_page_entry_text(),
            app.settings().start_page,
            "open_settings should pre-populate the start-page field from current settings"
        );

        app.open_switcher();
        assert!(!app.is_settings_open(), "opening the switcher while settings is open should close settings");
        assert!(app.is_switcher_open(), "opening the switcher while settings is open should still open the switcher");
        app.close_switcher();

        app.open_settings();
        app.open_settings(); // re-opening while already open should stay open, not toggle closed
        assert!(app.is_settings_open(), "opening settings while already open should leave it open");

        let edited_start_page = "https://edited-start-page.example";
        app.set_settings_start_page(edited_start_page);
        app.save_settings();
        assert_eq!(app.settings().start_page, edited_start_page, "Save should persist an edited start page into Settings");
        assert!(!app.is_settings_open(), "Save should close the settings overlay");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn closing_every_page_leaves_one_fallback() {
    run_on_gtk_thread(|| {
        let profile = test_profile("close-to-fallback");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&fixture_url("page_a.html")).expect("add_page should succeed");
        app.add_page(&fixture_url("page_b.html")).expect("add_page should succeed");
        app.add_page(&fixture_url("page_c.html")).expect("add_page should succeed");

        // Closing every page shouldn't panic, and should land on some fallback page.
        for id in app.page_ids() {
            app.close_page(&id);
        }
        assert_eq!(app.page_ids().len(), 1, "closing every page should leave exactly one fallback page instead of zero");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn unified_address_bar_clears_on_open_and_restores_on_close() {
    run_on_gtk_thread(|| {
        let profile = test_profile("unified-address-bar");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        let url_a = fixture_url("page_a.html");
        app.add_page(&url_a).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));

        // The address bar doubles as the switcher's search box: opening the
        // switcher should clear it (ready for a filter), not still show the
        // active page's URL.
        app.open_switcher();
        assert_eq!(app.address_bar_text(), "", "open_switcher should clear the address bar for filtering");

        // Typing a filter and then closing WITHOUT selecting anything (e.g.
        // Escape) should put the active page's URL back — otherwise the
        // toolbar would be left showing a stale filter string instead of
        // where the user actually is.
        app.set_address_bar_text("some filter text");
        app.close_switcher();
        assert_eq!(
            app.address_bar_text(),
            url_a,
            "closing the switcher without a selection should restore the active page's URL"
        );

        cleanup_test_profile(&profile);
    });
}

#[test]
fn bookmarks_toggle_and_overlay() {
    run_on_gtk_thread(|| {
        let profile = test_profile("bookmarks");
        let url_a = fixture_url("page_a.html");
        let url_b = fixture_url("page_b.html");

        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));
        assert!(!app.is_active_bookmarked(), "a freshly opened page shouldn't start bookmarked");

        app.toggle_bookmark_for_active();
        assert!(app.is_active_bookmarked(), "toggling should bookmark the active page");
        assert_eq!(app.bookmarked_urls(), vec![url_a.clone()]);

        app.toggle_bookmark_for_active();
        assert!(!app.is_active_bookmarked(), "toggling again should un-bookmark it");
        assert!(app.bookmarked_urls().is_empty());

        app.toggle_bookmark_for_active();
        app.add_page(&url_b).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_b.as_str())));
        assert!(!app.is_active_bookmarked(), "switching to a different, unbookmarked page shouldn't show it as bookmarked");

        // The bookmarks overlay is mutually exclusive with the others, same
        // as settings/profile-picker/keybindings.
        app.open_settings();
        app.open_bookmarks();
        assert!(!app.is_settings_open(), "opening bookmarks while settings is open should close settings");
        assert!(app.is_bookmarks_open(), "open_bookmarks should show the bookmarks overlay");
        assert!(!app.is_background_page_interactive(), "open_bookmarks should make the background page stack insensitive");

        app.close_bookmarks();
        assert!(!app.is_bookmarks_open());
        assert!(app.is_background_page_interactive(), "closing bookmarks should restore background page interactivity");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn password_vault_setup_add_and_overlay_mutual_exclusion() {
    run_on_gtk_thread(|| {
        let profile = test_profile("password-vault-basics");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        assert!(app.password_vault_usernames().is_empty());
        assert!(!profile.has_vault_passphrase(), "a fresh profile shouldn't have a vault passphrase yet");

        // Simulates the user completing show_vault_passphrase_prompt's
        // setup flow.
        assert!(
            app.try_open_vault_with("correct horse battery staple", true),
            "setting up a fresh vault should succeed"
        );
        assert!(profile.has_vault_passphrase(), "setting up the vault should mark the profile as vault-protected");

        app.add_password_via_fields("https://example.com", "alice", "hunter2", "personal account");
        app.add_password_via_fields("https://example.com", "bob", "letmein", "");
        assert_eq!(
            app.password_vault_usernames(),
            vec!["bob".to_string(), "alice".to_string()],
            "most-recently-added credential should list first"
        );

        // Mutually exclusive with the other overlays, same as bookmarks.
        app.open_settings();
        app.open_passwords();
        assert!(!app.is_settings_open(), "opening the password manager while settings is open should close settings");
        assert!(app.is_passwords_open(), "open_passwords should show the password manager overlay");
        assert!(!app.is_background_page_interactive(), "open_passwords should make the background page stack insensitive");

        app.close_passwords();
        assert!(!app.is_passwords_open());
        assert!(app.is_background_page_interactive(), "closing the password manager should restore background page interactivity");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn password_vault_edit_updates_a_login_in_place() {
    run_on_gtk_thread(|| {
        let profile = test_profile("password-vault-edit");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        assert!(app.try_open_vault_with("correct horse battery staple", true));

        app.add_password_via_fields("https://example.com", "alice", "old-pw", "old notes");
        app.add_password_via_fields("https://b.example", "bob", "pw", "");
        assert_eq!(app.password_vault_usernames().len(), 2, "sanity check: both logins should exist before editing either");

        let alice_id = app.password_vault_id_for_username("alice").expect("alice's login should exist");
        app.start_editing_local_login(&alice_id);
        // Editing reuses the same add form, so submitting again should
        // update in place rather than create a third entry.
        app.add_password_via_fields("https://example.com", "alice", "new-pw", "updated notes");

        assert_eq!(app.password_vault_usernames().len(), 2, "editing shouldn't create a third entry");
        let updated_id = app.password_vault_id_for_username("alice").expect("alice's login should still exist");
        assert_eq!(updated_id, alice_id, "the same row should have been updated, not replaced");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn password_vault_reuses_an_already_known_passphrase_with_no_second_prompt() {
    run_on_gtk_thread(|| {
        let profile = test_profile("password-vault-shared-passphrase");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        // Simulates history already having been unlocked with this
        // passphrase at startup (see `show_passphrase_prompt`'s success
        // branch, which calls this same method) — the vault has no
        // passphrase of its own set up yet.
        assert!(!profile.has_vault_passphrase());
        app.note_unlocked_with_passphrase("shared passphrase");

        // Opening the vault for the first time should silently establish
        // it under the *same* passphrase — straight to the panel, no
        // prompt-completion step needed (unlike the test above, which
        // simulates completing a real prompt since no passphrase was known
        // yet in that scenario).
        app.open_passwords();
        assert!(
            app.is_passwords_open(),
            "a passphrase already known this session should silently unlock/set up the vault, not prompt for a new one"
        );
        assert!(profile.has_vault_passphrase(), "opening the vault should have set up its own marker under the shared passphrase");

        app.add_password_via_fields("https://example.com", "alice", "hunter2", "");
        assert_eq!(app.password_vault_usernames(), vec!["alice".to_string()]);

        app.close_passwords();
        assert!(!app.is_passwords_open());

        cleanup_test_profile(&profile);
    });
}

/// A fake local server standing in for `bw serve`, tracking a single
/// locked/unlocked flag so a test can exercise the real locked → unlock →
/// unlocked transition `rebuild_passwords_list` renders. Shuts down (via
/// `Server::unblock`) when dropped — same technique as `browser-core`'s own
/// `bitwarden.rs` tests, just not shared code (different crates).
struct FakeBitwardenServer {
    server: std::sync::Arc<tiny_http::Server>,
    join: Option<std::thread::JoinHandle<()>>,
    base_url: String,
}

impl Drop for FakeBitwardenServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Spawns a fake `bw serve`, stateful enough to prove edits/deletes
/// actually persist (not just that the HTTP call didn't error): items live
/// in a shared, mutable list seeded with one "Fake Bank / carol" login;
/// `PUT`/`DELETE` really mutate it, and `GET /list/object/items` always
/// reflects the current state.
fn spawn_fake_bitwarden_server() -> FakeBitwardenServer {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    let unlocked = Arc::new(AtomicBool::new(false));
    let items = Arc::new(Mutex::new(vec![serde_json::json!({
        "id": "1", "type": 1, "name": "Fake Bank", "notes": "",
        "login": {"username": "carol", "password": "pw", "uris": [{"uri": "https://fakebank.example"}]}
    })]));
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("binding a loopback test server should succeed"));
    let addr = server.server_addr().to_ip().expect("this test server always binds an IP socket, not a unix one");
    let base_url = format!("http://{addr}");
    let server_for_thread = Arc::clone(&server);
    let join = std::thread::spawn(move || {
        while let Ok(mut request) = server_for_thread.recv() {
            let mut body_text = String::new();
            let _ = std::io::Read::read_to_string(request.as_reader(), &mut body_text);
            let method = request.method().to_string();
            let url = request.url().to_string();

            let (status, body) = if method == "GET" && url == "/status" {
                let status_str = if unlocked.load(Ordering::SeqCst) { "unlocked" } else { "locked" };
                (200, format!(r#"{{"success":true,"data":{{"template":{{"status":"{status_str}"}}}}}}"#))
            } else if method == "POST" && url == "/unlock" {
                unlocked.store(true, Ordering::SeqCst);
                (200, r#"{"success":true}"#.to_string())
            } else if method == "GET" && url == "/list/object/items" {
                let items = items.lock().unwrap();
                (200, serde_json::json!({"success": true, "data": {"data": *items}}).to_string())
            } else if method == "PUT" && url.starts_with("/object/item/") {
                let id = url.trim_start_matches("/object/item/");
                let sent: serde_json::Value = serde_json::from_str(&body_text).unwrap_or_default();
                let mut items = items.lock().unwrap();
                match items.iter_mut().find(|item| item["id"] == id) {
                    Some(item) => {
                        item["name"] = sent["name"].clone();
                        item["notes"] = sent["notes"].clone();
                        item["login"] = sent["login"].clone();
                        (200, r#"{"success":true}"#.to_string())
                    }
                    None => (404, r#"{"success":false,"message":"not found"}"#.to_string()),
                }
            } else if method == "DELETE" && url.starts_with("/object/item/") {
                let id = url.trim_start_matches("/object/item/");
                items.lock().unwrap().retain(|item| item["id"] != id);
                (200, r#"{"success":true}"#.to_string())
            } else {
                (404, r#"{"success":false,"message":"not found"}"#.to_string())
            };
            let _ = request.respond(tiny_http::Response::from_string(body).with_status_code(status));
        }
    });
    FakeBitwardenServer { server, join: Some(join), base_url }
}

#[test]
fn bitwarden_section_reflects_locked_then_unlocked_state_and_lists_its_items() {
    run_on_gtk_thread(|| {
        let profile = test_profile("bitwarden-section");
        let fake_server = spawn_fake_bitwarden_server();
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        // open_passwords() only reaches rebuild_passwords_list (and so the
        // Bitwarden section) once the *local* vault is past its own setup/
        // unlock prompt — unrelated to Bitwarden, but a fresh profile has
        // neither set up yet, so this test's own local vault needs setting
        // up first (same as `password_vault_setup_add_and_overlay_mutual_exclusion`).
        assert!(app.try_open_vault_with("local vault passphrase", true), "setting up the local vault should succeed");

        app.open_settings();
        app.set_bitwarden_fields(true, &fake_server.base_url);
        app.save_settings();

        app.open_passwords();
        assert!(
            app.passwords_list_contains_text("Bitwarden is locked"),
            "the Bitwarden section should show the locked state before unlocking"
        );
        assert!(!app.passwords_list_contains_text("carol"), "a locked Bitwarden shouldn't list any items yet");

        // Drives the real backend the same way `show_bitwarden_unlock_prompt`
        // would, minus its own GTK widgets — same "drive AppState/backend
        // methods directly" approach the vault's own tests use.
        app.bitwarden_backend().expect("Bitwarden should be enabled").unlock("anything").expect("the fake server always accepts unlock");

        app.open_passwords(); // re-renders the list against the now-unlocked fake server
        assert!(app.passwords_list_contains_text("carol"), "the fake server's item should show up in the Bitwarden section once unlocked");
        assert!(app.passwords_list_contains_text("fakebank.example"));

        app.close_passwords();
        cleanup_test_profile(&profile);
    });
}

#[test]
fn bitwarden_edit_and_delete_round_trip_through_the_fake_server() {
    run_on_gtk_thread(|| {
        let profile = test_profile("bitwarden-edit-delete");
        let fake_server = spawn_fake_bitwarden_server();
        // Bitwarden rows only render once the *local* vault is past its own
        // setup prompt — see the equivalent comment in the locked/unlocked
        // test above.
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        assert!(app.try_open_vault_with("local vault passphrase", true));

        app.open_settings();
        app.set_bitwarden_fields(true, &fake_server.base_url);
        app.save_settings();

        let backend = app.bitwarden_backend().expect("Bitwarden should be enabled");
        backend.unlock("anything").expect("the fake server always accepts unlock");

        // Edit: same "reuse the add form" flow as the local-vault edit test,
        // just targeting the Bitwarden row instead.
        app.open_passwords();
        assert!(app.passwords_list_contains_text("carol"));
        app.start_editing_bitwarden_login("1");
        app.add_password_via_fields("https://fakebank.example", "carol-updated", "new-pw", "");

        app.open_passwords(); // re-render against the fake server's now-updated state
        assert!(app.passwords_list_contains_text("carol-updated"), "the edit should have persisted through PUT /object/item/1");
        let entries = backend.list().unwrap();
        assert_eq!(entries.len(), 1, "editing shouldn't create a second item");
        assert_eq!(entries[0].username, "carol-updated", "the update should have replaced the username, not just added text alongside it");
        assert_eq!(entries[0].password.as_deref(), Some("new-pw"));

        // Delete.
        assert_eq!(backend.list().unwrap().len(), 1, "sanity check: exactly one Bitwarden item before deleting it");
        app.delete_bitwarden_login_for_test("1");
        assert!(backend.list().unwrap().is_empty(), "DELETE /object/item/1 should have removed it from the fake server");

        app.close_passwords();
        cleanup_test_profile(&profile);
    });
}

/// Polls `app`'s active page (via `evaluate_script_on_active_page_for_test`)
/// until the login form's current `username|password` values equal
/// `expected`, or `TIMEOUT` elapses — same "keep re-issuing a read-only
/// query until it reflects reality" approach `wait_until` itself uses for
/// other async webview state, just layered on top of a callback-based script
/// evaluation instead of a plain synchronous getter.
fn wait_until_login_form_shows(app: &Rc<AppState>, expected: &str) -> bool {
    use std::sync::{Arc, Mutex};
    let result: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let query = "document.getElementById('username').value + '|' + document.getElementById('password').value";
    wait_until(|| {
        // `evaluate_script_with_callback` (see `WryEngine::evaluate_script_for_test`'s
        // doc comment) delivers its result JSON-serialized — a JS string
        // comes back JSON-quoted (e.g. `"\"alice|hunter2\""`), not the bare
        // text, so this decodes it back to a plain string before comparing.
        let matched = result
            .lock()
            .unwrap()
            .as_deref()
            .and_then(|json| serde_json::from_str::<String>(json).ok())
            .as_deref()
            == Some(expected);
        if !matched {
            let slot = Arc::clone(&result);
            app.evaluate_script_on_active_page_for_test(query, move |value| {
                *slot.lock().unwrap() = Some(value);
            });
        }
        matched
    })
}

#[test]
fn credential_fill_populates_the_pages_login_form() {
    run_on_gtk_thread(|| {
        let profile = test_profile("credential-fill");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        assert!(app.try_open_vault_with("correct horse battery staple", true));

        let login_url = fixture_url("login_form.html");
        app.add_page(&login_url).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(login_url.as_str())));

        // `login_url` is a `file://` URL, so `domain_of` (a plain host
        // extractor, not URL-scheme-aware) computes an empty string for it
        // on both sides — using the exact same URL for the login's `site`
        // as the page actually open keeps the domain-match check consistent
        // regardless of what `domain_of` returns for this scheme.
        app.add_password_via_fields(&login_url, "alice", "hunter2", "");

        app.fill_active_page_with_local_login("alice");
        assert!(wait_until_login_form_shows(&app, "alice|hunter2"), "the login form's fields should reflect the filled values");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn credential_fill_prefers_autocomplete_attributes_over_position() {
    run_on_gtk_thread(|| {
        let profile = test_profile("credential-fill-autocomplete");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        assert!(app.try_open_vault_with("correct horse battery staple", true));

        // `login_form_autocomplete.html` is laid out so a purely positional
        // heuristic (first `input[type="password"]`, and whichever text-like
        // input most immediately precedes it) would pick "decoy_password"/
        // "decoy_text" instead — only `autocomplete="current-password"`/
        // `"username"` picks the real "password"/"username" fields correctly
        // here, regardless of position.
        let login_url = fixture_url("login_form_autocomplete.html");
        app.add_page(&login_url).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(login_url.as_str())));

        app.add_password_via_fields(&login_url, "alice", "hunter2", "");
        app.fill_active_page_with_local_login("alice");
        assert!(
            wait_until_login_form_shows(&app, "alice|hunter2"),
            "autocomplete-marked fields should be filled correctly even though position alone would pick the wrong ones"
        );

        cleanup_test_profile(&profile);
    });
}

#[test]
fn credential_fill_does_nothing_when_the_logins_domain_doesnt_match_the_active_page() {
    run_on_gtk_thread(|| {
        let profile = test_profile("credential-fill-mismatch");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        assert!(app.try_open_vault_with("correct horse battery staple", true));

        let login_url = fixture_url("login_form.html");
        app.add_page(&login_url).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(login_url.as_str())));

        // A real (non-empty-domain) site, deliberately not the active
        // page's own — the fill should be refused, not just unfilled by
        // coincidence.
        app.add_password_via_fields("https://not-the-active-page.example", "mallory", "shouldnt-appear", "");

        app.fill_active_page_with_local_login("mallory");
        // There's nothing to positively wait for here (a fill that correctly
        // never happens), so this waits only for the read-back itself to
        // complete, then asserts on its content.
        assert!(wait_until_login_form_shows(&app, "|"), "a domain mismatch should leave the login form untouched");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn edit_url_opens_switcher_with_current_url_selected_not_blanked() {
    run_on_gtk_thread(|| {
        let profile = test_profile("edit-url");
        let url_a = fixture_url("page_a.html");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));

        // Unlike open_switcher (blank, ready to filter), open_switcher_editing_url
        // is Ctrl+L's role: preload the current URL, fully selected, not blank.
        app.open_switcher_editing_url();
        assert!(app.is_switcher_open(), "open_switcher_editing_url should still show the grid underneath");
        assert_eq!(app.address_bar_text(), url_a, "the address bar should be preloaded with the current URL, not blanked");
        assert!(app.address_bar_is_fully_selected(), "the preloaded URL should be fully selected, ready to be typed over");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn focusing_address_bar_opens_the_switcher_preloaded_with_current_url() {
    run_on_gtk_thread(|| {
        let profile = test_profile("focus-opens-switcher");
        let url_a = fixture_url("page_a.html");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));
        assert!(!app.is_switcher_open(), "sanity check: the switcher shouldn't already be open");

        // Exercises the same `AppState::address_bar_focused` the address
        // bar's real `focus-in-event` handler calls — driven directly
        // rather than via a real focus change, since this headless test
        // compositor never actually gives the window real window-manager
        // focus (confirmed by direct experiment — see that method's doc
        // comment), so the underlying GDK signal never fires here at all.
        app.address_bar_focused();
        assert!(app.is_switcher_open(), "focusing the address bar should open the switcher");
        assert_eq!(
            app.address_bar_text(),
            url_a,
            "focusing should preload the current URL, same as Ctrl+L, not blank the bar"
        );
        assert!(app.address_bar_is_fully_selected(), "the preloaded URL should be fully selected, ready to be typed over");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn refocusing_address_bar_while_switcher_already_open_does_not_clobber_a_typed_filter() {
    run_on_gtk_thread(|| {
        let profile = test_profile("refocus-does-not-clobber");
        let url_a = fixture_url("page_a.html");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));

        app.open_switcher();
        app.set_address_bar_text("something the user is mid-typing");

        // Refocusing (e.g. clicking back into the bar) while the switcher is
        // already open must not reset it back to the current URL — only a
        // *fresh* focus-in (switcher not yet open) should do that.
        app.address_bar_focused();
        assert_eq!(
            app.address_bar_text(),
            "something the user is mid-typing",
            "refocusing an already-open switcher's address bar shouldn't clobber what's typed"
        );

        cleanup_test_profile(&profile);
    });
}

#[test]
fn focus_switcher_grid_moves_keyboard_focus_onto_a_tile() {
    run_on_gtk_thread(|| {
        let profile = test_profile("down-arrow-focuses-grid");
        let url_a = fixture_url("page_a.html");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));

        app.open_switcher();
        assert!(!app.switcher_grid_has_focused_tile(), "sanity check: focus starts in the address bar, not the grid");

        // Exercises the same `AppState::focus_switcher_grid` the address
        // bar's real Down-arrow key-press handler calls — driven directly
        // rather than via a synthetic key event, this suite's established
        // approach for GTK behavior (see this file's module doc comment).
        app.focus_switcher_grid();
        assert!(app.switcher_grid_has_focused_tile(), "should move keyboard focus onto a tile in the grid");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn ctrl_enter_forces_a_new_page_even_when_one_match_exists() {
    run_on_gtk_thread(|| {
        let profile = test_profile("ctrl-enter-force-open");
        let url_a = fixture_url("page_a.html");
        let url_b = fixture_url("page_b.html");

        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        let id_b = {
            app.add_page(&url_b).expect("add_page should succeed");
            app.active_id()
        };
        assert!(wait_until(|| app.page_title(&id_b).as_deref() == Some("Page B")));

        // Plain Enter with a single match switches to the existing page
        // instead of opening a new one (already covered by
        // `switcher_search_and_grid`) — Ctrl+Enter is the escape hatch from
        // that: it should open a brand-new page at the same URL instead.
        app.open_switcher();
        let count_before = app.page_ids().len();
        app.force_new_page_from_search("page b");
        assert_eq!(app.page_ids().len(), count_before + 1, "Ctrl+Enter should open a new page rather than switching to the match");
        let new_id = app.page_ids().last().cloned().unwrap_or_default();
        assert_ne!(new_id, id_b, "the new page should be a distinct page from the existing match");
        assert_eq!(app.active_id(), new_id, "the freshly opened page should become active");
        assert!(!app.is_switcher_open(), "Ctrl+Enter should close the switcher, same as plain Enter");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn ephemeral_profile_never_persists_and_marks_the_window_private() {
    run_on_gtk_thread(|| {
        let profile = Profile::ephemeral();
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        // Not checking `_window.title()` here: with a custom GtkHeaderBar as
        // the titlebar (which this app always sets), `gtk_window_get_title()`
        // doesn't reliably reflect what `build_window_and_app` passed to
        // `set_title` under this headless compositor — confirmed empirically
        // while writing this test, not merely suspected. `is_ephemeral()` is
        // the reliable, direct way to check this instead.
        assert!(app.is_ephemeral(), "an AppState built from an ephemeral profile should report itself as such");

        app.add_page(&fixture_url("page_a.html")).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().is_some()));

        app.toggle_bookmark_for_active();
        assert!(app.is_active_bookmarked(), "bookmarking should still work in-memory for the session");

        app.set_settings_start_page("https://should-not-persist.example");
        app.save_settings();

        // None of that should ever touch disk — an ephemeral profile never
        // gets a directory of its own at all, unlike a real named profile.
        assert!(profile.settings_path().map(|p| !p.exists()).unwrap_or(true), "settings should never be written to disk");
        assert!(profile.bookmarks_path().map(|p| !p.exists()).unwrap_or(true), "bookmarks should never be written to disk");
        assert!(profile.keybindings_path().map(|p| !p.exists()).unwrap_or(true), "keybindings should never be written to disk");
    });
}

#[test]
fn keybindings_editor_lives_inside_settings() {
    run_on_gtk_thread(|| {
        let profile = test_profile("keybindings-in-settings");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        // The keybindings editor is no longer its own overlay/toolbar
        // button — it's folded into settings, rebuilt every time settings
        // opens, one row per Action::ALL.
        app.open_settings();
        assert!(app.is_settings_open());
        assert_eq!(
            app.keybindings_row_count(),
            Action::ALL.len(),
            "opening settings should populate the keybindings editor with one row per action"
        );

        app.close_settings();
        assert!(!app.is_settings_open());

        cleanup_test_profile(&profile);
    });
}

#[test]
fn search_engine_management_add_and_remove() {
    run_on_gtk_thread(|| {
        let profile = test_profile("engine-management");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        app.open_settings();
        let default_count = app.settings_engine_names().len();
        assert_eq!(app.engines_row_count(), default_count, "the management list should start with one row per default engine");

        // Adding a new engine should update the real Settings data, the
        // management list, and the default-engine dropdown (the fix for the
        // dropdown previously always showing a fixed list regardless of
        // what Settings actually contained).
        app.add_search_engine_via_fields("Kagi", "https://kagi.com/search?q={query}");
        assert!(app.settings_engine_names().contains(&"Kagi".to_string()));
        assert_eq!(app.engines_row_count(), default_count + 1);

        // Re-adding the same name updates in place rather than duplicating.
        app.add_search_engine_via_fields("Kagi", "https://kagi.com/search?q={query}&updated=1");
        assert_eq!(app.engines_row_count(), default_count + 1, "re-adding an existing name shouldn't duplicate its row");

        // Removing every engine down to one should leave the dropdown
        // correctly reflecting whichever one engine remains as the default.
        while app.settings_engine_names().len() > 1 {
            let name = app.settings_engine_names()[0].clone();
            app.remove_search_engine_by_name(&name);
        }
        assert_eq!(app.settings_engine_names().len(), 1);
        assert_eq!(app.engine_combo_active_id().as_deref(), Some(app.settings_engine_names()[0].as_str()));

        cleanup_test_profile(&profile);
    });
}

#[test]
fn switcher_grid_shows_bookmark_matches_not_currently_open() {
    run_on_gtk_thread(|| {
        let profile = test_profile("grid-bookmarks");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        // Bookmarked directly rather than opened as a real page, so it can
        // never pick up a history entry (which would make the search match
        // via the history-tile path instead — already covered by its own
        // existing test) — isolates the bookmark-tile path specifically.
        app.bookmark_url_for_test("https://bookmarked-not-open.example", "Bookmarked Not Open");
        assert!(app.bookmarked_urls().contains(&"https://bookmarked-not-open.example".to_string()));

        app.open_switcher();
        app.set_address_bar_text("bookmarked not open");
        assert!(
            wait_until(|| app.switcher_grid_has_tile_with_class("bookmark-tile")),
            "searching for a bookmarked page should show it as a bookmark tile in the grid"
        );

        cleanup_test_profile(&profile);
    });
}

#[test]
fn switcher_grid_shows_lexically_similar_history_matches() {
    run_on_gtk_thread(|| {
        let profile = test_profile("grid-similar");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        // Recorded directly (not opened as a real page — the fixture
        // pages' titles are single words, not enough vocabulary for a
        // meaningful similarity comparison) with a title sharing most of
        // its vocabulary with the query below but no literal substring in
        // common ("guide" replaces "tutorial") — isolates the
        // similar-tile path from the (already-covered) plain history-tile
        // substring-match path.
        app.record_history_visit_for_test("https://rust-lang.org/tutorial", "Rust Programming Language Tutorial")
            .expect("record_history_visit_for_test should succeed");

        app.open_switcher();
        app.set_address_bar_text("rust programming language guide");
        assert!(
            wait_until(|| app.switcher_grid_has_tile_with_class("similar-tile")),
            "searching for a lexically similar (but not substring-matching) query should show a similar-history tile"
        );

        cleanup_test_profile(&profile);
    });
}

#[test]
fn screenshot_saves_a_real_png_file() {
    run_on_gtk_thread(|| {
        let profile = test_profile("screenshot");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&fixture_url("page_a.html")).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().is_some()));

        let path = std::env::temp_dir().join(format!("claude-browser-test-screenshot-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&path);

        // save_screenshot_to (the capture/write logic) rather than
        // take_screenshot (which shows a real, blocking native save dialog
        // that nothing in an automated test can drive).
        app.save_screenshot_to(path.clone());
        assert!(wait_until(|| path.exists()), "a screenshot file should be written");

        let bytes = std::fs::read(&path).expect("screenshot file should be readable");
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']), "the saved file should be a real PNG");

        let _ = std::fs::remove_file(&path);
        cleanup_test_profile(&profile);
    });
}

#[test]
fn switching_to_light_theme_reloads_the_theme_css() {
    run_on_gtk_thread(|| {
        let profile = test_profile("light-theme");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        // Dark is the default, so the theme provider should start out with
        // dark-theme rules — the switcher grid's `.history-tile` background
        // is the surface that's still actually theme-dependent (the
        // settings/profile/keybindings/bookmarks/passwords overlay boxes no
        // longer have a background of their own — see `base_provider`'s doc
        // comment in `build_window_and_app`). Checking for GTK's own
        // re-serialized form, not the literal source text — `CssProvider::
        // to_str()` returns the *parsed* stylesheet rendered back out in its
        // own canonical form, confirmed by inspecting the actual output
        // while writing this test, not assumed.
        assert!(
            app.theme_provider_css().contains("rgba(255,255,255,0.12)"),
            "the theme provider should start with the default dark theme's CSS"
        );

        app.open_settings();
        app.select_light_theme_radio();
        app.save_settings();

        assert!(
            app.theme_provider_css().contains("rgba(0,0,0,0.06)"),
            "saving with the light theme selected should reload the theme provider with light-theme CSS"
        );
        assert!(
            !app.theme_provider_css().contains("rgba(255,255,255,0.12)"),
            "the old dark-theme CSS shouldn't still be loaded after switching to light"
        );

        cleanup_test_profile(&profile);
    });
}

#[test]
fn encrypted_profile_records_visits_through_the_real_app() {
    run_on_gtk_thread(|| {
        let profile = test_profile("encrypted-app");
        let history = HistoryStore::open_encrypted(&profile, "a test passphrase").expect("open_encrypted should succeed");
        let (_window, app) =
            build_window_and_app_with_history(profile.clone(), history).expect("build_window_and_app_with_history should succeed");

        app.add_page(&fixture_url("page_a.html")).expect("add_page should succeed");
        let id_a = app.active_id();
        assert!(wait_until(|| app.page_title(&id_a).as_deref() == Some("Page A")));

        // A separate connection to the same encrypted database, opened with
        // the same passphrase, should see the visit the app's own
        // HistoryStore just recorded — proving the running app is actually
        // writing through to the real encrypted store, not silently
        // failing (record_visit's errors are only logged, not propagated).
        let verify =
            HistoryStore::open_encrypted(&profile, "a test passphrase").expect("reopening with the same passphrase should succeed");
        assert!(
            wait_until(|| !verify.search("page a", 10).unwrap_or_default().is_empty()),
            "a visit made through the app should be readable back from a separate connection to the same encrypted database"
        );

        cleanup_test_profile(&profile);
    });
}
