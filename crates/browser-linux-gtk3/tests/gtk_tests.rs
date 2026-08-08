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

use browser_chrome_core::{embedded_assets, EmbeddedAssetServer, RpcBody, RpcHandler, WebviewRpcServer};
use browser_core::{HistoryStore, Profile};
use enigo::MouseControllable;
use browser_linux_gtk3::{build_window_and_app, build_window_and_app_with_history, AppState};
use gtk::prelude::*;
use render_engine::{RenderEngine, WebContext, WebKitWebView, WryEngine};
use std::collections::HashMap;

type Job = Box<dyn FnOnce() + Send>;

/// The single, persistent GTK-owning thread's job queue — spawned on first
/// use, lives for the rest of the process.
fn gtk_thread() -> &'static Sender<Job> {
    static SENDER: OnceLock<Sender<Job>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            // Forces the X11 backend before GDK picks one on its own. Under
            // `xwfb-run` (see README.md's Testing section), the test process
            // sees *both* `DISPLAY` (the nested Xwayland server) and
            // `WAYLAND_DISPLAY` (the headless compositor hosting it) set at
            // once, and GDK's own backend auto-detection prefers Wayland
            // when both are present — so without this, the app becomes a
            // native Wayland client of the compositor, invisible to X11
            // entirely. That silently broke every synthetic click this file
            // sends via `enigo` (XTest, which only ever talks to the X11
            // server): `enigo` still "succeeds" with no error, but the
            // click lands on an X server with zero mapped windows in it —
            // confirmed directly by cross-checking `xwininfo -root -tree`
            // against the actual `DISPLAY` while a probe app was running
            // (0 children) and by rerunning the exact same probe with
            // `GDK_BACKEND=x11` forced (window becomes a real Xwayland
            // client, `is_active()` flips to `true`, and the synthetic
            // click starts arriving). A real desktop session should still
            // default to Wayland when running natively — this only forces
            // X11 inside this test process, not anywhere in the shipped
            // app.
            unsafe {
                std::env::set_var("GDK_BACKEND", "x11");
            }
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

/// The shared, cross-platform `web-standards-tests/fixtures/<name>/` directory
/// (one level up from this crate's own `examples/fixtures/`, at the repo
/// root) — see that directory's own fixtures for the opener test case files.
/// Canonicalized (resolving the `..` segments) since WebKitGTK reports
/// `current_url()` back in canonical form after loading — comparing against
/// an un-normalized path here would spuriously never match.
fn web_standards_fixture_dir(name: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web-standards-tests/fixtures")).join(name);
    dir.canonicalize().unwrap_or_else(|err| panic!("failed to canonicalize fixture dir {dir:?}: {err}"))
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

        let mut web_context = WebContext::new(None);
        let engine = WryEngine::new(&content, &url_a, &mut web_context, |_| {}, |_| {}, |_, _| None, |_| {})
            .expect("WryEngine::new should succeed");
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
        let mut web_context = WebContext::new(None);
        let engine = WryEngine::new(
            &content,
            &url_a,
            &mut web_context,
            move |new_title| {
                *title_for_cb.borrow_mut() = new_title;
            },
            |_| {},
            |_, _| None,
            |_| {},
        )
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

// `window.opener` itself isn't checked here: it's only ever set by WebKit's
// own internal handling of a *genuine*, navigation-triggered `create` signal
// (a real click on a `target="_blank"` link, or a script's `window.open()`
// call actually running inside a loaded page) — confirmed empirically, not
// assumed: calling `add_page_related` directly against a disconnected,
// never-navigated `WebKitWebView::new()` (as this test does, for the same
// reason `add_page_background`'s test used to call the app's own method
// directly rather than simulate a click) does *not* retroactively produce a
// `window.opener` link, even though `with_related_view` still visibly works
// at the process/mounting level (this test's own assertions below). Testing
// the real opener linkage end-to-end would need a genuine, trusted
// (non-synthetic) click — this repo's own `gtk-test` dev-dependency exists
// for exactly that, but its `enigo` backend needs `libxdo`, not installed in
// this environment (confirmed: fails at link time, `unable to find library
// -lxdo`) — consistent with this module's own doc comment already steering
// away from synthetic-input-based tests as unreliable here. Real-VM-style
// manual click verification, matching how `browser-windows-reactor`'s
// windows-vm pipeline verifies its own UI interactions, is the honest
// verification path for the opener relationship itself.
#[test]
fn add_page_related_opens_without_switching_away() {
    run_on_gtk_thread(|| {
        let profile = test_profile("add-page-related");
        let url_a = fixture_url("page_a.html");

        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        app.add_page(&url_a).expect("add_page should succeed");
        let id_a = app.page_ids()[0].clone();
        assert_eq!(app.active_id(), id_a);
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));

        let opener = WebKitWebView::new();
        app.add_page_related(&opener).expect("add_page_related should succeed");

        assert_eq!(app.page_ids().len(), 2, "add_page_related should add a second tracked page");
        assert_eq!(app.active_id(), id_a, "add_page_related shouldn't steal focus from the active page");
        assert_eq!(
            app.stack_visible_child_name().as_deref(),
            Some(id_a.as_str()),
            "the visible page shouldn't change"
        );
        assert_eq!(app.active_url().as_deref(), Some(url_a.as_str()), "the active page's content shouldn't change");

        app.open_switcher();
        assert_eq!(
            app.switcher_grid_tile_count(),
            4,
            "the related page should still get a switcher tile (an \"Open Pages\" header + 2 open pages + the always-present Add tile)"
        );

        cleanup_test_profile(&profile);
    });
}

