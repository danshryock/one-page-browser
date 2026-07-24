//! Same as `minimal_smoke_test.rs`, plus *only*
//! `SetExtendsContentIntoTitleBar(true)` + `SetTitleBar(...)` (the custom
//! title bar `browser-windows-winui`'s real window uses — see `lib.rs`
//! around its `window.SetExtendsContentIntoTitleBar(true)` call) — still no
//! `install_hwnd_subclass`/`subclass_proc`. `minimal_smoke_test` already
//! proved a bare WinUI 3 window survives fine on this CI runner; this
//! isolates whether the custom title bar specifically (rather than HWND
//! subclassing) is what the real app's crash traces back to — its trace log
//! ended right at `WM_DWMNCRENDERINGCHANGED`/`WM_SETCURSOR`, exactly the
//! non-client-area/DWM interaction this feature depends on.

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("titlebar-smoke-trace.log") {
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
    use winui3::Microsoft::UI::Xaml::Controls::TextBlock;

    let _app_instance = winui3::Microsoft::UI::Xaml::Application::new()?;
    trace("run: after Application::new");

    let window = winui3::Microsoft::UI::Xaml::Window::new()?;
    trace("run: after Window::new");
    window.SetTitle(&windows_core::HSTRING::from("Titlebar WinUI 3 smoke test"))?;
    trace("run: after SetTitle");

    let titlebar = TextBlock::new()?;
    titlebar.SetText(&windows_core::HSTRING::from("Custom title bar"))?;
    trace("run: after TextBlock::new for titlebar");
    window.SetContent(&titlebar)?;
    trace("run: after SetContent");
    window.SetExtendsContentIntoTitleBar(true)?;
    trace("run: after SetExtendsContentIntoTitleBar");
    window.SetTitleBar(&titlebar)?;
    trace("run: after SetTitleBar");

    window.Activate()?;
    trace("run: after Activate");
    Ok(())
}

#[cfg(not(all(target_os = "windows", target_env = "msvc")))]
fn main() {
    eprintln!(
        "titlebar_smoke_test is an MSVC-Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-msvc via `cargo xwin build` (see README.md)."
    );
}
