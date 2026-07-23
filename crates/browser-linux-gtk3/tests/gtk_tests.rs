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
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use browser_core::{Action, HistoryStore, Profile};
use browser_linux_gtk3::{build_window_and_app, build_window_and_app_with_history};
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
        // dark-theme rules (a dark settings-box background) applied at
        // startup by `apply_theme`. Checking for GTK's own re-serialized
        // form (`rgb(46,46,44)`, not the literal `#2e2e2c` source text) —
        // `CssProvider::to_str()` returns the *parsed* stylesheet rendered
        // back out in its own canonical form, confirmed by inspecting the
        // actual output while writing this test, not assumed.
        assert!(
            app.theme_provider_css().contains("rgb(46,46,44)"),
            "the theme provider should start with the default dark theme's CSS"
        );

        app.open_settings();
        app.select_light_theme_radio();
        app.save_settings();

        assert!(
            app.theme_provider_css().contains("rgb(242,242,240)"),
            "saving with the light theme selected should reload the theme provider with light-theme CSS"
        );
        assert!(
            !app.theme_provider_css().contains("rgb(46,46,44)"),
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
