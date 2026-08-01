use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Profile, HOME_URL};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchEngine {
    pub name: String,
    /// Query URL with `{query}` where the URL-encoded search text goes.
    pub query_url_template: String,
}

/// The native chrome's color theme — each frontend is responsible for
/// actually applying it (e.g. `browser-linux-gtk3` swaps which of two CSS
/// blocks is loaded), `Settings` just stores the choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub start_page: String,
    pub search_engines: Vec<SearchEngine>,
    /// Name of the entry in `search_engines` that's preferred — a name
    /// rather than an index so it doesn't go stale if the list is ever
    /// edited (e.g. by a future settings UI).
    pub default_search_engine: String,
    /// How many pages may stay loaded at once. `None` means unlimited.
    pub max_loaded_pages: Option<usize>,
    pub theme: Theme,
    /// Base URL of a locally running `bw` CLI `bw serve` instance (e.g.
    /// `"http://127.0.0.1:8087"`) — `None` means Bitwarden integration is
    /// disabled. See `browser_core::BitwardenBackend`. `#[serde(default)]`
    /// so a `settings.json` written before this field existed still parses.
    #[serde(default)]
    pub bitwarden_server_url: Option<String>,
}

impl Settings {
    pub fn default_search_engine(&self) -> Option<&SearchEngine> {
        self.search_engines.iter().find(|e| e.name == self.default_search_engine)
    }

    /// Adds a search engine, or updates its query URL template in place if
    /// an engine with this name already exists — never creates a duplicate
    /// entry for the same name, matching `Bookmarks::add`'s convention.
    pub fn add_search_engine(&mut self, name: &str, query_url_template: &str) {
        if let Some(existing) = self.search_engines.iter_mut().find(|e| e.name == name) {
            existing.query_url_template = query_url_template.to_string();
        } else {
            self.search_engines.push(SearchEngine { name: name.to_string(), query_url_template: query_url_template.to_string() });
        }
    }

    /// Removes a search engine by name. Refuses to remove the last
    /// remaining engine — there must always be at least one to fall back to
    /// — and reassigns `default_search_engine` to the first remaining entry
    /// if the one removed was the default. Returns whether anything was
    /// actually removed.
    pub fn remove_search_engine(&mut self, name: &str) -> bool {
        if self.search_engines.len() <= 1 {
            return false;
        }
        let before = self.search_engines.len();
        self.search_engines.retain(|e| e.name != name);
        let removed = self.search_engines.len() != before;
        if removed && self.default_search_engine == name {
            if let Some(first) = self.search_engines.first() {
                self.default_search_engine = first.name.clone();
            }
        }
        removed
    }

    /// Loads settings from `profile`'s config directory, falling back to
    /// `Settings::default()` if there's no file yet (first run) or it fails
    /// to read/parse (e.g. from an incompatible older version). An
    /// `ephemeral` profile (private/incognito/guest) always gets a fresh
    /// `Settings::default()`, never reading anything from disk.
    pub fn load(profile: &Profile) -> Self {
        if profile.ephemeral {
            return Self::default();
        }
        profile.settings_path().and_then(|path| Self::load_from(&path)).unwrap_or_default()
    }

