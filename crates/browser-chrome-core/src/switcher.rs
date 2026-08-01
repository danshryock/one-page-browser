//! The switcher's row list: which rows exist, in what order, and what
//! happens when one is activated — extracted from (and, until now,
//! independently hand-copied by) `browser-linux-gtk3`'s
//! `rebuild_switcher_grid`, `browser-windows-reactor`'s `Tile`/
//! `switcher_overlay`, and `browser-macos-appkit`'s `SwitcherRow`/
//! `rebuild_switcher_rows` — see `ARCHITECTURE.md` §3.2 for the duplication
//! this closes.
//!
//! Each frontend's job now shrinks to: call `build_switcher_rows` whenever
//! the switcher opens or its query changes, render the result as native
//! widgets, and call `activate_row` when the user picks one (a click, or —
//! restoring `browser-linux-gtk3`'s Ctrl+Enter escape hatch, see
//! `ARCHITECTURE.md` §3.3 — a forced-new-page shortcut) to learn what should
//! happen next.

use browser_core::{domain_of, HistoryBackend, HistoryEntry, PageManager};
use render_engine::RenderEngine;

/// One row in the switcher's list — open pages matching the current query
/// first, then a trailing "add page" row, then (only once there's a query)
/// history matches for URLs not already open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitcherRow {
    Open {
        id: String,
        title: String,
        domain: String,
        /// Per-page color from `PageManager`'s palette — present in
        /// `browser-linux-gtk3`'s tiles from the start, but quietly dropped
        /// when `browser-windows-reactor`'s `Tile` and
        /// `browser-macos-appkit`'s `SwitcherRow` were built independently
        /// (see `ARCHITECTURE.md` §3.2) — restored here since every
        /// frontend gets it back for free once they render from this type
        /// instead of their own.
        color: &'static str,
    },
    Add,
    History {
        url: String,
        title: String,
        domain: String,
    },
}

/// What activating a row (clicking it, or a single unambiguous match from a
/// unified address-bar/search box — see `ARCHITECTURE.md` §3.3) should do.
/// A frontend matches on this and does native things; nothing here touches
/// a widget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitcherActivation {
    SwitchTo(String),
    OpenNewPage(String),
}

/// Rebuilds every switcher row from scratch: open pages matching `query`
/// (via `PageManager::matching_ids`, which already treats an empty query as
/// "match everything"), a trailing "add page" row, then, only once there's
/// a non-empty query, history matches whose URL isn't already open. Mirrors
/// `browser-linux-gtk3`'s `rebuild_switcher_grid` exactly (the first
/// implementation of this logic, and the one every later port re-derived).
pub fn build_switcher_rows<E: RenderEngine, H: HistoryBackend>(
    core: &PageManager<E>,
    history: &H,
    query: &str,
) -> Vec<SwitcherRow> {
    let open_matches = core.matching_ids(query);
    let mut rows: Vec<SwitcherRow> = Vec::new();
    for page in core.pages() {
        if !open_matches.contains(&page.id) {
            continue;
        }
        let title = page.title.borrow().clone();
        let title = if title.is_empty() { "New Page".to_string() } else { title };
        let url = page.current_url();
        let domain = domain_of(&url);
        let domain = if page.loaded { domain } else { format!("{domain} \u{b7} unloaded") };
        rows.push(SwitcherRow::Open { id: page.id.clone(), title, domain, color: page.color });
    }
    rows.push(SwitcherRow::Add);

    if !query.trim().is_empty() {
        let open_urls: Vec<String> = core.pages().iter().map(|p| p.current_url()).collect();
        let history_matches: Vec<HistoryEntry> = history
            .search(query, 8)
            .unwrap_or_else(|err| {
                eprintln!("history search failed: {err}");
                Vec::new()
            })
            .into_iter()
            .filter(|entry| !open_urls.contains(&entry.url))
            .collect();
        for entry in history_matches {
            let title = if entry.title.is_empty() { "New Page".to_string() } else { entry.title };
            rows.push(SwitcherRow::History { url: entry.url, title, domain: format!("{} \u{b7} history", entry.domain) });
        }
    }
    rows
}

