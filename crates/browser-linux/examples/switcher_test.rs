use std::time::{Duration, Instant};

use browser_linux::build_window_and_app;

/// Generous ceiling for how long a webview-derived value (current URL,
/// document title) may take to settle after a navigation. Wide on purpose:
/// this only slows a run down when a check genuinely never becomes true (a
/// real failure) — `wait_until` returns the moment the condition is met, so
/// under normal load this costs milliseconds, not seconds.
const TIMEOUT: Duration = Duration::from_secs(10);

fn check(label: &str, ok: bool) -> bool {
    println!("[{}] {}", if ok { "PASS" } else { "FAIL" }, label);
    ok
}

/// Minimum extra time to keep pumping after `condition` first becomes true,
/// before trusting it. E.g. `active_url()` can report the destination URL
/// before WebKitGTK has actually finished registering that navigation in its
/// joint history stack — returning the instant a condition matches leaves
/// too little real time elapsed for whatever runs next (another navigation,
/// a search that reads other pages' settled state) to be reliable. Confirmed
/// empirically against nav_test.rs's go_back check: without this, polling
/// that returns as soon as the condition matches reproduces a real flake on
/// every run; with it, ten runs straight pass.
const SETTLE: Duration = Duration::from_millis(200);

/// Polls `condition` (pumping the GTK loop between attempts) until it's true
/// or `timeout` elapses. Use this for anything that depends on the embedded
/// webview's async state (URL after a navigation, title after a document
/// load) — plain app state (page list, active id, switcher visibility)
/// updates synchronously in Rust and never needs this.
fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
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

