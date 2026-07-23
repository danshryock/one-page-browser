//! Custom title bar for Linux: a real GTK `HeaderBar`, installed via
//! `gtk_window_set_titlebar` on the raw `GtkWindow*` wxWidgets' own `Frame`
//! already wraps — confirmed by reading wxWidgets' own GTK port source:
//! `include/wx/gtk/window.h`'s `GetHandle()` returns `m_widget`, and
//! `src/gtk/toplevel.cpp`'s top-level constructor sets
//! `m_widget = gtk_window_new(...)`. GTK then owns window drag, native
//! min/max/close, and double-click-to-maximize automatically — exactly what
//! `browser-linux-gtk3` already gets for free today.
//!
//! This bypasses wxDragon (and wxWidgets' own widget tree) entirely for the
//! title bar's contents: the buttons/address entry here are real
//! `gtk::Button`/`gtk::Entry` widgets, not wx ones, wired directly to
//! `Rc<AppState>`. That's safe because wxWidgets' GTK port is itself just a
//! thin wrapper around the same single-threaded `gtk_main()` loop, so GTK's
//! own signal callbacks and wx's event callbacks share one loop — no new
//! threading concerns.
//!
//! Consequence: this header bar occupies no space in wx's own sizer tree at
//! all (GTK's title bar is a separate decoration area outside the normal
//! client-area widget hierarchy) — so `build_frame_and_app` must not build
//! the wx `toolbar_panel` row on Linux at all; `content_panel` gets the
//! frame's full height instead.

use std::rc::Rc;

use gtk::glib::translate::FromGlibPtrNone;
use gtk::prelude::*;
use render_engine::RenderEngine;
use wxdragon::widgets::frame::Frame;
use wxdragon::window::WxWidget;

use crate::AppState;

/// Widgets built by `build`, needed by `wire` (called once `Rc<AppState>`
/// exists) and by `AppState` itself (`address_bar`).
pub struct Widgets {
    pub address_bar: gtk::Entry,
    back_button: gtk::Button,
    forward_button: gtk::Button,
    reload_button: gtk::Button,
    switcher_toggle: gtk::Button,
    settings_button: gtk::Button,
    gtk_window: gtk::Window,
}

/// Builds the header bar and installs it as `frame`'s title bar. Call before
/// `frame.show(true)` (mirrors `browser-linux-gtk3`, which sets its header
/// bar before `show_all()`).
pub fn build(frame: &Frame) -> Widgets {
    // wxWidgets has already initialized the underlying C GTK library itself
    // (as part of its own App startup) — but gtk-rs tracks its *own*,
    // separate "has `gtk::init` been called" flag, which is still false
    // since we never called the safe Rust wrapper ourselves, and every
    // gtk-rs widget constructor asserts it before doing anything. This is
    // gtk-rs's own sanctioned escape hatch for exactly this situation
    // (its docs: safe to call when GTK was initialized elsewhere, on this
    // same thread, which is the main one — all true here).
    unsafe { gtk::set_initialized() };

    // `frame.get_handle()` returns the raw `GtkWindow*` (see module doc).
    // `from_glib_none`: takes a *borrowed* ref (adds a GObject ref) rather
    // than stealing ownership — wxWidgets still owns this GtkWindow's
    // lifetime; our wrapper just needs safe access to it.
    let raw = frame.get_handle() as *mut gtk::ffi::GtkWindow;
    let gtk_window: gtk::Window = unsafe { gtk::Window::from_glib_none(raw) };

    let header_bar = gtk::HeaderBar::new();
    header_bar.set_show_close_button(true);
    header_bar.set_decoration_layout(Some(":minimize,maximize,close"));

    let back_button = gtk::Button::new();
    back_button.set_image(Some(&gtk::Image::from_icon_name(Some("pan-start-symbolic"), gtk::IconSize::Button)));
    let forward_button = gtk::Button::new();
    forward_button.set_image(Some(&gtk::Image::from_icon_name(Some("pan-end-symbolic"), gtk::IconSize::Button)));
    let reload_button = gtk::Button::new();
    reload_button.set_image(Some(&gtk::Image::from_icon_name(Some("view-refresh-symbolic"), gtk::IconSize::Button)));
    let switcher_toggle = gtk::Button::new();
    switcher_toggle.set_image(Some(&gtk::Image::from_icon_name(Some("view-grid-symbolic"), gtk::IconSize::Button)));
    let settings_button = gtk::Button::new();
    settings_button.set_image(Some(&gtk::Image::from_icon_name(Some("preferences-system-symbolic"), gtk::IconSize::Button)));
    for button in [&back_button, &forward_button, &reload_button, &switcher_toggle, &settings_button] {
        button.style_context().add_class("flat");
    }

    header_bar.pack_start(&back_button);
    header_bar.pack_start(&forward_button);

    let address_bar = gtk::Entry::new();
    address_bar.set_width_chars(50);
    address_bar.set_hexpand(true);

    // Spacers flanking the address group double as extra draggable header-bar
    // space, same as browser-linux-gtk3.
    const TOOLBAR_BUTTON_WIDTH: i32 = 36;
    let spacer_before = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer_before.set_size_request(TOOLBAR_BUTTON_WIDTH, -1);
    let spacer_after = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer_after.set_size_request(TOOLBAR_BUTTON_WIDTH, -1);

    let address_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    address_group.pack_start(&spacer_before, false, false, 0);
    address_group.pack_start(&address_bar, true, true, 0);
    address_group.pack_start(&reload_button, false, false, 0);
    address_group.pack_start(&spacer_after, false, false, 0);
    header_bar.set_custom_title(Some(&address_group));

    header_bar.pack_end(&switcher_toggle);
    header_bar.pack_end(&settings_button);

    header_bar.show_all();
    gtk_window.set_titlebar(Some(&header_bar));

    Widgets { address_bar, back_button, forward_button, reload_button, switcher_toggle, settings_button, gtk_window }
}

