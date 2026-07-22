#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use browser_core::{resolve_profile_name, Profile};
    use browser_linux_gtk3::build_window_and_app;

    gtk::init()?;

    let profile = Profile::new(resolve_profile_name(std::env::args()));
    let (_window, app) = build_window_and_app(profile)?;
    let start_page = app.settings().start_page.clone();
    app.add_page(&start_page)?;

    gtk::main();
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!(
        "browser-linux-gtk3 is a Linux-only binary; nothing to run on this platform. \
         Build browser-windows-win32 or browser-windows-nwg instead (--target x86_64-pc-windows-gnu, \
         via cargo build or cross build)."
    );
}
