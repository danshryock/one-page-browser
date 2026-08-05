//! The application's identity — kept as two separate constants rather than
//! one, since they serve genuinely different purposes and don't want the
//! same value: `APP_ID` is a path-safe, lowercase-hyphenated identifier
//! (`directories::ProjectDirs`'s `application` argument, which determines
//! where profile config/data directories live, and platform paths like
//! WebView2's user-data folder), while `APP_TITLE` is the human-readable
//! name shown in every front end's window title. Single source of truth for
//! each so renaming the app is a one-line change here — plus, for `APP_ID`,
//! prepending the outgoing value to `LEGACY_APP_IDS` so `init_app_id` can
//! migrate an existing user's data forward — instead of a grep-and-replace
//! across every front end plus abandoned data on every existing install.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Path-safe identifier — profile config/data directory names, and platform
/// paths (e.g. WebView2's user-data folder) that shouldn't contain spaces.
pub const APP_ID: &str = "claude-browser";

/// Human-readable name shown in every front end's window title.
pub const APP_TITLE: &str = "Claude Browser";

/// Previous `APP_ID` values, most recently used first — when renaming the
/// app, prepend the outgoing `APP_ID` here (don't append, don't remove
/// older entries). `init_app_id` walks this in order so an existing user's
/// config/data directories are found and migrated forward the first time
/// they launch after a rename. Empty until the first rename actually
/// happens.
pub const LEGACY_APP_IDS: &[&str] = &[];

const APP_ID_ENV_VAR: &str = "CLAUDE_BROWSER_APP_ID";

static EFFECTIVE_APP_ID: OnceLock<String> = OnceLock::new();

/// Must be called at most once, at the very start of every front end's
/// `main`, before any `Profile` is constructed or any of its path methods
/// run (they resolve against `effective_app_id()`, not the bare `APP_ID`
/// constant, specifically so this can override it). Resolves the effective
/// `APP_ID` for this run — an `--app-id NAME`/`--app-id=NAME` CLI argument
/// (checked first), else the `CLAUDE_BROWSER_APP_ID` environment variable,
/// else the compiled-in `APP_ID` — and, *only* when neither override was
/// given, runs the legacy-data migration first. A manual override means
/// deliberately picking an identity, not asking to inherit an existing
/// user's history from a rename, so migration is skipped entirely in that
/// case (matches every other explicit-argument-wins-over-inference
/// convention elsewhere in this crate, e.g. a caller-supplied launch URL
/// always winning over a restored session).
pub fn init_app_id<I: IntoIterator<Item = String>>(args: I) {
    match resolve_app_id_override(args) {
        Some(id) => {
            let _ = EFFECTIVE_APP_ID.set(id);
        }
        None => {
            migrate_legacy_app_id_data(APP_ID);
            let _ = EFFECTIVE_APP_ID.set(APP_ID.to_string());
        }
    }
}

/// The `APP_ID` every `Profile` path method actually resolves against.
/// Falls back to the compiled-in `APP_ID` if `init_app_id` was never called
/// — every real front end calls it, but tests construct `Profile`s
/// directly without going through a front end's `main`, so they run
/// against the compiled-in default, same as before this module existed.
pub(crate) fn effective_app_id() -> &'static str {
    EFFECTIVE_APP_ID.get().map(String::as_str).unwrap_or(APP_ID)
}

/// `--app-id NAME`/`--app-id=NAME` (checked first), else
/// `CLAUDE_BROWSER_APP_ID` — same shape as `resolve_profile_name`'s
/// `--profile`/`--profile=`, CLI taking precedence over the environment
/// variable. `None` means neither was given, i.e. the compiled-in `APP_ID`
/// should be used (and migration should run).
fn resolve_app_id_override<I: IntoIterator<Item = String>>(args: I) -> Option<String> {
    resolve_app_id_override_with_env(args, std::env::var(APP_ID_ENV_VAR).ok())
}

/// Split out from `resolve_app_id_override` so a test can supply `env_value`
/// directly instead of mutating the real process environment — `std::env::
/// set_var` affects the whole process, not just one thread, which would be
/// flaky under `cargo test`'s default parallel execution.
fn resolve_app_id_override_with_env<I: IntoIterator<Item = String>>(args: I, env_value: Option<String>) -> Option<String> {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--app-id=") {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        } else if arg == "--app-id" {
            if let Some(value) = args.next() {
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
    }
    env_value.filter(|v| !v.is_empty())
}

/// See `init_app_id`'s doc comment for when/why this runs. Best-effort: a
/// failed rename is logged, not propagated — this must never block the app
/// from starting, worst case a user just doesn't see their old data until
/// they notice and file a bug.
fn migrate_legacy_app_id_data(new_app_id: &str) {
    let Some((new_config, new_data)) = project_dirs(new_app_id) else { return };
    let legacy: Vec<(PathBuf, PathBuf)> = LEGACY_APP_IDS.iter().filter_map(|id| project_dirs(id)).collect();
    migrate_legacy_app_id_data_at(&new_config, &new_data, &legacy);
}

fn project_dirs(app_id: &str) -> Option<(PathBuf, PathBuf)> {
    let dirs = directories::ProjectDirs::from("", "", app_id)?;
    Some((dirs.config_dir().to_path_buf(), dirs.data_dir().to_path_buf()))
}

