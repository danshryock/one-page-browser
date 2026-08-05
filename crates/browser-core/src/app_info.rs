//! The application's identity — kept as two separate constants rather than
//! one, since they serve genuinely different purposes and don't want the
//! same value: `APP_ID` is a path-safe, lowercase-hyphenated identifier
//! (`directories::ProjectDirs`'s `application` argument, which determines
//! where profile config/data directories live, and platform paths like
//! WebView2's user-data folder), while `APP_TITLE` is the human-readable
//! name shown in every front end's window title. Single source of truth for
//! each so renaming the app is a one-line change here — plus a real data
//! migration for existing users' profile directories, which changing
//! `APP_ID` alone doesn't handle — instead of a grep-and-replace across
//! every front end.

/// Path-safe identifier — profile config/data directory names, and platform
/// paths (e.g. WebView2's user-data folder) that shouldn't contain spaces.
pub const APP_ID: &str = "claude-browser";

/// Human-readable name shown in every front end's window title.
pub const APP_TITLE: &str = "Claude Browser";
