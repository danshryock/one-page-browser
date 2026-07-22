use native_windows_gui as nwg;
use nwg::NativeUi;

fn main() -> anyhow::Result<()> {
    nwg::init()?;

    let app = browser_windows_nwg::App::build_ui(Default::default())?;

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
