//! Same as `minimal_smoke_test.rs`, plus *only* a `WebView2` XAML control
//! (`render_engine::WebView2Engine`'s construction, mirrored inline here
//! rather than depending on `render-engine` — see that crate's `winui.rs`)
//! navigated to a real URL — still no HWND subclassing, no custom title bar.
//!
//! `minimal_smoke_test` and `titlebar_smoke_test` both survived cleanly (see
//! `summaries/windows-github-actions-ci.md`), ruling out a bare window and
//! the custom title bar as the real app's crash cause in isolation. `WebView2`
//! is the one remaining major piece of the real window neither test
//! exercises: a much heavier native control than anything tried so far,
//! backed by a real Edge WebView2 process with its own runtime/user-data-
//! folder requirements — a genuinely plausible independent crash source this
//! debugging pass hadn't considered until ruling out the other two.

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("webview2-smoke-trace.log") {
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
    use windows::Foundation::Uri;
    use winui3::Microsoft::UI::Xaml::Controls::{Grid, WebView2};

    let _app_instance = winui3::Microsoft::UI::Xaml::Application::new()?;
    trace("run: after Application::new");

    let window = winui3::Microsoft::UI::Xaml::Window::new()?;
    trace("run: after Window::new");
    window.SetTitle(&windows_core::HSTRING::from("WebView2 WinUI 3 smoke test"))?;
    trace("run: after SetTitle");

    let grid = Grid::new()?;
    trace("run: after Grid::new");
    window.SetContent(&grid)?;
    trace("run: after SetContent");

    let webview = WebView2::new()?;
    trace("run: after WebView2::new");
    grid.Children()?.Append(&webview)?;
    trace("run: after appending WebView2 to grid");
    webview.SetSource(&Uri::CreateUri(&windows_core::HSTRING::from("https://example.com"))?)?;
    trace("run: after SetSource");

    window.Activate()?;
    trace("run: after Activate");
    Ok(())
}

#[cfg(not(all(target_os = "windows", target_env = "msvc")))]
fn main() {
    eprintln!(
        "webview2_smoke_test is an MSVC-Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-msvc via `cargo xwin build` (see README.md)."
    );
}
