//! A named identity that scopes per-user resources — `Settings` today, and
//! a reserved (not yet opened) path for a per-profile history database.
//! Every profile, including the implicit `"default"` one, gets its own
//! subdirectory rather than special-casing a legacy top-level path — there's
//! no shipped install with existing unscoped data to stay compatible with.

use std::path::{Path, PathBuf};

use crate::app_info::effective_app_id;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    /// A private/incognito/guest session: nothing about it is ever written
    /// to disk (`Settings`/`Keybindings` fall back to their defaults and
    /// silently no-op on save, `Bookmarks` stays empty and never saves, and
    /// `HistoryStore` is opened in-memory via `HistoryStore::open_in_memory`)
    /// so the whole profile vanishes the moment the process exits — see
    /// `Profile::ephemeral`. `name` is still set (to `"Private Browsing"`)
    /// for display purposes even though no path is ever derived from it.
    pub ephemeral: bool,
}

impl Profile {
    /// Sanitizes defensively: this is a local CLI flag, not attacker-
    /// controlled input, but a typo like `--profile ../../etc` shouldn't be
    /// able to write outside the intended tree. Empty, `.`, `..`, or a name
    /// containing a path separator falls back to `"default"`.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let is_safe = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']);
        Self { name: if is_safe { name } else { "default".to_string() }, ephemeral: false }
    }

    /// A private/incognito/guest profile: see the `ephemeral` field doc for
    /// exactly what "private" means here. Every call creates a distinct,
    /// unlinked session — there's nothing to name or look up later, unlike
    /// `Profile::new`'s named, persistent profiles.
    pub fn ephemeral() -> Self {
        Self { name: "Private Browsing".to_string(), ephemeral: true }
    }

    pub fn settings_path(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", effective_app_id())?;
        Some(dirs.config_dir().join(&self.name).join("settings.json"))
    }

    pub fn keybindings_path(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", effective_app_id())?;
        Some(dirs.config_dir().join(&self.name).join("keybindings.json"))
    }

    pub fn bookmarks_path(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", effective_app_id())?;
        Some(dirs.config_dir().join(&self.name).join("bookmarks.json"))
    }

    /// The currently-open pages (URLs + which one was active), saved on
    /// quit and reopened on next launch — see `Session`'s doc comment.
    pub fn session_path(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", effective_app_id())?;
        Some(dirs.config_dir().join(&self.name).join("session.json"))
    }

    /// Reserved for the per-profile history database — resolved here so the
    /// path is settled and testable, but nothing opens a connection at this
    /// path yet (a separate step: `libsql`'s core API is async, and this
    /// project is entirely synchronous today).
    pub fn history_db_path(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", effective_app_id())?;
        Some(dirs.data_dir().join(&self.name).join("history.db"))
    }

    /// Reserved for the per-profile password vault database — same
    /// resolve-the-path-without-opening-anything split as `history_db_path`.
    pub fn passwords_db_path(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", effective_app_id())?;
        Some(dirs.data_dir().join(&self.name).join("passwords.db"))
    }

    /// Where this profile's page screenshots are saved by default — kept
    /// per-profile (rather than a shared system Pictures folder) for the
    /// same reason every other piece of profile data is: a browser profile
    /// keeps its own things to itself. Just a starting suggestion for the
    /// save dialog, not a forced location — the directory isn't created
    /// until a screenshot is actually about to be saved there.
    pub fn screenshots_dir(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", effective_app_id())?;
        Some(dirs.data_dir().join(&self.name).join("screenshots"))
    }

    /// Where this profile's web engine keeps its persistent browsing data —
    /// cookies, localStorage, cache — so it survives across restarts. Same
    /// resolve-the-path-without-opening-anything split as `history_db_path`;
    /// each front end's own engine construction is what actually points its
    /// web context/environment at this directory (or skips doing so for an
    /// `ephemeral` profile, same convention as every other path here).
    pub fn webview_data_dir(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", effective_app_id())?;
        Some(dirs.data_dir().join(&self.name).join("webview"))
    }

    /// Path to this profile's passphrase marker — an empty sentinel file
    /// (never holds the passphrase itself, or anything derived from it)
    /// whose mere *existence* means `history_db_path()` is encrypted and a
    /// passphrase must be collected before `HistoryStore::open_encrypted`
    /// can succeed. Needed because there's no way to tell an encrypted
    /// database from a corrupt/missing one just by trying to open it
    /// without a key first — SQLite (and the cipher extension `libsql`
    /// layers on top of it) only discovers that on the first real read.
    pub fn passphrase_marker_path(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", effective_app_id())?;
        Some(dirs.data_dir().join(&self.name).join("passphrase-enabled"))
    }

    /// Whether this profile's history database is passphrase-protected —
    /// always `false` for an `ephemeral` profile (it never touches disk at
    /// all, see `ephemeral`'s doc comment, so there's nothing to check).
    pub fn has_passphrase(&self) -> bool {
        !self.ephemeral && self.passphrase_marker_path().map(|p| p.exists()).unwrap_or(false)
    }

    /// Marks this profile as passphrase-protected going forward — called
    /// once, right after successfully establishing encryption on a brand
    /// new (empty) history database with a chosen passphrase (see
    /// `HistoryStore::open_encrypted`'s doc comment for why that's the
    /// point encryption gets "set up" — there's no separate step). Does
    /// nothing to the database itself; purely bookkeeping so future launches
    /// know to prompt for a passphrase before trying to open it.
    pub fn enable_passphrase(&self) -> anyhow::Result<()> {
        let path = self
            .passphrase_marker_path()
            .ok_or_else(|| anyhow::anyhow!("no data directory available on this platform"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, b"")?;
        Ok(())
    }

    /// Path to the password vault's own passphrase marker — deliberately a
    /// *separate* file from `passphrase_marker_path`, not a rename/reuse of
    /// it: the vault and the history database are independently encrypted
    /// (a profile can have one, the other, both, or neither), even though
    /// whenever both are on they share one passphrase value (see
    /// `passwords::decide_vault_unlock_action`, which is what actually
    /// implements that sharing — nothing at the storage layer links the two
    /// databases together).
    pub fn vault_passphrase_marker_path(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", effective_app_id())?;
        Some(dirs.data_dir().join(&self.name).join("vault-passphrase-enabled"))
    }

    /// Whether this profile's password vault is passphrase-protected —
    /// independent of `has_passphrase()` (history's marker). Always `false`
    /// for an `ephemeral` profile, same reasoning as `has_passphrase`.
    pub fn has_vault_passphrase(&self) -> bool {
        !self.ephemeral && self.vault_passphrase_marker_path().map(|p| p.exists()).unwrap_or(false)
    }

    /// Marks this profile's vault as passphrase-protected going forward —
    /// called once, right after successfully establishing encryption on a
    /// brand new (empty) vault database, same timing as `enable_passphrase`.
    pub fn enable_vault_passphrase(&self) -> anyhow::Result<()> {
        let path = self
            .vault_passphrase_marker_path()
            .ok_or_else(|| anyhow::anyhow!("no data directory available on this platform"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, b"")?;
        Ok(())
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

/// Whether `--incognito`, `--private`, or `--guest` was passed — three names
/// for the same `Profile::ephemeral()` session (see its doc comment), kept
/// as synonyms since different users reach for different terms for exactly
/// the same thing. Takes an iterator for the same testability reason as
/// `resolve_profile_name`.
pub fn resolve_ephemeral_requested<I: IntoIterator<Item = String>>(args: I) -> bool {
    args.into_iter().any(|arg| arg == "--incognito" || arg == "--private" || arg == "--guest")
}

/// Whether `--setup-passphrase` was passed — tells a freshly launched
/// process to prompt for a *new* passphrase and establish encryption on
/// `--profile`'s (necessarily brand-new) history database, rather than
/// prompting to *unlock* an already-encrypted one (which instead is driven
/// by `Profile::has_passphrase()`, not a flag). Deliberately a flag, not the
/// passphrase itself: passphrases must never cross a process boundary via
/// `argv`, since that's visible to any other user on the system (e.g. via
/// `ps`) — see `launch_new_profile_process`'s callers for how this is used.
pub fn resolve_passphrase_setup_requested<I: IntoIterator<Item = String>>(args: I) -> bool {
    args.into_iter().any(|arg| arg == "--setup-passphrase")
}

/// Pulls the first positional (non-flag) argument out — used for launching
/// with a URL (e.g. from the OS's "open with"/default-browser handoff):
/// `browser-linux-gtk3 https://example.com`. Skips `argv[0]` and
/// `--profile`'s/`--app-id`'s consumed value so a launch like `program
/// --profile work https://example.com` still finds the URL, not `"work"`.
/// Any other `--foo` flag is skipped generically (not enumerated by name)
/// so this doesn't need updating every time a new flag is added elsewhere
/// — `--profile`/`--app-id` need the extra handling specifically because
/// they're the only two-token (space-separated) flags this crate parses;
/// everything else is a bare presence check (`--incognito`) or already
/// single-token via `=` (`--profile=work`, which `arg.starts_with("--")`
/// already skips whole). Returns the raw token unchanged — unlike
/// `resolve_address_input`, this isn't run through bare-domain/search-query
/// resolution: a real external-link launch always hands over a
/// fully-qualified URL, and resolving one would need a profile's
/// `Settings` before a profile has even been picked.
pub fn resolve_url_argument<I: IntoIterator<Item = String>>(args: I) -> Option<String> {
    let mut args = args.into_iter().skip(1); // skip argv[0]
    while let Some(arg) = args.next() {
        // `--test-command-socket`: only `browser-macos-appkit` parses this
        // (see its `main.rs` — `web-standards-tests/src/bin/macos_driver.rs`
        // passes it), but this scan has to know to skip its value too, the
        // same as `--profile`/`--app-id` below — otherwise the socket path
        // gets mistaken for a bare positional URL, silently routing to the
        // external-link chooser window instead of a normal launch. Confirmed
        // directly: the app launched fine and looked completely idle, no
        // crash, no socket ever created, no output at all — because it was
        // sitting in `run_chooser`, waiting for a chooser interaction that
        // was never coming.
        if arg == "--profile" || arg == "--app-id" || arg == "--test-command-socket" {
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
    match directories::ProjectDirs::from("", "", effective_app_id()) {
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

/// Same as `launch_new_profile_process`, but also passes `--setup-passphrase`
/// so the new process prompts to set up (rather than unlock) passphrase
/// protection for this brand-new profile — see
/// `resolve_passphrase_setup_requested`'s doc comment for why the passphrase
/// itself is never passed here, only this flag.
pub fn launch_new_encrypted_profile_process(profile_name: &str) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe).arg("--profile").arg(profile_name).arg("--setup-passphrase").spawn()?;
    Ok(())
}

/// Launches a new, independent instance of this same binary in a fresh
/// `Profile::ephemeral()` session — same spawn-and-forget reasoning as
/// `launch_new_profile_process`.
pub fn launch_new_ephemeral_process() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe).arg("--incognito").spawn()?;
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
    fn bookmarks_path_is_scoped_under_the_profile_name() {
        let profile = Profile::new("work");
        let path = profile.bookmarks_path().expect("a config dir should be available in tests");
        assert!(path.ends_with("work/bookmarks.json"), "{path:?}");
    }

    #[test]
    fn session_path_is_scoped_under_the_profile_name() {
        let profile = Profile::new("work");
        let path = profile.session_path().expect("a config dir should be available in tests");
        assert!(path.ends_with("work/session.json"), "{path:?}");
    }

    #[test]
    fn screenshots_dir_is_scoped_under_the_profile_name() {
        let profile = Profile::new("work");
        let path = profile.screenshots_dir().expect("a data dir should be available in tests");
        assert!(path.ends_with("work/screenshots"), "{path:?}");
    }

    #[test]
    fn webview_data_dir_is_scoped_under_the_profile_name() {
        let profile = Profile::new("work");
        let path = profile.webview_data_dir().expect("a data dir should be available in tests");
        assert!(path.ends_with("work/webview"), "{path:?}");
    }

    #[test]
    fn default_profile_is_named_default() {
        assert_eq!(Profile::default().name, "default");
        assert!(!Profile::default().ephemeral, "the default profile should be a normal, persistent one");
    }

    #[test]
    fn ephemeral_profile_is_marked_ephemeral() {
        let profile = Profile::ephemeral();
        assert!(profile.ephemeral);
    }

    #[test]
    fn resolve_ephemeral_requested_recognizes_all_three_synonyms() {
        assert!(resolve_ephemeral_requested(args(&["program", "--incognito"])));
        assert!(resolve_ephemeral_requested(args(&["program", "--private"])));
        assert!(resolve_ephemeral_requested(args(&["program", "--guest"])));
        assert!(!resolve_ephemeral_requested(args(&["program", "--profile", "work"])));
        assert!(!resolve_ephemeral_requested(args(&[])));
    }

    #[test]
    fn resolve_passphrase_setup_requested_recognizes_the_flag() {
        assert!(resolve_passphrase_setup_requested(args(&["program", "--setup-passphrase"])));
        assert!(!resolve_passphrase_setup_requested(args(&["program", "--profile", "work"])));
        assert!(!resolve_passphrase_setup_requested(args(&[])));
    }

    #[test]
    fn has_passphrase_reflects_the_marker_file() {
        let profile = Profile::new(format!("passphrase-marker-test-{}", std::process::id()));
        let marker = profile.passphrase_marker_path().expect("a data dir should be available in tests");
        let _ = std::fs::remove_dir_all(marker.parent().unwrap());
        assert!(!profile.has_passphrase(), "a fresh profile shouldn't have a passphrase marker yet");

        profile.enable_passphrase().expect("enable_passphrase should succeed");
        assert!(profile.has_passphrase(), "after enable_passphrase, has_passphrase should be true");
        assert!(marker.exists());

        let _ = std::fs::remove_dir_all(marker.parent().unwrap());
    }

    #[test]
    fn ephemeral_profiles_never_report_having_a_passphrase() {
        assert!(!Profile::ephemeral().has_passphrase());
    }

    #[test]
    fn has_vault_passphrase_reflects_its_own_marker_file() {
        let profile = Profile::new(format!("vault-passphrase-marker-test-{}", std::process::id()));
        let marker = profile.vault_passphrase_marker_path().expect("a data dir should be available in tests");
        let _ = std::fs::remove_file(&marker);
        assert!(!profile.has_vault_passphrase(), "a fresh profile shouldn't have a vault passphrase marker yet");

        profile.enable_vault_passphrase().expect("enable_vault_passphrase should succeed");
        assert!(profile.has_vault_passphrase(), "after enable_vault_passphrase, has_vault_passphrase should be true");
        assert!(marker.exists());

        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn vault_passphrase_and_history_passphrase_markers_are_independent() {
        let profile = Profile::new(format!("independent-passphrase-markers-test-{}", std::process::id()));
        let history_marker = profile.passphrase_marker_path().unwrap();
        let vault_marker = profile.vault_passphrase_marker_path().unwrap();
        let _ = std::fs::remove_file(&history_marker);
        let _ = std::fs::remove_file(&vault_marker);

        profile.enable_vault_passphrase().expect("enable_vault_passphrase should succeed");
        assert!(profile.has_vault_passphrase(), "the vault marker should be set");
        assert!(!profile.has_passphrase(), "enabling the vault passphrase shouldn't also enable history's");

        let _ = std::fs::remove_file(&vault_marker);
    }

    #[test]
    fn ephemeral_profiles_never_report_having_a_vault_passphrase() {
        assert!(!Profile::ephemeral().has_vault_passphrase());
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

    #[test]
    fn resolve_url_argument_skips_the_app_id_flags_value() {
        assert_eq!(
            resolve_url_argument(args(&["program", "--app-id", "renamed", "https://example.com"])),
            Some("https://example.com".to_string()),
            "the URL, not the --app-id flag's value, should be returned"
        );
        assert_eq!(
            resolve_url_argument(args(&["program", "--app-id=renamed", "https://example.com"])),
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
