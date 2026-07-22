//! A named identity that scopes per-user resources — `Settings` today, and
//! a reserved (not yet opened) path for a per-profile history database.
//! Every profile, including the implicit `"default"` one, gets its own
//! subdirectory rather than special-casing a legacy top-level path — there's
//! no shipped install with existing unscoped data to stay compatible with.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
}

impl Profile {
    /// Sanitizes defensively: this is a local CLI flag, not attacker-
    /// controlled input, but a typo like `--profile ../../etc` shouldn't be
    /// able to write outside the intended tree. Empty, `.`, `..`, or a name
    /// containing a path separator falls back to `"default"`.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let is_safe = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']);
        Self { name: if is_safe { name } else { "default".to_string() } }
    }

    pub fn settings_path(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "claude-browser")?;
        Some(dirs.config_dir().join(&self.name).join("settings.json"))
    }

    /// Reserved for the per-profile history database — resolved here so the
    /// path is settled and testable, but nothing opens a connection at this
    /// path yet (a separate step: `libsql`'s core API is async, and this
    /// project is entirely synchronous today).
    pub fn history_db_path(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "claude-browser")?;
        Some(dirs.data_dir().join(&self.name).join("history.db"))
    }
}

impl Default for Profile {
    fn default() -> Self {
        Profile::new("default")
    }
}

/// Resolves the profile name from CLI-style arguments: looks for
/// `--profile NAME` or `--profile=NAME`, defaulting to `"default"` if
/// absent or malformed (e.g. `--profile` with nothing after it). Takes an
/// iterator rather than reading `std::env::args()` itself so this is
/// unit-testable directly.
pub fn resolve_profile_name<I: IntoIterator<Item = String>>(args: I) -> String {
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--profile=") {
            if !value.is_empty() {
                return value.to_string();
            }
        } else if arg == "--profile" {
            if let Some(value) = args.next() {
                if !value.is_empty() {
                    return value;
                }
            }
        }
    }
    "default".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolve_profile_name_defaults_when_flag_absent() {
        assert_eq!(resolve_profile_name(args(&["program"])), "default");
        assert_eq!(resolve_profile_name(args(&[])), "default");
    }

    #[test]
    fn resolve_profile_name_parses_space_separated_form() {
        assert_eq!(resolve_profile_name(args(&["program", "--profile", "work"])), "work");
    }

    #[test]
    fn resolve_profile_name_parses_equals_form() {
        assert_eq!(resolve_profile_name(args(&["program", "--profile=work"])), "work");
    }

    #[test]
    fn resolve_profile_name_defaults_when_flag_has_no_value() {
        assert_eq!(resolve_profile_name(args(&["program", "--profile"])), "default");
        assert_eq!(resolve_profile_name(args(&["program", "--profile="])), "default");
    }

    #[test]
    fn resolve_profile_name_ignores_unrelated_flags() {
        assert_eq!(resolve_profile_name(args(&["program", "--verbose", "--profile", "work"])), "work");
    }

    #[test]
    fn new_accepts_a_plain_name() {
        assert_eq!(Profile::new("work").name, "work");
    }

    #[test]
    fn new_rejects_empty_and_dot_names() {
        assert_eq!(Profile::new("").name, "default");
        assert_eq!(Profile::new(".").name, "default");
        assert_eq!(Profile::new("..").name, "default");
    }

    #[test]
    fn new_rejects_path_separators() {
        assert_eq!(Profile::new("../../etc").name, "default");
        assert_eq!(Profile::new("a/b").name, "default");
        assert_eq!(Profile::new("a\\b").name, "default");
    }

    #[test]
    fn settings_path_and_history_db_path_are_scoped_under_the_profile_name() {
        let profile = Profile::new("work");
        let settings_path = profile.settings_path().expect("a config dir should be available in tests");
        let history_path = profile.history_db_path().expect("a data dir should be available in tests");

        assert!(settings_path.ends_with("work/settings.json"), "{settings_path:?}");
        assert!(history_path.ends_with("work/history.db"), "{history_path:?}");
    }

    #[test]
    fn default_profile_is_named_default() {
        assert_eq!(Profile::default().name, "default");
    }
}
