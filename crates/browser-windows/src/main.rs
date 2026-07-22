fn main() -> anyhow::Result<()> {
    let (hwnd, app) = browser_windows::create_window()?;
    let start_page = app.settings().start_page.clone();
    app.add_page(&start_page)?;
    browser_windows::run_message_loop(hwnd);
    Ok(())
}
