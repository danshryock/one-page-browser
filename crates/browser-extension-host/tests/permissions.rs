//! Spawns a real `browser-extension-host` process (via `rpc_protocol::
//! RpcChildProcess`, the exact same client the main browser process would
//! use to drive a real extension) against a small fixture extension —
//! containing one real `.ts` file, proving actual transpilation happens,
//! not just `.js` passthrough — and asserts on its real, observed
//! behavior: a denied capability surfaces a genuine Deno permission error,
//! not a silent bypass, and a granted one actually works. Nothing here is
//! mocked: this is a real V8 isolate, real `deno_permissions` enforcement,
//! and the real stdout-redirect relay (see `main.rs`'s own doc comment for
//! why that exists) round-tripping over the real stdio protocol.

use rpc_protocol::RpcChildProcess;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

/// Builds a tiny extension zip (one `.ts` entry) at a fresh temp path and
/// returns it. `target_path` gets baked directly into the script source —
/// simplest way to parameterize a fixture per test without a second
/// out-of-band channel to the child process, which only ever reads its
/// configuration from argv (see `main.rs`'s own `parse_args`).
fn build_fixture_extension(target_path: &Path) -> std::path::PathBuf {
    let script = format!(
        r#"
const path: string = {target_path:?};
try {{
  const contents: string = Deno.readTextFileSync(path);
  console.log("read_succeeded:" + contents.trim());
}} catch (e) {{
  console.log("read_denied:" + e.constructor.name);
}}
"#
    );

    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("main.ts", options).expect("starting the fixture zip entry should succeed");
        writer.write_all(script.as_bytes()).expect("writing the fixture zip entry should succeed");
        writer.finish().expect("finishing the fixture zip should succeed");
    }

    let zip_path = std::env::temp_dir().join(format!("browser-extension-host-test-{}-{}.zip", std::process::id(), rand_suffix()));
    std::fs::write(&zip_path, &buf).expect("writing the fixture zip to disk should succeed");
    zip_path
}

/// Not cryptographic — just enough to keep concurrently-running tests'
/// temp files from colliding.
fn rand_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
}

/// Spawns the extension host and collects every `console.log` relay
/// message it sends within a bounded window, then shuts the connection
/// down. `RpcChildProcess::serve_incoming` runs the collection loop on a
/// background thread (it blocks until the connection closes); this
/// function owns starting/stopping that for the caller.
fn run_and_collect_console_logs(zip_path: &Path, allow_read: Option<&Path>) -> Vec<String> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_browser-extension-host"));
    cmd.arg("--extension-zip").arg(zip_path).arg("--entry").arg("main.ts");
    if let Some(path) = allow_read {
        cmd.arg("--allow-read").arg(path);
    }
    let child = Arc::new(RpcChildProcess::spawn(cmd).expect("spawning browser-extension-host should succeed"));

    let messages = Arc::new(Mutex::new(Vec::new()));
    let serving = {
        let child = Arc::clone(&child);
        let messages = Arc::clone(&messages);
        std::thread::spawn(move || {
            child.serve_incoming(|method, params, _binary| {
                if method == "console.log" {
                    if let Some(text) = params.as_str() {
                        messages.lock().unwrap().push(text.to_string());
                    }
                }
                Ok((serde_json::Value::Null, None))
            });
        })
    };

    // Polls for the one expected message rather than a fixed sleep-then-
    // kill: a blind short sleep was confirmed directly to be a real,
    // reproducible race, not just theoretically possible — a fresh V8
    // isolate's cold-start time varies with system load, and killing the
    // child before it's actually finished (rather than waiting for it to
    // exit on its own) loses whatever it hadn't relayed yet, regardless of
    // `main.rs`'s own relay-thread-join fix (that fix only guarantees a
    // *graceful* exit fully drains the pipe — `SIGKILL` doesn't let it run
    // at all). Generous ceiling on purpose (same reasoning as `browser-
    // linux-gtk3/tests/gtk_tests.rs`'s own `wait_until`): this only costs
    // real time when the condition genuinely never becomes true.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while messages.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    child.shutdown();
    serving.join().expect("the serve_incoming thread should exit cleanly once the connection closes");

    Arc::try_unwrap(messages).unwrap().into_inner().unwrap()
}

#[test]
fn a_denied_capability_surfaces_a_real_permission_error_not_a_silent_bypass() {
    let target = std::env::temp_dir().join(format!("browser-extension-host-denied-target-{}.txt", rand_suffix()));
    std::fs::write(&target, "should never be readable\n").unwrap();

    let zip_path = build_fixture_extension(&target);
    let messages = run_and_collect_console_logs(&zip_path, None);

    std::fs::remove_file(&target).ok();
    std::fs::remove_file(&zip_path).ok();

    assert_eq!(messages.len(), 1, "expected exactly one console.log relay, got {messages:?}");
    assert!(messages[0].starts_with("read_denied:"), "expected a denial, got {messages:?}");
    assert!(messages[0].contains("NotCapable"), "expected a real Deno NotCapable error class, got {messages:?}");
}

#[test]
fn a_granted_capability_actually_works() {
    let target = std::env::temp_dir().join(format!("browser-extension-host-granted-target-{}.txt", rand_suffix()));
    std::fs::write(&target, "real file contents\n").unwrap();

    let zip_path = build_fixture_extension(&target);
    let messages = run_and_collect_console_logs(&zip_path, Some(&target));

    std::fs::remove_file(&target).ok();
    std::fs::remove_file(&zip_path).ok();

    assert_eq!(messages.len(), 1, "expected exactly one console.log relay, got {messages:?}");
    assert_eq!(messages[0], "read_succeeded:real file contents", "expected the real file contents to have been read, got {messages:?}");
}

#[test]
fn typescript_type_annotations_are_really_transpiled_not_just_ignored_as_js() {
    // Reuses the same fixture as the other tests (it already has a real
    // `const path: string = ...` annotation) — if `ZipModuleLoader` were
    // secretly just passing `.ts` bytes through as if they were already
    // JavaScript (rather than actually transpiling via `deno_ast`), V8
    // would reject the type annotation outright as a syntax error and the
    // process would exit non-zero instead of producing any console.log
    // relay at all. A successful run *is* the proof.
    let target = std::env::temp_dir().join(format!("browser-extension-host-ts-proof-target-{}.txt", rand_suffix()));
    std::fs::write(&target, "typescript works\n").unwrap();

    let zip_path = build_fixture_extension(&target);
    let messages = run_and_collect_console_logs(&zip_path, Some(&target));

    std::fs::remove_file(&target).ok();
    std::fs::remove_file(&zip_path).ok();

    assert_eq!(messages.len(), 1, "a real syntax error from un-transpiled TS would produce zero console.log relays, got {messages:?}");
}
