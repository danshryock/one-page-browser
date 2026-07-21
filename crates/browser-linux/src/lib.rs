use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use render_engine::{RenderEngine, WryEngine};

pub const HOME_URL: &str = "https://www.rust-lang.org";

const PALETTE: &[&str] = &[
    "#3b6fd4", "#d4573b", "#3bd46f", "#8f3bd4", "#d4a63b", "#3bc7d4",
];

fn domain_of(url: &str) -> String {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    without_scheme.split('/').next().unwrap_or(without_scheme).to_string()
}

struct Page {
    id: String,
    container: gtk::Box,
    engine: WryEngine,
    title: Rc<RefCell<String>>,
    color: &'static str,
}

pub struct AppState {
    address_bar: gtk::Entry,
    stack: gtk::Stack,
    switcher_panel: gtk::Widget,
    search_entry: gtk::SearchEntry,
    flowbox: gtk::FlowBox,
    pages: RefCell<Vec<Page>>,
    active_id: RefCell<String>,
    next_id: Cell<u64>,
}

impl AppState {
    fn with_active<F: FnOnce(&Page) -> anyhow::Result<()>>(&self, f: F) {
        let active_id = self.active_id.borrow().clone();
        let pages = self.pages.borrow();
        if let Some(page) = pages.iter().find(|p| p.id == active_id) {
            if let Err(err) = f(page) {
                eprintln!("action failed: {err}");
            }
        }
    }

    pub fn add_page(self: &Rc<Self>, url: &str) -> anyhow::Result<()> {
        let id = self.next_id.get().to_string();
        self.next_id.set(self.next_id.get() + 1);

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        self.stack.add_named(&container, &id);
        container.show_all();

        let title = Rc::new(RefCell::new(String::new()));
        let title_for_cb = Rc::clone(&title);
        let app_weak = Rc::downgrade(self);
        let engine = WryEngine::new(&container, url, move |new_title| {
            *title_for_cb.borrow_mut() = new_title;
            if let Some(app) = app_weak.upgrade() {
                app.rebuild_switcher_grid();
            }
        })?;

        let color = PALETTE[self.pages.borrow().len() % PALETTE.len()];
        self.pages.borrow_mut().push(Page {
            id: id.clone(),
            container,
            engine,
            title,
            color,
        });

        self.switch_to(&id);
        self.rebuild_switcher_grid();
        Ok(())
    }

    pub fn switch_to(self: &Rc<Self>, id: &str) {
        self.stack.set_visible_child_name(id);
        *self.active_id.borrow_mut() = id.to_string();
        if let Some(page) = self.pages.borrow().iter().find(|p| p.id == id) {
            if let Ok(url) = page.engine.current_url() {
                self.address_bar.set_text(&url);
            }
        }
        self.switcher_panel.hide();
    }

    pub fn close_page(self: &Rc<Self>, id: &str) {
        let was_active = *self.active_id.borrow() == id;

        if let Some(page) = self.pages.borrow().iter().find(|p| p.id == id) {
            self.stack.remove(&page.container);
        }
        self.pages.borrow_mut().retain(|p| p.id != id);

        if was_active {
            let next_id = self.pages.borrow().first().map(|p| p.id.clone());
            match next_id {
                Some(nid) => self.switch_to(&nid),
                None => {
                    if let Err(err) = self.add_page(HOME_URL) {
                        eprintln!("failed to open replacement page: {err}");
                    }
                }
            }
        }
        self.rebuild_switcher_grid();
    }

