//! Per-profile browsing history: recording visited pages and searching them
//! back — surfaced through the switcher grid in each native chrome, not a
//! separate view (this browser has no tabs by design; the switcher is
//! already the one navigation hub).
//!
//! `libsql`'s `Connection::execute`/`query` are `async fn`s, but every visit
//! here is a local SQLite call — effectively CPU-bound, not real async I/O —
//! so `HistoryStore` just owns a single-threaded `tokio` runtime and blocks
//! on each call. Keeps every call site in both frontends synchronous, the
//! same way `Settings::load`/`save` already do blocking file I/O.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{domain_of, Profile};

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS history (
    url TEXT PRIMARY KEY,
    domain TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    first_visited_at INTEGER NOT NULL,
    visited_at INTEGER NOT NULL,
    visit_count INTEGER NOT NULL DEFAULT 1
);
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub url: String,
    pub domain: String,
    pub title: String,
    pub first_visited_at: i64,
    pub visited_at: i64,
    pub visit_count: i64,
}

pub struct HistoryStore {
    /// Kept alive alongside `conn` even though nothing reads it directly —
    /// `Connection` doesn't own the database file handle itself.
    _db: libsql::Database,
    conn: libsql::Connection,
    rt: tokio::runtime::Runtime,
}

impl HistoryStore {
    /// Opens (creating if necessary) `profile`'s history database, applying
    /// the schema. Fails if this platform has no data directory available
    /// (see `Profile::history_db_path`) rather than silently no-op'ing —
    /// unlike `Settings::load`, a missing history store isn't something a
    /// caller can reasonably fall back from.
    pub fn open(profile: &Profile) -> anyhow::Result<Self> {
        let path = profile
            .history_db_path()
            .ok_or_else(|| anyhow::anyhow!("no data directory available on this platform"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::open_at(&path)
    }

    /// Split out from `open` so tests can round-trip through a throwaway
    /// path instead of the real user data directory — same reasoning as
    /// `Settings::load_from`/`save_to`.
    fn open_at(path: &Path) -> anyhow::Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread().build()?;
        let db = rt.block_on(libsql::Builder::new_local(path).build())?;
        let conn = db.connect()?;
        rt.block_on(conn.execute_batch(SCHEMA_SQL))?;
        Ok(Self { _db: db, conn, rt })
    }

    /// Opens a history store that never touches disk at all — nothing to
    /// clean up, and it vanishes the moment the process exits. Used for
    /// ephemeral (private/incognito/guest) profiles, where recording history
    /// at all would defeat the point, but the switcher grid's history search
    /// still expects a real `HistoryStore` to query against (an empty one,
    /// in this case, for the lifetime of the session).
    pub fn open_in_memory() -> anyhow::Result<Self> {
        // libsql (like the sqlite3 it wraps) treats the literal path
        // ":memory:" as a request for a private, in-process-only database
        // rather than a real file — confirmed against this exact libsql
        // version rather than assumed, since it's not a documented
        // `Builder` method of its own.
        Self::open_at(Path::new(":memory:"))
    }

    /// Records a fresh visit to `url` with its (possibly just-updated)
    /// `title`. Upserts by `url`: a repeat visit updates `title`/`visited_at`
    /// and increments `visit_count`, rather than growing an unbounded visit
    /// log — `first_visited_at`/`domain` are set once and never overwritten.
    pub fn record_visit(&self, url: &str, title: &str) -> anyhow::Result<()> {
        let domain = domain_of(url);
        let now = now_unix();
        self.rt.block_on(self.conn.execute(
            "INSERT INTO history (url, domain, title, first_visited_at, visited_at, visit_count) \
             VALUES (?1, ?2, ?3, ?4, ?4, 1) \
             ON CONFLICT(url) DO UPDATE SET \
                title = excluded.title, \
                visited_at = excluded.visited_at, \
                visit_count = visit_count + 1",
            libsql::params![url, domain, title, now],
        ))?;
        Ok(())
    }

    /// Entries whose `url` or `title` contains `query` (case-insensitive —
    /// SQLite's `LIKE` is ASCII case-insensitive by default, consistent
    /// enough with `page_matches_query`'s own lowercase-based matching),
    /// most-recently-visited first. `domain` isn't matched separately since
    /// it's always a substring of `url` already — matching it too couldn't
    /// change which rows come back.
    pub fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<HistoryEntry>> {
        let pattern = format!("%{query}%");
        let mut rows = self.rt.block_on(self.conn.query(
            "SELECT url, domain, title, first_visited_at, visited_at, visit_count FROM history \
             WHERE url LIKE ?1 OR title LIKE ?1 ORDER BY visited_at DESC LIMIT ?2",
            libsql::params![pattern, limit as i64],
        ))?;

        let mut entries = Vec::new();
        while let Some(row) = self.rt.block_on(rows.next())? {
            entries.push(HistoryEntry {
                url: row.get(0)?,
                domain: row.get(1)?,
                title: row.get(2)?,
                first_visited_at: row.get(3)?,
                visited_at: row.get(4)?,
                visit_count: row.get(5)?,
            });
        }
        Ok(entries)
    }
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> HistoryStore {
        let path = std::env::temp_dir().join(format!("claude-browser-test-history-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        HistoryStore::open_at(&path).expect("open_at should succeed against a throwaway path")
    }

    #[test]
    fn recording_a_visit_is_searchable_by_url_and_title() {
        let store = temp_store("search");
        store.record_visit("https://example.com/rust", "Rust Programming Language").unwrap();

        let by_url = store.search("example.com", 10).unwrap();
        assert_eq!(by_url.len(), 1);
        assert_eq!(by_url[0].url, "https://example.com/rust");
        assert_eq!(by_url[0].domain, "example.com");

        let by_title = store.search("rust programming", 10).unwrap();
        assert_eq!(by_title.len(), 1, "search should be case-insensitive");
        assert_eq!(by_title[0].title, "Rust Programming Language");
    }

    #[test]
    fn revisiting_a_url_updates_it_instead_of_duplicating() {
        let store = temp_store("revisit");
        store.record_visit("https://example.com/a", "First Title").unwrap();
        let first = store.search("example.com/a", 10).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].visit_count, 1);
        let first_visited_at = first[0].first_visited_at;

        store.record_visit("https://example.com/a", "Updated Title").unwrap();
        let second = store.search("example.com/a", 10).unwrap();
        assert_eq!(second.len(), 1, "revisiting shouldn't create a second row");
        assert_eq!(second[0].title, "Updated Title", "title should be refreshed");
        assert_eq!(second[0].visit_count, 2, "visit_count should increment");
        assert_eq!(second[0].first_visited_at, first_visited_at, "first_visited_at should never move");
    }

    #[test]
    fn search_orders_most_recently_visited_first() {
        let store = temp_store("ordering");
        store.record_visit("https://a.example/one", "One").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100)); // visited_at has 1-second resolution
        store.record_visit("https://a.example/two", "Two").unwrap();

        let results = store.search("a.example", 10).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://a.example/two", "most recently visited should come first");
        assert_eq!(results[1].url, "https://a.example/one");
    }

    #[test]
    fn empty_store_returns_no_results_without_erroring() {
        let store = temp_store("empty");
        assert_eq!(store.search("anything", 10).unwrap(), vec![]);
    }

    #[test]
    fn in_memory_store_records_and_searches_without_touching_disk() {
        let store = HistoryStore::open_in_memory().expect("in-memory open should succeed");
        store.record_visit("https://example.com/a", "A").unwrap();
        let results = store.search("example.com", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com/a");
    }
}
