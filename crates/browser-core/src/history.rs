//! Per-profile browsing history: recording visited pages and searching them
//! back — surfaced through the switcher grid in each native chrome, not a
//! separate view (this browser has no tabs by design; the switcher is
//! already the one navigation hub).
//!
//! `libsql`'s `Connection::execute`/`query` are `async fn`s, but for the
//! local (embedded) backend used here they're not real async I/O at all —
//! confirmed by reading libsql's own source (`local/impls.rs`): each one is
//! a synchronous SQLite FFI call with no `.await` point that ever actually
//! yields, wrapped in `async fn` purely for API uniformity with libsql's
//! separate remote/HTTP backend. So every call always completes on its very
//! first poll — there's nothing here for a real async runtime (a reactor,
//! a thread pool, task scheduling across polls) to actually do. Blocking on
//! each call via `futures_executor::block_on` (a plain function, no runtime
//! object, no I/O/timer driver, no threads — just polls the future to
//! completion) reflects that accurately, keeping every call site in both
//! frontends synchronous the same way `Settings::load`/`save` already do
//! blocking file I/O. (Previously used a single-threaded `tokio` runtime for
//! this same purpose — replaced during the `browser-windows-winui` Windows
//! CI launch-crash investigation: not implicated by that investigation's own
//! bisection testing, but removing it was a legitimate simplification
//! either way, since tokio's reactor/threading machinery was always dead
//! weight for a workload that's never actually asynchronous.)

use std::cell::RefCell;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{domain_of, embedding, Profile};