    fn rebuild_switcher_grid(self: &Rc<Self>) {
        for child in self.flowbox.children() {
            self.flowbox.remove(&child);
        }

        for page in self.pages.borrow().iter() {
            let id = page.id.clone();
            let title_text = {
                let t = page.title.borrow();
                if t.is_empty() { "New Page".to_string() } else { t.clone() }
            };
            let url = page.engine.current_url().unwrap_or_default();
            let domain = domain_of(&url);

            let tile = gtk::Button::new();
            tile.style_context().add_class("page-tile");
            let css = gtk::CssProvider::new();
            let _ = css.load_from_data(
                format!(".page-tile {{ background-color: {}; border-radius: 10px; color: #fff; }}", page.color)
                    .as_bytes(),
            );
            tile.style_context()
                .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
            tile.set_size_request(150, 110);

            let inner = gtk::Box::new(gtk::Orientation::Vertical, 2);
            inner.set_margin(10);
            inner.set_valign(gtk::Align::End);
            let title_label = gtk::Label::new(Some(&title_text));
            title_label.set_halign(gtk::Align::Start);
            let domain_label = gtk::Label::new(Some(&domain));
            domain_label.set_halign(gtk::Align::Start);
            inner.pack_start(&title_label, false, false, 0);
            inner.pack_start(&domain_label, false, false, 0);
            tile.add(&inner);

            let app_clone = Rc::clone(self);
            let id_clone = id.clone();
            tile.connect_clicked(move |_| {
                app_clone.switch_to(&id_clone);
            });

            let close_btn = gtk::Button::with_label("\u{d7}");
            close_btn.set_halign(gtk::Align::End);
            close_btn.set_valign(gtk::Align::Start);
            close_btn.set_size_request(22, 22);
            let app_clone = Rc::clone(self);
            let id_clone = id.clone();
            close_btn.connect_clicked(move |_| {
                app_clone.close_page(&id_clone);
            });

            let tile_overlay = gtk::Overlay::new();
            tile_overlay.add(&tile);
            tile_overlay.add_overlay(&close_btn);

            let flow_child = gtk::FlowBoxChild::new();
            flow_child.set_widget_name(&id);
            flow_child.add(&tile_overlay);
            flow_child.show_all();
            self.flowbox.insert(&flow_child, -1);
        }

        let add_tile = gtk::Button::new();
        add_tile.set_size_request(150, 110);
        add_tile.add(&gtk::Label::new(Some("+")));
        let app_clone = Rc::clone(self);
        add_tile.connect_clicked(move |_| {
            if let Err(err) = app_clone.add_page(HOME_URL) {
                eprintln!("failed to open new page: {err}");
            }
        });

        let add_child = gtk::FlowBoxChild::new();
        add_child.set_widget_name("__add__");
        add_child.add(&add_tile);
        add_child.show_all();
        self.flowbox.insert(&add_child, -1);
    }

    /// Page ids in creation order — test/inspection helper.
    pub fn page_ids(&self) -> Vec<String> {
        self.pages.borrow().iter().map(|p| p.id.clone()).collect()
    }

    /// Currently active page id — test/inspection helper.
    pub fn active_id(&self) -> String {
        self.active_id.borrow().clone()
    }

    /// The `Stack`'s visible child name, so tests can confirm the UI (not just
    /// internal state) actually switched — test/inspection helper.
    pub fn stack_visible_child_name(&self) -> Option<String> {
        self.stack.visible_child_name().map(|s| s.to_string())
    }

    /// The active page's current URL — test/inspection helper.
    pub fn active_url(&self) -> Option<String> {
        let active_id = self.active_id.borrow().clone();
        self.pages
            .borrow()
            .iter()
            .find(|p| p.id == active_id)
            .and_then(|p| p.engine.current_url().ok())
    }

    /// A page's tracked title (updated via wry's document-title-changed
    /// handler) — test/inspection helper.
    pub fn page_title(&self, id: &str) -> Option<String> {
        self.pages
            .borrow()
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.title.borrow().clone())
    }
}

