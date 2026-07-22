use browser_core::{resolve_profile_name, Profile};
use browser_linux_gtk3::build_window_and_app;

fn main() -> anyhow::Result<()> {
    gtk::init()?;

    let profile = Profile::new(resolve_profile_name(std::env::args()));
    let (_window, app) = build_window_and_app(profile)?;
    let start_page = app.settings().start_page.clone();
    app.add_page(&start_page)?;

    gtk::main();
    Ok(())
}
