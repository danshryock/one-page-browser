//! A named identity that scopes per-user resources — `Settings` today, and
//! a reserved (not yet opened) path for a per-profile history database.
//! Every profile, including the implicit `"default"` one, gets its own
//! subdirectory rather than special-casing a legacy top-level path — there's
//! no shipped install with existing unscoped data to stay compatible with.

use std::path::{Path, PathBuf};

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

    pub fn keybindings_path(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "claude-browser")?;
        Some(dirs.config_dir().join(&self.name).join("keybindings.json"))
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

/// Pulls the first positional (non-flag) argument out — used for launching
/// with a URL (e.g. from the OS's "open with"/default-browser handoff):
/// `browser-linux-gtk3 https://example.com`. Skips `argv[0]` and
/// `--profile`'s consumed value so a launch like `program --profile work
/// https://example.com` still finds the URL, not `"work"`. Any other `--foo`
/// flag is skipped generically (not enumerated by name) so this doesn't need
/// updating every time a new flag is added elsewhere. Returns the raw token
/// unchanged — unlike `resolve_address_input`, this isn't run through
/// bare-domain/search-query resolution: a real external-link launch always
/// hands over a fully-qualified URL, and resolving one would need a
/// profile's `Settings` before a profile has even been picked.
pub fn resolve_url_argument<I: IntoIterator<Item = String>>(args: I) -> Option<String> {
    let mut args = args.into_iter().skip(1); // skip argv[0]
    while let Some(arg) = args.next() {
        if arg == "--profile" {
            args.next(); // consume its value too, not the URL
            continue;
        }
        if arg.starts_with("--") {
            continue;
        }
        return Some(arg);
    }
    None
}

/// Names of every profile that has ever been used (existing subdirectories
/// under the config directory each profile's `settings_path` is scoped
/// under), plus `"default"` even on a fresh install where it doesn't exist
/// on disk yet. Used to populate the external-link chooser's profile picker.
pub fn list_profile_names() -> Vec<String> {
    match directories::ProjectDirs::from("", "", "claude-browser") {
        Some(dirs) => list_profile_names_in(dirs.config_dir()),
        None => vec!["default".to_string()],
    }
}

/// Split out from `list_profile_names` so tests can scan a throwaway
/// directory instead of the real user config directory — same reasoning as
/// `Settings::load`/`load_from` and `HistoryStore::open`/`open_at`.
fn list_profile_names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    if !names.iter().any(|n| n == "default") {
        names.push("default".to_string());
    }
    names.sort();
    names
}

/// Launches a new, independent instance of this same binary scoped to
/// `profile_name` — used by the in-app profile picker: switching profiles
/// means a new process, not swapping state in the running one, the same
/// reasoning as the external-link chooser (`show_external_link_chooser`)
/// already uses. Spawn-and-forget: the child runs independently of this
/// process either way, so nothing here waits on or tracks it further.
pub fn launch_new_profile_process(profile_name: &str) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe).arg("--profile").arg(profile_name).spawn()?;
    Ok(())
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
    fn keybindings_path_is_scoped_under_the_profile_name() {
        let profile = Profile::new("work");
        let path = profile.keybindings_path().expect("a config dir should be available in tests");
        assert!(path.ends_with("work/keybindings.json"), "{path:?}");
    }

    #[test]
    fn default_profile_is_named_default() {
        assert_eq!(Profile::default().name, "default");
    }

    #[test]
    fn resolve_url_argument_finds_a_bare_positional_argument() {
        assert_eq!(resolve_url_argument(args(&["program", "https://example.com"])), Some("https://example.com".to_string()));
    }

    #[test]
    fn resolve_url_argument_returns_none_when_absent() {
        assert_eq!(resolve_url_argument(args(&["program"])), None);
        assert_eq!(resolve_url_argument(args(&["program", "--profile", "work"])), None);
        assert_eq!(resolve_url_argument(args(&[])), None);
    }

    #[test]
    fn resolve_url_argument_skips_the_profile_flags_value() {
        assert_eq!(
            resolve_url_argument(args(&["program", "--profile", "work", "https://example.com"])),
            Some("https://example.com".to_string()),
            "the URL, not the --profile flag's value, should be returned"
        );
        assert_eq!(
            resolve_url_argument(args(&["program", "--profile=work", "https://example.com"])),
            Some("https://example.com".to_string())
        );
    }

    #[test]
    fn resolve_url_argument_skips_other_flags_generically() {
        assert_eq!(
            resolve_url_argument(args(&["program", "--verbose", "https://example.com"])),
            Some("https://example.com".to_string())
        );
    }

    fn make_dir(root: &std::path::Path, name: &str) {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }

    #[test]
    fn list_profile_names_in_always_includes_default() {
        let root = std::env::temp_dir().join(format!("claude-browser-test-profiles-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        make_dir(&root, ""); // just the root itself, no profile subdirs yet
        assert_eq!(list_profile_names_in(&root), vec!["default".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_profile_names_in_finds_existing_profile_directories() {
        let root = std::env::temp_dir().join(format!("claude-browser-test-profiles-existing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        make_dir(&root, "work");
        make_dir(&root, "personal");
        assert_eq!(list_profile_names_in(&root), vec!["default".to_string(), "personal".to_string(), "work".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_profile_names_in_does_not_duplicate_an_existing_default_directory() {
        let root = std::env::temp_dir().join(format!("claude-browser-test-profiles-default-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        make_dir(&root, "default");
        assert_eq!(list_profile_names_in(&root), vec!["default".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
