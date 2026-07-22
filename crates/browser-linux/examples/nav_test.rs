use std::time::{Duration, Instant};

use gtk::prelude::*;
use render_engine::{RenderEngine, WryEngine};

/// Generous ceiling for how long a webview navigation/history operation may
/// take to settle. Wide on purpose: this only slows a run down when a check
/// genuinely never becomes true (a real failure), since `wait_for_eq` returns
/// the moment the condition is met — under normal load that's milliseconds.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Minimum extra time to keep pumping after `get_actual` first matches
/// `expected`, before trusting it. `current_url()` can report the
/// destination URL before WebKitGTK has actually finished registering that
/// navigation in its joint history stack — returning the instant the URL
/// matches leaves too little real time elapsed for an immediately-following
/// `go_back()` to have anything to go back to. Confirmed empirically:
/// without this settle window, polling that returns as soon as the URL
/// matches reproduces the bug on every run; with it, ten runs straight pass.
const SETTLE: Duration = Duration::from_millis(200);

/// Polls `get_actual` (pumping the GTK loop between attempts) until it equals
/// `expected` or `timeout` elapses, then reports PASS/FAIL. Replaces a fixed
/// sleep-and-hope wait: WebKit navigation is asynchronous, so a hardcoded
/// delay is either wasteful (usually) or flaky under system load (sometimes)
/// — polling with a generous ceiling is robust to both.
fn wait_for_eq(
    label: &str,
    timeout: Duration,
    expected: &str,
    mut get_actual: impl FnMut() -> anyhow::Result<String>,
) -> bool {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    loop {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        if let Ok(actual) = get_actual() {
            last = actual.clone();
            if actual == expected {
                let settle_until = Instant::now() + SETTLE;
                while Instant::now() < settle_until {
                    while gtk::events_pending() {
                        gtk::main_iteration_do(false);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                println!("[PASS] {label} -- expected {expected}, got {actual}");
                return true;
            }
        }
        if Instant::now() >= deadline {
            println!("[FAIL] {label} -- expected {expected}, got {last} (timed out after {timeout:?})");
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn main() -> anyhow::Result<()> {
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/fixtures");
    let url_a = format!("file://{fixtures}/page_a.html");
    let url_b = format!("file://{fixtures}/page_b.html");

    gtk::init()?;
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.add(&content);
    window.show_all();

    let engine = WryEngine::new(&content, &url_a, |_| {})?;

    let mut all_ok = true;
    all_ok &= wait_for_eq("initial load", TIMEOUT, &url_a, || engine.current_url());

    engine.navigate(&url_b)?;
    all_ok &= wait_for_eq("navigate to B", TIMEOUT, &url_b, || engine.current_url());

    engine.go_back()?;
    all_ok &= wait_for_eq("go_back to A", TIMEOUT, &url_a, || engine.current_url());

    engine.go_forward()?;
    all_ok &= wait_for_eq("go_forward to B", TIMEOUT, &url_b, || engine.current_url());

    engine.reload()?;
    all_ok &= wait_for_eq("reload stays on B", TIMEOUT, &url_b, || engine.current_url());

    println!("{}", if all_ok { "ALL PASS" } else { "SOME FAILED" });
    std::process::exit(if all_ok { 0 } else { 1 });
}