/// Split out from `migrate_legacy_app_id_data` so a test can exercise the
/// real rename logic against throwaway directories instead of the real
/// user config/data directories — same reasoning as `Settings::load_from`/
/// `list_profile_names_in`. `new_config`/`new_data` are the current
/// `APP_ID`'s resolved directories; `legacy` is each `LEGACY_APP_IDS`
/// entry's resolved `(config_dir, data_dir)` pair, most recently used
/// first, already filtered to ones the platform dirs crate could resolve
/// at all.
///
/// Checks *either* directory existing under the new `APP_ID` as "already
/// migrated, nothing to do" (a profile's settings/keybindings/bookmarks/
/// session live under `config_dir`, history/passwords/screenshots/webview
/// data live under `data_dir` — both matter, and on a platform where they
/// happen to coincide, either check alone is already correct). Migrates
/// the *first* legacy id (most recent to oldest) that has anything at all,
/// moving whichever of its two directories actually exist — a real
/// filesystem rename, not a copy, so old data doesn't linger duplicated on
/// disk — then stops; it never merges more than one legacy identity's data
/// into the new location.
fn migrate_legacy_app_id_data_at(new_config: &Path, new_data: &Path, legacy: &[(PathBuf, PathBuf)]) {
    if new_config.exists() || new_data.exists() {
        return;
    }
    for (old_config, old_data) in legacy {
        if !old_config.exists() && !old_data.exists() {
            continue;
        }
        if old_config.exists() {
            if let Some(parent) = new_config.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(err) = std::fs::rename(old_config, new_config) {
                eprintln!("failed to migrate config directory to the current app id: {err}");
            }
        }
        if old_data.exists() {
            if let Some(parent) = new_data.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(err) = std::fs::rename(old_data, new_data) {
                eprintln!("failed to migrate data directory to the current app id: {err}");
            }
        }
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolve_app_id_override_returns_none_when_nothing_is_given() {
        assert_eq!(resolve_app_id_override_with_env(args(&["program"]), None), None);
    }

    #[test]
    fn resolve_app_id_override_parses_space_separated_form() {
        assert_eq!(resolve_app_id_override_with_env(args(&["program", "--app-id", "renamed"]), None), Some("renamed".to_string()));
    }

    #[test]
    fn resolve_app_id_override_parses_equals_form() {
        assert_eq!(resolve_app_id_override_with_env(args(&["program", "--app-id=renamed"]), None), Some("renamed".to_string()));
    }

    #[test]
    fn resolve_app_id_override_falls_back_to_the_env_var_when_no_flag_is_given() {
        assert_eq!(
            resolve_app_id_override_with_env(args(&["program"]), Some("from-env".to_string())),
            Some("from-env".to_string())
        );
    }

    #[test]
    fn resolve_app_id_override_prefers_the_cli_flag_over_the_env_var() {
        assert_eq!(
            resolve_app_id_override_with_env(args(&["program", "--app-id", "from-cli"]), Some("from-env".to_string())),
            Some("from-cli".to_string())
        );
    }

    #[test]
    fn resolve_app_id_override_ignores_an_empty_flag_or_env_value() {
        assert_eq!(resolve_app_id_override_with_env(args(&["program", "--app-id="]), Some(String::new())), None);
    }

    #[test]
    fn does_nothing_when_the_new_app_ids_directories_already_exist() {
        let root = std::env::temp_dir().join(format!("claude-browser-test-migrate-noop-{}", std::process::id()));
        let new_config = root.join("new-config");
        let new_data = root.join("new-data");
        let old_config = root.join("old-config");
        let old_data = root.join("old-data");
        std::fs::create_dir_all(&new_config).unwrap();
        std::fs::create_dir_all(&old_config).unwrap();

        migrate_legacy_app_id_data_at(&new_config, &new_data, &[(old_config.clone(), old_data.clone())]);

        assert!(old_config.exists(), "an already-migrated new location shouldn't touch the old one");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn migrates_the_most_recently_used_legacy_id_that_has_data() {
        let root = std::env::temp_dir().join(format!("claude-browser-test-migrate-found-{}", std::process::id()));
        let new_config = root.join("new-config");
        let new_data = root.join("new-data");
        let newest_config = root.join("newest-config");
        let newest_data = root.join("newest-data");
        let oldest_config = root.join("oldest-config");
        std::fs::create_dir_all(&newest_config).unwrap();
        std::fs::write(newest_config.join("settings.json"), "{}").unwrap();
        std::fs::create_dir_all(&oldest_config).unwrap();

        // Both a "newest" and an "oldest" legacy id have data on disk —
        // only the newest (first in the list) should be migrated.
        migrate_legacy_app_id_data_at(
            &new_config,
            &new_data,
            &[(newest_config.clone(), newest_data.clone()), (oldest_config.clone(), root.join("oldest-data"))],
        );

        assert!(!newest_config.exists(), "the migrated legacy config dir should have been moved, not copied");
        assert!(new_config.join("settings.json").exists(), "its contents should now live at the new location");
        assert!(oldest_config.exists(), "an older legacy id shouldn't be touched once a more recent one was found");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn skips_a_legacy_id_with_neither_directory_present() {
        let root = std::env::temp_dir().join(format!("claude-browser-test-migrate-skip-{}", std::process::id()));
        let new_config = root.join("new-config");
        let new_data = root.join("new-data");
        let missing_config = root.join("missing-config");
        let missing_data = root.join("missing-data");
        let real_config = root.join("real-config");
        std::fs::create_dir_all(&real_config).unwrap();

        migrate_legacy_app_id_data_at(
            &new_config,
            &new_data,
            &[(missing_config, missing_data), (real_config.clone(), root.join("real-data"))],
        );

        assert!(!real_config.exists(), "the legacy id that actually has data should still be found and migrated");
        assert!(new_config.exists());
        std::fs::remove_dir_all(&root).ok();
    }
}
