#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn main() -> anyhow::Result<()> {
    winui3::init_apartment(winui3::ApartmentType::SingleThreaded)?;
    // Not optional: `Microsoft.UI.Xaml.*` types aren't in the OS's WinRT
    // catalog until the Windows App SDK's framework package is located via
    // this bootstrap call (see the WinUI 3 smoke-test history in git log for
    // why — skipping it produces `Class not registered`).
    let _dependency =
        winui3::bootstrap::PackageDependency::initialize_version(winui3::bootstrap::WindowsAppSDKVersion::V2)?;

    // `Application::Start`'s callback is the only place WinUI 3's XAML
    // runtime accepts control construction (see `browser-windows-winui`'s
    // module doc comment) — it also owns the message pump from here on,
    // running until something calls `Application::Current()?.Exit()`, which
    // the HWND subclass installed by `build_window_and_app` does on
    // `WM_DESTROY` (there's no working `Window::Closed` event to use instead).
    winui3::Microsoft::UI::Xaml::Application::Start(&winui3::Microsoft::UI::Xaml::ApplicationInitializationCallback::new(
        move |_params| {
            if let Err(err) = run() {
                eprintln!("failed to start browser-windows-winui: {err}");
            }
            Ok(())
        },
    ))?;
    Ok(())
}

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn run() -> anyhow::Result<()> {
    use browser_core::{resolve_profile_name, Profile};
    use browser_windows_winui::build_window_and_app;

    // Establishes the WinRT `Microsoft.UI.Xaml.Application` singleton for
    // this thread — required before any `Microsoft.UI.Xaml` object (the
    // window, its controls) can be activated.
    let _app_instance = winui3::Microsoft::UI::Xaml::Application::new()?;

    let profile = Profile::new(resolve_profile_name(std::env::args()));
    let app = build_window_and_app(profile)?;
    let start_page = app.settings().start_page.clone();
    app.add_page(&start_page)?;
    app.activate()?;
    Ok(())
}

#[cfg(not(all(target_os = "windows", target_env = "msvc")))]
fn main() {
    eprintln!(
        "browser-windows-winui is an MSVC-Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-msvc via `cargo xwin build` (see README.md)."
    );
}
