//! External driver for `browser-windows-reactor`'s web-standards tests — see
//! `web-standards-tests/fixtures/`'s own layout and this repo's `ROADMAP.md`
//! for why an external driver exists at all for this platform (no
//! in-process test harness, unlike `browser-linux-gtk3`'s `tests/
//! gtk_tests.rs`).
//!
//! Launches the real app (no `--url` — that routes through the profile
//! chooser window on every platform, see each `main.rs` directly), opens
//! the switcher via the default `Ctrl+T` (`browser_core::keybindings`'s
//! `Action::OpenSwitcher` default binding), types a fixture's `file://`
//! URL, presses Enter, sends one real `SendInput` click, and reads whatever
//! the fixture page's own `console.log` output the app relays to stdout
//! (via `browser_windows_reactor::CONSOLE_CAPTURE_SCRIPT`'s
//! `on_web_message_received` -> `println!` wiring — real, standard
//! `console.log` content, no custom test-only line format) — diffed against
//! `expected.txt`, same golden-file convention as `browser-linux-gtk3`'s
//! test.
//!
//! Usage: `web-standards-driver-windows.exe <app.exe path> <fixtures root>`
//! Runs every fixture case under `<fixtures root>` (each subdirectory with
//! an `index.html`, except `shared`) in turn; exits 0 only if every case
//! passes.

