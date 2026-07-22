use browser_core::{resolve_profile_name, Profile};

fn main() -> anyhow::Result<()> {
    let profile = Profile::new(resolve_profile_name(std::env::args()));
    let (hwnd, app) = browser_windows_win32::create_window(profile)?;
    let start_page = app.settings().start_page.clone();
    app.add_page(&start_page)?;
    browser_windows_win32::run_message_loop(hwnd);
    Ok(())
}
