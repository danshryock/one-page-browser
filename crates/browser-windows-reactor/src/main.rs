#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn main() -> anyhow::Result<()> {
    browser_windows_reactor::trace("main: start");
    set_webview2_user_data_folder();
    windows_reactor::bootstrap()?;
    browser_windows_reactor::trace("main: after bootstrap");
    let args: Vec<String> = std::env::args().collect();
    let result = if let Some(url) = browser_core::resolve_url_argument(args.clone()) {
        let default_profile = browser_core::resolve_profile_name(args);
        browser_windows_reactor::run_chooser(url, default_profile)
    } else {
        let profile = browser_core::Profile::new(browser_core::resolve_profile_name(args));
        browser_windows_reactor::run(profile)
    };
    browser_windows_reactor::trace(&format!("main: run returned {result:?}"));
    result?;
    Ok(())
}

/// `WebView2`, for an unpackaged app like this one, defaults its user data
/// folder to a location *next to the executable* — a real, documented
/// `WebView2` behavior, but a fragile one: if that location isn't writable
/// (Program Files, a read-only mount, a network share — every VM test this
/// session ran the exe from exactly such a UNC path), `CoreWebView2`
/// initialization fails silently rather than with a visible error (see
/// `engine.rs`'s `RenderEngine` impl and `on_ready` in `lib.rs` — `on_ready`
/// simply never fires, with nothing in `windows-webview`'s reactor bridge
/// surfacing *why*, since it doesn't bind `CoreWebView2InitializedEventArgs`'s
/// `Exception` property at all — checked by reading its generated bindings).
/// Root-caused a real user report of the page never rendering on their
/// machine (confirmed there that `WebView2` itself works fine for other
/// apps, ruling out a missing/broken runtime) landing on this as the most
/// likely remaining explanation.
///
/// `WEBVIEW2_USER_DATA_FOLDER` is a real, Microsoft-documented override
/// (must be set before the first `WebView2` control initializes) — pointing
/// it at `%LOCALAPPDATA%\claude-browser\webview2` instead gives every
/// `WebView2` control in this process a location guaranteed writable by the
/// current user, regardless of where the exe itself lives.
#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn set_webview2_user_data_folder() {
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let path = std::path::Path::new(&local_app_data).join("claude-browser").join("webview2");
        // SAFETY: called once, at the very start of `main`, before any
        // other thread exists (nothing has spawned one yet) and before
        // anything reads this variable.
        unsafe {
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", path);
        }
    }
}

#[cfg(not(all(target_os = "windows", target_env = "msvc")))]
fn main() {
    eprintln!(
        "browser-windows-reactor is an MSVC-Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-msvc via `cargo xwin build` (see README.md)."
    );
}
