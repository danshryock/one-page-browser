//! External driver for `browser-windows-reactor`'s web-standards tests — see
//! `macos_driver.rs`'s doc comment for the shared design (same fixture
//! layout, same golden-file diffing via `expected.txt`, same "launch the
//! real app, drive it over a local command channel, read its piped output"
//! approach — "output," not "stdout": see `spawn_line_reader`'s own doc
//! comment for why this file reads the child's *stderr* too, unlike
//! `macos_driver.rs`) — this file used to drive the app via real `SendInput`
//! synthetic mouse/keyboard input and calibrated screen coordinates, mirrors
//! of what the original macOS driver did before its own command-socket
//! rewrite. Replaced with `browser-windows-reactor`'s own `test_command_server`
//! (see that module's doc comment) for the same reason macOS's driver
//! switched: not a permission workaround here (Windows has no
//! Accessibility/TCC-style gate on `SendInput`), but the same reliability
//! win — no screen-coordinate calibration, no window-focus/z-order
//! workarounds, no timing-sensitive clicks.
//!
//! Per-fixture interactions come from `fixtures_root/<case>/actions.json`
//! (`{"action": "click", "target": "<name>"}` steps), sent as `click_js
//! <target>` commands — the app runs `document.querySelector(...).click()`
//! against the real page via `WebView::execute_script`. `navigate <url>`
//! opens each fixture directly (over a local `http://127.0.0.1` fixture
//! server, not `file://` — see `FixtureServer`'s doc comment) rather than
//! needing every case pre-seeded into `session.json` as an already-open
//! page the way the old `SendInput`-driven version did: `add_page_and_switch`
//! is a real, direct command now, so there's no need to work around the
//! lack of one.
//!
//! `opener-default`/`opener-explicit-opener` motivated one extra allowance
//! in `browser_windows_reactor::page_element`'s `new_window_requested`
//! handler: it gates *every* new-window request (not just an unclicked
//! `window.open()` call, the way `wry` does on macOS/gtk3 — see
//! `macos_driver.rs`'s own doc comment on that narrower distinction) on
//! `NewWindowRequestedArgs::is_user_initiated()` (`ICoreWebView2NewWindowRequestedEventArgs::IsUserInitiated`,
//! a real native WebView2/Chromium concept). `click_js`'s script-dispatched
//! `document.querySelector(...).click()` turns out to *still* satisfy that
//! check in practice (confirmed directly in the real VM: `is_user_initiated()`
//! reports `true` for it) — WebView2 apparently treats a host-driven
//! `ExecuteScript` call's own synchronous effects as carrying real user
//! activation, unlike a plain in-page script running on its own. That gate
//! is real, security-relevant production behavior either way, so
//! `page_element`'s own handler also explicitly allows the request through
//! whenever `--test-command-port` is set (see that closure's own comment) —
//! never true for a real launch, only for this driver's own runs — as a
//! defensive fallback in case that WebView2 behavior ever changes.
//!
//! Every case launches under `--incognito` (see `run_case`): otherwise one
//! case's own continuous-session-sync writes would leak into the *next*
//! case's freshly spawned process via `session.json` on restore, including,
//! confirmed directly, a stray `console.log` from a restored popup page
//! re-navigating on its own.
//!
//! Usage: `web-standards-driver-windows.exe <app.exe path> <fixtures root>`

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
    use std::io::Write;
    use std::net::TcpStream;
    use std::path::Path;
    use std::process::{Child, Command, ExitCode, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const APP_START_WAIT: Duration = Duration::from_secs(5);
    const SOCKET_CONNECT_WAIT: Duration = Duration::from_secs(5);
    const NAVIGATION_SETTLE: Duration = Duration::from_millis(3000);
    const MESSAGE_WAIT: Duration = Duration::from_secs(15);
    /// Fixed rather than `:0`-and-ask-the-OS (`spawn_fixture_server`'s HTTP
    /// server does exactly that): `--test-command-port` has to be known
    /// *before* the app process is spawned, and there's no cross-process
    /// "tell me what port you picked" handshake here — a single driver run
    /// only ever has one app instance alive at a time, so a fixed port is
    /// safe, not just convenient.
    const TEST_COMMAND_PORT: u16 = 47821;

    pub fn run() -> ExitCode {
        let args: Vec<String> = std::env::args().collect();
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
    /// `file://` — same fix, same reasoning, as `macos_driver.rs`'s own
    /// `FixtureServer` for the analogous wry bug (kept here too for
    /// consistency between the two drivers, not because WebView2 is known
    /// to have the identical issue — better to serve fixtures the same way
    /// on both than assume `file://` is fine on this one without ever
    /// having actually calibrated/verified that here). Lives for the whole
    /// process, not per-case, since every case serves from the same root.
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
                // WebView2 may render a fixture's markup as literal text
                // instead of parsing it.
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

    fn run_case(app_exe: &str, fixtures_root: &str, base_url: &str, case: &str) -> anyhow::Result<bool> {
        let case_dir = Path::new(fixtures_root).join(case);
        let expected = std::fs::read_to_string(case_dir.join("expected.txt"))?;
        let index_url = format!("{base_url}/{case}/index.html");
        let steps = read_actions(&case_dir)?;

        let mut child = Command::new(app_exe)
            .arg("--test-command-port")
            .arg(TEST_COMMAND_PORT.to_string())
            // Ephemeral: never touches `session.json` — without this, one
            // case's own continuous-session-sync writes leak into the
            // *next* case's freshly spawned process on restore (confirmed
            // directly: a stray, unrelated `console.log` line from a
            // restored popup page re-navigating on its own broke an
            // `expected.txt` diff). Each case gets a genuinely clean slate
            // this way, matching `macos_driver.rs`'s own fixture isolation
            // (a fresh fixture server per run, not a shared mutable one).
            .arg("--incognito")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let result = drive_and_capture(&mut child, &index_url, &steps, &expected);
        // Checked *before* killing it: if the app already exited on its own
        // (crashed, or otherwise) `try_wait` reports that real exit
        // status — a diagnostic a silent "child stdout ended" error can't
        // provide.
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

    /// Reads `reader` line-by-line into the returned channel until it hits
    /// EOF (the child closed the stream — normal on exit) or a read error.
    /// Deliberately *not* `std::io::BufRead::lines()`: that yields an `Err`
    /// for any line that isn't valid UTF-8, and `.map_while(Result::ok)`
    /// treats the first such `Err` as the end of the whole iterator —
    /// silently abandoning the rest of a still-running child's output
    /// rather than actually meaning the child exited. `String::from_utf8_lossy`
    /// degrades one bad line instead of ending the stream.
    ///
    /// Takes an existing `Sender` rather than creating/returning its own
    /// `Receiver` — `drive_and_capture` shares one channel between this and
    /// a second call for the child's other stream, merging both into one
    /// `captured` list. Real, confirmed-not-assumed reason both need
    /// reading: this app's own `console.log` relay writes to *stderr*, not
    /// stdout (see `browser_windows_reactor`'s `console_message_received`
    /// doc comment — a genuine, still not fully root-caused Windows
    /// GUI-subsystem stdio quirk where this app's stdout never reaches a
    /// piped parent at all, confirmed directly with extensive tracing:
    /// the write and an explicit flush both report success app-side, nothing
    /// arrives driver-side, and the *identical* mechanism on stderr works).
    /// Reading only stdout (this file's original approach, matching gtk3/
    /// macOS where it *is* the right stream) silently saw nothing here.
    fn spawn_line_reader<R: std::io::Read + Send + 'static>(reader: R, tx: mpsc::Sender<String>) {
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
    }

    /// Polls for the app's `--test-command-port` listener to accept a
    /// connection — it needs a moment after launch to reach
    /// `test_command_server::start` and bind it, so the very first
    /// connection attempt right after spawning would usually just fail with
    /// "connection refused."
    fn connect_command_socket() -> anyhow::Result<TcpStream> {
        let deadline = Instant::now() + SOCKET_CONNECT_WAIT;
        loop {
            match TcpStream::connect(("127.0.0.1", TEST_COMMAND_PORT)) {
                Ok(stream) => return Ok(stream),
                Err(err) if Instant::now() < deadline => {
                    let _ = err;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(err) => anyhow::bail!("couldn't connect to 127.0.0.1:{TEST_COMMAND_PORT} within {SOCKET_CONNECT_WAIT:?}: {err}"),
            }
        }
    }

    fn send_command(socket: &mut TcpStream, command: &str) -> anyhow::Result<()> {
        socket.write_all(command.as_bytes())?;
        socket.write_all(b"\n")?;
        Ok(())
    }

    fn drive_and_capture(child: &mut Child, index_url: &str, steps: &[Step], expected: &str) -> anyhow::Result<String> {
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("child had no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow::anyhow!("child had no stderr"))?;
        // Both streams merged into one `rx` — see `spawn_line_reader`'s doc
        // comment for why stderr (not just stdout) needs reading here at
        // all: this app's real `console.log` relay writes there.
        let (tx, rx) = mpsc::channel::<String>();
        spawn_line_reader(stdout, tx.clone());
        spawn_line_reader(stderr, tx);

        // Real launch/startup time — no window-finding/foreground-focus
        // workaround needed at all anymore: the command channel doesn't
        // care whether the window is focused, foreground, or even visible.
        std::thread::sleep(APP_START_WAIT);
        println!("checkpoint: app should be up, connecting to the command channel");

        let mut socket = connect_command_socket()?;
        send_command(&mut socket, "open_switcher")?;
        send_command(&mut socket, &format!("navigate {index_url}"))?;
        println!("checkpoint: sent navigate {index_url:?}");
        std::thread::sleep(NAVIGATION_SETTLE);

        let mut captured: Vec<String> = Vec::new();
        for step in steps {
            match step.action.as_str() {
                "click" => {
                    send_command(&mut socket, &format!("click_js {}", step.target))?;
                    println!("checkpoint: sent click_js {:?}", step.target);
                }
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
    /// (a leftover of the `getBoundingClientRect()` reporting `CONSOLE_CAPTURE_SCRIPT`
    /// still does on every platform, even though this driver no longer
    /// needs on-screen coordinates for anything) filtered out, joined back
    /// into the same newline-terminated shape `expected.txt` uses — the real
    /// page console output a fixture's own assertion actually checks.
    fn real_assertion_lines(captured: &[String]) -> String {
        captured.iter().filter(|line| !line.starts_with("__test_target__ ")).flat_map(|line| [line.as_str(), "\n"]).collect()
    }
}
