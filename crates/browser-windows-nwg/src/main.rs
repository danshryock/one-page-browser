#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    use browser_core::{resolve_profile_name, Profile};
    use native_windows_gui as nwg;
    use nwg::NativeUi;

    nwg::init()?;

    let app = browser_windows_nwg::App::build_ui(Default::default())?;

    // #[derive(NwgUi)] only ever default-constructs App's fields, so the
    // real profile/settings have to be loaded explicitly, right after
    // build_ui — see App::load_settings.
    let profile = Profile::new(resolve_profile_name(std::env::args()));
    app.load_settings(profile);

    // Every NWG control defaults to VISIBLE (confirmed against the vendored
    // source's per-control `flags()` methods) — hide the switcher's own
    // controls up front; `close_switcher` is idempotent and already does
    // exactly this (plus a layout pass), so it doubles as the initial state.
    app.close_switcher_for_startup();

    let start_page = app.settings().start_page.clone();
    app.add_page(&start_page)?;

    nwg::dispatch_thread_events();
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!(
        "browser-windows-nwg is a Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-gnu (cargo build or cross build)."
    );
}
