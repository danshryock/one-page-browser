//! A local TCP command channel for `web-standards-driver-windows` (and any
//! other automated test) to drive this app the same zero-synthetic-input
//! way `browser-macos-appkit`'s Unix-socket command channel already does —
//! see that crate's `AppState::start_test_command_listener` doc comment for
//! the full motivation over there (a genuine TCC/Accessibility permission
//! dead end on macOS). Windows doesn't have that specific gate, so
//! `SendInput`-based synthetic input (this platform's driver's original
//! approach) "just works" without any permission grant — but it's still
//! real-screen-coordinate-dependent and timing-sensitive in a way a direct
//! command channel isn't (see `web-standards-tests/src/bin/
//! windows_driver.rs`'s own calibration history). Reusing the same
//! architecture here trades that fragility for the same reliability
//! macOS's driver already gained — consistency and robustness, not a
//! permission workaround.
//!
//! `std::os::unix::net::UnixListener` isn't available when targeting
//! Windows (gated out at the source level by target family, not just
//! unsupported at runtime) — a plain TCP listener on `127.0.0.1` is the
//! least surprising portable substitute, needing only a port number
//! instead of a socket path.
//!
//! Only ever active when a caller explicitly opts in
//! (`--test-command-port <port>`, see `main.rs`) — never touched by a
//! normal launch.

use std::cell::RefCell;
use std::io::BufRead;
use std::net::TcpListener;
use std::sync::mpsc;

use crate::xaml_interop::main_hwnd;

thread_local! {
    static COMMAND_RX: RefCell<Option<mpsc::Receiver<String>>> = const { RefCell::new(None) };
    static COMMAND_HANDLER: RefCell<Option<Box<dyn Fn(String)>>> = const { RefCell::new(None) };
}

const TEST_COMMAND_TIMER_ID: usize = 0xc0771e57;

/// Runs the socket-accepting/line-reading loop on a background thread
/// (blocking I/O is fine there) and marshals each command onto the main
/// thread via a repeating Win32 `SetTimer` draining an `mpsc` channel —
/// this crate's own UI state (`core`, the various `Callback<T>`s `handler`
/// closes over) is only ever safe to touch from the main STA thread, the
/// same constraint `xaml_interop::defer_to_next_tick` already works around
/// for a different reason (and the same reason `browser-macos-appkit`'s
/// own version of this uses an `NSTimer` poll rather than touching
/// `Rc<AppState>` straight from the socket's background thread).
///
/// Binds the socket and spawns the reader thread unconditionally — do this
/// exactly once (the caller's job to guard, since binding the same port
/// twice fails outright). Arming the poll timer is a separate, retriable
/// step (this function's own return value, meant to be threaded into
/// `ensure_timer_armed`'s same "did it actually happen yet" bookkeeping —
/// see that function's doc comment for why): `xaml_interop::main_hwnd`
/// (`windows_reactor::with_active_host` underneath) confirmed directly to
/// return `None` on this app's very first render or two — the same reason
/// `xaml_interop::setup_titlebar_passthrough` needs its own lazy-retry
/// shape. Calling `start` only once, unconditionally trusting its return
/// value as "done," silently left every test command received but never
/// drained: the socket bound fine (so a client's `write_all` succeeded)
/// and queued into `COMMAND_RX`, but nothing ever polled it out, because
/// `SetTimer` was never actually reached.
pub(crate) fn start(port: u16, handler: impl Fn(String) + 'static) -> bool {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("failed to bind test command port {port}: {err}");
            return false;
        }
    };
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            for line in std::io::BufReader::new(stream).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        }
    });
    COMMAND_RX.with(|cell| *cell.borrow_mut() = Some(rx));
    COMMAND_HANDLER.with(|cell| *cell.borrow_mut() = Some(Box::new(handler)));
    ensure_timer_armed()
}

/// Retries arming the poll timer until `main_hwnd()` finally succeeds — see
/// `start`'s doc comment for why a single attempt isn't reliable. Safe to
/// call repeatedly even after it's already armed (`SetTimer` with the same
/// `TEST_COMMAND_TIMER_ID` just re-arms the same timer, not a second one),
/// though the caller only actually needs to once it's returned `true`.
pub(crate) fn ensure_timer_armed() -> bool {
    match main_hwnd() {
        Some(hwnd) => {
            unsafe {
                windows::Win32::winuser::SetTimer(Some(hwnd), TEST_COMMAND_TIMER_ID, 20, Some(poll_commands));
            }
            true
        }
        None => false,
    }
}

unsafe extern "system" fn poll_commands(_hwnd: windows::Win32::HWND, _msg: u32, _timer_id: usize, _time: u32) {
    let lines: Vec<String> = COMMAND_RX.with(|cell| {
        let borrow = cell.borrow();
        match borrow.as_ref() {
            Some(rx) => std::iter::from_fn(|| rx.try_recv().ok()).collect(),
            None => Vec::new(),
        }
    });
    for line in lines {
        COMMAND_HANDLER.with(|cell| {
            if let Some(handler) = cell.borrow().as_ref() {
                handler(line);
            }
        });
    }
}