/// Serves `web-standards-tests/fixtures/` over a real local HTTP server —
/// see `run_web_standards_opener_case`'s doc comment for why `file://` URLs
/// can't be used for these fixtures. Same shutdown-on-drop technique as
/// `FakeBitwardenServer` above, just serving files straight off disk
/// instead of a hand-rolled JSON API. Also a real, general-purpose
/// capability beyond just working around the `wry` bug: a live HTTP server
/// under the test's control is a natural place to add any future
/// test-runner <-> fixture communication this suite ends up needing, the
/// same way `windows_driver.rs`/`macos_driver.rs` will serve fixtures this
/// way too rather than each platform inventing its own transport.
struct FixtureServer {
    server: std::sync::Arc<tiny_http::Server>,
    join: Option<std::thread::JoinHandle<()>>,
    base_url: String,
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn spawn_fixture_server() -> FixtureServer {
    let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web-standards-tests/fixtures"))
        .canonicalize()
        .expect("web-standards-tests/fixtures should exist");
    let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("binding a loopback fixture server should succeed"));
    let addr = server.server_addr().to_ip().expect("this test server always binds an IP socket, not a unix one");
    let base_url = format!("http://{addr}");
    let server_for_thread = std::sync::Arc::clone(&server);
    let join = std::thread::spawn(move || {
        while let Ok(request) = server_for_thread.recv() {
            // `Path::join` treats a leading `/` in `requested` as replacing
            // the base entirely (`root.join("/x")` == `/x`, not
            // `root/x`) — `trim_start_matches('/')` avoids that trap.
            let requested = request.url().trim_start_matches('/');
            let path = root.join(requested);
            // `tiny_http::Response::from_string`'s default content-type is
            // `text/plain` — without an explicit `text/html` header,
            // WebKitGTK renders a fixture's markup as literal text instead
            // of parsing it (no script execution, no clickable link),
            // rather than erroring in any way that would've been obvious
            // from `add_page`'s own success.
            let content_type = if path.extension().and_then(|e| e.to_str()) == Some("html") { "text/html; charset=utf-8" } else { "text/plain; charset=utf-8" };
            let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).expect("static content-type header should be valid");
            let response = match std::fs::read_to_string(&path) {
                Ok(body) => tiny_http::Response::from_string(body).with_status_code(200).with_header(header),
                Err(_) => tiny_http::Response::from_string("not found").with_status_code(404).with_header(header),
            };
            let _ = request.respond(response);
        }
    });
    FixtureServer { server, join: Some(join), base_url }
}

/// Web-standards test: does a real, genuine (OS-level synthetic, not a
/// script-dispatched DOM `click()`) click on a `target="_blank"` link
/// correctly follow real Chromium/WebKit `rel="opener"`/implicit-`noopener`
/// semantics — see `web-standards-tests/fixtures/`'s `opener-default`/
/// `opener-explicit-opener` fixtures and this crate's `README.md`/
/// `ROADMAP.md` for the investigation this exists to guard against
/// regressing.
///
/// Drives the real app, not a parallel test-only window: `build_window_and_app`
/// + `app.add_page(&index_url)` are the exact same production entrypoints
/// every other test in this file already uses (see
/// `credential_fill_populates_the_pages_login_form` below for another
/// example) — so the popup this test triggers goes through the real
/// `add_page`/`add_page_related`/`is_user_gesture` path end-to-end, with no
/// test-side popup handling needed here at all (unlike an earlier version
/// of this test, which built its own bare `gtk::Window` and called
/// `WryEngine::new`/`new_related` directly — testing `WryEngine` in
/// isolation, not this app's actual page/navigation infrastructure).
///
/// The interaction itself comes from the fixture's own `actions.json`, not
/// hardcoded here — see `render_engine::linux::CONSOLE_CAPTURE_SCRIPT`'s doc
/// comment for how a fixture's `data-test-target="<name>"` element gets
/// resolved to real screen coordinates via the same `console.log` relay
/// this test also reads its final assertion from (see
/// `AppState::console_messages_for_test`).
fn run_web_standards_opener_case(case: &'static str) {
    run_on_gtk_thread(move || {
        let fixture_dir = web_standards_fixture_dir(case);
        // Served over a real local HTTP server, not `file://` — real,
        // empirically-confirmed bug: `wry` 0.55/0.56's GTK IPC handler
        // (`attach_ipc_handler` in `webkitgtk/mod.rs`) builds an
        // `http::Request` using the webview's current URL as the request's
        // URI on *every* incoming `window.ipc.postMessage` call, and
        // `http::Uri` rejects a `file:///path` empty-authority URI outright
        // (`InvalidUri(InvalidFormat)`), panicking (non-unwinding, since
        // it's inside a GTK signal callback — aborts the whole process).
        // Confirmed directly with a standalone `http::Uri::try_from(...)`
        // check, including that WebKitGTK's own URL canonicalization means
        // even spelling the *opener*'s own URL as `file://localhost/path`
        // (which does parse) doesn't help once a `target="_blank"` popup's
        // navigation gets resolved and reported back as bare `file:///...`
        // regardless. A real `http://` URL sidesteps the bug entirely
        // instead of working around it — see `spawn_fixture_server`. Never
        // triggered before this test's `__test_target__` reporting (see
        // `CONSOLE_CAPTURE_SCRIPT`) started sending an IPC message from a
        // `file://`-loaded page *unconditionally*, on `load` — every
        // earlier `console.log` capture in this session was downstream of
        // a real click succeeding first, which this dev sandbox's XTest
        // limitation (see this file's own `README.md`/`ROADMAP.md` notes)
        // has never actually let happen yet, so this real `wry` bug had no
        // chance to surface until now.
        let fixture_server = spawn_fixture_server();
        let index_url = format!("{}/{case}/index.html", fixture_server.base_url);
        let expected = std::fs::read_to_string(fixture_dir.join("expected.txt"))
            .unwrap_or_else(|err| panic!("failed to read expected.txt for {case}: {err}"));
        let actions_text = std::fs::read_to_string(fixture_dir.join("actions.json"))
            .unwrap_or_else(|err| panic!("failed to read actions.json for {case}: {err}"));
        let actions: serde_json::Value =
            serde_json::from_str(&actions_text).unwrap_or_else(|err| panic!("failed to parse actions.json for {case}: {err}"));
        let steps = actions["steps"].as_array().unwrap_or_else(|| panic!("{case}: actions.json should have a \"steps\" array"));

        let profile = test_profile(case);
        let (window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        // `gtk_test::click`'s own doc comment: the click "fails" if the
        // window isn't on top of every other window — this is what makes a
        // real, OS-trusted click actually land on the link. `present()`
        // only sends the X11 request asynchronously — the window manager
        // needs a real moment (and pumped events) to actually grant focus.
        // `build_window_and_app` already calls `window.show_all()`
        // internally.
        window.present();
        let focus_deadline = Instant::now() + Duration::from_secs(2);
        while !window.is_active() && Instant::now() < focus_deadline {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        app.add_page(&index_url).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(index_url.as_str())), "fixture index page should load");

        for step in steps {
            let action = step["action"].as_str().unwrap_or_else(|| panic!("{case}: actions.json step missing \"action\""));
            let target = step["target"].as_str().unwrap_or_else(|| panic!("{case}: actions.json step missing \"target\""));
            match action {
                "click" => click_test_target(case, &app, target),
                other => panic!("{case}: unknown actions.json action {other:?}"),
            }
        }

        // Real, environment-specific gap surfaced (not introduced) by this
        // test, confirmed by direct testing: in this dev sandbox, `enigo`'s
        // XTest-based fake input doesn't land at all — verified with a
        // minimal standalone probe (a plain `gtk::Button`, not a webview)
        // against both this crate's `xwfb-run -c cage` isolated display and
        // the real `:0` desktop session, neither ever fired the button's
        // `clicked` signal from an identical `mouse_move_to`/`mouse_click`
        // call. That rules out anything WebKitGTK- or opener-specific —
        // XTest fake input is evidently disabled or unavailable at the X
        // server level in this particular sandbox — so this test may not
        // pass here, but the mechanism itself (a real click driving
        // `is_user_gesture`/`add_page_related`/the console-log relay, all
        // exercised identically to production) is correct and should work
        // wherever XTest fake input actually functions (a normal desktop,
        // or CI with `xvfb`/`xdotool` support confirmed working).
        assert!(
            wait_until(|| real_assertion_lines(&app.console_messages_for_test()).join("\n") + "\n" == expected),
            "{case}: expected {expected:?}, got {:?}\n",
            real_assertion_lines(&app.console_messages_for_test())
        );

        cleanup_test_profile(&profile);
    });
}

