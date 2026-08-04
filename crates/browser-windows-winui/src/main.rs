#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn main() -> anyhow::Result<()> {
    browser_windows_winui::trace("main: start");
    winui3::init_apartment(winui3::ApartmentType::SingleThreaded)?;
    browser_windows_winui::trace("main: after init_apartment");
    // Not optional: `Microsoft.UI.Xaml.*` types aren't in the OS's WinRT
    // catalog until the Windows App SDK's framework package is located via
    // this bootstrap call (see the WinUI 3 smoke-test history in git log for
    // why — skipping it produces `Class not registered`).
    let _dependency =
        winui3::bootstrap::PackageDependency::initialize_version(winui3::bootstrap::WindowsAppSDKVersion::V2)?;
    browser_windows_winui::trace("main: after bootstrap PackageDependency");

    // `Application::Start`'s callback is the only place WinUI 3's XAML
    // runtime accepts control construction (see `browser-windows-winui`'s
    // module doc comment) — it also owns the message pump from here on,
    // running until something calls `Application::Current()?.Exit()`, which
    // the HWND subclass installed by `build_window_and_app` does on
    // `WM_DESTROY` (there's no working `Window::Closed` event to use instead).
    browser_windows_winui::trace("main: calling Application::Start");
    winui3::Microsoft::UI::Xaml::Application::Start(&winui3::Microsoft::UI::Xaml::ApplicationInitializationCallback::new(
        move |_params| {
            browser_windows_winui::trace("callback: entered");
            if let Err(err) = run() {
                browser_windows_winui::trace(&format!("callback: run() returned Err: {err}"));
                eprintln!("failed to start browser-windows-winui: {err}");
            } else {
                browser_windows_winui::trace("callback: run() returned Ok");
            }
            Ok(())
        },
    ))?;
    browser_windows_winui::trace("main: Application::Start returned");
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
    browser_windows_winui::trace("run: after Application::new");

    let args: Vec<String> = std::env::args().collect();
    if let Some(url) = resolve_url_argument(args.clone()) {
        show_external_link_chooser(url, resolve_profile_name(args))?;
        browser_windows_winui::trace("run: after show_external_link_chooser");
    } else {
        let profile = Profile::new(resolve_profile_name(args));
        browser_windows_winui::trace("run: about to call build_window_and_app");
        let app = build_window_and_app(profile)?;
        browser_windows_winui::trace("run: after build_window_and_app");
        app.open_start_page_or_restored_session();
        browser_windows_winui::trace("run: after open_start_page_or_restored_session");
        app.activate()?;
        browser_windows_winui::trace("run: after activate");
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
