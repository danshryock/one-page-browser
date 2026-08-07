//! External driver for `browser-windows-reactor`'s web-standards tests — see
//! `web-standards-tests/fixtures/`'s own layout and this repo's `ROADMAP.md`
//! for why an external driver exists at all for this platform (no
//! in-process test harness, unlike `browser-linux-gtk3`'s `tests/
//! gtk_tests.rs`).
//!
//! Launches the real app (no `--url` — that routes through the profile
//! chooser window on every platform, see each `main.rs` directly), switches
//! to a pre-seeded fixture page (see `drive_and_capture`'s doc comment for
//! why an already-open page, not a fresh navigation), then drives whatever
//! interactions `fixtures_root/<case>/actions.json` describes (currently
//! just `{"action": "click", "target": "<name>"}` steps) before reading the
//! fixture page's own `console.log` output the app relays to stdout (via
//! `browser_windows_reactor::CONSOLE_CAPTURE_SCRIPT`'s
//! `on_web_message_received` -> `println!` wiring — real, standard
//! `console.log` content, no custom test-only line format) — diffed against
//! `expected.txt`, same golden-file convention as `browser-linux-gtk3`'s
//! test.
//!
//! A `click` step's on-screen target point comes from the same
//! `__test_target__` mechanism `browser-linux-gtk3`'s in-process test uses:
//! `CONSOLE_CAPTURE_SCRIPT` reports every `[data-test-target]` element's
//! `getBoundingClientRect()` via `console.log('__test_target__ <name>
//! <rect-json>')` on page load, riding the exact same stdout-relay channel
//! already read for the real assertion — this driver has no separate RPC
//! channel into the app to ask for element positions directly, so reusing
//! the one channel that already exists end-to-end is the only option that
//! doesn't require inventing a new one. Those `__test_target__` lines are
//! filtered out before comparing captured output against `expected.txt`.
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
        let steps = read_actions(&case_dir)?;

        let mut child = Command::new(app_exe).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let result = drive_and_capture(&mut child, case, &steps, &expected);
        let _ = child.kill();
        let _ = child.wait();
        let actual = result?;

        println!("expected: {:?}", expected.trim_end());
        println!("actual:   {:?}", actual.trim_end());
        Ok(actual == expected)
    }

    /// One `{"action": "click", "target": "<name>"}` entry from a fixture's
    /// `actions.json` — see this crate's top-of-file doc comment.
    struct Step {
        action: String,
        target: String,
    }

    fn read_actions(case_dir: &Path) -> anyhow::Result<Vec<Step>> {
        let text = std::fs::read_to_string(case_dir.join("actions.json"))?;
        let parsed: serde_json::Value = serde_json::from_str(&text)?;
        let steps = parsed["steps"].as_array().ok_or_else(|| anyhow::anyhow!("actions.json should have a \"steps\" array"))?;
        steps
            .iter()
            .map(|step| {
                let action = step["action"].as_str().ok_or_else(|| anyhow::anyhow!("actions.json step missing \"action\""))?;
                let target = step["target"].as_str().ok_or_else(|| anyhow::anyhow!("actions.json step missing \"target\""))?;
                Ok(Step { action: action.to_string(), target: target.to_string() })
            })
            .collect()
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
    /// Reads `reader` line-by-line into the returned channel until it hits
    /// EOF (the child closed the stream — normal on exit) or a read error.
    /// Deliberately *not* `std::io::BufRead::lines()`: that yields an `Err`
    /// for any line that isn't valid UTF-8, and `.map_while(Result::ok)`
    /// (this file's original approach) treats the first such `Err` as the
    /// end of the whole iterator — silently abandoning the rest of a still-
    /// running child's output after one stray non-UTF-8 byte sequence
    /// rather than actually meaning the child exited. `String::from_utf8_lossy`
    /// degrades one bad line instead of ending the stream.
    fn spawn_line_reader<R: std::io::Read + Send + 'static>(reader: R) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let mut reader = std::io::BufReader::new(reader);
            loop {
                let mut buf = Vec::new();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        if buf.last() == Some(&b'\n') {
                            buf.pop();
                        }
                        if buf.last() == Some(&b'\r') {
                            buf.pop();
                        }
                        if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        rx
    }

    fn drive_and_capture(child: &mut Child, case: &str, steps: &[Step], expected: &str) -> anyhow::Result<String> {
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("child had no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow::anyhow!("child had no stderr"))?;
        let rx = spawn_line_reader(stdout);
        // Relayed straight to our own stderr (prefixed, so it's easy to
        // pick out in CI/VM logs) rather than dropped — the channel a real
        // app crash/panic would otherwise vanish into.
        let stderr_rx = spawn_line_reader(stderr);
        std::thread::spawn(move || {
            for line in stderr_rx {
                eprintln!("[app stderr] {line}");
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
        // picked up the already-correct value"). Nudges once (clicking the
        // switcher button open-then-shut again) before proceeding, the same
        // workaround this block has always used.
        click_at(switcher_x, switcher_y);
        std::thread::sleep(Duration::from_millis(300));
        click_at(switcher_x, switcher_y);
        std::thread::sleep(Duration::from_millis(2000));

        let mut captured: Vec<String> = Vec::new();
        for step in steps {
            match step.action.as_str() {
                "click" => {
                    let (x, y) = resolve_target_point(&rx, &mut captured, &step.target, &rect)?;
                    // Deliberately only one click per target, not several:
                    // confirmed by direct testing that *re*-clicking while a
                    // popup from an earlier click in the same run is still
                    // being created interferes rather than helps (a second
                    // click can land on/close the very popup the first one
                    // just opened).
                    click_at(x, y);
                    println!("checkpoint: clicked {:?} at ({x}, {y})", step.target);
                }
                other => anyhow::bail!("{case}: unknown actions.json action {other:?}"),
            }
        }
        if let Ok(dir) = std::env::var("WEB_STANDARDS_DEBUG_SCREENSHOT_DIR") {
            std::thread::sleep(Duration::from_millis(300));
            let path = Path::new(&dir).join("after-click.bmp");
            let _ = save_window_screenshot(hwnd, &path);
        }

        let deadline = Instant::now() + MESSAGE_WAIT;
        loop {
            if real_assertion_lines(&captured) == expected {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(line) => captured.push(line),
                Err(_) => break,
            }
        }
        Ok(real_assertion_lines(&captured))
    }

    /// `captured`'s lines with any `__test_target__ ...` coordinate reports
    /// (see this file's top-of-file doc comment) filtered out, joined back
    /// into the same newline-terminated shape `expected.txt` uses — the real
    /// page console output a fixture's own assertion actually checks.
    fn real_assertion_lines(captured: &[String]) -> String {
        let joined: String = captured.iter().filter(|line| !line.starts_with("__test_target__ ")).flat_map(|line| [line.as_str(), "\n"]).collect();
        joined
    }

    /// Waits for a `__test_target__ <target> <rect-json>` line (already
    /// received into `captured`, or arriving on `rx` before `MESSAGE_WAIT`
    /// elapses) and resolves it to a real screen point: the app window's
    /// content-area origin (see `content_area_origin`) plus the reported
    /// rect's center.
    fn resolve_target_point(rx: &mpsc::Receiver<String>, captured: &mut Vec<String>, target: &str, rect: &RECT) -> anyhow::Result<(i32, i32)> {
        let prefix = format!("__test_target__ {target} ");
        let deadline = Instant::now() + MESSAGE_WAIT;
        loop {
            if let Some(rect_json) = captured.iter().find_map(|line| line.strip_prefix(prefix.as_str())) {
                let parsed: serde_json::Value = serde_json::from_str(rect_json)?;
                let rx_ = parsed["x"].as_f64().ok_or_else(|| anyhow::anyhow!("__test_target__ rect missing x"))?;
                let ry_ = parsed["y"].as_f64().ok_or_else(|| anyhow::anyhow!("__test_target__ rect missing y"))?;
                let rw = parsed["width"].as_f64().ok_or_else(|| anyhow::anyhow!("__test_target__ rect missing width"))?;
                let rh = parsed["height"].as_f64().ok_or_else(|| anyhow::anyhow!("__test_target__ rect missing height"))?;
                let (origin_x, origin_y) = content_area_origin(rect);
                return Ok((origin_x + (rx_ + rw / 2.0) as i32, origin_y + (ry_ + rh / 2.0) as i32));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("no __test_target__ report for {target:?} arrived within {MESSAGE_WAIT:?}");
            }
            match rx.recv_timeout(remaining) {
                Ok(line) => captured.push(line),
                Err(_) => anyhow::bail!("child stdout ended before a __test_target__ report for {target:?} arrived"),
            }
        }
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

    /// The app window's content area (below the toolbar strip) top-left
    /// corner, in absolute screen coordinates — a `__test_target__` rect is
    /// reported by the page in CSS/viewport coordinates (`getBoundingClient
    /// Rect()`, relative to the content area's own top-left), so this is
    /// the offset needed to turn one into a real screen point. `+ 47`: the
    /// toolbar's known height from this session's own calibration
    /// screenshots, the same session `switcher_button_pos`/`search_box_pos`
    /// were calibrated against.
    fn content_area_origin(rect: &RECT) -> (i32, i32) {
        (rect.left, rect.top + 47)
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
