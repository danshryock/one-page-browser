#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    use browser_core::{resolve_profile_name, Profile};

    let profile = Profile::new(resolve_profile_name(std::env::args()));
    let (hwnd, app) = browser_windows_win32::create_window(profile)?;
    let start_page = app.settings().start_page.clone();
    app.add_page(&start_page)?;
    browser_windows_win32::run_message_loop(hwnd);
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!(
        "browser-windows-win32 is a Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-gnu (cargo build or cross build)."
    );
}
