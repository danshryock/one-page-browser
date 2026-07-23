//! Per-profile bookmarks: a simple, small, ordered list — unlike
//! `HistoryStore` (a SQLite table, sized for a potentially large, ever-
//! growing visit log queried by recency/substring), bookmarks are expected
//! to stay small (tens to low hundreds of entries) and are edited rarely, so
//! a plain JSON file (same persistence shape as `Settings`/`Keybindings`)
//! is simpler and sufficient — no database needed.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{domain_of, Profile};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    pub domain: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bookmarks {
    entries: Vec<Bookmark>,
}

impl Bookmarks {
    /// Loads bookmarks from `profile`'s config directory, falling back to
    /// an empty list if there's no file yet (first run) or it fails to
    /// read/parse (e.g. from an incompatible older version). An `ephemeral`
    /// profile (private/incognito/guest) always starts empty, never reading
    /// anything from disk.
    pub fn load(profile: &Profile) -> Self {
        if profile.ephemeral {
            return Self::default();
        }
        profile.bookmarks_path().and_then(|path| Self::load_from(&path)).unwrap_or_default()
    }

    /// Saves bookmarks to `profile`'s config directory. Fails (rather than
    /// panicking) on I/O errors — callers should log and continue, not treat
    /// this as fatal. A no-op for an `ephemeral` profile: nothing about it is
    /// ever written to disk.
    pub fn save(&self, profile: &Profile) -> anyhow::Result<()> {
        if profile.ephemeral {
            return Ok(());
        }
        let path = profile
            .bookmarks_path()
            .ok_or_else(|| anyhow::anyhow!("no config directory available on this platform"))?;
        self.save_to(&path)
    }

    /// Split out from `load()` so tests can round-trip through a throwaway
    /// path instead of the real user config directory.
    fn load_from(path: &Path) -> Option<Self> {
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Split out from `save()` for the same reason as `load_from`.
    fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, data)?;
        Ok(())
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.entries.iter().any(|b| b.url == url)
    }

    /// Adds `url` (computing `domain` from it, same as `HistoryStore`), or
    /// updates its title in place if it's already bookmarked — never
    /// creates a duplicate entry for the same URL.
    pub fn add(&mut self, url: &str, title: &str, created_at: i64) {
        if let Some(existing) = self.entries.iter_mut().find(|b| b.url == url) {
            existing.title = title.to_string();
            return;
        }
        self.entries.push(Bookmark { url: url.to_string(), title: title.to_string(), domain: domain_of(url), created_at });
    }

    pub fn remove(&mut self, url: &str) {
        self.entries.retain(|b| b.url != url);
    }

    /// Adds `url` if it isn't already bookmarked, or removes it if it is.
    /// Returns the new state (`true` = now bookmarked) — the toolbar star
    /// button's toggle action.
    pub fn toggle(&mut self, url: &str, title: &str, created_at: i64) -> bool {
        if self.is_bookmarked(url) {
            self.remove(url);
            false
        } else {
            self.add(url, title, created_at);
            true
        }
    }

    /// All bookmarks, most-recently-added first — the bookmarks overlay's
    /// list, unfiltered.
    pub fn all(&self) -> Vec<&Bookmark> {
        let mut entries: Vec<&Bookmark> = self.entries.iter().collect();
        entries.sort_by_key(|b| std::cmp::Reverse(b.created_at));
        entries
    }

    /// Bookmarks whose `url` or `title` contains `query` (case-insensitive),
    /// most-recently-added first.
    pub fn search(&self, query: &str) -> Vec<&Bookmark> {
        let query = query.to_lowercase();
        let mut entries: Vec<&Bookmark> = self
            .entries
            .iter()
            .filter(|b| b.url.to_lowercase().contains(&query) || b.title.to_lowercase().contains(&query))
            .collect();
        entries.sort_by_key(|b| std::cmp::Reverse(b.created_at));
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("claude-browser-test-bookmarks-{name}-{}.json", std::process::id()))
    }

    #[test]
    fn adding_a_bookmark_makes_it_findable() {
        let mut bookmarks = Bookmarks::default();
        assert!(!bookmarks.is_bookmarked("https://example.com/rust"));

        bookmarks.add("https://example.com/rust", "Rust Programming Language", 100);
        assert!(bookmarks.is_bookmarked("https://example.com/rust"));
        let found = bookmarks.search("rust programming");
        assert_eq!(found.len(), 1, "search should be case-insensitive");
        assert_eq!(found[0].domain, "example.com");
    }

    #[test]
    fn adding_the_same_url_twice_updates_the_title_instead_of_duplicating() {
        let mut bookmarks = Bookmarks::default();
        bookmarks.add("https://example.com/a", "First Title", 100);
        bookmarks.add("https://example.com/a", "Updated Title", 200);
        assert_eq!(bookmarks.all().len(), 1, "adding the same URL again shouldn't create a second entry");
        assert_eq!(bookmarks.all()[0].title, "Updated Title");
    }

    #[test]
    fn remove_deletes_the_bookmark() {
        let mut bookmarks = Bookmarks::default();
        bookmarks.add("https://example.com/a", "A", 100);
        bookmarks.remove("https://example.com/a");
        assert!(!bookmarks.is_bookmarked("https://example.com/a"));
        assert!(bookmarks.all().is_empty());
    }

    #[test]
    fn toggle_adds_then_removes() {
        let mut bookmarks = Bookmarks::default();
        assert!(bookmarks.toggle("https://example.com/a", "A", 100), "toggling an unbookmarked URL should bookmark it");
        assert!(bookmarks.is_bookmarked("https://example.com/a"));
        assert!(!bookmarks.toggle("https://example.com/a", "A", 100), "toggling again should un-bookmark it");
        assert!(!bookmarks.is_bookmarked("https://example.com/a"));
    }

    #[test]
    fn all_and_search_order_most_recently_added_first() {
        let mut bookmarks = Bookmarks::default();
        bookmarks.add("https://a.example/one", "One", 100);
        bookmarks.add("https://a.example/two", "Two", 200);

        let all = bookmarks.all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].url, "https://a.example/two", "most recently added should come first");
        assert_eq!(all[1].url, "https://a.example/one");

        let found = bookmarks.search("a.example");
        assert_eq!(found[0].url, "https://a.example/two");
    }

    #[test]
    fn round_trips_through_disk() {
        let path = temp_path("round-trip");
        let mut bookmarks = Bookmarks::default();
        bookmarks.add("https://example.com/a", "A", 100);
        bookmarks.add("https://example.com/b", "B", 200);

        bookmarks.save_to(&path).expect("save should succeed");
        let loaded = Bookmarks::load_from(&path).expect("load should find and parse the file");
        assert_eq!(loaded, bookmarks);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_from_missing_file_returns_none() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path);
        assert!(Bookmarks::load_from(&path).is_none());
    }

    #[test]
    fn ephemeral_profile_never_touches_disk() {
        let profile = crate::Profile::ephemeral();
        let mut bookmarks = Bookmarks::load(&profile);
        assert_eq!(bookmarks, Bookmarks::default(), "an ephemeral profile should always start empty");

        bookmarks.add("https://example.com/a", "A", 100);
        bookmarks.save(&profile).expect("save should report success even though it's a no-op");
        assert!(
            profile.bookmarks_path().map(|p| !p.exists()).unwrap_or(true),
            "an ephemeral profile's bookmarks should never actually be written to disk"
        );
    }
}