/// Wires every header-bar button and the window-level keyboard shortcuts.
/// Call once, after the `Rc<AppState>` these close over exists.
pub fn wire(app: &Rc<AppState>, widgets: &Widgets) {
    {
        let app = Rc::clone(app);
        widgets.back_button.connect_clicked(move |_| app.with_active(|p| p.go_back()));
    }
    {
        let app = Rc::clone(app);
        widgets.forward_button.connect_clicked(move |_| app.with_active(|p| p.go_forward()));
    }
    {
        let app = Rc::clone(app);
        widgets.reload_button.connect_clicked(move |_| app.with_active(|p| p.reload()));
    }
    {
        let app = Rc::clone(app);
        widgets.address_bar.connect_activate(move |entry| {
            let text = entry.text().to_string();
            let url = browser_core::resolve_address_input(&text, &app.settings());
            app.with_active(|p| p.navigate(&url));
        });
    }
    {
        let app = Rc::clone(app);
        widgets.switcher_toggle.connect_clicked(move |_| {
            if app.is_switcher_open() {
                app.close_switcher();
            } else {
                app.open_switcher();
            }
        });
    }
    {
        let app = Rc::clone(app);
        widgets.settings_button.connect_clicked(move |_| {
            crate::show_settings_dialog(&app);
        });
    }

    // F1 / Ctrl+T / Ctrl+L / Escape / Ctrl+W — bound on the raw GTK window's
    // own key-press-event, mirroring browser-linux-gtk3's window-level
    // handler exactly (none of this row is a wx widget, so wx's own
    // shortcut-binding trick used on Windows doesn't apply here).
    let app = Rc::clone(app);
    widgets.gtk_window.connect_key_press_event(move |_, event| {
        let ctrl = event.state().contains(gtk::gdk::ModifierType::CONTROL_MASK);
        let keyval = event.keyval();
        let is_f1 = keyval == gtk::gdk::keys::Key::from_name("F1");
        let unicode = keyval.to_unicode();
        let is_t = unicode.map(|c| c.eq_ignore_ascii_case(&'t')).unwrap_or(false);
        let is_l = unicode.map(|c| c.eq_ignore_ascii_case(&'l')).unwrap_or(false);
        let is_w = unicode.map(|c| c.eq_ignore_ascii_case(&'w')).unwrap_or(false);
        let is_escape = keyval == gtk::gdk::keys::Key::from_name("Escape");
        if is_f1 || (ctrl && is_t) || (ctrl && is_l) {
            app.open_switcher();
            gtk::glib::Propagation::Stop
        } else if is_escape && app.is_switcher_open() {
            app.close_switcher();
            gtk::glib::Propagation::Stop
        } else if ctrl && is_w {
            app.close_page(&app.active_id());
            gtk::glib::Propagation::Stop
        } else {
            gtk::glib::Propagation::Proceed
        }
    });
}