/// Builds the full window + chrome (header bar, page stack, switcher overlay)
/// and wires up all signal handlers. Does not create any page — call
/// `app.add_page(HOME_URL)` (or any other URL) afterward to open the first one.
///
/// Assumes `gtk::init()` has already been called.
pub fn build_window_and_app() -> anyhow::Result<(gtk::Window, Rc<AppState>)> {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("claude-browser");
    window.set_default_size(1024, 768);
    window.connect_delete_event(|_, _| {
        gtk::main_quit();
        gtk::glib::Propagation::Proceed
    });

    let header_bar = gtk::HeaderBar::new();
    header_bar.set_show_close_button(true);
    header_bar.set_decoration_layout(Some(":close"));

    let back_button = gtk::Button::with_label("\u{2190}");
    let forward_button = gtk::Button::with_label("\u{2192}");
    let reload_button = gtk::Button::with_label("\u{27f3}");
    header_bar.pack_start(&back_button);
    header_bar.pack_start(&forward_button);
    header_bar.pack_start(&reload_button);

    let address_bar = gtk::Entry::new();
    address_bar.set_width_chars(50);
    header_bar.set_custom_title(Some(&address_bar));

    let switcher_toggle = gtk::Button::new();
    switcher_toggle.set_image(Some(&gtk::Image::from_icon_name(
        Some("view-grid-symbolic"),
        gtk::IconSize::Button,
    )));
    header_bar.pack_end(&switcher_toggle);

    window.set_titlebar(Some(&header_bar));

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.set_hexpand(true);

    let scrim = gtk::EventBox::new();
    scrim.style_context().add_class("switcher-scrim");
    let scrim_css = gtk::CssProvider::new();
    let _ = scrim_css.load_from_data(b".switcher-scrim { background-color: rgba(20,20,18,0.55); }");
    scrim
        .style_context()
        .add_provider(&scrim_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let search_entry = gtk::SearchEntry::new();
    search_entry.set_placeholder_text(Some("Type to filter open pages\u{2026}"));
    search_entry.set_halign(gtk::Align::Center);
    search_entry.set_width_chars(40);

    let flowbox = gtk::FlowBox::new();
    flowbox.set_valign(gtk::Align::Start);
    flowbox.set_selection_mode(gtk::SelectionMode::None);
    flowbox.set_homogeneous(true);
    flowbox.set_margin(24);
    flowbox.set_row_spacing(16);
    flowbox.set_column_spacing(16);

    let grid_content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    grid_content.set_halign(gtk::Align::Fill);
    grid_content.set_valign(gtk::Align::Start);
    grid_content.set_margin_top(40);
    grid_content.pack_start(&search_entry, false, false, 0);
    grid_content.pack_start(&flowbox, true, true, 0);

    let switcher_overlay = gtk::Overlay::new();
    switcher_overlay.add(&scrim);
    switcher_overlay.add_overlay(&grid_content);

    let root_overlay = gtk::Overlay::new();
    root_overlay.add(&stack);
    root_overlay.add_overlay(&switcher_overlay);

    window.add(&root_overlay);
    window.show_all();
    switcher_overlay.hide();
    window.set_title("claude-browser");

    let app = Rc::new(AppState {
        address_bar: address_bar.clone(),
        stack,
        switcher_panel: switcher_overlay.clone().upcast::<gtk::Widget>(),
        search_entry: search_entry.clone(),
        flowbox: flowbox.clone(),
        pages: RefCell::new(Vec::new()),
        active_id: RefCell::new(String::new()),
        next_id: Cell::new(0),
    });

    let app_weak = Rc::downgrade(&app);
    flowbox.set_filter_func(Some(Box::new(move |child: &gtk::FlowBoxChild| {
        let Some(app) = app_weak.upgrade() else {
            return true;
        };
        let name = child.widget_name();
        if name.as_str() == "__add__" {
            return true;
        }
        let text = app.search_entry.text().to_lowercase();
        if text.is_empty() {
            return true;
        }
        let pages = app.pages.borrow();
        pages
            .iter()
            .find(|p| p.id == name.as_str())
            .map(|p| {
                let title = p.title.borrow().to_lowercase();
                let url = p.engine.current_url().unwrap_or_default().to_lowercase();
                title.contains(&text) || url.contains(&text)
            })
            .unwrap_or(false)
    })));

    {
        let flowbox = flowbox.clone();
        search_entry.connect_changed(move |_| {
            flowbox.invalidate_filter();
        });
    }

    {
        let app = Rc::clone(&app);
        back_button.connect_clicked(move |_| app.with_active(|p| p.engine.go_back()));
    }
    {
        let app = Rc::clone(&app);
        forward_button.connect_clicked(move |_| app.with_active(|p| p.engine.go_forward()));
    }
    {
        let app = Rc::clone(&app);
        reload_button.connect_clicked(move |_| app.with_active(|p| p.engine.reload()));
    }
    {
        let app = Rc::clone(&app);
        address_bar.connect_activate(move |entry| {
            let url = entry.text().to_string();
            app.with_active(|p| p.engine.navigate(&url));
        });
    }
    {
        let app = Rc::clone(&app);
        switcher_toggle.connect_clicked(move |_| {
            if app.switcher_panel.is_visible() {
                app.switcher_panel.hide();
            } else {
                app.rebuild_switcher_grid();
                app.switcher_panel.show();
            }
        });
    }
    {
        let app = Rc::clone(&app);
        scrim.connect_button_press_event(move |_, _| {
            app.switcher_panel.hide();
            gtk::glib::Propagation::Stop
        });
    }
    {
        let app = Rc::clone(&app);
        window.connect_key_press_event(move |_, event| {
            let ctrl = event.state().contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let is_t = event
                .keyval()
                .to_unicode()
                .map(|c| c.eq_ignore_ascii_case(&'t'))
                .unwrap_or(false);
            if ctrl && is_t {
                if let Err(err) = app.add_page(HOME_URL) {
                    eprintln!("failed to open new page: {err}");
                }
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
    }

    Ok((window, app))
}
