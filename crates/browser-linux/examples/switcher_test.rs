use std::time::{Duration, Instant};

use browser_linux::build_window_and_app;

fn pump_for(duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
    }
}

fn check(label: &str, ok: bool) -> bool {
    println!("[{}] {}", if ok { "PASS" } else { "FAIL" }, label);
    ok
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
    pump_for(Duration::from_millis(800));
    all_ok &= check("adding first page opens exactly 1 page", app.page_ids().len() == 1);
    let id_a = app.page_ids()[0].clone();
    all_ok &= check("first page becomes active", app.active_id() == id_a);
    all_ok &= check(
        "stack's visible child matches active page",
        app.stack_visible_child_name().as_deref() == Some(id_a.as_str()),
    );
    all_ok &= check("active url is page A", app.active_url().as_deref() == Some(url_a.as_str()));
    all_ok &= check(
        "page A's title got tracked from the document",
        app.page_title(&id_a).as_deref() == Some("Page A"),
    );

    app.add_page(&url_b)?;
    pump_for(Duration::from_millis(800));
    all_ok &= check("adding second page grows the list to 2", app.page_ids().len() == 2);
    let id_b = app.page_ids()[1].clone();
    all_ok &= check("new page becomes active", app.active_id() == id_b);
    all_ok &= check("active url is page B", app.active_url().as_deref() == Some(url_b.as_str()));

    app.add_page(&url_c)?;
    pump_for(Duration::from_millis(800));
    all_ok &= check("adding third page grows the list to 3", app.page_ids().len() == 3);
    let id_c = app.page_ids()[2].clone();
    all_ok &= check("third page is active", app.active_id() == id_c);

    app.switch_to(&id_a);
    pump_for(Duration::from_millis(200));
    all_ok &= check("switching back to A updates active id", app.active_id() == id_a);
    all_ok &= check(
        "switching back to A updates the stack's visible child",
        app.stack_visible_child_name().as_deref() == Some(id_a.as_str()),
    );
    all_ok &= check("switching back to A updates the address/url", app.active_url().as_deref() == Some(url_a.as_str()));
    all_ok &= check(
        "switching doesn't drop other pages",
        app.page_ids().len() == 3,
    );

    // A is active; closing it should fall back to a remaining page, not vanish.
    app.close_page(&id_a);
    pump_for(Duration::from_millis(200));
    all_ok &= check("closing active page removes it from the list", app.page_ids().len() == 2);
    all_ok &= check("closing active page picks a new active page", app.active_id() != id_a);
    all_ok &= check(
        "the new active page is one of the remaining ones",
        app.active_id() == id_b || app.active_id() == id_c,
    );

    // Close down to zero: shouldn't panic, and should land on some fallback page.
    let remaining = app.page_ids();
    for id in remaining {
        app.close_page(&id);
        pump_for(Duration::from_millis(200));
    }
    pump_for(Duration::from_millis(500));
    all_ok &= check(
        "closing every page leaves exactly one fallback page instead of zero",
        app.page_ids().len() == 1,
    );

    println!("{}", if all_ok { "ALL PASS" } else { "SOME FAILED" });
    std::process::exit(if all_ok { 0 } else { 1 });
}
