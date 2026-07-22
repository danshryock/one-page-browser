use browser_linux_gtk3::build_window_and_app;

fn main() -> anyhow::Result<()> {
    gtk::init()?;

    let (_window, app) = build_window_and_app()?;
    let start_page = app.settings().start_page.clone();
    app.add_page(&start_page)?;

    gtk::main();
    Ok(())
}