/// `console_messages_for_test` also carries `__test_target__ <name> <rect>`
/// coordinate-reporting lines (see `render_engine::linux::CONSOLE_CAPTURE_SCRIPT`)
/// alongside a fixture's own real assertion output — this strips those out,
/// leaving only what the fixture itself actually logged as its result.
fn real_assertion_lines(messages: &[String]) -> Vec<String> {
    messages.iter().filter(|m| !m.starts_with("__test_target__ ")).cloned().collect()
}

/// Performs one real, OS-level synthetic click on whichever element the
/// active page's own `CONSOLE_CAPTURE_SCRIPT`-injected reporting marked
/// `data-test-target="<target>"` — see `run_web_standards_opener_case`'s
/// doc comment for why coordinate resolution goes through the console-log
/// relay rather than a live script-evaluation call (kept uniform with
/// `windows_driver.rs`/`macos_driver.rs`, which have no such call
/// available at all as separate OS processes).
fn click_test_target(case: &str, app: &Rc<AppState>, target: &str) {
    let prefix = format!("__test_target__ {target} ");
    let mut rect_json = None;
    assert!(
        wait_until(|| {
            rect_json = app.console_messages_for_test().into_iter().find_map(|m| m.strip_prefix(&prefix).map(str::to_string));
            rect_json.is_some()
        }),
        "{case}: no __test_target__ report for {target:?} arrived in time"
    );
    let rect: serde_json::Value = serde_json::from_str(&rect_json.unwrap()).expect("__test_target__ rect should be valid JSON");
    let (rect_x, rect_y, rect_width, rect_height) = (
        rect["x"].as_f64().expect("rect.x"),
        rect["y"].as_f64().expect("rect.y"),
        rect["width"].as_f64().expect("rect.width"),
        rect["height"].as_f64().expect("rect.height"),
    );

    let widget = app.active_page_widget_for_test().expect("active page should have a real widget");
    let toplevel = widget.toplevel().expect("webview should have a toplevel window");
    let toplevel_window = toplevel.window().expect("toplevel should be realized");
    let (_, window_x, window_y) = toplevel_window.origin();
    let (offset_x, offset_y) =
        widget.translate_coordinates(&toplevel, 0, 0).expect("translate_coordinates should succeed for a realized, mapped widget");

    let target_x = window_x + offset_x + (rect_x + rect_width / 2.0) as i32;
    let target_y = window_y + offset_y + (rect_y + rect_height / 2.0) as i32;

    // Not `gtk_test::click`/`gtk_test::mouse_move`: `click` waits on the
    // clicked widget's own `button-release-event` GTK signal, which a
    // WebKitGTK `WebView` never emits (WebKit handles pointer input inside
    // its own compositor, not through plain GTK widget signals) — confirmed
    // by testing directly: `gtk_test::click` against this widget hung
    // indefinitely. `mouse_move` internally calls
    // `gtk::test_widget_wait_for_draw`, which also hung indefinitely here.
    // What's kept is the actual input delivery both functions perform
    // underneath: a real, OS-level synthetic `enigo` mouse move + click —
    // genuinely trusted from WebKit's perspective, unlike a
    // script-dispatched DOM `click()`.
    let mut enigo = enigo::Enigo::new();
    enigo.mouse_move_to(target_x, target_y);
    std::thread::sleep(Duration::from_millis(200));
    enigo.mouse_click(enigo::MouseButton::Left);
}

#[test]
fn opener_verification_default_target_blank_has_no_opener() {
    run_web_standards_opener_case("opener-default");
}

