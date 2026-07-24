//! Tests a genuinely different suspect from the other six bisection binaries
//! (see `summaries/windows-github-actions-ci.md`), all of which survived
//! cleanly: none of them touch `browser_core` at all. The real app's
//! `build_window_and_app` calls `HistoryStore::open(&profile)`, which opens
//! a real libsql database and spins up its own `tokio::runtime::Runtime`
//! (see `browser_core::history`'s `self.rt.block_on(...)` calls) — mixing
//! that multi-threaded async runtime with the WinRT single-threaded STA
//! apartment (`init_apartment(ApartmentType::SingleThreaded)`) is a
//! genuinely plausible, previously untested crash source.
//!
//! Uses `HistoryStore::open_in_memory()` (same real libsql/tokio machinery,
//! no disk I/O) rather than a real `Profile`, and *actually runs queries*
//! against it — `record_visit` then `search` — displaying the real result in
//! the window's content, not just opening the store and leaving it idle.
//! Still no custom title bar, `WebView2`, or HWND subclassing — isolates the
//! `browser_core`/tokio question on its own.

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("historystore-smoke-trace.log") {
        let _ = writeln!(f, "{msg}");
        let _ = f.sync_all();
    }
}

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn main() -> anyhow::Result<()> {
    trace("main: start");
    winui3::init_apartment(winui3::ApartmentType::SingleThreaded)?;
    trace("main: after init_apartment");
    let _dependency =
        winui3::bootstrap::PackageDependency::initialize_version(winui3::bootstrap::WindowsAppSDKVersion::V2)?;
    trace("main: after bootstrap PackageDependency");

    trace("main: calling Application::Start");
    winui3::Microsoft::UI::Xaml::Application::Start(&winui3::Microsoft::UI::Xaml::ApplicationInitializationCallback::new(
        move |_params| {
            trace("callback: entered");
            if let Err(err) = run() {
                trace(&format!("callback: run() returned Err: {err}"));
            } else {
                trace("callback: run() returned Ok");
            }
            Ok(())
        },
    ))?;
    trace("main: Application::Start returned");
    Ok(())
}

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn run() -> anyhow::Result<()> {
    use browser_core::HistoryStore;
    use winui3::Microsoft::UI::Xaml::Controls::TextBlock;

    let _app_instance = winui3::Microsoft::UI::Xaml::Application::new()?;
    trace("run: after Application::new");

    let history = HistoryStore::open_in_memory()?;
    trace("run: after HistoryStore::open_in_memory");
    history.record_visit("https://example.com", "Example Domain")?;
    trace("run: after record_visit");
    history.record_visit("https://example.org", "Example Org")?;
    trace("run: after second record_visit");
    let results = history.search("example", 10)?;
    trace(&format!("run: after search, found {} entries", results.len()));

    let window = winui3::Microsoft::UI::Xaml::Window::new()?;
    trace("run: after Window::new");
    window.SetTitle(&windows_core::HSTRING::from("HistoryStore WinUI 3 smoke test"))?;
    trace("run: after SetTitle");

    let summary = format!(
        "Found {} entries: {}",
        results.len(),
        results.iter().map(|e| e.title.as_str()).collect::<Vec<_>>().join(", ")
    );
    trace(&format!("run: display text = {summary:?}"));
    let text = TextBlock::new()?;
    text.SetText(&windows_core::HSTRING::from(summary.as_str()))?;
    window.SetContent(&text)?;
    trace("run: after SetContent");

    window.Activate()?;
    trace("run: after Activate");
    Ok(())
}

#[cfg(not(all(target_os = "windows", target_env = "msvc")))]
fn main() {
    eprintln!(
        "historystore_smoke_test is an MSVC-Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-msvc via `cargo xwin build` (see README.md)."
    );
}