/// What activating `rows[idx]` should do — `None` if `idx` is out of range
/// (a stale index from a race between a click and a rebuild; every existing
/// frontend already guards this the same way, just per-call-site instead of
/// once here). `start_page` is what an `Add` row resolves to (mirrors every
/// frontend's `add_page_and_switch.invoke(start_page.clone())` on that
/// tile).
pub fn activate_row(rows: &[SwitcherRow], idx: usize, start_page: &str) -> Option<SwitcherActivation> {
    match rows.get(idx)? {
        SwitcherRow::Open { id, .. } => Some(SwitcherActivation::SwitchTo(id.clone())),
        SwitcherRow::Add => Some(SwitcherActivation::OpenNewPage(start_page.to_string())),
        SwitcherRow::History { url, .. } => Some(SwitcherActivation::OpenNewPage(url.clone())),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use browser_core::{MemoryHistoryStore, PageManager};

    use super::*;
    use browser_core::testing::MockEngine;

    fn insert_page(mgr: &mut PageManager<MockEngine>, url: &str) -> String {
        let id = mgr.allocate_id();
        mgr.insert(id.clone(), MockEngine::new(url), Rc::new(RefCell::new(String::new())));
        id
    }

    #[test]
    fn empty_query_lists_every_open_page_plus_add() {
        let mut mgr = PageManager::<MockEngine>::new(None);
        insert_page(&mut mgr, "https://example.com");
        insert_page(&mut mgr, "https://rust-lang.org");
        let history = MemoryHistoryStore::default();

        let rows = build_switcher_rows(&mgr, &history, "");
        assert_eq!(rows.len(), 3); // 2 open pages + Add
        assert!(matches!(rows[2], SwitcherRow::Add));
    }

    #[test]
    fn query_filters_open_pages_and_includes_history_matches_not_already_open() {
        let mut mgr = PageManager::<MockEngine>::new(None);
        insert_page(&mut mgr, "https://example.com");
        let history = MemoryHistoryStore::default();
        history.record_visit("https://rust-lang.org/learn", "Learn Rust").unwrap();

        let rows = build_switcher_rows(&mgr, &history, "rust");
        // The open "example.com" page shouldn't match "rust" at all, so
        // just Add + the one history entry.
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0], SwitcherRow::Add));
        assert!(matches!(&rows[1], SwitcherRow::History { url, .. } if url == "https://rust-lang.org/learn"));
    }

    #[test]
    fn history_match_already_open_is_not_duplicated() {
        let mut mgr = PageManager::<MockEngine>::new(None);
        insert_page(&mut mgr, "https://rust-lang.org/learn");
        let history = MemoryHistoryStore::default();
        history.record_visit("https://rust-lang.org/learn", "Learn Rust").unwrap();

        let rows = build_switcher_rows(&mgr, &history, "rust");
        // Open page matches "rust" in its URL, so: Open row + Add — no
        // second History row for the same URL.
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0], SwitcherRow::Open { .. }));
        assert!(matches!(rows[1], SwitcherRow::Add));
    }

    #[test]
    fn open_row_carries_the_page_palette_color() {
        let mut mgr = PageManager::<MockEngine>::new(None);
        insert_page(&mut mgr, "https://example.com");
        let history = MemoryHistoryStore::default();

        let rows = build_switcher_rows(&mgr, &history, "");
        let SwitcherRow::Open { color, .. } = &rows[0] else { panic!("expected an Open row") };
        assert!(!color.is_empty());
    }

    #[test]
    fn unloaded_open_page_is_marked_in_its_domain() {
        let mut mgr = PageManager::<MockEngine>::new(Some(1));
        insert_page(&mut mgr, "https://example.com");
        // A second page evicts the first (limit of 1) but keeps it in the
        // list, just unloaded.
        insert_page(&mut mgr, "https://rust-lang.org");

        let history = MemoryHistoryStore::default();
        let rows = build_switcher_rows(&mgr, &history, "");
        let unloaded = rows.iter().find(|r| matches!(r, SwitcherRow::Open { id, .. } if id == "0")).unwrap();
        let SwitcherRow::Open { domain, .. } = unloaded else { unreachable!() };
        assert!(domain.contains("unloaded"), "expected 'unloaded' marker in {domain:?}");
    }

    #[test]
    fn activate_open_row_switches_to_it() {
        let mut mgr = PageManager::<MockEngine>::new(None);
        insert_page(&mut mgr, "https://example.com");
        let history = MemoryHistoryStore::default();
        let rows = build_switcher_rows(&mgr, &history, "");

        assert_eq!(activate_row(&rows, 0, "https://start.example"), Some(SwitcherActivation::SwitchTo("0".to_string())));
    }

    #[test]
    fn activate_add_row_opens_the_start_page() {
        let mgr = PageManager::<MockEngine>::new(None);
        let history = MemoryHistoryStore::default();
        let rows = build_switcher_rows(&mgr, &history, "");

        assert_eq!(rows, vec![SwitcherRow::Add]);
        assert_eq!(activate_row(&rows, 0, "https://start.example"), Some(SwitcherActivation::OpenNewPage("https://start.example".to_string())));
    }

    #[test]
    fn activate_history_row_opens_its_url() {
        let mgr = PageManager::<MockEngine>::new(None);
        let history = MemoryHistoryStore::default();
        history.record_visit("https://rust-lang.org/learn", "Learn Rust").unwrap();
        let rows = build_switcher_rows(&mgr, &history, "rust");

        let history_idx = rows.iter().position(|r| matches!(r, SwitcherRow::History { .. })).unwrap();
        assert_eq!(
            activate_row(&rows, history_idx, "https://start.example"),
            Some(SwitcherActivation::OpenNewPage("https://rust-lang.org/learn".to_string()))
        );
    }

    #[test]
    fn activate_out_of_range_index_is_none() {
        let mgr = PageManager::<MockEngine>::new(None);
        let history = MemoryHistoryStore::default();
        let rows = build_switcher_rows(&mgr, &history, "");
        assert_eq!(activate_row(&rows, 99, "https://start.example"), None);
    }
}