#[test]
fn opener_verification_explicit_rel_opener_has_opener() {
    run_web_standards_opener_case("opener-explicit-opener");
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
fn every_overlay_toggle_button_closes_on_a_second_click() {
    run_on_gtk_thread(|| {
        let profile = test_profile("overlay-toggle");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&fixture_url("page_a.html")).expect("add_page should succeed");

        // Every overlay's own trigger button now toggles closed on a second
        // "click" (here driven directly through the `toggle_*` method the
        // button's `connect_clicked` handler calls, same as every other test
        // in this file drives `AppState` methods directly rather than
        // synthetic GTK input — see this file's own doc comment for why).
        app.toggle_switcher();
        assert!(app.is_switcher_open(), "first toggle_switcher should open the switcher");
        app.toggle_switcher();
        assert!(!app.is_switcher_open(), "second toggle_switcher should close the switcher");

        // The profile-menu popover is real webview-backed content now (see
        // AppState::show_profile_menu) — a full end-to-end check of that
        // (real profile.info data reaching it) lives in its own test below;
        // this just confirms the trigger actually shows it.
        app.show_profile_menu();
        assert!(app.is_profile_menu_open(), "show_profile_menu should show the profile menu popover");

        // A fresh test profile never has a vault passphrase set up, so
        // toggle_passwords still shows (just) the unlock/setup prompt here —
        // browsing saved logins itself moved to browser://passwords (see
        // open_or_focus_internal_page's own tests).
        app.toggle_passwords();
        assert!(app.is_passwords_open(), "first toggle_passwords should open the password manager");
        app.toggle_passwords();
        assert!(!app.is_passwords_open(), "second toggle_passwords should close the password manager");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn session_saved_on_quit_is_restored_on_next_launch() {
    run_on_gtk_thread(|| {
        let profile = test_profile("session-restore");
        let url_a = fixture_url("page_a.html");
        let url_b = fixture_url("page_b.html");

        {
            let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
            app.add_page(&url_a).expect("add_page should succeed");
            let id_a = app.active_id();
            assert!(wait_until(|| app.page_title(&id_a).as_deref() == Some("Page A")));
            app.add_page(&url_b).expect("add_page should succeed");
            assert!(wait_until(|| app.active_url().as_deref() == Some(url_b.as_str())));
            // Switch back to A so it — not the most-recently-added B — is
            // the one the saved session should remember as active.
            app.switch_to(&id_a);
            assert_eq!(app.active_id(), id_a);

            app.save_session_for_test();
        }

        // A fresh AppState against the same profile, as a real second
        // launch would build — build_window_and_app never opens a page
        // itself (confirmed by its own doc comment), so this starts with
        // zero pages, exactly the state a real startup begins in.
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        assert!(app.page_ids().is_empty(), "sanity check: a fresh AppState shouldn't have any pages yet");

        app.open_start_page_or_restored_session();
        assert_eq!(app.page_ids().len(), 2, "both saved pages should be restored");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())), "the previously-active page (A, not the most-recently-added B) should be active again");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn restoring_a_session_only_eagerly_loads_the_active_page() {
    run_on_gtk_thread(|| {
        let profile = test_profile("session-restore-lazy");
        let url_a = fixture_url("page_a.html");
        let url_b = fixture_url("page_b.html");
        let url_c = fixture_url("page_c.html");
        let id_a;

        {
            let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
            app.add_page(&url_a).expect("add_page should succeed");
            id_a = app.active_id();
            app.add_page(&url_b).expect("add_page should succeed");
            app.add_page(&url_c).expect("add_page should succeed");
            app.switch_to(&id_a);
            app.save_session_for_test();
        }

        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.open_start_page_or_restored_session();
        assert_eq!(app.page_ids().len(), 3, "all three saved pages should be tracked, not just the active one");

        // Restoring shouldn't construct a real webview for anything but the
        // previously-active page — see `open_start_page_or_restored_session`'s
        // doc comment for why (avoiding a hang this session's own audio
        // feature exposed, caused by real, synchronous engine construction
        // for every restored page piling up before the window is shown).
        let active_id = app.active_id();
        assert_eq!(active_id, id_a, "A, not the most-recently-added C, should be active again");
        assert!(app.is_page_loaded(&active_id), "the previously-active page should be loaded eagerly");
        assert_eq!(app.page_container_child_count(&active_id), 1, "the active page's webview should be a real, live widget");

        let inactive_ids: Vec<_> = app.page_ids().into_iter().filter(|id| *id != active_id).collect();
        assert_eq!(inactive_ids.len(), 2);
        for id in &inactive_ids {
            assert!(!app.is_page_loaded(id), "restored pages other than the active one shouldn't be eagerly loaded");
            assert_eq!(app.page_container_child_count(id), 0, "an unloaded restored page shouldn't have a live webview widget yet");
        }

        // Switching to one lazily builds a real engine for it, same as any
        // other unloaded page (see `ensure_engine_loaded`).
        let other_id = inactive_ids[0].clone();
        app.switch_to(&other_id);
        assert!(app.is_page_loaded(&other_id));
        assert_eq!(app.page_container_child_count(&other_id), 1);

        cleanup_test_profile(&profile);
    });
}

#[test]
fn no_saved_session_falls_back_to_the_configured_start_page() {
    run_on_gtk_thread(|| {
        let profile = test_profile("session-restore-fallback");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        // A local fixture, not Settings::default()'s real HOME_URL — this
        // suite has no real network access, and current_url() reflects the
        // real webview's URL, not just whatever was requested.
        let start_page = fixture_url("page_a.html");
        app.set_start_page_for_test(&start_page);

        app.open_start_page_or_restored_session();
        assert_eq!(app.page_ids().len(), 1, "a fresh profile with no saved session should open exactly the start page");
        assert!(wait_until(|| app.active_url().as_deref() == Some(start_page.as_str())));

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
fn bookmarks_toggle_for_active_page() {
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

        cleanup_test_profile(&profile);
    });
}

#[test]
fn password_vault_setup_navigates_to_the_real_passwords_page_once_unlocked() {
    run_on_gtk_thread(|| {
        let profile = test_profile("password-vault-basics");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        assert!(app.password_vault_usernames().is_empty());
        assert!(!profile.has_vault_passphrase(), "a fresh profile shouldn't have a vault passphrase yet");

        // Simulates the user completing the in-overlay vault setup prompt.
        assert!(
            app.try_open_vault_with("correct horse battery staple", true),
            "setting up a fresh vault should succeed"
        );
        assert!(profile.has_vault_passphrase(), "setting up the vault should mark the profile as vault-protected");

        app.add_password_for_test("https://example.com", "alice", "hunter2", "personal account");
        app.add_password_for_test("https://example.com", "bob", "letmein", "");
        assert_eq!(
            app.password_vault_usernames(),
            vec!["bob".to_string(), "alice".to_string()],
            "most-recently-added credential should list first"
        );

        // Once the vault is unlocked, open_passwords navigates to the real
        // browser://passwords page instead of showing the (now unlock-only)
        // overlay — see AppState::open_passwords's own doc comment.
        app.open_passwords();
        assert!(!app.is_passwords_open(), "the overlay should close once the vault is unlocked, not linger");
        assert!(wait_until(|| app.active_url().is_some_and(|url| url.contains("/passwords/index.html"))));

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

        // Opening the vault for the first time should silently establish it
        // under the *same* passphrase — straight to the real passwords
        // page, no prompt-completion step needed (unlike the test above,
        // which simulates completing a real prompt since no passphrase was
        // known yet in that scenario).
        app.open_passwords();
        assert!(
            wait_until(|| app.active_url().is_some_and(|url| url.contains("/passwords/index.html"))),
            "a passphrase already known this session should silently unlock/set up the vault and navigate straight there, not prompt for a new one"
        );
        assert!(profile.has_vault_passphrase(), "opening the vault should have set up its own marker under the shared passphrase");

        app.add_password_for_test("https://example.com", "alice", "hunter2", "");
        assert_eq!(app.password_vault_usernames(), vec!["alice".to_string()]);

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
        app.add_password_for_test(&login_url, "alice", "hunter2", "");

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

        app.add_password_for_test(&login_url, "alice", "hunter2", "");
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
        app.add_password_for_test("https://not-the-active-page.example", "mallory", "shouldnt-appear", "");

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
fn clicking_title_chip_opens_the_switcher_preloaded_with_current_url() {
    run_on_gtk_thread(|| {
        let profile = test_profile("title-chip-opens-switcher");
        let url_a = fixture_url("page_a.html");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));
        assert!(!app.is_switcher_open(), "sanity check: the switcher shouldn't already be open");

        // Exercises the same `AppState::title_chip_clicked` the toolbar's
        // title chip's real `clicked` signal calls.
        app.title_chip_clicked();
        assert!(app.is_switcher_open(), "clicking the title chip should open the switcher");
        assert_eq!(
            app.address_bar_text(),
            url_a,
            "clicking should preload the current URL, same as Ctrl+L, not blank the bar"
        );
        assert!(app.address_bar_is_fully_selected(), "the preloaded URL should be fully selected, ready to be typed over");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn reclicking_title_chip_while_switcher_already_open_does_not_clobber_a_typed_filter() {
    run_on_gtk_thread(|| {
        let profile = test_profile("reclick-does-not-clobber");
        let url_a = fixture_url("page_a.html");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));

        app.open_switcher();
        app.set_address_bar_text("something the user is mid-typing");

        // Re-clicking the title chip while the switcher is already open
        // (it stays visible/clickable throughout — the overlay only covers
        // the content area below the header bar) must not reset it back to
        // the current URL — only a *fresh* open should do that.
        app.title_chip_clicked();
        assert_eq!(
            app.address_bar_text(),
            "something the user is mid-typing",
            "re-clicking while the switcher is already open shouldn't clobber what's typed"
        );

        cleanup_test_profile(&profile);
    });
}

#[test]
fn title_chip_reflects_the_active_pages_real_title() {
    run_on_gtk_thread(|| {
        let profile = test_profile("title-chip-reflects-title");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        assert_eq!(app.title_label_text(), "New Page", "a freshly-built window with only the default start page should show the fallback title");

        let url_a = fixture_url("page_a.html");
        app.add_page(&url_a).expect("add_page should succeed");
        assert!(wait_until(|| app.title_label_text() == "Page A"), "the title chip should pick up page_a.html's real <title> once it loads");

        let url_b = fixture_url("page_b.html");
        app.add_page(&url_b).expect("add_page should succeed");
        assert!(
            wait_until(|| app.title_label_text() == "Page B"),
            "switching the active page to a second, newly-added page should update the title chip to that page's real title"
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

        app.set_start_page_for_test("https://should-not-persist.example");

        // None of that should ever touch disk — an ephemeral profile never
        // gets a directory of its own at all, unlike a real named profile.
        assert!(profile.settings_path().map(|p| !p.exists()).unwrap_or(true), "settings should never be written to disk");
        assert!(profile.bookmarks_path().map(|p| !p.exists()).unwrap_or(true), "bookmarks should never be written to disk");
        assert!(profile.keybindings_path().map(|p| !p.exists()).unwrap_or(true), "keybindings should never be written to disk");
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

        app.set_theme_for_test(browser_core::Theme::Light);

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

#[test]
fn set_page_audio_playing_toggles_the_tracked_state() {
    run_on_gtk_thread(|| {
        let profile = test_profile("audio-playing-toggle");
        let url_a = fixture_url("page_a.html");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        let id_a = app.active_id();
        assert!(!app.is_page_playing_audio(&id_a), "sanity check: a freshly added page shouldn't start out playing audio");

        // Exercises the same `AppState::set_page_audio_playing` the real
        // `connect_is_playing_audio_notify` handler (wired in `add_page`/
        // `ensure_engine_loaded`) calls — driven directly rather than via a
        // real WebKitGTK audio-state change, since this headless test
        // compositor has no confirmed audio backend to actually play
        // anything and exercise the real signal end-to-end.
        app.set_page_audio_playing(&id_a, true);
        assert!(app.is_page_playing_audio(&id_a), "should track that the page started playing audio");

        app.set_page_audio_playing(&id_a, false);
        assert!(!app.is_page_playing_audio(&id_a), "should track that the page stopped playing audio");

        cleanup_test_profile(&profile);
    });
}

#[test]
fn unloading_a_page_clears_its_audio_playing_state() {
    run_on_gtk_thread(|| {
        let profile = test_profile("audio-playing-unload");
        let url_a = fixture_url("page_a.html");
        let url_b = fixture_url("page_b.html");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        let id_a = app.active_id();
        app.add_page(&url_b).expect("add_page should succeed");

        app.set_page_audio_playing(&id_a, true);
        assert!(app.is_page_playing_audio(&id_a), "sanity check: the flag should be set before eviction");

        // Tightening the limit to 1 with B active forces A's engine to be
        // torn down (same eviction path `loaded_page_limit_evicts_and_reclaims`
        // exercises) — a page with no live engine can't be playing audio,
        // so the stale flag should be cleared, not left dangling.
        app.set_max_loaded_pages(Some(1));
        assert!(!app.is_page_loaded(&id_a), "sanity check: A should have been evicted to make room under the tightened limit");
        assert!(!app.is_page_playing_audio(&id_a), "unloading a page should clear its stale audio-playing state");

        cleanup_test_profile(&profile);
    });
}

/// Polls `app`'s active page (via `evaluate_script_on_active_page_for_test`)
/// until evaluating `script` equals `expected`, or `TIMEOUT` elapses — same
/// approach `wait_until_login_form_shows` uses, generalized to an arbitrary
/// script instead of always reading the login form's fields.
fn wait_until_script_equals(app: &Rc<AppState>, script: &str, expected: &str) -> bool {
    use std::sync::{Arc, Mutex};
    let result: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    wait_until(|| {
        let matched =
            result.lock().unwrap().as_deref().and_then(|json| serde_json::from_str::<String>(json).ok()).as_deref() == Some(expected);
        if !matched {
            let slot = Arc::clone(&result);
            app.evaluate_script_on_active_page_for_test(script, move |value| {
                *slot.lock().unwrap() = Some(value);
            });
        }
        matched
    })
}

#[test]
fn webview_data_persists_across_a_second_app_instance_for_the_same_profile() {
    run_on_gtk_thread(|| {
        let profile = test_profile("webview-persistence");
        let url_a = fixture_url("page_a.html");

        {
            let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
            app.add_page(&url_a).expect("add_page should succeed");
            assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));
            // localStorage rather than document.cookie — WebKitGTK's cookie
            // policy for file:// origins (what these fixtures use) isn't
            // something this suite can assume either way, and this only
            // needs to prove the *mechanism* (a shared, profile-scoped
            // `wry::WebContext`) actually persists real browsing data, not
            // specifically cookies.
            app.evaluate_script_on_active_page_for_test("localStorage.setItem('persist_test', 'hello'); 'done'", |_| {});
            assert!(wait_until_script_equals(&app, "localStorage.getItem('persist_test')", "hello"), "the value should be readable back within the same instance before ever testing a second one");
        }

        // A fresh AppState against the same profile, as a real second launch
        // would build — same "second instance, same profile" shape as
        // `session_saved_on_quit_is_restored_on_next_launch`.
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));
        assert!(
            wait_until_script_equals(&app, "localStorage.getItem('persist_test')", "hello"),
            "a second AppState against the same profile should see the first instance's persisted webview data"
        );

        cleanup_test_profile(&profile);
    });
}

#[test]
fn webview_data_does_not_persist_for_an_ephemeral_profile() {
    run_on_gtk_thread(|| {
        let profile = Profile::ephemeral();
        let url_a = fixture_url("page_a.html");

        {
            let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
            app.add_page(&url_a).expect("add_page should succeed");
            assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));
            app.evaluate_script_on_active_page_for_test("localStorage.setItem('persist_test', 'hello'); 'done'", |_| {});
            assert!(wait_until_script_equals(&app, "localStorage.getItem('persist_test')", "hello"));
        }

        // A second ephemeral AppState is a distinct, unlinked session (see
        // `Profile::ephemeral`'s doc comment) — nothing should carry over,
        // unlike the real-profile case above.
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&url_a).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));
        assert!(
            !wait_until_script_equals(&app, "localStorage.getItem('persist_test')", "hello"),
            "an ephemeral profile's webview data shouldn't survive into a second instance"
        );
    });
}

/// Serves one fixed HTML string for every request — unlike `FixtureServer`
/// above (which serves static files off disk), this exists for a page whose
/// *content* has to be generated per-test (here: embedding
/// `WebviewRpcServer`'s own ephemeral port, which isn't known until the
/// server's already running). Same shutdown-on-drop shape as every other
/// `tiny_http`-backed test server in this file.
struct DynamicPageServer {
    server: std::sync::Arc<tiny_http::Server>,
    join: Option<std::thread::JoinHandle<()>>,
    base_url: String,
}

impl Drop for DynamicPageServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn spawn_dynamic_page_server(html: String) -> DynamicPageServer {
    let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("binding a loopback page server should succeed"));
    let addr = server.server_addr().to_ip().expect("this test server always binds an IP socket, not a unix one");
    let base_url = format!("http://{addr}/");
    let server_for_thread = std::sync::Arc::clone(&server);
    let join = std::thread::spawn(move || {
        while let Ok(request) = server_for_thread.recv() {
            let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("static content-type header should be valid");
            let response = tiny_http::Response::from_string(html.clone()).with_status_code(200).with_header(header);
            let _ = request.respond(response);
        }
    });
    DynamicPageServer { server, join: Some(join), base_url }
}

