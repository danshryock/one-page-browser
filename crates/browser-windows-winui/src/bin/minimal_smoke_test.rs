//! A deliberately bare-bones WinUI 3 app: init, bootstrap, one `Window` with
//! a title, `Activate()` — no `HWND` subclassing (`SetWindowSubclass`), no
//! controls, no `WebView2`, none of `browser-windows-winui`'s own workarounds
//! for gaps in `winio-winui3`'s bindings (see `lib.rs`'s module doc comment).
//!
//! Exists to answer one question during the first real CI run's debugging
//! (see `summaries/windows-github-actions-ci.md`): does *any* WinUI 3 app
//! crash at first paint on this GitHub Actions runner (a genuine environment/
//! WinUI 3 limitation — GitHub Actions' `windows-latest` runners have no
//! real GPU), or is it specific to something in `browser-windows-winui`'s own
//! code? Two suspects there, both genuinely unusual code sitting directly in
//! the window's setup/message path, deliberately absent here:
//! - `install_hwnd_subclass`/`subclass_proc`'s raw `WNDPROC` interception —
//!   a workaround for `winio-winui3` bindings gaps (see `lib.rs`'s module doc
//!   comment).
//! - `window.SetExtendsContentIntoTitleBar(true)` + `SetTitleBar(&toolbar)`
//!   (custom title bar) — this app's real trace showed the crash landing
//!   right around `WM_DWMNCRENDERINGCHANGED`/`WM_SETCURSOR`, i.e. exactly
//!   the non-client-area/DWM interaction this feature depends on, which
//!   makes it at least as plausible a suspect as raw GPU absence.
//!
//! Uses the same `trace()` checkpoint logging as the main binary, writing to
//! a separate file so the two don't overwrite each other in the same CI job.

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("minimal-smoke-trace.log") {
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
    window.SetTitle(&windows_core::HSTRING::from("Minimal WinUI 3 smoke test"))?;
    trace("run: after SetTitle");
    window.Activate()?;
    trace("run: after Activate");
    Ok(())
}

#[cfg(not(all(target_os = "windows", target_env = "msvc")))]
fn main() {
    eprintln!(
        "minimal_smoke_test is an MSVC-Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-msvc via `cargo xwin build` (see README.md)."
    );
}
