//! The currently-open pages, persisted per-profile — small and JSON-backed,
//! same shape/reasoning as `Bookmarks`/`Settings`/`Keybindings` (not a
//! SQLite database like `HistoryStore`/`PasswordStore`: a handful of open
//! tabs needs no querying, just "load the whole thing, save the whole
//! thing"). Saved on quit, loaded at startup — see
//! `browser_chrome_core::resolve_restore_plan` for the toolkit-agnostic
//! "what to actually open" decision each frontend shares, built on top of
//! this.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Profile;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionPage {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub pages: Vec<SessionPage>,
    /// Index into `pages` of the page that was active when the session was
    /// saved — kept separate from `pages`' own order (rather than always
    /// saving/restoring the active page last) so restoring doesn't shuffle
    /// tile/tab order relative to how the user left it.
    pub active_index: Option<usize>,
}

impl Session {
    /// Loads the session from `profile`'s config directory, falling back to
    /// an empty session if there's no file yet (first run) or it fails to
    /// read/parse (e.g. from an incompatible older version). An `ephemeral`
    /// profile (private/incognito/guest) always starts empty, never reading
    /// anything from disk.
    pub fn load(profile: &Profile) -> Self {
        if profile.ephemeral {
            return Self::default();
        }
        profile.session_path().and_then(|path| Self::load_from(&path)).unwrap_or_default()
    }

    /// Saves the session to `profile`'s config directory. Fails (rather than
    /// panicking) on I/O errors — callers should log and continue, not treat
    /// this as fatal. A no-op for an `ephemeral` profile: nothing about it is
    /// ever written to disk.
    pub fn save(&self, profile: &Profile) -> anyhow::Result<()> {
        if profile.ephemeral {
            return Ok(());
        }
        let path = profile
            .session_path()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("claude-browser-test-session-{name}-{}.json", std::process::id()))
    }

    #[test]
    fn round_trips_through_disk() {
        let path = temp_path("round-trip");
        let session = Session {
            pages: vec![
                SessionPage { url: "https://a.example".to_string(), title: "A".to_string() },
                SessionPage { url: "https://b.example".to_string(), title: "B".to_string() },
            ],
            active_index: Some(1),
        };

        session.save_to(&path).expect("save should succeed");
        let loaded = Session::load_from(&path).expect("load should find and parse the file");
        assert_eq!(loaded, session);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_from_missing_file_returns_none() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path);
        assert!(Session::load_from(&path).is_none());
    }

    #[test]
    fn ephemeral_profile_never_touches_disk() {
        let profile = crate::Profile::ephemeral();
        let session = Session::load(&profile);
        assert_eq!(session, Session::default(), "an ephemeral profile should always start with an empty session");

        let session = Session {
            pages: vec![SessionPage { url: "https://example.com".to_string(), title: "Example".to_string() }],
            active_index: Some(0),
        };
        session.save(&profile).expect("save should report success even though it's a no-op");
        assert!(
            profile.session_path().map(|p| !p.exists()).unwrap_or(true),
            "an ephemeral profile's session should never actually be written to disk"
        );
    }
}