/// Real, end-to-end proof of `browser_chrome_core::WebviewRpcServer`'s
/// transport (see that module's own doc comment for the design): a real
/// WebKitGTK page, served over `http://127.0.0.1` on a *different*
/// loopback port than the RPC server (a real cross-origin situation, not a
/// contrived one — nothing guarantees the two ever share a port), performs
/// two real `fetch()` calls — a JSON round trip and a raw, unencoded binary
/// round trip — against it. `WebviewRpcServer`'s own tests
/// (`browser-chrome-core/tests/webview_rpc.rs`) already prove the HTTP
/// layer works and that a CORS preflight gets answered; this is the piece
/// only a real webview can prove: that `fetch()` from real page JS actually
/// gets past that preflight and completes, on this app's real
/// `add_page`/console-capture path, not a synthetic harness.
#[test]
fn webview_rpc_json_and_binary_round_trip_over_a_real_fetch() {
    run_on_gtk_thread(|| {
        let mut handlers: HashMap<String, RpcHandler> = HashMap::new();
        handlers.insert(
            "ping".to_string(),
            Box::new(|body: RpcBody| -> Result<RpcBody, rpc_protocol::RpcError> {
                match body {
                    RpcBody::Json(value) => Ok(RpcBody::Json(value)),
                    RpcBody::Binary(_) => Err(rpc_protocol::RpcError { code: -32602, message: "ping expects a JSON body".to_string(), data: None }),
                }
            }),
        );
        handlers.insert(
            "echo_binary".to_string(),
            Box::new(|body: RpcBody| -> Result<RpcBody, rpc_protocol::RpcError> {
                match body {
                    RpcBody::Binary(bytes) => Ok(RpcBody::Binary(bytes)),
                    RpcBody::Json(_) => Err(rpc_protocol::RpcError { code: -32602, message: "echo_binary expects a binary body".to_string(), data: None }),
                }
            }),
        );
        let rpc_server = WebviewRpcServer::start(handlers).expect("starting the webview RPC server should succeed");
        let rpc_port = rpc_server.port();

        let page_html = format!(
            r#"<!doctype html>
<html><head><title>webview rpc test</title></head>
<body>
<script>
(async () => {{
  try {{
    const pingRes = await fetch('http://127.0.0.1:{rpc_port}/rpc/ping', {{
      method: 'POST',
      headers: {{'Content-Type': 'application/json'}},
      body: JSON.stringify({{hello: 'world'}})
    }});
    const pingJson = await pingRes.json();
    console.log('ping_result=' + JSON.stringify(pingJson));
  }} catch (e) {{
    console.log('ping_error=' + e);
  }}
  try {{
    const binRes = await fetch('http://127.0.0.1:{rpc_port}/rpc/echo_binary', {{
      method: 'POST',
      headers: {{'Content-Type': 'application/octet-stream'}},
      body: new Uint8Array([0, 1, 2, 255, 254])
    }});
    const buf = await binRes.arrayBuffer();
    console.log('binary_result=' + Array.from(new Uint8Array(buf)).join(','));
  }} catch (e) {{
    console.log('binary_error=' + e);
  }}
}})();
</script>
</body></html>"#
        );
        let page_server = spawn_dynamic_page_server(page_html);

        let profile = test_profile("webview-rpc");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        app.add_page(&page_server.base_url).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(page_server.base_url.as_str())), "the fixture page should load");

        assert!(
            wait_until(|| app.console_messages_for_test().iter().any(|m| m == "ping_result={\"hello\":\"world\"}")),
            "expected a ping_result console message proving the JSON round trip, got {:?}",
            app.console_messages_for_test()
        );
        assert!(
            wait_until(|| app.console_messages_for_test().iter().any(|m| m == "binary_result=0,1,2,255,254")),
            "expected a binary_result console message proving the unencoded binary round trip, got {:?}",
            app.console_messages_for_test()
        );

        cleanup_test_profile(&profile);
    });
}

