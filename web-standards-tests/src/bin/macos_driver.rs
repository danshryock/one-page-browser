//! External driver for `browser-macos-appkit`'s web-standards tests — see
//! `windows_driver.rs`'s doc comment for the shared design (same fixture
//! layout, same golden-file diffing via `expected.txt`, same "launch the
//! real app, drive it with real OS input, read its piped stdout" approach).
//! `browser-macos-appkit`'s `on_console_message` wiring (a plain
//! `println!`, matching `browser-windows-reactor`'s) is where the captured
//! `console.log` output on this platform's stdout comes from.
//!
//! Real, honest caveat, same as every other macOS deliverable in this
//! codebase: this only ever gets *cross-compile-verified* from this Linux
//! dev machine (`.cargo/build-macos-appkit.sh`) — nobody has run it against
//! a real window on real hardware. In particular, `content_click_point`'s
//! hardcoded window-bounds guess (see its own doc comment) is the one piece
//! here most likely to need real-hardware calibration, the same way
//! `windows_driver.rs`'s `switcher_button_pos`/`search_box_pos` needed a
//! real VM screenshot session to get right rather than being guessed
//! correctly on the first try.
//!
//! Usage: `web-standards-driver-macos <app path> <fixtures root>`

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    macos_impl::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("web-standards-driver-macos is a macOS-only binary; nothing to run on this platform.");
}

#[cfg(target_os = "macos")]
mod macos_impl {
    use std::path::Path;
    use std::process::{Child, Command, ExitCode, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;

    /// Real macOS virtual keycodes (`Events.h`'s `kVK_*` constants — stable,
    /// hardware-layout-independent for these particular keys) — `core-
    /// graphics` doesn't define named constants for these, only the raw
    /// `CGKeyCode` type.
    const KEYCODE_T: CGKeyCode = 0x11;
    const KEYCODE_RETURN: CGKeyCode = 0x24;

    const WINDOW_WAIT: Duration = Duration::from_secs(5);
    const NAVIGATION_SETTLE: Duration = Duration::from_millis(2000);
    const MESSAGE_WAIT: Duration = Duration::from_secs(10);

    pub fn run() -> ExitCode {
        let args: Vec<String> = std::env::args().collect();
        let (Some(app_exe), Some(fixtures_root)) = (args.get(1), args.get(2)) else {
            eprintln!("usage: web-standards-driver-macos <app path> <fixtures root>");
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

    /// Every immediate subdirectory of `fixtures_root` that has its own
    /// `index.html`, except `shared` (the common popup target page — see
    /// `web-standards-tests/fixtures/`'s layout).
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
        let index_url = format!("file://{}", case_dir.join("index.html").to_string_lossy());

        let mut child = Command::new(app_exe).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let result = drive_and_capture(&mut child, &index_url, &expected);
        let _ = child.kill();
        let _ = child.wait();
        let actual = result?;

        println!("expected: {:?}", expected.trim_end());
        println!("actual:   {:?}", actual.trim_end());
        Ok(actual == expected)
    }

    fn drive_and_capture(child: &mut Child, index_url: &str, expected: &str) -> anyhow::Result<String> {
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

        // A freshly launched app becomes the frontmost/key application on
        // macOS by default (unlike the Windows VM pipeline, no
        // `SetForegroundWindow`-equivalent foreground-lock workaround is
        // needed here) — just a real wait for it to finish starting up.
        std::thread::sleep(WINDOW_WAIT);

        // `Ctrl` in `browser_core::KeyChord` maps to `Cmd` on this platform
        // (see this crate's own module doc comment: "`ctrl` -> Command,
        // `alt` -> Option") — `Cmd+T` is the default `OpenSwitcher`
        // binding. Unlike `browser-windows-reactor`'s `KeyboardAccelerator`
        // (confirmed to not fire while a `WebView2` has focus — see
        // `windows_driver.rs`), AppKit's menu-key-equivalent dispatch
        // routes through the responder chain regardless of which view has
        // first responder status, so no toolbar-button-click workaround is
        // needed here.
        send_key(KEYCODE_T, CGEventFlags::CGEventFlagCommand);
        std::thread::sleep(Duration::from_millis(500));

        send_text(index_url);
        std::thread::sleep(Duration::from_millis(300));
        send_key(KEYCODE_RETURN, CGEventFlags::empty());
        std::thread::sleep(NAVIGATION_SETTLE);

        let (x, y) = content_click_point();
        click_at(x, y);

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

    /// Real, honest limitation: unlike `windows_driver.rs`'s
    /// `switcher_button_pos`/`search_box_pos` (calibrated against an actual
    /// screenshot of a real running window in the Windows VM), there's no
    /// equivalent real-hardware macOS session available in this environment
    /// to calibrate against — this is a plausible guess (a window opened
    /// near the top-left of a typical display, below the menu bar/title
    /// bar, sized similarly to `browser-windows-reactor`'s own observed
    /// default window), not a verified one. The fixture's own link covers a
    /// large fixed 4000x4000px area starting at the page's top-left (see
    /// `web-standards-tests/fixtures/opener-default/index.html`'s doc
    /// comment for why), so this only needs to land *somewhere* inside the
    /// content area below the toolbar, not hit a precise target — but
    /// "somewhere inside the content area" still depends on knowing
    /// roughly where the window is, which is the part that needs real-
    /// hardware verification to get right.
    fn content_click_point() -> (f64, f64) {
        (300.0, 300.0)
    }

    fn click_at(x: f64, y: f64) {
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else { return };
        let point = CGPoint::new(x, y);
        for event_type in [CGEventType::LeftMouseDown, CGEventType::LeftMouseUp] {
            if let Ok(event) = CGEvent::new_mouse_event(source.clone(), event_type, point, CGMouseButton::Left) {
                event.post(CGEventTapLocation::HID);
            }
        }
    }

    fn send_key(keycode: CGKeyCode, flags: CGEventFlags) {
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else { return };
        for key_down in [true, false] {
            if let Ok(event) = CGEvent::new_keyboard_event(source.clone(), keycode, key_down) {
                event.set_flags(flags);
                event.post(CGEventTapLocation::HID);
            }
        }
    }

    /// Injects arbitrary Unicode text as one synthetic keystroke event per
    /// call (`CGEventKeyboardSetUnicodeString`, wrapped by `core-graphics`
    /// as `set_string`) — the macOS equivalent of `windows_driver.rs`'s
    /// `KEYEVENTF_UNICODE` approach, and for the same reason: typing an
    /// arbitrary `file://` URL character-by-character via real keycodes
    /// would need a full keyboard-layout table this driver has no reason to
    /// carry.
    fn send_text(text: &str) {
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else { return };
        let Ok(event) = CGEvent::new_keyboard_event(source, 0, true) else { return };
        event.set_string(text);
        event.post(CGEventTapLocation::HID);
    }
}
