fn main() -> anyhow::Result<()> {
    let profile = browser_core::Profile::new(browser_core::resolve_profile_name(std::env::args()));

    wxdragon::main(move |_app| {
        let app_state = browser_wx::build_frame_and_app(profile);
        let start_page = app_state.settings().start_page.clone();
        if let Err(err) = app_state.add_page(&start_page) {
            eprintln!("failed to open start page: {err}");
        }
    })
    .map_err(|err| anyhow::anyhow!("wxdragon::main failed: {err}"))
}
