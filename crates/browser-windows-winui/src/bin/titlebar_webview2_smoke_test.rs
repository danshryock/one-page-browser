//! Combination test: `SetExtendsContentIntoTitleBar` + `SetTitleBar` (custom
//! title bar) *and* an embedded `WebView2` control together — still no HWND
//! subclassing. All four of `minimal_smoke_test`, `titlebar_smoke_test`,
//! `webview2_smoke_test`, and `subclass_smoke_test` survived individually
//! (see `summaries/windows-github-actions-ci.md`) — even
//! `subclass_smoke_test` received the exact same message sequence through
//! `WM_SETCURSOR` (twice) that the real app crashes at, and kept running.
//! Since no single piece reproduces it, this tests the specific combination
//! most likely to interact badly: a `WebView2` surface rendering underneath/
//! near a custom-drawn, DWM-extended title bar region is a real, previously
//! documented tricky pairing for WinUI 3 apps in general (independent of
//! this CI environment).

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("titlebar-webview2-smoke-trace.log") {
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
    use winui3::Microsoft::UI::Xaml::Controls::{Grid, TextBlock, WebView2};

    let _app_instance = winui3::Microsoft::UI::Xaml::Application::new()?;
    trace("run: after Application::new");

    let window = winui3::Microsoft::UI::Xaml::Window::new()?;
    trace("run: after Window::new");
    window.SetTitle(&windows_core::HSTRING::from("Titlebar+WebView2 WinUI 3 smoke test"))?;
    trace("run: after SetTitle");

    let root = Grid::new()?;
    trace("run: after Grid::new");

    let titlebar = TextBlock::new()?;
    titlebar.SetText(&windows_core::HSTRING::from("Custom title bar"))?;
    root.Children()?.Append(&titlebar)?;
    trace("run: after appending titlebar TextBlock");

    let webview = WebView2::new()?;
    root.Children()?.Append(&webview)?;
    trace("run: after appending WebView2");
    webview.SetSource(&Uri::CreateUri(&windows_core::HSTRING::from("https://example.com"))?)?;
    trace("run: after SetSource");

    window.SetContent(&root)?;
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
        "titlebar_webview2_smoke_test is an MSVC-Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-msvc via `cargo xwin build` (see README.md)."
    );
}
