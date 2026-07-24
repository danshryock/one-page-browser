//! Same as `minimal_smoke_test.rs`, plus *only* `SetWindowSubclass`-based
//! `WNDPROC` interception on the window's `HWND` — the technique
//! `lib.rs`'s `install_hwnd_subclass`/`subclass_proc` uses as a workaround
//! for `winio-winui3`'s missing `KeyDown`/`Window::Closed` delegates (see
//! that module's doc comment) — still no custom title bar, no `WebView2`.
//! Reimplemented standalone here rather than calling the real
//! `install_hwnd_subclass` (private to `lib.rs`, not exposed to a separate
//! `src/bin/` binary crate), but the shape is the same: subclass the raw
//! `HWND`, forward every message to `DefSubclassProc` unmodified, reclaim
//! the boxed state on `WM_NCDESTROY`.
//!
//! `minimal_smoke_test`, `titlebar_smoke_test`, and `webview2_smoke_test`
//! all survived cleanly (see `summaries/windows-github-actions-ci.md`) —
//! this is the last of the real window's genuinely unusual pieces of code
//! left untested in isolation.

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("subclass-smoke-trace.log") {
        let _ = writeln!(f, "{msg}");
        let _ = f.sync_all();
    }
}

#[cfg(all(target_os = "windows", target_env = "msvc"))]
const SUBCLASS_ID: usize = 0x8b40_5759;

#[cfg(all(target_os = "windows", target_env = "msvc"))]
unsafe extern "system" fn subclass_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::WM_NCDESTROY;

    trace(&format!("subclass_proc: msg=0x{msg:04X} wparam={} lparam={}", wparam.0, lparam.0));

    if msg == WM_NCDESTROY {
        let _ = unsafe { Box::from_raw(ref_data as *mut ()) };
        let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
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
    use windows::Win32::UI::Shell::SetWindowSubclass;

    let _app_instance = winui3::Microsoft::UI::Xaml::Application::new()?;
    trace("run: after Application::new");

    let window = winui3::Microsoft::UI::Xaml::Window::new()?;
    trace("run: after Window::new");
    window.SetTitle(&windows_core::HSTRING::from("Subclass WinUI 3 smoke test"))?;
    trace("run: after SetTitle");

    let native = windows_core::Interface::cast::<winui3::IWindowNative>(&window)?;
    let hwnd = unsafe { native.WindowHandle()? };
    trace("run: after getting HWND");
    let state: Box<()> = Box::new(());
    let ref_data = Box::into_raw(state) as usize;
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, ref_data);
    }
    trace("run: after SetWindowSubclass");

    window.Activate()?;
    trace("run: after Activate");
    Ok(())
}

#[cfg(not(all(target_os = "windows", target_env = "msvc")))]
fn main() {
    eprintln!(
        "subclass_smoke_test is an MSVC-Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-msvc via `cargo xwin build` (see README.md)."
    );
}
