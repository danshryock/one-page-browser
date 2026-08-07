//! External driver for `browser-macos-appkit`'s web-standards tests — see
//! `windows_driver.rs`'s doc comment for the shared design (same fixture
//! layout, same golden-file diffing via `expected.txt`, same "launch the
//! real app, drive it, read its piped stdout" approach). `browser-macos-
//! appkit`'s `on_console_message` wiring (a plain `println!`, matching
//! `browser-windows-reactor`'s) is where the captured `console.log` output
//! on this platform's stdout comes from.
//!
//! Unlike `windows_driver.rs`/`browser-linux-gtk3`'s in-process test, this
//! does **not** drive the app via OS-level synthetic input by default —
//! `browser-macos-appkit`'s own `AppState::start_test_command_listener`
//! (see its doc comment) is used instead, over a Unix domain socket passed
//! via `--test-command-socket`. This exists because the CGEvent-based
//! approach (still available, see `CLICK_MODE`/`click_at` below) needs
//! Accessibility/Input Monitoring TCC permission, and confirmed directly
//! against real hardware (a 2014 MacBook, `scripts/macos-mac/`): that
//! permission cannot be granted non-interactively over SSH on this macOS
//! version — not just "not yet configured," genuinely not scriptable, down
//! to the private-key operations needed for even a *stable* signing
//! identity requiring a live GUI session for every use, not just the first.
//! The socket-based command channel sidesteps the whole problem: no
//! synthetic OS input, no TCC dependency at all.
//!
//! Per-fixture interactions still come from `fixtures_root/<case>/
//! actions.json` (`{"action": "click", "target": "<name>"}` steps) — only
//! *how* a `click` step is carried out changed. By default it's sent as a
//! `click_js <target>` command (the app runs `document.querySelector(...)
//! .click()` against the real page via its own script-evaluation channel —
//! not "trusted" in the DOM sense, but this only ever matters for
//! `window.open()`'s popup-blocking heuristics, not for a real `<a
//! target="_blank">` navigation like these fixtures use, which browsers
//! have always followed identically regardless of how the click was
//! triggered). Set `WEB_STANDARDS_MACOS_CLICK_MODE=native` to fall back to
//! the original CGEvent-based real click instead, for direct comparison —
//! that path still needs Accessibility permission and the
//! `__test_target__`/`content_area_origin` machinery below.
//!
//! Real, honest caveat, same as every other macOS deliverable in this
//! codebase: `content_area_origin`'s hardcoded window-bounds guess (used
//! only by `native` click mode) has still never been calibrated against a
//! real screenshot, unlike `windows_driver.rs`'s `switcher_button_pos`/
//! `search_box_pos`.
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
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::process::{Child, Command, ExitCode, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;

    /// Real macOS virtual keycodes (`Events.h`'s `kVK_*` constants) — only
    /// used by `native` click mode's fallback navigation (see this file's
    /// top-of-file doc comment); the default command-socket path needs
    /// none of this.
    const KEYCODE_T: CGKeyCode = 0x11;
    const KEYCODE_RETURN: CGKeyCode = 0x24;
    const KEYCODE_COMMAND: CGKeyCode = 0x37;

    const WINDOW_WAIT: Duration = Duration::from_secs(5);
    const SOCKET_CONNECT_WAIT: Duration = Duration::from_secs(5);
    const NAVIGATION_SETTLE: Duration = Duration::from_millis(2000);
    const MESSAGE_WAIT: Duration = Duration::from_secs(10);

    /// `native` opts into the original CGEvent-based real click (see this
    /// file's top-of-file doc comment) — anything else, including unset,
    /// uses the default `click_js` command-socket path.
    fn click_mode_is_native() -> bool {
        std::env::var("WEB_STANDARDS_MACOS_CLICK_MODE").as_deref() == Ok("native")
    }

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

        let fixture_server = match spawn_fixture_server(Path::new(fixtures_root)) {
            Ok(server) => server,
            Err(err) => {
                eprintln!("error: failed to start the local fixture server: {err}");
                return ExitCode::FAILURE;
            }
        };

        let mut all_passed = true;
        for case in &cases {
            println!("== running case: {case} ==");
            match run_case(app_exe, fixtures_root, &fixture_server.base_url, case) {
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

    /// Serves `fixtures_root` over real `http://127.0.0.1` instead of
    /// `file://` — see this file's top-of-`run`-function comment history
    /// (and `crates/browser-linux-gtk3/tests/gtk_tests.rs`'s own
    /// `FixtureServer`, the analogous fix for a different wry file:// bug
    /// on GTK) for why. Lives for the whole process, not per-case, since
    /// every case serves from the same root.
    struct FixtureServer {
        server: std::sync::Arc<tiny_http::Server>,
        join: Option<std::thread::JoinHandle<()>>,
        base_url: String,
    }

    impl Drop for FixtureServer {
        fn drop(&mut self) {
            self.server.unblock();
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    fn spawn_fixture_server(fixtures_root: &Path) -> anyhow::Result<FixtureServer> {
        let root = fixtures_root.canonicalize()?;
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").map_err(|err| anyhow::anyhow!("binding a loopback fixture server: {err}"))?);
        let addr = server.server_addr().to_ip().ok_or_else(|| anyhow::anyhow!("fixture server should bind an IP socket, not a unix one"))?;
        let base_url = format!("http://{addr}");
        let server_for_thread = std::sync::Arc::clone(&server);
        let join = std::thread::spawn(move || {
            while let Ok(request) = server_for_thread.recv() {
                // `Path::join` treats a leading `/` in `requested` as
                // replacing the base entirely — `trim_start_matches('/')`
                // avoids that trap.
                let requested = request.url().trim_start_matches('/');
                let path = root.join(requested);
                // `tiny_http::Response::from_string`'s default content-type
                // is `text/plain` — without an explicit `text/html` header,
                // WebKit renders a fixture's markup as literal text instead
                // of parsing it.
                let content_type = if path.extension().and_then(|e| e.to_str()) == Some("html") { "text/html; charset=utf-8" } else { "text/plain; charset=utf-8" };
                let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).expect("static content-type header should be valid");
                let response = match std::fs::read_to_string(&path) {
                    Ok(body) => tiny_http::Response::from_string(body).with_status_code(200).with_header(header),
                    Err(_) => tiny_http::Response::from_string("not found").with_status_code(404).with_header(header),
                };
                let _ = request.respond(response);
            }
        });
        Ok(FixtureServer { server, join: Some(join), base_url })
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

    fn run_case(app_exe: &str, fixtures_root: &str, base_url: &str, case: &str) -> anyhow::Result<bool> {
        let case_dir = Path::new(fixtures_root).join(case);
        let expected = std::fs::read_to_string(case_dir.join("expected.txt"))?;
        // `http://127.0.0.1/...` (`base_url`, from `spawn_fixture_server`),
        // not `file://` — see this file's `FixtureServer` doc comment for
        // why: wry's macOS `navigate_to_url` never properly loads `file://`
        // URLs at all. Used for `native` mode too, not just the default
        // command-socket path — both reach the exact same
        // `navigate_to_url` call underneath, so both need it.
        let index_url = format!("{base_url}/{case}/index.html");
        let steps = read_actions(&case_dir)?;

        let native = click_mode_is_native();
        let socket_path = std::env::temp_dir().join(format!("web-standards-driver-macos-{case}.sock"));
        let _ = std::fs::remove_file(&socket_path);

        let mut command = Command::new(app_exe);
        // The command socket is harmless to pass even in `native` click
        // mode (the app just listens on it without anything using it), but
        // skipped there anyway to keep that mode as close as possible to
        // the original, pre-socket CGEvent-only behavior for comparison.
        if !native {
            command.arg("--test-command-socket").arg(&socket_path);
        }
        let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let result = drive_and_capture(&mut child, &index_url, &steps, &expected, &socket_path, native);
        // Checked *before* killing it: if the app already exited on its own
        // (crashed, or otherwise) `try_wait` reports that real exit
        // status — `ExitStatus`'s own `Display` includes signal info on
        // Unix (e.g. "signal: 11 (SIGSEGV)"), which is exactly the
        // diagnostic a silent "child stdout ended" error can't provide on
        // its own.
        match child.try_wait() {
            Ok(Some(status)) => eprintln!("[driver] app process had already exited: {status}"),
            Ok(None) => {
                let _ = child.kill();
            }
            Err(err) => eprintln!("[driver] error checking app process status: {err}"),
        }
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

    /// Reads `reader` line-by-line into the returned channel until it hits
    /// EOF (the child closed the stream — normal on exit) or a read error.
    /// Deliberately *not* `std::io::BufRead::lines()`: that yields an `Err`
    /// for any line that isn't valid UTF-8, and `.map_while(Result::ok)`
    /// (this file's original approach) treats the first such `Err` as the
    /// end of the whole iterator — silently abandoning the rest of a still-
    /// running child's output after one stray non-UTF-8 byte sequence
    /// (WebKit/AppKit framework logging noise, say) rather than actually
    /// meaning the child exited. `String::from_utf8_lossy` degrades one bad
    /// line instead of ending the stream.
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

    /// Polls for `socket_path` to exist and be connectable — the app needs
    /// a moment after launch to reach `AppState::start_test_command_listener`
    /// and bind it, so the very first connection attempt right after
    /// spawning would usually just fail with "No such file or directory."
    fn connect_command_socket(socket_path: &Path) -> anyhow::Result<UnixStream> {
        let deadline = Instant::now() + SOCKET_CONNECT_WAIT;
        loop {
            match UnixStream::connect(socket_path) {
                Ok(stream) => return Ok(stream),
                Err(err) if Instant::now() < deadline => {
                    let _ = err;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(err) => anyhow::bail!("couldn't connect to {socket_path:?} within {SOCKET_CONNECT_WAIT:?}: {err}"),
            }
        }
    }

    fn send_command(socket: &mut UnixStream, command: &str) -> anyhow::Result<()> {
        socket.write_all(command.as_bytes())?;
        socket.write_all(b"\n")?;
        Ok(())
    }

    fn drive_and_capture(
        child: &mut Child,
        index_url: &str,
        steps: &[Step],
        expected: &str,
        socket_path: &Path,
        native: bool,
    ) -> anyhow::Result<String> {
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("child had no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow::anyhow!("child had no stderr"))?;
        let rx = spawn_line_reader(stdout);
        // Relayed straight to our own stderr (prefixed, so it's easy to
        // pick out in CI logs) rather than dropped — this is exactly the
        // channel a real app crash/panic would otherwise vanish into, with
        // no other way to see why the driver stopped getting output.
        let stderr_rx = spawn_line_reader(stderr);
        std::thread::spawn(move || {
            for line in stderr_rx {
                eprintln!("[app stderr] {line}");
            }
        });

        // A freshly launched app becomes the frontmost/key application on
        // macOS by default (unlike the Windows VM pipeline, no
        // `SetForegroundWindow`-equivalent foreground-lock workaround is
        // needed here) — just a real wait for it to finish starting up.
        std::thread::sleep(WINDOW_WAIT);

        let mut socket = if native { None } else { Some(connect_command_socket(socket_path)?) };

        if let Some(socket) = socket.as_mut() {
            send_command(socket, "open_switcher")?;
            send_command(socket, &format!("navigate {index_url}"))?;
        } else {
            // `native` mode: no command socket at all, so navigation has to
            // go through real CGEvent keystrokes too — the original
            // mechanism this file used before the command-socket approach
            // existed. `Ctrl` in `browser_core::KeyChord` maps to `Cmd` on
            // this platform, and `Cmd+T` is the default `OpenSwitcher`
            // binding; AppKit's menu-key-equivalent dispatch routes through
            // the responder chain regardless of which view has first
            // responder status, unlike `browser-windows-reactor`'s
            // `KeyboardAccelerator` (confirmed to not fire while a
            // `WebView2` has focus — see `windows_driver.rs`).
            send_key(KEYCODE_T, CGEventFlags::CGEventFlagCommand);
            std::thread::sleep(Duration::from_millis(500));
            send_text(index_url);
            std::thread::sleep(Duration::from_millis(300));
            send_key(KEYCODE_RETURN, CGEventFlags::empty());
        }
        std::thread::sleep(NAVIGATION_SETTLE);

        let mut captured: Vec<String> = Vec::new();
        for step in steps {
            match step.action.as_str() {
                "click" => match socket.as_mut() {
                    Some(socket) => send_command(socket, &format!("click_js {}", step.target))?,
                    None => {
                        let (x, y) = resolve_target_point(&rx, &mut captured, &step.target)?;
                        click_at(x, y);
                    }
                },
                other => anyhow::bail!("unknown actions.json action {other:?}"),
            }
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
    /// filtered out, joined back into the same newline-terminated shape
    /// `expected.txt` uses.
    fn real_assertion_lines(captured: &[String]) -> String {
        captured.iter().filter(|line| !line.starts_with("__test_target__ ")).flat_map(|line| [line.as_str(), "\n"]).collect()
    }

    /// Waits for a `__test_target__ <target> <rect-json>` line (already
    /// received into `captured`, or arriving on `rx` before `MESSAGE_WAIT`
    /// elapses) and resolves it to a real screen point: `content_area_origin`
    /// plus the reported rect's center.
    fn resolve_target_point(rx: &mpsc::Receiver<String>, captured: &mut Vec<String>, target: &str) -> anyhow::Result<(f64, f64)> {
        let prefix = format!("__test_target__ {target} ");
        let deadline = Instant::now() + MESSAGE_WAIT;
        loop {
            if let Some(rect_json) = captured.iter().find_map(|line| line.strip_prefix(prefix.as_str())) {
                let parsed: serde_json::Value = serde_json::from_str(rect_json)?;
                let rx_ = parsed["x"].as_f64().ok_or_else(|| anyhow::anyhow!("__test_target__ rect missing x"))?;
                let ry_ = parsed["y"].as_f64().ok_or_else(|| anyhow::anyhow!("__test_target__ rect missing y"))?;
                let rw = parsed["width"].as_f64().ok_or_else(|| anyhow::anyhow!("__test_target__ rect missing width"))?;
                let rh = parsed["height"].as_f64().ok_or_else(|| anyhow::anyhow!("__test_target__ rect missing height"))?;
                let (origin_x, origin_y) = content_area_origin();
                return Ok((origin_x + rx_ + rw / 2.0, origin_y + ry_ + rh / 2.0));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("no __test_target__ report for {target:?} arrived within {MESSAGE_WAIT:?}");
            }
            // `recv_timeout` returns `Err` both when `remaining` genuinely
            // elapses with nothing sent (`Timeout` — a plain "never showed
            // up," no different from the `remaining.is_zero()` case above)
            // and when the sender was dropped (`Disconnected` — the reader
            // thread's stream really did end). Conflating them here would
            // misreport an ordinary timeout as "the app crashed," which is
            // exactly the wrong diagnosis to hand someone debugging this.
            match rx.recv_timeout(remaining) {
                Ok(line) => captured.push(line),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    anyhow::bail!("no __test_target__ report for {target:?} arrived within {MESSAGE_WAIT:?}")
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("child stdout ended before a __test_target__ report for {target:?} arrived")
                }
            }
        }
    }

    /// The app window's content area (below the toolbar strip) top-left
    /// corner, in absolute screen coordinates — a `__test_target__` rect is
    /// reported by the page in CSS/viewport coordinates
    /// (`getBoundingClientRect()`, relative to the content area's own
    /// top-left), so this is the offset needed to turn one into a real
    /// screen point.
    ///
    /// Real, honest limitation: unlike `windows_driver.rs`'s
    /// `switcher_button_pos`/`search_box_pos` (calibrated against an actual
    /// screenshot of a real running window in the Windows VM), there's no
    /// equivalent real-hardware macOS session available in this environment
    /// to calibrate against — this is a plausible guess (a window opened
    /// near the top-left of a typical display, below the menu bar/title
    /// bar, sized similarly to `browser-windows-reactor`'s own observed
    /// default window), not a verified one.
    fn content_area_origin() -> (f64, f64) {
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

    /// Posts a real Command-key keyDown/keyUp around `keycode`'s own
    /// keyDown/keyUp when `flags` includes Command, rather than only
    /// setting the flag bit on `keycode`'s events — confirmed necessary
    /// directly on real hardware: setting just the flag bit was enough for
    /// some apps (Safari opened a new tab fine) but not others, and a real
    /// modifier-key event pair is what an actual physical Cmd+key keypress
    /// always produces, so it's the more correct sequence to synthesize
    /// either way.
    fn send_key(keycode: CGKeyCode, flags: CGEventFlags) {
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else { return };
        let needs_command = flags.contains(CGEventFlags::CGEventFlagCommand);
        if needs_command {
            if let Ok(event) = CGEvent::new_keyboard_event(source.clone(), KEYCODE_COMMAND, true) {
                event.set_flags(CGEventFlags::CGEventFlagCommand);
                event.post(CGEventTapLocation::HID);
            }
        }
        for key_down in [true, false] {
            if let Ok(event) = CGEvent::new_keyboard_event(source.clone(), keycode, key_down) {
                event.set_flags(flags);
                event.post(CGEventTapLocation::HID);
            }
        }
        if needs_command {
            if let Ok(event) = CGEvent::new_keyboard_event(source.clone(), KEYCODE_COMMAND, false) {
                event.set_flags(CGEventFlags::empty());
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
