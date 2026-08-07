//! The switcher's row list: which rows exist, in what order, and what
//! happens when one is activated — extracted from (and, until now,
//! independently hand-copied by) `browser-linux-gtk3`'s
//! `rebuild_switcher_grid`, `browser-windows-reactor`'s `Tile`/
//! `switcher_overlay`, and `browser-macos-appkit`'s `SwitcherRow`/
//! `rebuild_switcher_rows` — see `ARCHITECTURE.md` §3.2 for the duplication
//! this closes.
//!
//! Modeled on `browser-linux-gtk3`'s version specifically, not the (simpler)
//! shape `browser-windows-reactor`/`browser-macos-appkit` independently
//! re-derived: gtk3's is the original, and the *only* one of the three that
//! also searches bookmarks and lexically-similar history matches
//! (`HistoryBackend::search_similar`) — dropping those to migrate gtk3 onto
//! the simpler shape would have been exactly the kind of silent feature
//! loss this whole extraction exists to prevent (see `ARCHITECTURE.md`
//! §3.2's "drift already visible" callout). `bookmarks` is `Option<&
//! Bookmarks>` rather than a required parameter since only `browser-linux-
//! gtk3` has bookmark support wired up at all right now — `None` simply
//! skips that source, so the other three frontends keep their current
//! (bookmark-free) behavior exactly.
//!
//! Each frontend's job now shrinks to: call `build_switcher_rows` whenever
//! the switcher opens or its query changes, render the result as native
//! widgets, and call `activate_row` when the user picks one (a click, or —
//! restoring `browser-linux-gtk3`'s Ctrl+Enter escape hatch, see
//! `ARCHITECTURE.md` §3.3 — a forced-new-page shortcut) to learn what should
//! happen next.

use browser_core::{domain_of, Bookmarks, HistoryBackend, HistoryEntry, PageManager};
use render_engine::RenderEngine;