/// Real, end-to-end proof of `browser_chrome_core::EmbeddedAssetServer`
/// serving the browser's own compiled-in `assets/` (see that crate's
/// `build.rs`/`embedded_assets.rs`) to a real WebKitGTK page — not just
/// that the HTTP layer works (`browser-chrome-core/tests/
/// embedded_asset_server.rs` already proves that), but that a real webview
/// actually loads and runs HTML, CSS, *and* JS pulled straight out of the
/// unextracted, in-memory archive, on this app's real `add_page`/
/// console-capture path. The page's own title and its script's
/// `getComputedStyle`-read CSS color are both asserted, so this fails if
/// either the HTML or the CSS silently didn't load, not just the JS.
#[test]
fn embedded_assets_load_and_run_in_a_real_webview() {
    run_on_gtk_thread(|| {
        let server = EmbeddedAssetServer::start(embedded_assets(), "index.html").expect("starting the embedded asset server should succeed");
        let url = format!("http://127.0.0.1:{}/", server.port());

        let profile = test_profile("embedded-assets");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        app.add_page(&url).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url.as_str())), "the embedded page should load");
        let id = app.page_ids()[0].clone();
        assert!(wait_until(|| app.page_title(&id).as_deref() == Some("embedded assets example")), "the page title should reflect the embedded HTML");

        assert!(
            wait_until(|| app.console_messages_for_test().iter().any(|m| m == "embedded_assets_loaded color=rgb(18, 52, 86)")),
            "expected the embedded_assets_loaded console message proving HTML+CSS+JS all loaded from the embedded zip, got {:?}",
            app.console_messages_for_test()
        );

        cleanup_test_profile(&profile);
    });
}

