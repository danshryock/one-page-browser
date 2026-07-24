//! Same as `minimal_smoke_test.rs`, plus *only* `window.AppWindow()?.Resize(...)`
//! — the real `build_window_and_app`'s very first action after `Window::new()`
//! (see `lib.rs`), and something **none** of the previous nine bisection
//! binaries ever called (all of them ran at the WinUI 3 default window size).
//! Explicitly resizing the `AppWindow` is a genuinely different, previously
//! untested code path from anything tried so far, and — unlike blaming
//! Microsoft's own well-tested WinRT/Composition internals directly — a
//! wrong/unusual *call* into a real, specific API is a far more likely place
//! for an actual bug (ours, or in the `winio-winui3` wrapper crate we depend
//! on) to live.

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("appwindow-resize-smoke-trace.log") {
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
    let _app_instance = winui3::Microsoft::UI::Xaml::Application::new()?;
    trace("run: after Application::new");

    let window = winui3::Microsoft::UI::Xaml::Window::new()?;
    trace("run: after Window::new");
    window.SetTitle(&windows_core::HSTRING::from("AppWindow-resize WinUI 3 smoke test"))?;
    trace("run: after SetTitle");

    match window.AppWindow() {
        Ok(app_window) => {
            trace("run: after AppWindow()");
            match app_window.Resize(windows::Graphics::SizeInt32 { Width: 1024, Height: 768 }) {
                Ok(()) => trace("run: after Resize"),
                Err(err) => trace(&format!("run: Resize returned Err: {err}")),
            }
        }
        Err(err) => trace(&format!("run: AppWindow() returned Err: {err}")),
    }

    window.Activate()?;
    trace("run: after Activate");
    Ok(())
}

#[cfg(not(all(target_os = "windows", target_env = "msvc")))]
fn main() {
    eprintln!(
        "appwindow_resize_smoke_test is an MSVC-Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-msvc via `cargo xwin build` (see README.md)."
    );
}
