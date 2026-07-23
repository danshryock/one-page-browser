//! Configurable, per-profile keyboard shortcuts — previously hardcoded in
//! each frontend's keydown dispatch. Escape-closes-whichever-overlay and
//! Enter's context-dependent navigate/switch/search behavior are
//! deliberately *not* part of this: both are fixed UI conventions rather
//! than standalone rebindable commands, so each frontend still hardcodes
//! those checks alongside dispatching through `Keybindings`.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Profile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    OpenSwitcher,
    EditUrl,
    ClosePage,
    Reload,
    GoBack,
    GoForward,
    OpenSettings,
    OpenProfilePicker,
    ToggleBookmark,
    OpenBookmarks,
}

impl Action {
    pub const ALL: &'static [Action] = &[
        Action::OpenSwitcher,
        Action::EditUrl,
        Action::ClosePage,
        Action::Reload,
        Action::GoBack,
        Action::GoForward,
        Action::OpenSettings,
        Action::OpenProfilePicker,
        Action::ToggleBookmark,
        Action::OpenBookmarks,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Action::OpenSwitcher => "Open switcher (new page)",
            Action::EditUrl => "Edit current URL",
            Action::ClosePage => "Close page",
            Action::Reload => "Reload",
            Action::GoBack => "Go back",
            Action::GoForward => "Go forward",
            Action::OpenSettings => "Open settings",
            Action::OpenProfilePicker => "Open profile picker",
            Action::ToggleBookmark => "Bookmark this page",
            Action::OpenBookmarks => "Open bookmarks",
        }
    }
}

/// A single key combination. `key` is a normalized token: an uppercase
/// letter/digit (`"T"`, `"1"`) or a named key (`"F1"`, `"Escape"`,
/// `"Left"`, `"Right"`) — each frontend normalizes its own raw keyval/
/// virtual-key into this shape before comparing/storing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyChord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: String,
}

impl KeyChord {
    pub fn new(ctrl: bool, alt: bool, shift: bool, key: impl Into<String>) -> Self {
        Self { ctrl, alt, shift, key: key.into() }
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            write!(f, "Ctrl+")?;
        }
        if self.alt {
            write!(f, "Alt+")?;
        }
        if self.shift {
            write!(f, "Shift+")?;
        }
        write!(f, "{}", self.key)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keybindings(HashMap<Action, Vec<KeyChord>>);

impl Keybindings {
    /// Loads keybindings from `profile`'s config directory, falling back to
    /// `Keybindings::default()` if there's no file yet (first run) or it
    /// fails to read/parse (e.g. from an incompatible older version).
    pub fn load(profile: &Profile) -> Self {
        profile.keybindings_path().and_then(|path| Self::load_from(&path)).unwrap_or_default()
    }

    /// Saves keybindings to `profile`'s config directory. Fails (rather than
    /// panicking) on I/O errors — callers should log and continue, not treat
    /// this as fatal.
    pub fn save(&self, profile: &Profile) -> anyhow::Result<()> {
        let path = profile
            .keybindings_path()
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

    /// The action `chord` should trigger, if any — used by each frontend's
    /// keydown dispatch.
    pub fn action_for(&self, chord: &KeyChord) -> Option<Action> {
        self.0.iter().find(|(_, chords)| chords.contains(chord)).map(|(action, _)| *action)
    }

    /// `action`'s current chords, for the editor UI to render as removable
    /// tags.
    pub fn bindings_for(&self, action: Action) -> &[KeyChord] {
        self.0.get(&action).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Replaces `action`'s chords wholesale — the editor UI's add/remove
    /// both do a full read-modify-write of the current list rather than
    /// incremental single-chord edits.
    pub fn set_bindings(&mut self, action: Action, chords: Vec<KeyChord>) {
        self.0.insert(action, chords);
    }
}

impl Default for Keybindings {
    fn default() -> Self {
        let mut map = HashMap::new();
        map.insert(
            Action::OpenSwitcher,
            vec![KeyChord::new(false, false, false, "F1"), KeyChord::new(true, false, false, "T")],
        );
        map.insert(Action::EditUrl, vec![KeyChord::new(true, false, false, "L")]);
        map.insert(Action::ClosePage, vec![KeyChord::new(true, false, false, "W")]);
        map.insert(Action::Reload, vec![KeyChord::new(false, false, false, "F5")]);
        map.insert(Action::GoBack, vec![KeyChord::new(false, true, false, "Left")]);
        map.insert(Action::GoForward, vec![KeyChord::new(false, true, false, "Right")]);
        map.insert(Action::ToggleBookmark, vec![KeyChord::new(true, false, false, "D")]);
        // OpenSettings/OpenProfilePicker/OpenBookmarks: unbound by default —
        // toolbar-button-only, used far less often than the other six.
        Self(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("claude-browser-test-keybindings-{name}-{}.json", std::process::id()))
    }

    #[test]
    fn default_preserves_todays_hardcoded_bindings() {
        let defaults = Keybindings::default();
        assert_eq!(
            defaults.action_for(&KeyChord::new(false, false, false, "F1")),
            Some(Action::OpenSwitcher)
        );
        assert_eq!(defaults.action_for(&KeyChord::new(true, false, false, "T")), Some(Action::OpenSwitcher));
        assert_eq!(
            defaults.action_for(&KeyChord::new(true, false, false, "L")),
            Some(Action::EditUrl),
            "Ctrl+L is the traditional \"edit the URL\" binding, distinct from Ctrl+T/F1's \"open switcher for a new page\""
        );
        assert_eq!(defaults.action_for(&KeyChord::new(true, false, false, "W")), Some(Action::ClosePage));
    }

    #[test]
    fn action_for_returns_none_for_an_unbound_chord() {
        let defaults = Keybindings::default();
        assert_eq!(defaults.action_for(&KeyChord::new(true, true, true, "Q")), None);
    }

    #[test]
    fn set_bindings_replaces_the_whole_list() {
        let mut kb = Keybindings::default();
        kb.set_bindings(Action::ClosePage, vec![KeyChord::new(false, true, false, "F4")]);
        assert_eq!(kb.bindings_for(Action::ClosePage), &[KeyChord::new(false, true, false, "F4")]);
        assert_eq!(kb.action_for(&KeyChord::new(true, false, false, "W")), None, "the old binding should be gone");
        assert_eq!(kb.action_for(&KeyChord::new(false, true, false, "F4")), Some(Action::ClosePage));
    }

    #[test]
    fn round_trips_through_disk() {
        // Also exercises serde_json's handling of a HashMap keyed by a
        // fieldless enum (serialized as its variant name) — this is the
        // first place this codebase does that, worth locking in with a real
        // test rather than assuming it works.
        let path = temp_path("round-trip");
        let mut kb = Keybindings::default();
        kb.set_bindings(Action::OpenSettings, vec![KeyChord::new(true, false, true, "S")]);

        kb.save_to(&path).expect("save should succeed");
        let loaded = Keybindings::load_from(&path).expect("load should find and parse the file");
        assert_eq!(loaded, kb);
        assert_eq!(loaded.action_for(&KeyChord::new(true, false, true, "S")), Some(Action::OpenSettings));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_from_missing_file_returns_none() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path);
        assert!(Keybindings::load_from(&path).is_none());
    }

    #[test]
    fn key_chord_display_renders_modifiers_in_order() {
        assert_eq!(KeyChord::new(true, true, true, "T").to_string(), "Ctrl+Alt+Shift+T");
        assert_eq!(KeyChord::new(false, false, false, "F1").to_string(), "F1");
    }
}
