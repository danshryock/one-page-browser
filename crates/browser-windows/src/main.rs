fn main() -> anyhow::Result<()> {
    browser_windows::create_window()?;
    browser_windows::run_message_loop();
    Ok(())
}