fn main() -> anyhow::Result<()> {
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/fixtures");
    let url_a = format!("file://{fixtures}/page_a.html");
    let url_b = format!("file://{fixtures}/page_b.html");
    let url_c = format!("file://{fixtures}/page_c.html");

    gtk::init()?;
    let (_window, app) = build_window_and_app()?;

    let mut all_ok = true;

    app.add_page(&url_a)?;
    all_ok &= check("adding first page opens exactly 1 page", app.page_ids().len() == 1);
    let id_a = app.page_ids()[0].clone();
    all_ok &= check("first page becomes active", app.active_id() == id_a);
    all_ok &= check(
        "stack's visible child matches active page",
        app.stack_visible_child_name().as_deref() == Some(id_a.as_str()),
    );
    all_ok &= check(
        "active url is page A",
        wait_until(TIMEOUT, || app.active_url().as_deref() == Some(url_a.as_str())),
    );
    all_ok &= check(
        "page A's title got tracked from the document",
        wait_until(TIMEOUT, || app.page_title(&id_a).as_deref() == Some("Page A")),
    );

    app.add_page(&url_b)?;
    all_ok &= check("adding second page grows the list to 2", app.page_ids().len() == 2);
    let id_b = app.page_ids()[1].clone();
    all_ok &= check("new page becomes active", app.active_id() == id_b);
    all_ok &= check(
        "active url is page B",
        wait_until(TIMEOUT, || app.active_url().as_deref() == Some(url_b.as_str())),
    );
    // The later "page b" search test matches on title (the URL has an
    // underscore, not a space, so it can't match "page b"), so page B's
    // title has to have settled before that point — wait for it here rather
    // than relying on incidental pumping from unrelated later waits.
    all_ok &= check(
        "page B's title got tracked from the document",
        wait_until(TIMEOUT, || app.page_title(&id_b).as_deref() == Some("Page B")),
    );

    app.add_page(&url_c)?;
    all_ok &= check("adding third page grows the list to 3", app.page_ids().len() == 3);
    let id_c = app.page_ids()[2].clone();
    all_ok &= check("third page is active", app.active_id() == id_c);
    all_ok &= check(
        "page C's title got tracked from the document",
        wait_until(TIMEOUT, || app.page_title(&id_c).as_deref() == Some("Page C")),
    );

    app.switch_to(&id_a);
    all_ok &= check("switching back to A updates active id", app.active_id() == id_a);
    all_ok &= check(
        "switching back to A updates the stack's visible child",
        app.stack_visible_child_name().as_deref() == Some(id_a.as_str()),
    );
    all_ok &= check(
        "switching back to A updates the address/url",
        wait_until(TIMEOUT, || app.active_url().as_deref() == Some(url_a.as_str())),
    );
    all_ok &= check("switching doesn't drop other pages", app.page_ids().len() == 3);

    // A is active; closing it should fall back to a remaining page, not vanish.
    app.close_page(&id_a);
    all_ok &= check("closing active page removes it from the list", app.page_ids().len() == 2);
    all_ok &= check("closing active page picks a new active page", app.active_id() != id_a);
    all_ok &= check(
        "the new active page is one of the remaining ones",
        app.active_id() == id_b || app.active_id() == id_c,
    );

    // open_switcher shows the panel with a cleared, focused search box (this
    // is what F1 / Ctrl+T / Ctrl+L and the grid button all trigger now).
    app.open_switcher();
    all_ok &= check("open_switcher shows the switcher panel", app.is_switcher_open());
    all_ok &= check(
        "open_switcher makes the background page stack insensitive",
        !app.is_background_page_interactive(),
    );

    // Typing a query that matches no open page and pressing Enter should
    // open a new page from it instead of doing nothing.
    let before_count = app.page_ids().len();
    app.search_activate("some-nonexistent-domain-example");
    all_ok &= check(
        "search box opens a new page when nothing matches",
        app.page_ids().len() == before_count + 1,
    );
    let new_id = app.page_ids().last().cloned().unwrap_or_default();
    all_ok &= check("the new page from search becomes active", app.active_id() == new_id);
    all_ok &= check(
        "the new page's url gets an https:// prefix added",
        // WebKitGTK normalizes a bare-domain URL by adding a trailing slash.
        wait_until(TIMEOUT, || {
            app.active_url().as_deref() == Some("https://some-nonexistent-domain-example/")
        }),
    );
    all_ok &= check("opening a page from search closes the switcher", !app.is_switcher_open());
    all_ok &= check(
        "closing the switcher restores background page interactivity",
        app.is_background_page_interactive(),
    );

    // Filtering down to exactly one matching page and pressing Enter should
    // switch to it (not create a duplicate).
    app.open_switcher();
    let existing_count = app.page_ids().len();
    app.search_activate("page b");
    all_ok &= check(
        "search box does not open a new page when a single match exists",
        app.page_ids().len() == existing_count,
    );
    all_ok &= check("search box switches to the single matching page", app.active_id() == id_b);
    all_ok &= check("switching via search closes the switcher", !app.is_switcher_open());

    // Filtering to a query matching MORE than one page shouldn't switch
    // anywhere (ambiguous) or create a duplicate.
    app.open_switcher();
    let active_before = app.active_id();
    app.search_activate("page");
    all_ok &= check(
        "search box does not open a new page when multiple pages match",
        app.page_ids().len() == existing_count,
    );
    all_ok &= check(
        "search box does not switch when multiple pages match",
        app.active_id() == active_before,
    );

    // Closing the ACTIVE page from an open grid should keep the grid open
    // and switch to the nearest remaining page, not dismiss the grid.
    app.open_switcher();
    let active_before_close = app.active_id();
    let count_before_close = app.page_ids().len();
    app.close_page(&active_before_close);
    all_ok &= check(
        "closing the active page from the grid keeps the grid open",
        app.is_switcher_open(),
    );
    all_ok &= check(
        "closing the active page from the grid removes it from the list",
        app.page_ids().len() == count_before_close - 1,
    );
    all_ok &= check(
        "closing the active page from the grid switches to a remaining page",
        app.active_id() != active_before_close,
    );

    // Real end-to-end check that loaded/unloaded tracking works through the
    // actual WryEngine-backed PageManager and AppState::set_max_loaded_pages
    // — browser-core's own unit tests for this only exercise a mock engine.
    let count_before_limit = app.page_ids().len();
    app.set_max_loaded_pages(Some(2));
    let loaded_after_limit = app.page_ids().iter().filter(|id| app.is_page_loaded(id)).count();
    all_ok &= check(
        "tightening the limit evicts down to it immediately",
        loaded_after_limit == count_before_limit.min(2),
    );

    app.add_page(&url_a)?;
    let loaded_after_new_page = app.page_ids().iter().filter(|id| app.is_page_loaded(id)).count();
    all_ok &= check("loading a new page past the limit evicts the oldest again", loaded_after_new_page == 2);
    let newest_id = app.page_ids().last().cloned().unwrap_or_default();
    all_ok &= check("the newly loaded page itself is loaded", app.is_page_loaded(&newest_id));

    app.set_max_loaded_pages(None);
    let loaded_after_unlimited = app.page_ids().iter().filter(|id| app.is_page_loaded(id)).count();
    all_ok &= check(
        "removing the limit doesn't retroactively reload anything unloaded",
        loaded_after_unlimited == 2,
    );

    // The `loaded` flag alone only proves bookkeeping, not real resource
    // reclamation. Tighten the limit to 1 to force an eviction, confirm the
    // evicted page's webview widget was actually torn down (its stack
    // container has zero children — the `loaded` checks above never assert
    // this), then confirm switching back to it rebuilds a live widget
    // reloaded at its original URL.
    app.set_max_loaded_pages(Some(1));
    let reclaimed_id = app
        .page_ids()
        .into_iter()
        .find(|id| !app.is_page_loaded(id))
        .expect("tightening to limit 1 with more than one open page should evict at least one");
    let reclaimed_url = app.page_url(&reclaimed_id).expect("an evicted page still remembers its last URL");
    all_ok &= check(
        "the evicted page's webview widget is actually torn down, not just flagged",
        app.page_container_child_count(&reclaimed_id) == 0,
    );

    app.switch_to(&reclaimed_id);
    all_ok &= check(
        "switching to an unloaded page rebuilds a live webview widget",
        app.page_container_child_count(&reclaimed_id) == 1,
    );
    all_ok &= check("switching to an unloaded page marks it loaded again", app.is_page_loaded(&reclaimed_id));
    all_ok &= check(
        "the rebuilt webview reloads the page's original URL",
        wait_until(TIMEOUT, || app.active_url().as_deref() == Some(reclaimed_url.as_str())),
    );

    app.set_max_loaded_pages(None);

    // Real end-to-end check that the toolbar address bar (not just the
    // switcher's search box) resolves non-URL input via the preferred
    // search engine — this path had zero coverage before
    // resolve_address_input existed (the address bar used to navigate with
    // raw, unresolved text, so a bare search phrase would just fail to load).
    app.address_bar_activate("how to cook rice");
    let resolved_search_url = wait_until(TIMEOUT, || {
        app.active_url().as_deref() == Some("https://www.google.com/search?q=how%20to%20cook%20rice")
    });
    all_ok &= check("toolbar address bar resolves multi-word input via the search engine", resolved_search_url);

    // Close down to zero: shouldn't panic, and should land on some fallback page.
    for id in app.page_ids() {
        app.close_page(&id);
    }
    all_ok &= check(
        "closing every page leaves exactly one fallback page instead of zero",
        app.page_ids().len() == 1,
    );

    println!("{}", if all_ok { "ALL PASS" } else { "SOME FAILED" });
    std::process::exit(if all_ok { 0 } else { 1 });
}
