//! Same exact construction order as `exact_order_smoke_test.rs`, but using
//! `browser_core::MemoryHistoryStore` (see `history.rs`'s `HistoryBackend`
//! trait) instead of the real, libsql-backed `HistoryStore`. All eight prior
//! bisection binaries survived cleanly (see
//! `summaries/windows-github-actions-ci.md`), including
//! `historystore_smoke_test`/`exact_order_smoke_test`, which both used the
//! real `HistoryStore` (still real libsql, just `open_in_memory()` to avoid
//! disk I/O). This isolates `libsql` itself — not just the `tokio`/
//! `futures_executor` bridging its `async fn`s — as a variable: `libsql-ffi`
//! statically links a substantial bundled SQLite (plus SQLite3 Multiple
//! Ciphers) into the exe, a real difference from every other bisection
//! binary so far, none of which touched `libsql` at all.

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("memory-history-smoke-trace.log") {
        let _ = writeln!(f, "{msg}");
        let _ = f.sync_all();
    }
}

#[cfg(all(target_os = "windows", target_env = "msvc"))]
const SUBCLASS_ID: usize = 0x8b40_575c;

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
    use browser_core::{HistoryBackend, MemoryHistoryStore};
    use windows::Foundation::Uri;
    use windows::Win32::UI::Shell::SetWindowSubclass;
    use winui3::Microsoft::UI::Xaml::Controls::{Grid, TextBlock, WebView2};

    let _app_instance = winui3::Microsoft::UI::Xaml::Application::new()?;
    trace("run: after Application::new");

    let window = winui3::Microsoft::UI::Xaml::Window::new()?;
    trace("run: after Window::new");
    window.SetTitle(&windows_core::HSTRING::from("Memory-history WinUI 3 smoke test"))?;
    trace("run: after SetTitle");

    let root = Grid::new()?;
    let titlebar = TextBlock::new()?;
    titlebar.SetText(&windows_core::HSTRING::from("Custom title bar"))?;
    root.Children()?.Append(&titlebar)?;
    window.SetContent(&root)?;
    trace("run: after SetContent");
    window.SetExtendsContentIntoTitleBar(true)?;
    trace("run: after SetExtendsContentIntoTitleBar");
    window.SetTitleBar(&titlebar)?;
    trace("run: after SetTitleBar");

    // No libsql anywhere in this call chain — MemoryHistoryStore is pure
    // Rust (a RefCell<Vec<...>>), no SQL, no libsql-ffi.
    let history = MemoryHistoryStore::new();
    trace("run: after MemoryHistoryStore::new");
    history.record_visit("https://example.com", "Example Domain")?;
    history.record_visit("https://example.org", "Example Org")?;
    let results = history.search("example", 10)?;
    trace(&format!("run: after search, found {} entries", results.len()));

    let webview = WebView2::new()?;
    root.Children()?.Append(&webview)?;
    trace("run: after appending WebView2");
    webview.SetSource(&Uri::CreateUri(&windows_core::HSTRING::from("https://example.com"))?)?;
    trace("run: after SetSource");

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
        "memory_history_smoke_test is an MSVC-Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-msvc via `cargo xwin build` (see README.md)."
    );
}
