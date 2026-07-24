// TEMPORARY, for diagnosing the first-ever real launch of this binary on
// real Windows (see summaries/windows-github-actions-ci.md): the app fast-
// fails with STATUS_STOWED_EXCEPTION before writing anything to its own
// stdout/stderr (confirmed via a GitHub Actions run redirecting both to
// files — empty either way), which is consistent with a crash abrupt enough
// to skip normal stream flushing. Writing straight to a file with an
// explicit sync_all() after every line survives that kind of abrupt
// termination in a way buffered stdio doesn't, and lets each checkpoint
// below be individually confirmed present/absent in the resulting log —
// pinpointing how far startup got before the crash. Remove once the actual
// bug is found and fixed.
#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("winui-trace.log") {
        let _ = writeln!(f, "{msg}");
        let _ = f.sync_all();
    }
}

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn main() -> anyhow::Result<()> {
    trace("main: start");
    winui3::init_apartment(winui3::ApartmentType::SingleThreaded)?;
    trace("main: after init_apartment");
    // Not optional: `Microsoft.UI.Xaml.*` types aren't in the OS's WinRT
    // catalog until the Windows App SDK's framework package is located via
    // this bootstrap call (see the WinUI 3 smoke-test history in git log for
    // why — skipping it produces `Class not registered`).
    let _dependency =
        winui3::bootstrap::PackageDependency::initialize_version(winui3::bootstrap::WindowsAppSDKVersion::V2)?;
    trace("main: after bootstrap PackageDependency");

    // `Application::Start`'s callback is the only place WinUI 3's XAML
    // runtime accepts control construction (see `browser-windows-winui`'s
    // module doc comment) — it also owns the message pump from here on,
    // running until something calls `Application::Current()?.Exit()`, which
    // the HWND subclass installed by `build_window_and_app` does on
    // `WM_DESTROY` (there's no working `Window::Closed` event to use instead).
    trace("main: calling Application::Start");
    winui3::Microsoft::UI::Xaml::Application::Start(&winui3::Microsoft::UI::Xaml::ApplicationInitializationCallback::new(
        move |_params| {
            trace("callback: entered");
            if let Err(err) = run() {
                trace(&format!("callback: run() returned Err: {err}"));
                eprintln!("failed to start browser-windows-winui: {err}");
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
    use browser_core::{resolve_profile_name, resolve_url_argument, Profile};
    use browser_windows_winui::{build_window_and_app, show_external_link_chooser};

    // Establishes the WinRT `Microsoft.UI.Xaml.Application` singleton for
    // this thread — required before any `Microsoft.UI.Xaml` object (the
    // window, its controls) can be activated.
    let _app_instance = winui3::Microsoft::UI::Xaml::Application::new()?;
    trace("run: after Application::new");

    let args: Vec<String> = std::env::args().collect();
    if let Some(url) = resolve_url_argument(args.clone()) {
        show_external_link_chooser(url, resolve_profile_name(args))?;
        trace("run: after show_external_link_chooser");
    } else {
        let profile = Profile::new(resolve_profile_name(args));
        trace("run: about to call build_window_and_app");
        let app = build_window_and_app(profile)?;
        trace("run: after build_window_and_app");
        let start_page = app.settings().start_page.clone();
        app.add_page(&start_page)?;
        trace("run: after add_page");
        app.activate()?;
        trace("run: after activate");
    }
    Ok(())
}

#[cfg(not(all(target_os = "windows", target_env = "msvc")))]
fn main() {
    eprintln!(
        "browser-windows-winui is an MSVC-Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-msvc via `cargo xwin build` (see README.md)."
    );
}