/// Pulls `key=value` (an integer) out of a `key1=v1 key2=v2 ...`-shaped
/// console.log message — used by the `browser://...` tests below so they
/// assert real counts without requiring an exact string match (see
/// `browser_switcher_page_shows_real_open_pages_bookmarks_and_history`'s own
/// comment for why an exact `pages=` count would be flaky).
fn parse_metric(message: &str, key: &str) -> Option<i64> {
    message.split(' ').find_map(|token| token.strip_prefix(&format!("{key}=")).and_then(|v| v.parse().ok()))
}

/// End-to-end proof that `browser://switcher` (see `internal_pages::
/// resolve`, hooked into `add_page`) is a real, working page: seeds real
/// open pages, a real bookmark, and real history visits through this exact
/// `AppState`, navigates to it, and asserts its own `console.log` (emitted
/// by `assets/switcher/app.js` after it fetches everything over
/// `WebviewRpcServer`) reflects the seeded data — not a fixture, and not
/// just "the page loaded."
#[test]
fn browser_switcher_page_shows_real_open_pages_bookmarks_and_history() {
    run_on_gtk_thread(|| {
        let profile = test_profile("browser-switcher");
        let url_a = fixture_url("page_a.html");
        let url_b = fixture_url("page_b.html");

        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        app.add_page(&url_a).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_a.as_str())));
        let id_a = app.page_ids()[0].clone();
        assert!(wait_until(|| app.page_title(&id_a).as_deref() == Some("Page A")), "page A's title should arrive, recording a history visit");
        app.toggle_bookmark_for_active();
        assert!(app.is_active_bookmarked());

        app.add_page(&url_b).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().as_deref() == Some(url_b.as_str())));
        let id_b = app.page_ids().into_iter().find(|id| app.page_url(id).as_deref() == Some(url_b.as_str())).expect("page B should be tracked");
        assert!(wait_until(|| app.page_title(&id_b).as_deref() == Some("Page B")), "page B's title should arrive, recording a second history visit");

        app.add_page(browser_chrome_core::internal_pages::SWITCHER).expect("add_page should succeed");

        assert!(
            wait_until(|| app.console_messages_for_test().iter().any(|m| m.starts_with("switcher_loaded"))),
            "expected the switcher page to report switcher_loaded, got {:?}",
            app.console_messages_for_test()
        );
        let messages = app.console_messages_for_test();
        let last = messages.iter().rev().find(|m| m.starts_with("switcher_loaded")).expect("already asserted present above").clone();
        // `>=`, not `==`, for open pages: the switcher page is a real page
        // too (tracked by the same `PageManager` it's querying), so it may
        // or may not have finished registering itself by the moment its own
        // first fetch runs — this only asserts what must always be true.
        assert!(parse_metric(&last, "pages").unwrap_or(0) >= 2, "expected at least the two real open pages, got {last:?}");
        assert_eq!(parse_metric(&last, "bookmarks"), Some(1), "expected the one real bookmark, got {last:?}");
        assert!(parse_metric(&last, "history").unwrap_or(0) >= 2, "expected at least the two real history visits, got {last:?}");

        cleanup_test_profile(&profile);
    });
}