/// Abstracts over what a "history store" actually needs to do, independent
/// of how it's backed — added to isolate `libsql` itself (not just the
/// `tokio` runtime bridging its `async fn`s — see this module's doc comment
/// on that) as a variable during the `browser-windows-winui` Windows CI
/// launch-crash investigation (see `summaries/windows-github-actions-ci.md`):
/// `MemoryHistoryStore` below implements the same trait with no `libsql`/SQL
/// involved at all, so a WinUI 3 smoke test can exercise the exact same
/// `record_visit`/`search`/`search_similar` call shape the real app makes
/// without linking libsql-ffi's bundled SQLite into the picture.
///
/// `HistoryStore`'s own inherent methods (below) already have this exact
/// signature shape; this trait doesn't change any existing call site's
/// behavior, since inherent methods take priority over trait methods when
/// called directly (`store.search(...)` still resolves to the inherent
/// method) — it only matters for code that wants to be generic over which
/// backend it's talking to.
pub trait HistoryBackend {
    fn record_visit(&self, url: &str, title: &str) -> anyhow::Result<()>;
    fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<HistoryEntry>>;
    fn search_similar(&self, query: &str, limit: usize) -> anyhow::Result<Vec<HistoryEntry>>;
}

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS history (
    url TEXT PRIMARY KEY,
    domain TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    first_visited_at INTEGER NOT NULL,
    visited_at INTEGER NOT NULL,
    visit_count INTEGER NOT NULL DEFAULT 1,
    embedding BLOB
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

    /// Same as `open`, but encrypted at rest with `passphrase` — for a
    /// profile with `Profile::has_passphrase()` set. Uses libsql's native
    /// encryption support (SQLite3 Multiple Ciphers' AES-256-CBC cipher,
    /// applied via `sqlite3_key` under the hood): the raw passphrase bytes
    /// are handed straight to it, the same way SQLCipher/wxSQLite3-family
    /// APIs work — key derivation from the passphrase happens inside the
    /// cipher extension itself, not something this code needs to do.
    ///
    /// The very first successful open of a brand-new (empty) database file
    /// with a given passphrase is what *establishes* that database's
    /// encryption — there's no separate "set up encryption" step. Opening
    /// an *existing* encrypted database with the wrong passphrase doesn't
    /// fail immediately (the cipher extension doesn't know the key is wrong
    /// until it actually tries to decrypt a page): it's only once
    /// `execute_batch(SCHEMA_SQL)` below actually reads from the database
    /// that a bad passphrase surfaces as a real error — which callers should
    /// treat as "wrong passphrase, ask again," not a generic I/O failure.
    ///
    /// Only actually implemented on Linux and macOS (see the `libsql`
    /// dependency's `[target.'cfg(any(target_os = "linux", target_os =
    /// "macos"))']` scoping in `Cargo.toml` for why — confirmed to actually
    /// cross-compile/link on macOS via `cargo zigbuild`, the same finding
    /// that already unblocked `PasswordStore::open_encrypted` there) — every
    /// other target gets a plain error instead, deliberately *not* silently
    /// falling back to an unencrypted open, which would be a silent security
    /// downgrade for anyone who thinks they got encryption.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn open_encrypted(profile: &Profile, passphrase: &str) -> anyhow::Result<Self> {
        let path = profile
            .history_db_path()
            .ok_or_else(|| anyhow::anyhow!("no data directory available on this platform"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let builder = libsql::Builder::new_local(&path).encryption_config(libsql::EncryptionConfig::new(
            libsql::Cipher::Aes256Cbc,
            bytes::Bytes::copy_from_slice(passphrase.as_bytes()),
        ));
        let db = futures_executor::block_on(builder.build())?;
        let conn = db.connect()?;
        futures_executor::block_on(conn.execute_batch(SCHEMA_SQL))?;
        migrate_add_embedding_column(&conn);
        Ok(Self { _db: db, conn })
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub fn open_encrypted(_profile: &Profile, _passphrase: &str) -> anyhow::Result<Self> {
        anyhow::bail!("passphrase-protected profiles aren't available on this platform yet")
    }

    /// Split out from `open`/`open_in_memory` so tests can round-trip
    /// through a throwaway path instead of the real user data directory —
    /// same reasoning as `Settings::load_from`/`save_to`. Unlike
    /// `open_encrypted`, never touches encryption at all — no need for this
    /// one to vary by platform.
    fn open_at(path: &Path) -> anyhow::Result<Self> {
        let db = futures_executor::block_on(libsql::Builder::new_local(path).build())?;
        let conn = db.connect()?;
        futures_executor::block_on(conn.execute_batch(SCHEMA_SQL))?;
        migrate_add_embedding_column(&conn);
        Ok(Self { _db: db, conn })
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
    /// Also (re-)computes `title`'s embedding (see the `embedding` module)
    /// for `search_similar` to use, since a revisit's title may have changed
    /// since the last time this URL was recorded.
    pub fn record_visit(&self, url: &str, title: &str) -> anyhow::Result<()> {
        let domain = domain_of(url);
        let now = now_unix();
        let embedding_literal = embedding::to_sql_literal(&embedding::embed(title));
        futures_executor::block_on(self.conn.execute(
            "INSERT INTO history (url, domain, title, first_visited_at, visited_at, visit_count, embedding) \
             VALUES (?1, ?2, ?3, ?4, ?4, 1, vector32(?5)) \
             ON CONFLICT(url) DO UPDATE SET \
                title = excluded.title, \
                visited_at = excluded.visited_at, \
                visit_count = visit_count + 1, \
                embedding = excluded.embedding",
            libsql::params![url, domain, title, now, embedding_literal],
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
        let mut rows = futures_executor::block_on(self.conn.query(
            "SELECT url, domain, title, first_visited_at, visited_at, visit_count FROM history \
             WHERE url LIKE ?1 OR title LIKE ?1 ORDER BY visited_at DESC LIMIT ?2",
            libsql::params![pattern, limit as i64],
        ))?;
        collect_entries(&mut rows)
    }

    /// Entries whose *title* is lexically similar to `query`'s, by cosine
    /// distance between their hashing-trick embeddings (see the `embedding`
    /// module's doc comment for exactly what kind of "similar" this is —
    /// shared vocabulary, not semantic meaning), most-similar first. Uses
    /// libsql's native `vector_distance_cos` — a real, built-in SQL function
    /// in the bundled SQLite, not something this code implements itself.
    ///
    /// Caps results to distance `< 0.9` (empirically: two titles sharing
    /// essentially no vocabulary land close to `1.0`, near-orthogonal random
    /// hashed vectors) so a query with nothing genuinely similar in history
    /// doesn't just return whatever rows happen to be least-dissimilar —
    /// callers should treat this as "meaningfully similar or nothing," not
    /// "the N closest regardless of how close."
    pub fn search_similar(&self, query: &str, limit: usize) -> anyhow::Result<Vec<HistoryEntry>> {
        let embedding_literal = embedding::to_sql_literal(&embedding::embed(query));
        let mut rows = futures_executor::block_on(self.conn.query(
            "SELECT url, domain, title, first_visited_at, visited_at, visit_count, \
                    vector_distance_cos(embedding, vector32(?1)) AS distance \
             FROM history \
             WHERE embedding IS NOT NULL AND distance < 0.9 \
             ORDER BY distance ASC LIMIT ?2",
            libsql::params![embedding_literal, limit as i64],
        ))?;
        collect_entries(&mut rows)
    }
}

impl HistoryBackend for HistoryStore {
    fn record_visit(&self, url: &str, title: &str) -> anyhow::Result<()> {
        self.record_visit(url, title)
    }

    fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<HistoryEntry>> {
        self.search(query, limit)
    }

    fn search_similar(&self, query: &str, limit: usize) -> anyhow::Result<Vec<HistoryEntry>> {
        self.search_similar(query, limit)
    }
}

/// A `HistoryBackend` with no `libsql`/SQL involved at all — entries live in
/// a plain `Vec` behind a `RefCell`, `search` is a manual substring scan, and
/// `search_similar` uses the same `embedding::embed`/`cosine_distance` the
/// libsql-backed `HistoryStore` computes anyway, just compared directly in
/// Rust instead of via `vector_distance_cos` in SQL. Behavior deliberately
/// mirrors `HistoryStore` exactly (same upsert-by-url semantics in
/// `record_visit`, same `distance < 0.9` cutoff, same most-recent-first/
/// most-similar-first ordering) so the two are interchangeable from a
/// caller's perspective — this exists to isolate `libsql` itself as a
/// variable (see this module's `HistoryBackend` doc comment), not to be a
/// lesser/partial implementation.
#[derive(Default)]
pub struct MemoryHistoryStore {
    entries: RefCell<Vec<(HistoryEntry, [f32; embedding::DIMS])>>,
}

impl MemoryHistoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl HistoryBackend for MemoryHistoryStore {
    fn record_visit(&self, url: &str, title: &str) -> anyhow::Result<()> {
        let now = now_unix();
        let emb = embedding::embed(title);
        let mut entries = self.entries.borrow_mut();
        if let Some((entry, stored_emb)) = entries.iter_mut().find(|(entry, _)| entry.url == url) {
            entry.title = title.to_string();
            entry.visited_at = now;
            entry.visit_count += 1;
            *stored_emb = emb;
        } else {
            entries.push((
                HistoryEntry {
                    url: url.to_string(),
                    domain: domain_of(url),
                    title: title.to_string(),
                    first_visited_at: now,
                    visited_at: now,
                    visit_count: 1,
                },
                emb,
            ));
        }
        Ok(())
    }

    fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<HistoryEntry>> {
        let query = query.to_lowercase();
        let entries = self.entries.borrow();
        let mut matches: Vec<HistoryEntry> = entries
            .iter()
            .filter(|(entry, _)| entry.url.to_lowercase().contains(&query) || entry.title.to_lowercase().contains(&query))
            .map(|(entry, _)| entry.clone())
            .collect();
        matches.sort_by_key(|entry| std::cmp::Reverse(entry.visited_at));
        matches.truncate(limit);
        Ok(matches)
    }

    fn search_similar(&self, query: &str, limit: usize) -> anyhow::Result<Vec<HistoryEntry>> {
        let query_embedding = embedding::embed(query);
        let entries = self.entries.borrow();
        let mut scored: Vec<(f32, HistoryEntry)> = entries
            .iter()
            .map(|(entry, emb)| (embedding::cosine_distance(&query_embedding, emb), entry.clone()))
            .filter(|(distance, _)| *distance < 0.9)
            .collect();
        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("distances are never NaN"));
        scored.truncate(limit);
        Ok(scored.into_iter().map(|(_, entry)| entry).collect())
    }
}

/// Shared by `search`/`search_similar`: both select the same six columns in
/// the same order (an extra trailing `distance` column from
/// `search_similar`'s query is simply never read here).
fn collect_entries(rows: &mut libsql::Rows) -> anyhow::Result<Vec<HistoryEntry>> {
    let mut entries = Vec::new();
    while let Some(row) = futures_executor::block_on(rows.next())? {
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

/// Best-effort migration for a database created before the `embedding`
/// column existed — `CREATE TABLE IF NOT EXISTS` silently no-ops against an
/// existing table with an older shape, so a fresh `open`/`open_encrypted`
/// call on a profile used before this feature landed would otherwise be
/// missing the column entirely. Ignores the error `ALTER TABLE` raises when
/// the column already exists (the normal case, once every profile has been
/// opened at least once after this migration was added) — there's no
/// portable way to check for a column's existence up front that's simpler
/// than just trying to add it.
fn migrate_add_embedding_column(conn: &libsql::Connection) {
    let _ = futures_executor::block_on(conn.execute("ALTER TABLE history ADD COLUMN embedding BLOB", ()));
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
    fn search_similar_finds_lexically_related_titles_not_matched_by_substring_search() {
        let store = temp_store("similar");
        store.record_visit("https://rust-lang.org/tutorial", "Rust Programming Language Tutorial").unwrap();
        store.record_visit("https://example.com/bread", "Baking Sourdough Bread At Home").unwrap();

        // A query sharing most of its vocabulary with the Rust entry (but no
        // literal substring in common — "guide" appears nowhere in either
        // stored title/URL) shouldn't match via `search`, but should via
        // `search_similar`.
        assert_eq!(store.search("rust programming language guide", 10).unwrap(), vec![]);

        let similar = store.search_similar("Rust Programming Language Guide", 10).unwrap();
        assert_eq!(similar.len(), 1, "only the vocabulary-sharing entry should be similar enough to return");
        assert_eq!(similar[0].url, "https://rust-lang.org/tutorial");
    }

    #[test]
    fn search_similar_returns_nothing_for_an_unrelated_query() {
        let store = temp_store("similar-empty");
        store.record_visit("https://example.com/bread", "Baking Sourdough Bread At Home").unwrap();

        let results = store.search_similar("Quantum Computing Research Papers", 10).unwrap();
        assert_eq!(results, vec![], "a query sharing no vocabulary with anything in history should return nothing, not the least-bad match");
    }

    #[test]
    fn search_similar_on_an_empty_store_returns_no_results_without_erroring() {
        let store = temp_store("similar-on-empty");
        assert_eq!(store.search_similar("anything", 10).unwrap(), vec![]);
    }

    #[test]
    fn opening_a_database_created_before_the_embedding_column_existed_still_works() {
        // Simulates a real profile's history.db from before this feature
        // landed: the pre-migration schema, with a row already in it, and no
        // `embedding` column at all. `HistoryStore::open_at`'s
        // `migrate_add_embedding_column` step needs to add the column
        // without disturbing the existing row, and old rows (embedding
        // still NULL, since nothing backfills it retroactively) should be
        // silently excluded from `search_similar` rather than erroring.
        let path = std::env::temp_dir()
            .join(format!("claude-browser-test-history-pre-migration-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let db = futures_executor::block_on(libsql::Builder::new_local(&path).build()).unwrap();
            let conn = db.connect().unwrap();
            futures_executor::block_on(conn.execute_batch(
                "CREATE TABLE history (
                    url TEXT PRIMARY KEY,
                    domain TEXT NOT NULL,
                    title TEXT NOT NULL DEFAULT '',
                    first_visited_at INTEGER NOT NULL,
                    visited_at INTEGER NOT NULL,
                    visit_count INTEGER NOT NULL DEFAULT 1
                );",
            ))
            .unwrap();
            futures_executor::block_on(conn.execute(
                "INSERT INTO history (url, domain, title, first_visited_at, visited_at, visit_count) \
                 VALUES ('https://example.com/old', 'example.com', 'Old Entry', 100, 100, 1)",
                (),
            ))
            .unwrap();
        }

        let store = HistoryStore::open_at(&path).expect("opening a pre-migration database should succeed, not error");
        let results = store.search("old entry", 10).expect("substring search over a pre-existing row should still work");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com/old");

        // The old row's embedding is NULL (nothing backfills it
        // retroactively), so it should never surface via search_similar.
        assert_eq!(store.search_similar("old entry", 10).unwrap(), vec![]);

        // A freshly recorded visit after the migration should get a real
        // embedding and be findable via search_similar as usual.
        store.record_visit("https://example.com/new", "Rust Programming Language Tutorial").unwrap();
        let similar = store.search_similar("Rust Programming Language Guide", 10).unwrap();
        assert_eq!(similar.len(), 1);
        assert_eq!(similar[0].url, "https://example.com/new");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn in_memory_store_records_and_searches_without_touching_disk() {
        let store = HistoryStore::open_in_memory().expect("in-memory open should succeed");
        store.record_visit("https://example.com/a", "A").unwrap();
        let results = store.search("example.com", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com/a");
    }

    mod memory_history_store {
        use super::*;

        #[test]
        fn records_and_searches_by_url_and_title() {
            let store = MemoryHistoryStore::new();
            store.record_visit("https://example.com/rust", "Rust Programming Language").unwrap();

            let by_url = store.search("example.com", 10).unwrap();
            assert_eq!(by_url.len(), 1);
            assert_eq!(by_url[0].url, "https://example.com/rust");
            assert_eq!(by_url[0].domain, "example.com");

            let by_title = store.search("rust programming", 10).unwrap();
            assert_eq!(by_title.len(), 1, "search should be case-insensitive");
        }

        #[test]
        fn revisiting_a_url_updates_it_instead_of_duplicating() {
            let store = MemoryHistoryStore::new();
            store.record_visit("https://example.com/a", "First Title").unwrap();
            let first = store.search("example.com/a", 10).unwrap();
            assert_eq!(first[0].visit_count, 1);

            store.record_visit("https://example.com/a", "Updated Title").unwrap();
            let second = store.search("example.com/a", 10).unwrap();
            assert_eq!(second.len(), 1, "revisiting shouldn't create a second entry");
            assert_eq!(second[0].title, "Updated Title");
            assert_eq!(second[0].visit_count, 2);
        }

        #[test]
        fn search_orders_most_recently_visited_first() {
            let store = MemoryHistoryStore::new();
            store.record_visit("https://a.example/one", "One").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1100));
            store.record_visit("https://a.example/two", "Two").unwrap();

            let results = store.search("a.example", 10).unwrap();
            assert_eq!(results.len(), 2);
            assert_eq!(results[0].url, "https://a.example/two");
            assert_eq!(results[1].url, "https://a.example/one");
        }

        #[test]
        fn empty_store_returns_no_results_without_erroring() {
            let store = MemoryHistoryStore::new();
            assert_eq!(store.search("anything", 10).unwrap(), vec![]);
            assert_eq!(store.search_similar("anything", 10).unwrap(), vec![]);
        }

        #[test]
        fn search_similar_finds_lexically_related_titles_not_matched_by_substring_search() {
            let store = MemoryHistoryStore::new();
            store.record_visit("https://rust-lang.org/tutorial", "Rust Programming Language Tutorial").unwrap();
            store.record_visit("https://example.com/bread", "Baking Sourdough Bread At Home").unwrap();

            assert_eq!(store.search("rust programming language guide", 10).unwrap(), vec![]);

            let similar = store.search_similar("Rust Programming Language Guide", 10).unwrap();
            assert_eq!(similar.len(), 1, "only the vocabulary-sharing entry should be similar enough to return");
            assert_eq!(similar[0].url, "https://rust-lang.org/tutorial");
        }

        #[test]
        fn search_similar_returns_nothing_for_an_unrelated_query() {
            let store = MemoryHistoryStore::new();
            store.record_visit("https://example.com/bread", "Baking Sourdough Bread At Home").unwrap();

            let results = store.search_similar("Quantum Computing Research Papers", 10).unwrap();
            assert_eq!(results, vec![], "a query sharing no vocabulary with anything should return nothing");
        }

        /// Confirms `MemoryHistoryStore` is usable through the shared
        /// `HistoryBackend` trait, not just its own inherent methods — the
        /// actual point of having the trait.
        #[test]
        fn is_usable_as_a_trait_object() {
            let store: Box<dyn HistoryBackend> = Box::new(MemoryHistoryStore::new());
            store.record_visit("https://example.com", "Example Domain").unwrap();
            let results = store.search("example", 10).unwrap();
            assert_eq!(results.len(), 1);
        }
    }

    fn temp_profile(name: &str) -> Profile {
        Profile::new(format!("history-crypto-test-{name}-{}", std::process::id()))
    }

    fn cleanup_profile(profile: &Profile) {
        if let Some(parent) = profile.history_db_path().as_deref().and_then(Path::parent) {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    // These three exercise the *real* `open_encrypted` implementation, which
    // only exists on Linux and macOS (see its `#[cfg]` and doc comment) —
    // gated so they don't run (and fail on the stub's `Err`) on a genuine
    // non-Linux/non-macOS CI runner, e.g. GitHub Actions' `windows-latest`,
    // which actually *executes* `browser-core`'s test suite natively rather
    // than only cross-compiling it. On a real `macos-*` runner
    // (`.github/workflows/macos.yml`) these now actually run too, not just
    // cross-compile-link-check.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn encrypted_store_round_trips_with_the_right_passphrase() {
        let profile = temp_profile("round-trip");
        cleanup_profile(&profile);

        {
            let store = HistoryStore::open_encrypted(&profile, "correct horse battery staple")
                .expect("opening a fresh encrypted store should succeed");
            store.record_visit("https://example.com/a", "A").unwrap();
        }

        let reopened = HistoryStore::open_encrypted(&profile, "correct horse battery staple")
            .expect("reopening with the same passphrase should succeed");
        let results = reopened.search("example.com", 10).unwrap();
        assert_eq!(results.len(), 1, "the recorded visit should still be there after reopening");
        assert_eq!(results[0].url, "https://example.com/a");

        cleanup_profile(&profile);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn encrypted_store_rejects_the_wrong_passphrase() {
        let profile = temp_profile("wrong-passphrase");
        cleanup_profile(&profile);

        HistoryStore::open_encrypted(&profile, "the right one").expect("opening a fresh encrypted store should succeed");

        let result = HistoryStore::open_encrypted(&profile, "definitely not it");
        assert!(result.is_err(), "opening an encrypted store with the wrong passphrase should fail, not silently succeed");

        cleanup_profile(&profile);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_plain_unencrypted_open_cannot_read_an_encrypted_store() {
        let profile = temp_profile("plain-vs-encrypted");
        cleanup_profile(&profile);

        HistoryStore::open_encrypted(&profile, "some passphrase").expect("opening a fresh encrypted store should succeed");

        let result = HistoryStore::open(&profile);
        assert!(result.is_err(), "opening an encrypted store without any passphrase should fail, not silently succeed");

        cleanup_profile(&profile);
    }

    /// The non-Linux/non-macOS counterpart to the three tests above:
    /// confirms the stub `open_encrypted` (see its
    /// `#[cfg(not(any(target_os = "linux", target_os = "macos")))]` impl)
    /// does what it's documented to do — return an error — rather than
    /// silently falling back to an unencrypted open, which would be a silent
    /// security downgrade for anyone who thinks they got encryption.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn open_encrypted_returns_an_error_rather_than_silently_opening_unencrypted_on_this_platform() {
        let profile = temp_profile("stub-platform");
        cleanup_profile(&profile);

        let result = HistoryStore::open_encrypted(&profile, "some passphrase");
        assert!(result.is_err(), "open_encrypted should report an error on this platform, not silently succeed");

        cleanup_profile(&profile);
    }
}
