use std::time::{Duration, Instant};

use gtk::prelude::*;
use render_engine::{RenderEngine, WryEngine};

fn pump_for(duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
    }
}

fn check(label: &str, actual: &str, expected: &str) -> bool {
    let ok = actual == expected;
    println!(
        "[{}] {} -- expected {}, got {}",
        if ok { "PASS" } else { "FAIL" },
        label,
        expected,
        actual
    );
    ok
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

    let engine = WryEngine::new(&content, &url_a)?;
    pump_for(Duration::from_millis(800));

    let mut all_ok = true;
    all_ok &= check("initial load", &engine.current_url()?, &url_a);

    engine.navigate(&url_b)?;
    pump_for(Duration::from_millis(800));
    all_ok &= check("navigate to B", &engine.current_url()?, &url_b);

    engine.go_back()?;
    pump_for(Duration::from_millis(800));
    all_ok &= check("go_back to A", &engine.current_url()?, &url_a);

    engine.go_forward()?;
    pump_for(Duration::from_millis(800));
    all_ok &= check("go_forward to B", &engine.current_url()?, &url_b);

    engine.reload()?;
    pump_for(Duration::from_millis(800));
    all_ok &= check("reload stays on B", &engine.current_url()?, &url_b);

    println!("{}", if all_ok { "ALL PASS" } else { "SOME FAILED" });
    std::process::exit(if all_ok { 0 } else { 1 });
}
