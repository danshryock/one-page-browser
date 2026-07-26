//! Platform-agnostic page/tab management shared by every native chrome
//! (`browser-linux-gtk3`, and eventually `browser-windows-win32`/`browser-macos`).
//! Owns the page list, active-page tracking, search matching, and URL
//! helpers; deliberately knows nothing about any GUI toolkit — each
//! platform's chrome wires its own widgets to this and does its own native
//! container/window bookkeeping around it.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use render_engine::RenderEngine;

mod bookmarks;
mod embedding;
mod history;
mod keybindings;
mod profile;
mod settings;
pub use bookmarks::{Bookmark, Bookmarks};
pub use history::{HistoryBackend, HistoryEntry, HistoryStore, MemoryHistoryStore};
pub use keybindings::{Action, KeyChord, Keybindings};
pub use profile::{
    launch_new_encrypted_profile_process, launch_new_ephemeral_process, launch_new_profile_process, list_profile_names,
    resolve_ephemeral_requested, resolve_passphrase_setup_requested, resolve_profile_name, resolve_url_argument, Profile,
};
pub use settings::{SearchEngine, Settings, Theme};

pub const HOME_URL: &str = "https://www.google.com";

const PALETTE: &[&str] = &[
    "#3b6fd4", "#d4573b", "#3bd46f", "#8f3bd4", "#d4a63b", "#3bc7d4",
];

pub fn domain_of(url: &str) -> String {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    without_scheme.split('/').next().unwrap_or(without_scheme).to_string()
}

/// Turns whatever the user typed into an address bar (the toolbar one, or
/// the switcher's search box) into a URL to actually navigate to:
/// - Already looks like a URL → passed through unchanged.
/// - Contains whitespace → not a valid single host token, so build a search
///   URL from the preferred search engine instead.
/// - Otherwise → assume it's a bare host and prepend `https://` (unchanged
///   from this function's original, more limited behavior — this is the
///   case a fixed regression test locks in, so it deliberately didn't change
///   here even though the function itself grew search-engine awareness).
pub fn resolve_address_input(input: &str, settings: &Settings) -> String {
    let trimmed = input.trim();
    if trimmed.contains("://") || trimmed.starts_with("about:") {
        return trimmed.to_string();
    }
    if trimmed.chars().any(|c| c.is_whitespace()) {
        if let Some(engine) = settings.default_search_engine() {
            let encoded = percent_encoding::utf8_percent_encode(trimmed, percent_encoding::NON_ALPHANUMERIC);
            return engine.query_url_template.replace("{query}", &encoded.to_string());
        }
    }
    format!("https://{trimmed}")
}

pub struct Page<E> {
    pub id: String,
    /// `None` means this page has been unloaded: its `RenderEngine`/webview
    /// was actually torn down to reclaim resources, not just flagged. Only
    /// ever `None` for a non-foreground page — the active page is never
    /// unloaded (see `enforce_loaded_limit`).
    pub engine: Option<E>,
    pub title: Rc<RefCell<String>>,
    pub color: &'static str,
    /// Whether this page currently counts against `max_loaded_pages`. Named
    /// `loaded` rather than "active" to avoid colliding with
    /// `PageManager::active_id`, which already means "the page currently
    /// shown in the UI" — an unrelated concept.
    pub loaded: bool,
    /// Last time this page was created or switched to. Used to pick which
    /// loaded page to unload first when over the limit (oldest first).
    pub last_accessed: Instant,
    /// URL as of the last time `engine` was live. Frozen the moment the
    /// engine is taken away (see `PageManager::take_engine` callers) — the
    /// only source of truth for the URL/search-matching while unloaded.
    pub last_url: String,
}

impl<E: RenderEngine> Page<E> {
    /// The page's current URL: the live engine's, if it has one, else the
    /// frozen `last_url` from before it was unloaded.
    pub fn current_url(&self) -> String {
        self.engine.as_ref().and_then(|e| e.current_url().ok()).unwrap_or_else(|| self.last_url.clone())
    }
}

fn page_matches_query<E: RenderEngine>(page: &Page<E>, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    let title = page.title.borrow().to_lowercase();
    let url = page.current_url().to_lowercase();
    title.contains(query_lower) || url.contains(query_lower)
}

pub struct PageManager<E> {
    pages: Vec<Page<E>>,
    active_id: String,
    next_id: u64,
    /// How many pages may stay `loaded` at once. `None` means unlimited.
    max_loaded_pages: Option<usize>,
}

impl<E> Default for PageManager<E> {
    fn default() -> Self {
        Self::new(None)
    }
}

