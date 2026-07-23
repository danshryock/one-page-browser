#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use browser_core::{resolve_profile_name, resolve_url_argument, Profile};
    use browser_linux_gtk3::{build_window_and_app, show_external_link_chooser};

    gtk::init()?;

    let args: Vec<String> = std::env::args().collect();
    if let Some(url) = resolve_url_argument(args.clone()) {
        show_external_link_chooser(url, resolve_profile_name(args))?;
    } else {
        let profile = Profile::new(resolve_profile_name(args));
        let (_window, app) = build_window_and_app(profile)?;
        let start_page = app.settings().start_page.clone();
        app.add_page(&start_page)?;
    }

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