/// One row in the switcher's list — open pages matching the current query
/// first, then a trailing "add page" row, then (only once there's a query)
/// exact history matches, then bookmark matches, then lexically-similar
/// history matches — each source skips any URL a prior one already showed
/// (mirrors `browser-linux-gtk3`'s single running `shown_urls` list across
/// all three).
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
    /// Exact substring match against a `HistoryBackend` (URL or title).
    History { url: String, title: String, domain: String },
    /// Exact substring match against `Bookmarks` — only ever produced when
    /// `build_switcher_rows` is given `Some(bookmarks)`.
    Bookmark { url: String, title: String, domain: String },
    /// Lexically-similar (not necessarily substring-matching) history entry
    /// — see `HistoryBackend::search_similar`'s doc comment for exactly
    /// what "similar" means here.
    Similar { url: String, title: String, domain: String },
    /// A non-activatable section label ("Open Pages", "History", ...) —
    /// `activate_row` always returns `None` for one of these, and every
    /// frontend renders it as a divider/heading instead of a normal row.
    /// Inserted once per non-empty group, immediately before that group's
    /// first real row (never for an empty group — no "History" heading
    /// with nothing under it).
    Header(&'static str),
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

/// Rebuilds every switcher row from scratch. Mirrors `browser-linux-gtk3`'s
/// `rebuild_switcher_grid` exactly (the first implementation of this logic,
/// and the one every later port re-derived, some incompletely — see this
/// module's doc comment): open pages matching `query` (via
/// `PageManager::matching_ids`, which already treats an empty query as
/// "match everything"), a trailing "add page" row, then, only once there's
/// a non-empty query: exact history matches, bookmark matches (if
/// `bookmarks` is `Some`), then lexically-similar history matches — each
/// pass skipping any URL a prior one (an open page, or an earlier pass in
/// this same call) already produced.
pub fn build_switcher_rows<E: RenderEngine, H: HistoryBackend>(
    core: &PageManager<E>,
    history: &H,
    bookmarks: Option<&Bookmarks>,
    query: &str,
) -> Vec<SwitcherRow> {
    let open_matches = core.matching_ids(query);
    let mut rows: Vec<SwitcherRow> = Vec::new();
    let mut open_rows: Vec<SwitcherRow> = Vec::new();
    for page in core.pages() {
        if !open_matches.contains(&page.id) {
            continue;
        }
        let title = page.title.borrow().clone();
        let title = if title.is_empty() { "New Page".to_string() } else { title };
        let url = page.current_url();
        let domain = domain_of(&url);
        let domain = if page.loaded { domain } else { format!("{domain} \u{b7} unloaded") };
        open_rows.push(SwitcherRow::Open { id: page.id.clone(), title, domain, color: page.color });
    }
    // No header before an empty group, `Open` included — matches every
    // other group below (a "History" heading over zero history rows would
    // be exactly as pointless).
    if !open_rows.is_empty() {
        rows.push(SwitcherRow::Header("Open Pages"));
        rows.extend(open_rows);
    }
    rows.push(SwitcherRow::Add);

    if query.trim().is_empty() {
        return rows;
    }

    let mut shown_urls: Vec<String> = core.pages().iter().map(|p| p.current_url()).collect();

    let history_matches: Vec<HistoryEntry> = history.search(query, 8).unwrap_or_else(|err| {
        eprintln!("history search failed: {err}");
        Vec::new()
    });
    let mut history_rows: Vec<SwitcherRow> = Vec::new();
    for entry in history_matches {
        if shown_urls.contains(&entry.url) {
            continue;
        }
        shown_urls.push(entry.url.clone());
        let title = if entry.title.is_empty() { "New Page".to_string() } else { entry.title };
        history_rows.push(SwitcherRow::History { url: entry.url, title, domain: format!("{} \u{b7} history", entry.domain) });
    }
    if !history_rows.is_empty() {
        rows.push(SwitcherRow::Header("History"));
        rows.extend(history_rows);
    }

    if let Some(bookmarks) = bookmarks {
        let mut bookmark_rows: Vec<SwitcherRow> = Vec::new();
        for bookmark in bookmarks.search(query).into_iter().take(8) {
            if shown_urls.contains(&bookmark.url) {
                continue;
            }
            shown_urls.push(bookmark.url.clone());
            let title = if bookmark.title.is_empty() { "New Page".to_string() } else { bookmark.title.clone() };
            bookmark_rows.push(SwitcherRow::Bookmark {
                url: bookmark.url.clone(),
                title,
                domain: format!("{} \u{b7} bookmark", bookmark.domain),
            });
        }
        if !bookmark_rows.is_empty() {
            rows.push(SwitcherRow::Header("Bookmarks"));
            rows.extend(bookmark_rows);
        }
    }

    let similar_matches: Vec<HistoryEntry> = history.search_similar(query, 5).unwrap_or_else(|err| {
        eprintln!("history similarity search failed: {err}");
        Vec::new()
    });
    let mut similar_rows: Vec<SwitcherRow> = Vec::new();
    for entry in similar_matches {
        if shown_urls.contains(&entry.url) {
            continue;
        }
        shown_urls.push(entry.url.clone());
        let title = if entry.title.is_empty() { "New Page".to_string() } else { entry.title };
        similar_rows.push(SwitcherRow::Similar { url: entry.url, title, domain: format!("{} \u{b7} similar", entry.domain) });
    }
    if !similar_rows.is_empty() {
        rows.push(SwitcherRow::Header("Similar"));
        rows.extend(similar_rows);
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
        SwitcherRow::History { url, .. } | SwitcherRow::Bookmark { url, .. } | SwitcherRow::Similar { url, .. } => {
            Some(SwitcherActivation::OpenNewPage(url.clone()))
        }
        SwitcherRow::Header(_) => None,
    }
}

/// The nearest activatable row to `from` in `direction` (skipping any
/// `Header` rows in between) — the shared model for arrow-key navigation,
/// so each frontend's Up/Down handling is just "call this, then select
/// whatever it returns" rather than re-deriving the skip-headers logic
/// itself. `from: None` starts from the first (Down) or last (Up)
/// activatable row — the natural "nothing selected yet" case.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SwitcherNavDirection {
    Up,
    Down,
}