impl<E> PageManager<E> {
    pub fn new(max_loaded_pages: Option<usize>) -> Self {
        Self { pages: Vec::new(), active_id: String::new(), next_id: 0, max_loaded_pages }
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
    /// Returns the ids of any pages `enforce_loaded_limit` evicted to make
    /// room — the caller is responsible for actually tearing down their
    /// engines (see `take_engine`).
    pub fn insert(&mut self, id: String, engine: E, title: Rc<RefCell<String>>) -> Vec<String> {
        let color = PALETTE[self.pages.len() % PALETTE.len()];
        self.pages.push(Page {
            id: id.clone(),
            engine: Some(engine),
            title,
            color,
            loaded: true,
            last_accessed: Instant::now(),
            last_url: String::new(),
        });
        self.active_id = id;
        self.enforce_loaded_limit()
    }

    /// Removes a page and returns it so the caller can clean up its native
    /// container. Does not reassign `active_id` — the caller decides what
    /// becomes active next (a neighboring page, or a freshly created one).
    pub fn remove(&mut self, id: &str) -> Option<Page<E>> {
        let index = self.pages.iter().position(|p| p.id == id)?;
        Some(self.pages.remove(index))
    }

    pub fn page_mut(&mut self, id: &str) -> Option<&mut Page<E>> {
        self.pages.iter_mut().find(|p| p.id == id)
    }

    /// Removes and returns a page's live engine (e.g. so the caller can drop
    /// it to reclaim its resources). `None` if the page doesn't exist or is
    /// already engine-less.
    pub fn take_engine(&mut self, id: &str) -> Option<E> {
        self.page_mut(id)?.engine.take()
    }

    /// Installs a (re)created engine for a page — used after `take_engine`
    /// tore the old one down and the caller built a fresh one. No-op if the
    /// page no longer exists.
    pub fn install_engine(&mut self, id: &str, engine: E) {
        if let Some(page) = self.page_mut(id) {
            page.engine = Some(engine);
        }
    }

    /// Makes `id` the foreground page. Also marks it `loaded` and refreshes
    /// its `last_accessed` — switching to an old page makes it recently-used
    /// again — but deliberately does *not* re-enforce `max_loaded_pages`:
    /// per spec, the limit is only enforced when a new page loads, so
    /// reactivating an old page can transiently exceed it until then.
    pub fn set_active(&mut self, id: &str) {
        self.active_id = id.to_string();
        if let Some(page) = self.pages.iter_mut().find(|p| p.id == id) {
            page.loaded = true;
            page.last_accessed = Instant::now();
        }
    }

    /// While more pages are `loaded` than `max_loaded_pages` allows, unloads
    /// the loaded page with the oldest `last_accessed` — never the current
    /// foreground page, which is never unloaded regardless of age. Loops
    /// rather than a single pass so it fully converges even if several pages
    /// are loaded at once or the limit is small. Returns the ids it flipped
    /// to unloaded (in eviction order) so the caller can tear down their
    /// engines via `take_engine`.
    fn enforce_loaded_limit(&mut self) -> Vec<String> {
        let mut evicted = Vec::new();
        let Some(limit) = self.max_loaded_pages else { return evicted };
        loop {
            if self.pages.iter().filter(|p| p.loaded).count() <= limit {
                return evicted;
            }
            let active_id = self.active_id.clone();
            let oldest = self
                .pages
                .iter_mut()
                .filter(|p| p.loaded && p.id != active_id)
                .min_by_key(|p| p.last_accessed);
            match oldest {
                Some(page) => {
                    page.loaded = false;
                    evicted.push(page.id.clone());
                }
                None => return evicted, // nothing left that's safe to unload
            }
        }
    }

    /// Whether `id` currently counts against `max_loaded_pages`.
    pub fn is_page_loaded(&self, id: &str) -> bool {
        self.page(id).map(|p| p.loaded).unwrap_or(false)
    }

    /// How many pages currently count against `max_loaded_pages`.
    pub fn loaded_page_count(&self) -> usize {
        self.pages.iter().filter(|p| p.loaded).count()
    }

    /// Changes the loaded-page limit and enforces it immediately (unlike
    /// `set_active` reactivating a page, which defers enforcement to the
    /// next new page load — a user explicitly changing this setting should
    /// take effect right away). Returns any newly-evicted ids, same as
    /// `insert`.
    pub fn set_max_loaded_pages(&mut self, limit: Option<usize>) -> Vec<String> {
        self.max_loaded_pages = limit;
        self.enforce_loaded_limit()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Trivial `RenderEngine` for testing `PageManager`'s pure bookkeeping
    /// logic without any real webview/GTK involved — just enough to satisfy
    /// the trait bound `matching_ids`/`page_matches_query` need.
    struct MockEngine {
        url: RefCell<String>,
    }

    impl MockEngine {
        fn new(url: &str) -> Self {
            Self { url: RefCell::new(url.to_string()) }
        }
    }

    impl RenderEngine for MockEngine {
        fn navigate(&self, url: &str) -> anyhow::Result<()> {
            *self.url.borrow_mut() = url.to_string();
            Ok(())
        }
        fn current_url(&self) -> anyhow::Result<String> {
            Ok(self.url.borrow().clone())
        }
        fn go_back(&self) -> anyhow::Result<()> {
            Ok(())
        }
        fn go_forward(&self) -> anyhow::Result<()> {
            Ok(())
        }
        fn reload(&self) -> anyhow::Result<()> {
            Ok(())
        }
        fn screenshot(&self, callback: Box<dyn Fn(anyhow::Result<Vec<u8>>)>) {
            callback(Ok(Vec::new()));
        }
    }

    fn insert_page(mgr: &mut PageManager<MockEngine>, url: &str) -> String {
        let id = mgr.allocate_id();
        mgr.insert(id.clone(), MockEngine::new(url), Rc::new(RefCell::new(String::new())));
        // Guarantee distinct `last_accessed` timestamps regardless of clock
        // resolution, so oldest-first ordering in tests is deterministic.
        std::thread::sleep(Duration::from_millis(2));
        id
    }

    #[test]
    fn new_page_is_loaded_and_becomes_active() {
        let mut mgr: PageManager<MockEngine> = PageManager::new(None);
        let id = insert_page(&mut mgr, "https://a.example");
        assert_eq!(mgr.active_id(), id);
        assert!(mgr.is_page_loaded(&id));
        assert_eq!(mgr.loaded_page_count(), 1);
    }

    #[test]
    fn no_limit_never_unloads_anything() {
        let mut mgr: PageManager<MockEngine> = PageManager::new(None);
        let ids: Vec<_> = (0..5).map(|i| insert_page(&mut mgr, &format!("https://{i}.example"))).collect();
        assert_eq!(mgr.loaded_page_count(), 5);
        for id in ids {
            assert!(mgr.is_page_loaded(&id));
        }
    }

    #[test]
    fn loading_past_the_limit_unloads_the_oldest() {
        let mut mgr: PageManager<MockEngine> = PageManager::new(Some(2));
        let id1 = insert_page(&mut mgr, "https://a.example");
        let id2 = insert_page(&mut mgr, "https://b.example");
        assert_eq!(mgr.loaded_page_count(), 2, "still at the limit, nothing unloaded yet");

        let id3 = mgr.allocate_id();
        let evicted = mgr.insert(id3.clone(), MockEngine::new("https://c.example"), Rc::new(RefCell::new(String::new())));
        assert_eq!(mgr.loaded_page_count(), 2, "back down to the limit after the 3rd load");
        assert!(!mgr.is_page_loaded(&id1), "oldest page should be unloaded");
        assert!(mgr.is_page_loaded(&id2));
        assert!(mgr.is_page_loaded(&id3));
        assert_eq!(evicted, vec![id1], "insert reports exactly the page it evicted");
    }

    #[test]
    fn enforce_loaded_limit_evicts_more_than_one_page_in_a_single_call() {
        // Reactivating pages via set_active (which doesn't enforce the
        // limit) can stack up loaded pages beyond it; the next new page load
        // must then evict more than one to converge, exercising the loop
        // rather than a single-eviction pass.
        let mut mgr: PageManager<MockEngine> = PageManager::new(Some(1));
        let id1 = insert_page(&mut mgr, "https://a.example");
        let id2 = insert_page(&mut mgr, "https://b.example");
        assert!(!mgr.is_page_loaded(&id1), "over limit 1, id1 unloaded by id2's insert");

        mgr.set_active(&id1); // reactivates id1 without enforcing the limit
        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(mgr.loaded_page_count(), 2, "both loaded transiently, over the limit of 1");

        let id3 = insert_page(&mut mgr, "https://c.example");
        assert_eq!(mgr.loaded_page_count(), 1, "converged back to the limit in one call");
        assert!(mgr.is_page_loaded(&id3));
        assert!(!mgr.is_page_loaded(&id1));
        assert!(!mgr.is_page_loaded(&id2));
    }

    #[test]
    fn current_foreground_page_is_never_unloaded() {
        let mut mgr: PageManager<MockEngine> = PageManager::new(Some(1));
        let id1 = insert_page(&mut mgr, "https://a.example");
        assert!(mgr.is_page_loaded(&id1));
        // id1 is both the only loaded page and the foreground page; nothing
        // else exists to unload, so it must survive even at limit 1.
        assert_eq!(mgr.active_id(), id1);
    }

    #[test]
    fn switching_to_an_unloaded_page_reactivates_it_without_evicting_others() {
        let mut mgr: PageManager<MockEngine> = PageManager::new(Some(2));
        let id1 = insert_page(&mut mgr, "https://a.example");
        let id2 = insert_page(&mut mgr, "https://b.example");
        let id3 = insert_page(&mut mgr, "https://c.example");
        assert!(!mgr.is_page_loaded(&id1)); // unloaded by the 3rd insert

        mgr.set_active(&id1);
        assert!(mgr.is_page_loaded(&id1), "switching to it should reactivate it");
        assert_eq!(mgr.active_id(), id1);
        // set_active must not itself re-enforce the limit: all three may be
        // loaded at once transiently until the next new page load.
        assert!(mgr.is_page_loaded(&id2));
        assert!(mgr.is_page_loaded(&id3));
        assert_eq!(mgr.loaded_page_count(), 3);
    }

    #[test]
    fn set_max_loaded_pages_enforces_the_new_limit_immediately() {
        // Unlike set_active reactivating a page (which defers enforcement),
        // explicitly changing the limit should evict right away.
        let mut mgr: PageManager<MockEngine> = PageManager::new(None);
        let id1 = insert_page(&mut mgr, "https://a.example");
        let id2 = insert_page(&mut mgr, "https://b.example");
        let id3 = insert_page(&mut mgr, "https://c.example");
        assert_eq!(mgr.loaded_page_count(), 3, "no limit yet, all loaded");

        mgr.set_max_loaded_pages(Some(1));
        assert_eq!(mgr.loaded_page_count(), 1, "tightening the limit evicts immediately");
        assert!(mgr.is_page_loaded(&id3), "the foreground page survives");
        assert!(!mgr.is_page_loaded(&id1));
        assert!(!mgr.is_page_loaded(&id2));
    }

    #[test]
    fn take_engine_removes_the_live_engine() {
        let mut mgr: PageManager<MockEngine> = PageManager::new(None);
        let id = insert_page(&mut mgr, "https://a.example");
        assert!(mgr.take_engine(&id).is_some(), "a freshly-inserted page has a live engine to take");
        assert!(mgr.take_engine(&id).is_none(), "already taken — nothing left to take a second time");
        assert!(mgr.take_engine("nonexistent-id").is_none());
    }

    #[test]
    fn install_engine_after_take_engine_restores_current_url() {
        let mut mgr: PageManager<MockEngine> = PageManager::new(None);
        let id = insert_page(&mut mgr, "https://a.example");
        let taken = mgr.take_engine(&id).expect("should have a live engine to take");
        // Simulate the caller freezing last_url the way browser-linux-gtk3's
        // unload_engines does, right before dropping the taken engine.
        mgr.page_mut(&id).unwrap().last_url = taken.current_url().unwrap();
        drop(taken);
        assert_eq!(mgr.page(&id).unwrap().current_url(), "https://a.example", "falls back to last_url while unloaded");

        mgr.install_engine(&id, MockEngine::new("https://b.example"));
        assert_eq!(
            mgr.page(&id).unwrap().current_url(),
            "https://b.example",
            "a live engine's URL takes priority over the frozen last_url"
        );
    }

    #[test]
    fn resolve_address_input_passes_through_urls_unchanged() {
        let settings = Settings::default();
        assert_eq!(resolve_address_input("https://example.com/path", &settings), "https://example.com/path");
        assert_eq!(resolve_address_input("about:blank", &settings), "about:blank");
    }

    #[test]
    fn resolve_address_input_no_spaces_still_gets_https_prefix() {
        // Locked in by switcher_test.rs's existing regression check: this
        // exact input must keep resolving this exact way.
        let settings = Settings::default();
        assert_eq!(
            resolve_address_input("some-nonexistent-domain-example", &settings),
            "https://some-nonexistent-domain-example"
        );
    }

    #[test]
    fn resolve_address_input_with_spaces_builds_a_search_url() {
        let settings = Settings::default();
        let resolved = resolve_address_input("how to cook rice", &settings);
        assert_eq!(resolved, "https://www.google.com/search?q=how%20to%20cook%20rice");
    }

    #[test]
    fn resolve_address_input_search_uses_whichever_engine_is_default() {
        let mut settings = Settings::default();
        settings.default_search_engine = "DuckDuckGo".to_string();
        let resolved = resolve_address_input("rust programming language", &settings);
        assert_eq!(resolved, "https://duckduckgo.com/?q=rust%20programming%20language");
    }
}
