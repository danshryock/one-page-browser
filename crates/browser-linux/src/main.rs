use std::rc::Rc;

use gtk::prelude::*;
use render_engine::{RenderEngine, WryEngine};

const HOME_URL: &str = "https://www.rust-lang.org";

fn main() -> anyhow::Result<()> {
    gtk::init()?;

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("claude-browser");
    window.set_default_size(1024, 768);
    window.connect_delete_event(|_, _| {
        gtk::main_quit();
        gtk::glib::Propagation::Proceed
    });

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.add(&root);

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    toolbar.set_margin(4);

    let back_button = gtk::Button::with_label("\u{2190}");
    let forward_button = gtk::Button::with_label("\u{2192}");
    let reload_button = gtk::Button::with_label("\u{27f3}");
    toolbar.pack_start(&back_button, false, false, 0);
    toolbar.pack_start(&forward_button, false, false, 0);
    toolbar.pack_start(&reload_button, false, false, 0);

    let address_bar = gtk::Entry::new();
    address_bar.set_text(HOME_URL);
    address_bar.set_hexpand(true);
    toolbar.pack_start(&address_bar, true, true, 0);
    root.pack_start(&toolbar, false, false, 0);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_vexpand(true);
    root.pack_start(&content, true, true, 0);

    window.show_all();

    let engine = Rc::new(WryEngine::new(&content, HOME_URL)?);

    let engine_clone = Rc::clone(&engine);
    address_bar.connect_activate(move |entry| {
        let url = entry.text().to_string();
        if let Err(err) = engine_clone.navigate(&url) {
            eprintln!("navigation failed: {err}");
        }
    });

    let engine_clone = Rc::clone(&engine);
    back_button.connect_clicked(move |_| {
        if let Err(err) = engine_clone.go_back() {
            eprintln!("back failed: {err}");
        }
    });

    let engine_clone = Rc::clone(&engine);
    forward_button.connect_clicked(move |_| {
        if let Err(err) = engine_clone.go_forward() {
            eprintln!("forward failed: {err}");
        }
    });

    let engine_clone = Rc::clone(&engine);
    reload_button.connect_clicked(move |_| {
        if let Err(err) = engine_clone.reload() {
            eprintln!("reload failed: {err}");
        }
    });

    gtk::main();
    Ok(())
}