/// End-to-end proof that `browser://profile` renders real `Settings` data
/// (fetched via `profile.settings.get`), not placeholder content.
#[test]
fn browser_profile_page_shows_real_settings() {
    run_on_gtk_thread(|| {
        let profile = test_profile("browser-profile");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        app.add_page(browser_chrome_core::internal_pages::PROFILE).expect("add_page should succeed");
        assert!(
            wait_until(|| app.console_messages_for_test().iter().any(|m| m == "profile_section_rendered section=general")),
            "expected the profile page's General section to render, got {:?}",
            app.console_messages_for_test()
        );

        // A real round trip, not just a page load: the General section's
        // start-page field should reflect this profile's actual (default)
        // `Settings::start_page` — a fixture/mock would have no way to know
        // this value.
        assert!(
            wait_until_script_equals(&app, "document.getElementById('start-page-input').value", browser_core::HOME_URL),
            "expected the start-page field to reflect this profile's real Settings::start_page"
        );

        cleanup_test_profile(&profile);
    });
}

/// End-to-end proof that `browser://passwords` honestly reports a fresh
/// profile's real vault state (never set up) rather than silently showing
/// an empty-but-unlocked list.
#[test]
fn browser_passwords_page_shows_the_real_locked_state() {
    run_on_gtk_thread(|| {
        let profile = test_profile("browser-passwords");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        app.add_page(browser_chrome_core::internal_pages::PASSWORDS).expect("add_page should succeed");
        assert!(
            wait_until(|| app.console_messages_for_test().iter().any(|m| m == "passwords_loaded locked=true entries=0")),
            "expected the passwords page to honestly report the real (never set up) vault state, got {:?}",
            app.console_messages_for_test()
        );

        cleanup_test_profile(&profile);
    });
}

/// Internal pages (`browser://...`) are host UI, not something the user
/// "browsed to" — they must never show up in history, and the address bar
/// must show `browser://switcher` (etc.), not the raw loopback URL it's
/// actually served from, once pulled up to edit.
#[test]
fn internal_pages_are_excluded_from_history_and_shown_as_browser_urls_when_editing() {
    run_on_gtk_thread(|| {
        let profile = test_profile("internal-pages-display");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");

        // Visit every internal page — each has a real <title>, so if they
        // weren't excluded, each would record a history visit.
        app.add_page(browser_chrome_core::internal_pages::PROFILE).expect("add_page should succeed");
        assert!(wait_until(|| app.console_messages_for_test().iter().any(|m| m.starts_with("profile_section_rendered"))));
        app.add_page(browser_chrome_core::internal_pages::PASSWORDS).expect("add_page should succeed");
        assert!(wait_until(|| app.console_messages_for_test().iter().any(|m| m.starts_with("passwords_loaded"))));

        // The address bar, pulled up to edit the *current* page (passwords)
        // right now: should show the browser:// form, not the loopback URL.
        app.open_switcher_editing_url();
        assert_eq!(app.address_bar_text(), browser_chrome_core::internal_pages::PASSWORDS);
        app.close_switcher();
        assert_eq!(app.address_bar_text(), browser_chrome_core::internal_pages::PASSWORDS, "closing without a selection should restore the same browser:// display form");

        // The switcher page itself is the third internal page visited here —
        // its own `switcher.history` RPC call is the real, end-to-end check
        // that none of the three (including itself) got recorded.
        app.add_page(browser_chrome_core::internal_pages::SWITCHER).expect("add_page should succeed");
        assert!(
            wait_until(|| app.console_messages_for_test().iter().any(|m| m.starts_with("switcher_loaded"))),
            "expected the switcher page to report switcher_loaded, got {:?}",
            app.console_messages_for_test()
        );
        let messages = app.console_messages_for_test();
        let last = messages.iter().rev().find(|m| m.starts_with("switcher_loaded")).expect("already asserted present above").clone();
        assert_eq!(parse_metric(&last, "history"), Some(0), "no internal page should ever be recorded in history, got {last:?}");

        cleanup_test_profile(&profile);
    });
}

/// End-to-end proof of the newest piece of native/web integration: a real
/// `WryEngine` embedded inside a `gtk::Popover` (see
/// `AppState::show_profile_menu`), not just a page in the normal stack.
/// Asserts the popover's own webview actually loads and fetches real
/// `profile.info` data over RPC — the same real-webview-tests-real-data
/// pattern as `browser_profile_page_shows_real_settings`, applied to a
/// popover instead of a full page for the first time.
#[test]
fn profile_menu_popover_shows_real_profile_info() {
    run_on_gtk_thread(|| {
        let profile = test_profile("profile-menu-popover");
        let (_window, app) = build_window_and_app(profile.clone()).expect("build_window_and_app should succeed");
        app.add_page(&fixture_url("page_a.html")).expect("add_page should succeed");
        assert!(wait_until(|| app.active_url().is_some()));

        app.show_profile_menu();
        assert!(app.is_profile_menu_open());
        assert!(
            wait_until(|| app.console_messages_for_test().iter().any(|m| *m == format!("profile_menu_loaded name={}", profile.name))),
            "expected the profile menu popover's real webview to load and report the real profile name, got {:?}",
            app.console_messages_for_test()
        );

        cleanup_test_profile(&profile);
    });
}
