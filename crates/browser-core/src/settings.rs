use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Profile, HOME_URL};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchEngine {
    pub name: String,
    /// Query URL with `{query}` where the URL-encoded search text goes.
    pub query_url_template: String,
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
}

impl Settings {
    pub fn default_search_engine(&self) -> Option<&SearchEngine> {
        self.search_engines.iter().find(|e| e.name == self.default_search_engine)
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

        settings.save_to(&path).expect("save should succeed");
        let loaded = Settings::load_from(&path).expect("load should find and parse the file");
        assert_eq!(loaded, settings);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_from_missing_file_returns_none() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path); // in case a previous run left it
        assert!(Settings::load_from(&path).is_none());
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
}