pub fn next_activatable_row(rows: &[SwitcherRow], from: Option<usize>, direction: SwitcherNavDirection) -> Option<usize> {
    let is_activatable = |row: &SwitcherRow| !matches!(row, SwitcherRow::Header(_));
    match direction {
        SwitcherNavDirection::Down => {
            let start = from.map(|i| i + 1).unwrap_or(0);
            (start..rows.len()).find(|&i| is_activatable(&rows[i]))
        }
        SwitcherNavDirection::Up => {
            let start = from.unwrap_or(rows.len());
            (0..start).rev().find(|&i| is_activatable(&rows[i]))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use browser_core::{Bookmarks, MemoryHistoryStore, PageManager};

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

        let rows = build_switcher_rows(&mgr, &history, None, "");
        assert_eq!(rows.len(), 4); // Header + 2 open pages + Add
        assert!(matches!(rows[0], SwitcherRow::Header("Open Pages")));
        assert!(matches!(rows[3], SwitcherRow::Add));
    }

    #[test]
    fn query_filters_open_pages_and_includes_history_matches_not_already_open() {
        let mut mgr = PageManager::<MockEngine>::new(None);
        insert_page(&mut mgr, "https://example.com");
        let history = MemoryHistoryStore::default();
        history.record_visit("https://rust-lang.org/learn", "Learn Rust").unwrap();

        let rows = build_switcher_rows(&mgr, &history, None, "rust");
        // The open "example.com" page shouldn't match "rust" at all (no
        // "Open Pages" header at all, then), so: Add, then a "History"
        // header, then the one history entry.
        assert_eq!(rows.len(), 3);
        assert!(matches!(rows[0], SwitcherRow::Add));
        assert!(matches!(rows[1], SwitcherRow::Header("History")));
        assert!(matches!(&rows[2], SwitcherRow::History { url, .. } if url == "https://rust-lang.org/learn"));
    }

    #[test]
    fn history_match_already_open_is_not_duplicated() {
        let mut mgr = PageManager::<MockEngine>::new(None);
        insert_page(&mut mgr, "https://rust-lang.org/learn");
        let history = MemoryHistoryStore::default();
        history.record_visit("https://rust-lang.org/learn", "Learn Rust").unwrap();

        let rows = build_switcher_rows(&mgr, &history, None, "rust");
        // Open page matches "rust" in its URL, so: an "Open Pages" header,
        // the Open row, then Add — no second History row for the same URL.
        assert_eq!(rows.len(), 3);
        assert!(matches!(rows[0], SwitcherRow::Header("Open Pages")));
        assert!(matches!(rows[1], SwitcherRow::Open { .. }));
        assert!(matches!(rows[2], SwitcherRow::Add));
    }

    #[test]
    fn open_row_carries_the_page_palette_color() {
        let mut mgr = PageManager::<MockEngine>::new(None);
        insert_page(&mut mgr, "https://example.com");
        let history = MemoryHistoryStore::default();

        let rows = build_switcher_rows(&mgr, &history, None, "");
        let SwitcherRow::Open { color, .. } = &rows[1] else { panic!("expected an Open row") };
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
        let rows = build_switcher_rows(&mgr, &history, None, "");
        let unloaded = rows.iter().find(|r| matches!(r, SwitcherRow::Open { id, .. } if id == "0")).unwrap();
        let SwitcherRow::Open { domain, .. } = unloaded else { unreachable!() };
        assert!(domain.contains("unloaded"), "expected 'unloaded' marker in {domain:?}");
    }

    #[test]
    fn bookmark_match_is_included_when_bookmarks_provided() {
        let mgr = PageManager::<MockEngine>::new(None);
        let history = MemoryHistoryStore::default();
        let mut bookmarks = Bookmarks::default();
        bookmarks.add("https://rust-lang.org/book", "The Rust Book", 0);

        let rows = build_switcher_rows(&mgr, &history, Some(&bookmarks), "rust");
        assert!(rows.iter().any(|r| matches!(r, SwitcherRow::Bookmark { url, .. } if url == "https://rust-lang.org/book")));
    }

    #[test]
    fn bookmark_already_shown_as_history_is_not_duplicated() {
        let mgr = PageManager::<MockEngine>::new(None);
        let history = MemoryHistoryStore::default();
        history.record_visit("https://rust-lang.org/book", "The Rust Book").unwrap();
        let mut bookmarks = Bookmarks::default();
        bookmarks.add("https://rust-lang.org/book", "The Rust Book", 0);

        let rows = build_switcher_rows(&mgr, &history, Some(&bookmarks), "rust");
        let matches: Vec<_> = rows.iter().filter(|r| matches!(r, SwitcherRow::History { .. } | SwitcherRow::Bookmark { .. })).collect();
        assert_eq!(matches.len(), 1, "expected exactly one row for the shared URL, got {matches:?}");
        assert!(matches!(matches[0], SwitcherRow::History { .. }), "history match should win, being checked first");
    }

    #[test]
    fn no_bookmarks_source_means_no_bookmark_rows_at_all() {
        let mgr = PageManager::<MockEngine>::new(None);
        let history = MemoryHistoryStore::default();
        // No `Bookmarks::add` call needed — passing `None` should mean the
        // bookmark source is never consulted at all, matching
        // `browser-windows-reactor`/`browser-macos-appkit`/
        // `browser-windows-winui`'s current (bookmark-free) behavior.
        let rows = build_switcher_rows(&mgr, &history, None, "anything");
        assert!(!rows.iter().any(|r| matches!(r, SwitcherRow::Bookmark { .. })));
    }

    #[test]
    fn activate_open_row_switches_to_it() {
        let mut mgr = PageManager::<MockEngine>::new(None);
        insert_page(&mut mgr, "https://example.com");
        let history = MemoryHistoryStore::default();
        let rows = build_switcher_rows(&mgr, &history, None, "");

        // rows[0] is the "Open Pages" header now, not the row itself.
        assert_eq!(activate_row(&rows, 1, "https://start.example"), Some(SwitcherActivation::SwitchTo("0".to_string())));
    }

    #[test]
    fn activate_add_row_opens_the_start_page() {
        let mgr = PageManager::<MockEngine>::new(None);
        let history = MemoryHistoryStore::default();
        let rows = build_switcher_rows(&mgr, &history, None, "");

        assert_eq!(rows, vec![SwitcherRow::Add]);
        assert_eq!(activate_row(&rows, 0, "https://start.example"), Some(SwitcherActivation::OpenNewPage("https://start.example".to_string())));
    }

    #[test]
    fn activate_history_row_opens_its_url() {
        let mgr = PageManager::<MockEngine>::new(None);
        let history = MemoryHistoryStore::default();
        history.record_visit("https://rust-lang.org/learn", "Learn Rust").unwrap();
        let rows = build_switcher_rows(&mgr, &history, None, "rust");

        let history_idx = rows.iter().position(|r| matches!(r, SwitcherRow::History { .. })).unwrap();
        assert_eq!(
            activate_row(&rows, history_idx, "https://start.example"),
            Some(SwitcherActivation::OpenNewPage("https://rust-lang.org/learn".to_string()))
        );
    }

    #[test]
    fn activate_bookmark_row_opens_its_url() {
        let mgr = PageManager::<MockEngine>::new(None);
        let history = MemoryHistoryStore::default();
        let mut bookmarks = Bookmarks::default();
        bookmarks.add("https://rust-lang.org/book", "The Rust Book", 0);
        let rows = build_switcher_rows(&mgr, &history, Some(&bookmarks), "rust");

        let idx = rows.iter().position(|r| matches!(r, SwitcherRow::Bookmark { .. })).unwrap();
        assert_eq!(
            activate_row(&rows, idx, "https://start.example"),
            Some(SwitcherActivation::OpenNewPage("https://rust-lang.org/book".to_string()))
        );
    }

    #[test]
    fn activate_out_of_range_index_is_none() {
        let mgr = PageManager::<MockEngine>::new(None);
        let history = MemoryHistoryStore::default();
        let rows = build_switcher_rows(&mgr, &history, None, "");
        assert_eq!(activate_row(&rows, 99, "https://start.example"), None);
    }

    #[test]
    fn activate_header_row_is_none() {
        let mut mgr = PageManager::<MockEngine>::new(None);
        insert_page(&mut mgr, "https://example.com");
        let history = MemoryHistoryStore::default();
        let rows = build_switcher_rows(&mgr, &history, None, "");
        assert!(matches!(rows[0], SwitcherRow::Header("Open Pages")));
        assert_eq!(activate_row(&rows, 0, "https://start.example"), None);
    }

    #[test]
    fn no_header_for_an_empty_group() {
        // No open pages, no query: just Add — no "Open Pages" header with
        // nothing under it, no "History"/"Bookmarks" headers either since
        // there's no query to search with.
        let mgr = PageManager::<MockEngine>::new(None);
        let history = MemoryHistoryStore::default();
        let rows = build_switcher_rows(&mgr, &history, None, "");
        assert!(!rows.iter().any(|r| matches!(r, SwitcherRow::Header(_))));
    }

    #[test]
    fn next_activatable_row_skips_headers_in_both_directions() {
        let mut mgr = PageManager::<MockEngine>::new(None);
        insert_page(&mut mgr, "https://example.com");
        let history = MemoryHistoryStore::default();
        let rows = build_switcher_rows(&mgr, &history, None, "");
        // Header("Open Pages"), Open, Add.
        assert!(matches!(rows[0], SwitcherRow::Header(_)));

        // Starting from nothing, Down lands on the first real row (index 1,
        // skipping the header at 0), not the header itself.
        assert_eq!(next_activatable_row(&rows, None, SwitcherNavDirection::Down), Some(1));
        // From the last row, Up walks back to index 1 too — never landing
        // on index 0's header.
        assert_eq!(next_activatable_row(&rows, Some(2), SwitcherNavDirection::Up), Some(1));
        // From row 1, Up has nowhere activatable left (row 0 is a header) —
        // stays put by returning None, same as running off either end.
        assert_eq!(next_activatable_row(&rows, Some(1), SwitcherNavDirection::Up), None);
    }
}