#[cfg(target_os = "windows")]
fn main() -> std::process::ExitCode {
    windows_impl::run()
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("web-standards-driver-windows is a Windows-only binary; nothing to run on this platform.");
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::path::Path;
    use std::process::{Child, Command, ExitCode, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN,
        MOUSEEVENTF_LEFTUP, MOUSEINPUT, VIRTUAL_KEY, VK_MENU, VK_RETURN,
    };
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowRect, SetCursorPos, SetForegroundWindow, SetWindowPos, HWND_TOP, SWP_NOMOVE, SWP_NOSIZE};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, SRCCOPY,
    };

    /// How long to wait for the app's window to appear after launch, for
    /// the switcher/URL bar to reflect the typed navigation, and for the
    /// popup's `console.log` to arrive after the click — all generous on
    /// purpose: this only costs real time when a step genuinely never
    /// happens (a real failure), and `vm_run`'s own 60s ceiling (see
    /// `scripts/windows-vm/lib.sh`) is comfortably above the sum of these
    /// even run twice (one per fixture case).
    const WINDOW_WAIT: Duration = Duration::from_secs(5);
    const NAVIGATION_SETTLE: Duration = Duration::from_millis(1500);
    const MESSAGE_WAIT: Duration = Duration::from_secs(15);

    pub fn run() -> ExitCode {
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).map(String::as_str) == Some("--calibrate") {
            let Some(app_exe) = args.get(2) else {
                eprintln!("usage: web-standards-driver-windows.exe --calibrate <app.exe path>");
                return ExitCode::FAILURE;
            };
            return calibrate(app_exe);
        }
        let (Some(app_exe), Some(fixtures_root)) = (args.get(1), args.get(2)) else {
            eprintln!("usage: web-standards-driver-windows.exe <app.exe path> <fixtures root>");
            return ExitCode::FAILURE;
        };

        let cases = match discover_cases(Path::new(fixtures_root)) {
            Ok(cases) if !cases.is_empty() => cases,
            Ok(_) => {
                eprintln!("error: no fixture cases found under {fixtures_root}");
                return ExitCode::FAILURE;
            }
            Err(err) => {
                eprintln!("error: failed to list fixture cases under {fixtures_root}: {err}");
                return ExitCode::FAILURE;
            }
        };

        let mut all_passed = true;
        for case in &cases {
            println!("== running case: {case} ==");
            match run_case(app_exe, fixtures_root, case) {
                Ok(true) => println!("-- {case}: PASS --"),
                Ok(false) => {
                    println!("-- {case}: FAIL --");
                    all_passed = false;
                }
                Err(err) => {
                    println!("-- {case}: FAIL (error: {err}) --");
                    all_passed = false;
                }
            }
        }

        if all_passed {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }

    /// Launches the app, clicks where `switcher_button_pos` thinks the
    /// toolbar's switcher button is, and idles for 20s so an external
    /// screenshot (`scripts/windows-vm/screenshot.sh`) can be taken mid-run
    /// to visually confirm/calibrate click coordinates — not part of the
    /// normal test flow, a standalone diagnostic entry point only.
    fn calibrate(app_exe: &str) -> ExitCode {
        let mut child = match Command::new(app_exe).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(c) => c,
            Err(err) => {
                eprintln!("failed to spawn {app_exe}: {err}");
                return ExitCode::FAILURE;
            }
        };
        let hwnd = match wait_for_window(WINDOW_WAIT) {
            Ok(h) => h,
            Err(err) => {
                eprintln!("{err}");
                let _ = child.kill();
                return ExitCode::FAILURE;
            }
        };
        raise_and_focus(hwnd);
        std::thread::sleep(Duration::from_millis(300));
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(hwnd, &mut rect);
        }
        println!("window rect: left={} top={} right={} bottom={}", rect.left, rect.top, rect.right, rect.bottom);
        let (sx, sy) = switcher_button_pos(&rect);
        println!("clicking switcher button at ({sx}, {sy})");
        click_at(sx, sy);
        println!("sleeping 20s for an external screenshot...");
        std::thread::sleep(Duration::from_secs(20));
        let _ = child.kill();
        let _ = child.wait();
        ExitCode::SUCCESS
    }

    /// Every immediate subdirectory of `fixtures_root` that has its own
    /// `index.html`, except `shared` (the common popup target page, not a
    /// case of its own — see `web-standards-tests/fixtures/`'s layout).
    fn discover_cases(fixtures_root: &Path) -> std::io::Result<Vec<String>> {
        let mut cases = Vec::new();
        for entry in std::fs::read_dir(fixtures_root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "shared" {
                continue;
            }
            if entry.path().join("index.html").is_file() {
                cases.push(name);
            }
        }
        cases.sort();
        Ok(cases)
    }

    fn run_case(app_exe: &str, fixtures_root: &str, case: &str) -> anyhow::Result<bool> {
        let case_dir = Path::new(fixtures_root).join(case);
        let expected = std::fs::read_to_string(case_dir.join("expected.txt"))?;

        let mut child = Command::new(app_exe).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let result = drive_and_capture(&mut child, case, &expected);
        let _ = child.kill();
        let _ = child.wait();
        let actual = result?;

        println!("expected: {:?}", expected.trim_end());
        println!("actual:   {:?}", actual.trim_end());
        Ok(actual == expected)
    }

    /// `case` (e.g. `"opener-default"`) is typed into the switcher's search
    /// box as a query matching an *already-open* page's title/URL — not the
    /// page's full `file://` URL. Switching to an already-open page (via
    /// `switch_to`, the same code path a real user re-selecting an existing
    /// tab exercises) is what's driven here, not creating a brand new one
    /// (`do_add_page`): a real, separate gap confirmed by direct testing —
    /// a freshly created page's XAML visibility toggle doesn't reliably
    /// take effect on the very first render it becomes active in, leaving
    /// whatever page was previously showing still the one that's actually
    /// visible and receiving clicks (confirmed with a screenshot: the click
    /// meant for the fixture landed on the still-visible previous page).
    /// Requires the caller to have pre-seeded every fixture case as its own
    /// already-open page in `session.json` before launching the app (see
    /// `scripts/windows-vm/build-and-test.sh`'s seeding step) — with each
    /// case open from the start, this only ever needs to *switch to* one,
    /// the mechanism that's actually reliable.
    fn drive_and_capture(child: &mut Child, case: &str, expected: &str) -> anyhow::Result<String> {
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("child had no stdout"))?;
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        let hwnd = wait_for_window(WINDOW_WAIT)?;
        println!("checkpoint: window found");
        raise_and_focus(hwnd);
        std::thread::sleep(Duration::from_millis(500));

        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect)? };
        println!("checkpoint: rect = {},{} - {},{}", rect.left, rect.top, rect.right, rect.bottom);

        // Not `Ctrl+T` (the default `OpenSwitcher` keybinding): confirmed by
        // direct testing that it never reaches the app at all here — a
        // real, already-documented gap in `browser-windows-reactor`'s own
        // `shortcuts.rs` ("accelerators don't fire while focus is inside
        // WebView2"), and the freshly launched window's `WebView2` control
        // is exactly what has focus at this point. The toolbar's switcher
        // button (the "⊞" icon) is a plain XAML `Button` clicked directly
        // instead — not subject to that gap at all.
        let (switcher_x, switcher_y) = switcher_button_pos(&rect);
        click_at(switcher_x, switcher_y);
        println!("checkpoint: clicked switcher button at ({switcher_x}, {switcher_y})");
        std::thread::sleep(Duration::from_millis(2000));

        // The switcher's search box doesn't get programmatic focus either
        // (`open_switcher_editing_url`'s doc comment: no `Focus()`-style API
        // exists on `windows-reactor`'s `TextBox` — a real, separate,
        // already-documented gap from the accelerator one above) — clicking
        // it directly is what actually gives it focus, the same as any
        // text box in any UI framework.
        let (search_x, search_y) = search_box_pos(&rect);
        click_at(search_x, search_y);
        println!("checkpoint: clicked search box at ({search_x}, {search_y})");
        std::thread::sleep(Duration::from_millis(400));

        send_text(case);
        println!("checkpoint: typed {case:?}");
        std::thread::sleep(Duration::from_millis(300));
        if let Ok(dir) = std::env::var("WEB_STANDARDS_DEBUG_SCREENSHOT_DIR") {
            let path = Path::new(&dir).join("before-enter.bmp");
            match save_window_screenshot(hwnd, &path) {
                Ok(()) => println!("checkpoint: saved screenshot to {path:?}"),
                Err(err) => println!("checkpoint: screenshot failed: {err}"),
            }
        }
        send_key(VK_RETURN);
        println!("checkpoint: sent Enter");
        std::thread::sleep(NAVIGATION_SETTLE);

        // Real, separate gap from the deferred-script one `xaml_interop::
        // defer_to_next_tick` fixes: a newly-activated page's XAML
        // visibility toggle doesn't reliably apply on the very first render
        // after switching — confirmed genuinely *variable* by direct
        // testing (a fixed delay before one click passed on one run, then
        // failed identically the next), matching this codebase's own
        // documented note on `title_changed` ("doesn't reliably produce a
        // new render on its own... until some unrelated, genuinely
        // UI-thread-originated event... forced the next render, which then
        // picked up the already-correct value"). Since the exact number of
        // renders needed varies, this retries the whole
        // nudge-then-click-content sequence a few times, each with its own
        // short wait for the popup's console.log to arrive, rather than
        // gambling everything on one fixed-timing attempt.
        // Nudges once (see this block's earlier history for why: a real,
        // pre-existing windows-reactor gap where a newly-activated page's
        // XAML visibility toggle doesn't reliably apply on the render
        // immediately after switching — matching this codebase's own
        // documented note on `title_changed` needing "some unrelated,
        // genuinely UI-thread-originated event" to force the next render).
        // Deliberately only ONE click on the content area, not several:
        // confirmed by direct testing that *re*-clicking while a popup from
        // an earlier click in the same run is still being created interferes
        // rather than helps (a second click can land on/close the very
        // popup the first one just opened) — a single click, backed by a
        // generous wait below for whatever render count it actually takes
        // to settle, is more reliable than retrying the click itself.
        click_at(switcher_x, switcher_y);
        std::thread::sleep(Duration::from_millis(300));
        click_at(switcher_x, switcher_y);
        std::thread::sleep(Duration::from_millis(2000));

        click_window_content(&rect);
        println!("checkpoint: clicked content area");
        if let Ok(dir) = std::env::var("WEB_STANDARDS_DEBUG_SCREENSHOT_DIR") {
            std::thread::sleep(Duration::from_millis(300));
            let path = Path::new(&dir).join("after-click.bmp");
            let _ = save_window_screenshot(hwnd, &path);
        }

        let mut collected = String::new();
        let deadline = Instant::now() + MESSAGE_WAIT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(line) => {
                    collected.push_str(&line);
                    collected.push('\n');
                    if collected == expected {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        Ok(collected)
    }

    fn wait_for_window(timeout: Duration) -> anyhow::Result<HWND> {
        let title: Vec<u16> = "Claude Browser\0".encode_utf16().collect();
        let deadline = Instant::now() + timeout;
        loop {
            let hwnd = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) }.unwrap_or_default();
            if !hwnd.is_invalid() && hwnd.0 != std::ptr::null_mut() {
                return Ok(hwnd);
            }
            if Instant::now() >= deadline {
                anyhow::bail!("app window ('Claude Browser') never appeared within {timeout:?}");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Calibrated (see this crate's own calibration session — a real
    /// `browser-windows-reactor` window launched in the VM, screenshotted,
    /// measured directly against `TitleBar::new(APP_TITLE).content(toolbar)`'s
    /// known column layout in `lib.rs`) against this app's default,
    /// freshly-launched window size — offsets from the window's own
    /// top-left corner, not absolute screen coordinates, so this stays
    /// correct regardless of where the OS happens to place the window.
    fn switcher_button_pos(rect: &RECT) -> (i32, i32) {
        (rect.left + 312, rect.top + 25)
    }

    /// Same calibration session as `switcher_button_pos` — measured
    /// directly against a real screenshot of the opened switcher overlay
    /// (the search box's placeholder "Type to filter open pages…" spanned
    /// roughly x:[440,838] y:[214,244] in that screenshot, against this
    /// same window rect), comfortably inside its bounds.
    fn search_box_pos(rect: &RECT) -> (i32, i32) {
        (rect.left + 327, rect.top + 81)
    }

    /// Real, empirically-confirmed gap: a plain `SetForegroundWindow` call
    /// from this process (spawned as a background child of the VM's own
    /// `poll.bat`/`cmd.exe`, never itself the OS's "active" application)
    /// silently fails — the browser window's `GetWindowRect` still returns
    /// a valid rect, but a screenshot taken right after shows the launching
    /// `cmd.exe` console window still on top, covering it, so a click at
    /// any computed coordinate lands on the console instead. Sending a fake
    /// `Alt` key tap first is a well-known, documented Win32 workaround —
    /// Windows' foreground-lock timeout (the mechanism that normally
    /// blocks a background process from stealing focus) resets whenever a
    /// real `Alt` keypress is observed, since `Alt` is the menu-activation
    /// key and letting *that* through unconditionally is what real window
    /// managers rely on for menu keyboard navigation to work at all.
    /// `SetWindowPos(..., HWND_TOP, ...)` additionally forces the z-order
    /// directly — a real belt-and-suspenders fix, not just the accepted
    /// workaround, since z-order (what a click actually lands on) and
    /// input focus (what a keypress actually reaches) are two genuinely
    /// separate pieces of window state that can disagree.
    fn raise_and_focus(hwnd: HWND) {
        key_down(VK_MENU);
        key_up(VK_MENU);
        unsafe {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        }
    }

    /// Diagnostic-only: captures `hwnd`'s current appearance to a plain,
    /// uncompressed 24bpp BMP file at `path` — used during development to
    /// visually calibrate `switcher_button_pos`/`search_box_pos` against a
    /// real running window (an external QEMU-monitor-driven screenshot, the
    /// other tool available for this, has no way to land at a precise
    /// moment inside this driver's own timing, so this exists to take one
    /// deterministically instead). Not part of the pass/fail test flow —
    /// safe to leave in as a standing capability for diagnosing a future
    /// calibration drift (e.g. after a toolbar layout change) without
    /// needing to reinvent this.
    fn save_window_screenshot(hwnd: HWND, path: &Path) -> anyhow::Result<()> {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect)? };
        let width = (rect.right - rect.left).max(1);
        let height = (rect.bottom - rect.top).max(1);
        let mut buffer = vec![0u8; 0];
        unsafe {
            let hdc_screen = GetDC(None);
            let hdc_mem = CreateCompatibleDC(Some(hdc_screen));
            let hbitmap = CreateCompatibleBitmap(hdc_screen, width, height);
            let old = SelectObject(hdc_mem, hbitmap.into());
            // Copies straight off the screen DC at the window's on-screen
            // rect (not `PrintWindow`, unavailable in this crate version's
            // `windows` bindings) — equivalent as long as the window is
            // actually visible/on top, same assumption `raise_and_focus`
            // already establishes before every call site.
            let _ = BitBlt(hdc_mem, 0, 0, width, height, Some(hdc_screen), rect.left, rect.top, SRCCOPY);

            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = width;
            bmi.bmiHeader.biHeight = height; // positive: bottom-up, standard BMP row order
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 24;
            bmi.bmiHeader.biCompression = BI_RGB.0 as u32;

            let row_size = ((width * 3 + 3) / 4) * 4;
            buffer = vec![0u8; (row_size * height) as usize];
            GetDIBits(hdc_mem, hbitmap, 0, height as u32, Some(buffer.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS);

            SelectObject(hdc_mem, old);
            let _ = DeleteObject(hbitmap.into());
            let _ = DeleteDC(hdc_mem);
            ReleaseDC(None, hdc_screen);
        }

        let pixel_offset: u32 = 14 + 40;
        let file_size = pixel_offset + buffer.len() as u32;
        let mut out = Vec::with_capacity(file_size as usize);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&file_size.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&pixel_offset.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(buffer.len() as u32).to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&buffer);
        std::fs::write(path, out)?;
        Ok(())
    }

    fn click_at(x: i32, y: i32) {
        unsafe {
            let _ = SetCursorPos(x, y);
        }
        std::thread::sleep(Duration::from_millis(150));
        send_mouse_click();
    }

    fn click_window_content(rect: &RECT) {
        // Roughly centered, biased slightly down from the top so this never
        // lands on the toolbar strip regardless of its exact height — the
        // fixture's own link covers a large fixed pixel area (see
        // `web-standards-tests/fixtures/opener-default/index.html`'s doc
        // comment for why a precise DOM-element target isn't otherwise
        // reachable from a synthetic-input driver), so any point well
        // inside the content area hits it.
        let x = (rect.left + rect.right) / 2;
        let y = rect.top + (rect.bottom - rect.top) * 2 / 3;
        click_at(x, y);
    }

    fn send_key(vk: VIRTUAL_KEY) {
        key_down(vk);
        key_up(vk);
    }

    fn key_down(vk: VIRTUAL_KEY) {
        send_keybd_input(vk, 0, windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0));
    }

    fn key_up(vk: VIRTUAL_KEY) {
        send_keybd_input(vk, 0, KEYEVENTF_KEYUP);
    }

    fn send_text(text: &str) {
        for ch in text.encode_utf16() {
            send_keybd_input(VIRTUAL_KEY(0), ch, KEYEVENTF_UNICODE);
            send_keybd_input(VIRTUAL_KEY(0), ch, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
        }
    }

    fn send_keybd_input(vk: VIRTUAL_KEY, scan: u16, flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS) {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
            },
        };
        unsafe {
            SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        }
    }

    fn send_mouse_click() {
        for flags in [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP] {
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
                },
            };
            unsafe {
                SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
            }
        }
    }
}
