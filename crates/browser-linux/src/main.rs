use browser_linux::{build_window_and_app, HOME_URL};

fn main() -> anyhow::Result<()> {
    gtk::init()?;

    let (_window, app) = build_window_and_app()?;
    app.add_page(HOME_URL)?;

    gtk::main();
    Ok(())
}
