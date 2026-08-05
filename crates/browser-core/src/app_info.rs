//! The application's identity: one canonical name used both as the OS-level
//! identifier (`directories::ProjectDirs`'s `application` argument, which
//! determines where profile config/data directories live, and platform
//! paths like WebView2's user-data folder) and as the display name shown in
//! every front end's window title. Single source of truth so renaming the
//! app is a one-line change here — plus a real data migration for
//! existing users' profile directories, which this alone doesn't handle —
//! instead of a grep-and-replace across every front end.

pub const APP_NAME: &str = "claude-browser";
