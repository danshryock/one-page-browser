//! Platform-agnostic page/tab management shared by every native chrome
//! (`browser-linux`, and eventually `browser-windows`/`browser-macos`).
//! Owns the page list, active-page tracking, search matching, and URL
//! helpers; deliberately knows nothing about any GUI toolkit — each
//! platform's chrome wires its own widgets to this and does its own native
//! container/window bookkeeping around it.

use std::cell::RefCell;
use std::rc::Rc;

use render_engine::RenderEngine;

pub const HOME_URL: &str = "about:blank";

const PALETTE: &[&str] = &[
    "#3b6fd4", "#d4573b", "#3bd46f", "#8f3bd4", "#d4a63b", "#3bc7d4",
];

pub fn domain_of(url: &str) -> String {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    without_scheme.split('/').next().unwrap_or(without_scheme).to_string()
}

/// Turns whatever the user typed in the switcher search box into a URL: pass
/// through anything that already looks like one, otherwise assume `https://`.
pub fn normalize_url(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.contains("://") || trimmed.starts_with("about:") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

pub struct Page<E> {
    pub id: String,
    pub engine: E,
    pub title: Rc<RefCell<String>>,
    pub color: &'static str,
}

fn page_matches_query<E: RenderEngine>(page: &Page<E>, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    let title = page.title.borrow().to_lowercase();
    let url = page.engine.current_url().unwrap_or_default().to_lowercase();
    title.contains(query_lower) || url.contains(query_lower)
}

pub struct PageManager<E> {
    pages: Vec<Page<E>>,
    active_id: String,
    next_id: u64,
}

impl<E> Default for PageManager<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E> PageManager<E> {
    pub fn new() -> Self {
        Self { pages: Vec::new(), active_id: String::new(), next_id: 0 }
    }

    /// Reserves a fresh page id. Callers build their native container and
    /// `RenderEngine` using this id *before* calling `insert` — a page's
    /// native resources have to exist before it can be tracked.
    pub fn allocate_id(&mut self) -> String {
        let id = self.next_id.to_string();
        self.next_id += 1;
        id
    }

    /// Adds an already-constructed page and makes it active. `title` is
    /// taken as a parameter (rather than created here) so the caller can
    /// clone it into the engine's title-changed callback before this call —
    /// the stored `Page` and the callback then share the same cell.
    pub fn insert(&mut self, id: String, engine: E, title: Rc<RefCell<String>>) {
        let color = PALETTE[self.pages.len() % PALETTE.len()];
        self.pages.push(Page { id: id.clone(), engine, title, color });
        self.active_id = id;
    }

    /// Removes a page and returns it so the caller can clean up its native
    /// container. Does not reassign `active_id` — the caller decides what
    /// becomes active next (a neighboring page, or a freshly created one).
    pub fn remove(&mut self, id: &str) -> Option<Page<E>> {
        let index = self.pages.iter().position(|p| p.id == id)?;
        Some(self.pages.remove(index))
    }

    pub fn set_active(&mut self, id: &str) {
        self.active_id = id.to_string();
    }

    pub fn active_id(&self) -> &str {
        &self.active_id
    }

    pub fn active(&self) -> Option<&Page<E>> {
        self.page(&self.active_id)
    }

    pub fn page(&self, id: &str) -> Option<&Page<E>> {
        self.pages.iter().find(|p| p.id == id)
    }

    pub fn pages(&self) -> &[Page<E>] {
        &self.pages
    }

    pub fn page_ids(&self) -> Vec<String> {
        self.pages.iter().map(|p| p.id.clone()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }
}

impl<E: RenderEngine> PageManager<E> {
    /// Ids of pages matching `query` (case-insensitive substring of title or
    /// URL), in creation order.
    pub fn matching_ids(&self, query: &str) -> Vec<String> {
        let query = query.to_lowercase();
        self.pages.iter().filter(|p| page_matches_query(p, &query)).map(|p| p.id.clone()).collect()
    }
}