    /// Saves settings to `profile`'s config directory. Fails (rather than
    /// panicking) on I/O errors — callers should log and continue, not treat
    /// this as fatal. A no-op for an `ephemeral` profile: nothing about it is
    /// ever written to disk.
    pub fn save(&self, profile: &Profile) -> anyhow::Result<()> {
        if profile.ephemeral {
            return Ok(());
        }
        let path = profile
            .settings_path()
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            start_page: HOME_URL.to_string(),
            search_engines: vec![
                SearchEngine {
                    name: "Google".to_string(),
                    query_url_template: "https://www.google.com/search?q={query}".to_string(),
                },
                SearchEngine {
                    name: "DuckDuckGo".to_string(),
                    query_url_template: "https://duckduckgo.com/?q={query}".to_string(),
                },
                SearchEngine {
                    name: "Bing".to_string(),
                    query_url_template: "https://www.bing.com/search?q={query}".to_string(),
                },
            ],
            default_search_engine: "Google".to_string(),
            max_loaded_pages: Some(10),
            // Dark preserves this app's existing look (every native-chrome
            // frontend's overlay/tile CSS was written assuming a dark
            // background) for anyone upgrading from before themes existed.
            theme: Theme::Dark,
            bitwarden_server_url: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("claude-browser-test-{name}-{}.json", std::process::id()))
    }

    #[test]
    fn round_trips_through_disk() {
        let path = temp_path("round-trip");
        let mut settings = Settings::default();
        settings.start_page = "https://example.com".to_string();
        settings.max_loaded_pages = Some(3);
        settings.theme = Theme::Light;

        settings.save_to(&path).expect("save should succeed");
        let loaded = Settings::load_from(&path).expect("load should find and parse the file");
        assert_eq!(loaded, settings);
        assert_eq!(loaded.theme, Theme::Light);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_from_missing_file_returns_none() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path); // in case a previous run left it
        assert!(Settings::load_from(&path).is_none());
    }

    #[test]
    fn bitwarden_server_url_round_trips_and_defaults_to_none() {
        let path = temp_path("bitwarden-round-trip");
        let mut settings = Settings::default();
        assert_eq!(settings.bitwarden_server_url, None, "Bitwarden integration should be disabled by default");

        settings.bitwarden_server_url = Some("http://127.0.0.1:8087".to_string());
        settings.save_to(&path).expect("save should succeed");
        let loaded = Settings::load_from(&path).expect("load should find and parse the file");
        assert_eq!(loaded.bitwarden_server_url.as_deref(), Some("http://127.0.0.1:8087"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_settings_file_written_before_bitwarden_server_url_existed_still_parses() {
        let path = temp_path("pre-bitwarden-field");
        // No `bitwarden_server_url` key at all — simulates a settings.json
        // written by an older build, before this field was added.
        std::fs::write(
            &path,
            r#"{
                "start_page": "https://example.com",
                "search_engines": [],
                "default_search_engine": "",
                "max_loaded_pages": null,
                "theme": "Dark"
            }"#,
        )
        .unwrap();

        let loaded = Settings::load_from(&path).expect("a settings.json missing this field should still parse");
        assert_eq!(loaded.bitwarden_server_url, None, "a missing field should default to None, not fail to parse");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ephemeral_profile_never_touches_disk() {
        let profile = Profile::ephemeral();
        let mut settings = Settings::load(&profile);
        assert_eq!(settings, Settings::default(), "an ephemeral profile should always start from defaults");

        settings.start_page = "https://should-never-be-saved.example".to_string();
        settings.save(&profile).expect("save should report success even though it's a no-op");
        assert!(
            profile.settings_path().map(|p| !p.exists()).unwrap_or(true),
            "an ephemeral profile's settings should never actually be written to disk"
        );
    }

    #[test]
    fn add_search_engine_appends_a_new_one() {
        let mut settings = Settings::default();
        let before = settings.search_engines.len();
        settings.add_search_engine("Kagi", "https://kagi.com/search?q={query}");
        assert_eq!(settings.search_engines.len(), before + 1);
        assert_eq!(settings.search_engines.last().unwrap().name, "Kagi");
    }

    #[test]
    fn add_search_engine_with_an_existing_name_updates_instead_of_duplicating() {
        let mut settings = Settings::default();
        let before = settings.search_engines.len();
        settings.add_search_engine("Google", "https://google.example/?q={query}");
        assert_eq!(settings.search_engines.len(), before, "re-adding an existing name shouldn't duplicate it");
        assert_eq!(
            settings.search_engines.iter().find(|e| e.name == "Google").unwrap().query_url_template,
            "https://google.example/?q={query}"
        );
    }

    #[test]
    fn remove_search_engine_reassigns_the_default_if_it_was_removed() {
        let mut settings = Settings::default();
        assert_eq!(settings.default_search_engine, "Google");

        assert!(settings.remove_search_engine("Google"));
        assert!(settings.search_engines.iter().all(|e| e.name != "Google"));
        assert_ne!(settings.default_search_engine, "Google", "the default should have been reassigned");
        assert_eq!(settings.default_search_engine, settings.search_engines[0].name);
    }

    #[test]
    fn remove_search_engine_refuses_to_remove_the_last_one() {
        let mut settings = Settings::default();
        while settings.search_engines.len() > 1 {
            let name = settings.search_engines[0].name.clone();
            settings.remove_search_engine(&name);
        }
        assert_eq!(settings.search_engines.len(), 1);

        let last_name = settings.search_engines[0].name.clone();
        assert!(!settings.remove_search_engine(&last_name), "removing the last remaining engine should be refused");
        assert_eq!(settings.search_engines.len(), 1);
    }
}
