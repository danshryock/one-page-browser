//! `RpcChildProcess` — spawns a child process and owns a bidirectional
//! `RpcMessage` conversation with it over its stdin/stdout, using
//! `stdio::{read_message, write_message}` for framing.
//!
//! No sandboxing is applied to the spawned process here — that's a
//! deliberately separate, deferred piece of work (see `ROADMAP.md`), not
//! something this crate does on its own.
//!
//! Follows this codebase's established shape for "read from a background
//! thread, hand results across via a channel" (the same idiom
//! `browser-macos-appkit`'s test-command listener and `browser-windows-
//! reactor`'s `test_command_server` already use, and what `web-standards-
//! tests`' own driver processes do when reading a spawned app's stdout) —
//! entirely synchronous, no async runtime, matching the rest of this
//! workspace.
use crate::{read_message, write_message, RpcError, RpcMessage};
use std::collections::HashMap;
use std::io::BufReader;
use std::io::BufWriter;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

type PendingReply = Result<(serde_json::Value, Option<Vec<u8>>), RpcError>;

/// A JSON-RPC-over-stdio conversation with a spawned child process.
///
/// Bidirectional: `call` lets the host ask the child to do something and
/// block for the answer; `serve_incoming` runs the reverse direction — the
/// child calling back into the host — for as long as the connection stays
/// open. Both can be exercised concurrently (from different threads) on
/// the same `RpcChildProcess`, since the shared state (`writer`, `pending`)
/// is internally synchronized.
pub struct RpcChildProcess {
    // `Mutex`-wrapped so `shutdown()` can kill the child through a shared
    // `&self` (needed when a `serve_incoming` loop is running on another
    // thread that itself holds an `Arc<RpcChildProcess>` clone — see
    // `shutdown`'s own doc comment for why relying on `Drop` alone doesn't
    // work in that shape).
    child: Mutex<Child>,
    writer: Mutex<BufWriter<ChildStdin>>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<PendingReply>>>>,
    // `Mutex`-wrapped (not a bare `Receiver`) purely so `RpcChildProcess`
    // itself is `Sync` and can be shared via `Arc` across the thread that
    // calls `serve_incoming` and any threads calling `call` concurrently —
    // `Receiver` is `Send` but not `Sync` on its own. Only ever meant to be
    // drained from one thread at a time (`serve_incoming`'s own loop).
    incoming: Mutex<mpsc::Receiver<(RpcMessage, Option<Vec<u8>>)>>,
    // Set once by the reader thread, right before it drains `pending` on
    // its way out. Exists to close a real race, not a hypothetical one
    // (found by an actually-hanging test): the drain only ever runs *once*,
    // at the moment the reader thread notices the connection is dead — a
    // `call()` whose own `pending` entry gets inserted *after* that one-time
    // drain already ran would otherwise have nothing left to ever resolve
    // it, and `rx.recv()` would block forever. `call()` checks this after
    // inserting its own entry to catch exactly that ordering.
    closed: Arc<std::sync::atomic::AtomicBool>,
}

impl RpcChildProcess {
    /// Spawns `command` (its `stdin`/`stdout` are forced to `Stdio::piped()`
    /// regardless of how the caller configured them — this conversation
    /// needs to own both) and starts a background reader thread that
    /// demultiplexes the child's output: a `Response` gets routed to
    /// whichever `call()` is waiting on that `id`; a `Request`/`Notification`
    /// (the child calling *into* the host) gets queued for `serve_incoming`.
    ///
    /// If the child's stream closes (clean EOF) or a framing error occurs
    /// (a malformed line, a truncated binary attachment — see `stdio`'s own
    /// doc comment), the reader thread treats the whole connection as dead:
    /// it stops, and every still-pending `call()` gets a real error instead
    /// of hanging forever. This is a deliberately blunt policy — no
    /// resync/recovery attempt — matching this protocol's "simple" brief;
    /// a single malformed message ends the conversation rather than risking
    /// silently misinterpreting the stream from that point on.
    pub fn spawn(mut command: Command) -> std::io::Result<Self> {
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("stdin was just set to Stdio::piped()");
        let stdout = child.stdout.take().expect("stdout was just set to Stdio::piped()");

        let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<PendingReply>>>> = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (incoming_tx, incoming_rx) = mpsc::channel();

        let reader_pending = Arc::clone(&pending);
        let reader_closed = Arc::clone(&closed);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader) {
                    Ok(Some((message, binary))) => match message {
                        RpcMessage::Response { id, result, .. } => {
                            if let Some(sender) = reader_pending.lock().unwrap().remove(&id) {
                                let reply = match result {
                                    Ok(value) => Ok((value, binary)),
                                    Err(err) => Err(err),
                                };
                                // A `send` failure here just means the caller
                                // already gave up waiting — nothing to do.
                                let _ = sender.send(reply);
                            }
                            // Response to an id nothing's waiting on (already
                            // delivered an error on connection-close, or a
                            // bug on the child's side) — safe to drop.
                        }
                        incoming @ (RpcMessage::Request { .. } | RpcMessage::Notification { .. }) => {
                            if incoming_tx.send((incoming, binary)).is_err() {
                                break; // nothing reading `incoming` anymore
                            }
                        }
                    },
                    Ok(None) => break,  // clean EOF — child closed its end
                    Err(_) => break,    // framing error — see doc comment above
                }
            }
            // The connection is over, one way or another. Nobody still
            // waiting on a `call()` response is ever getting a real one now
            // — tell them, instead of leaving them blocked on `rx.recv()`
            // forever. `closed` is set *before* draining so that any
            // `call()` racing this exact moment (see that field's own doc
            // comment) is guaranteed to observe it as already true by the
            // time it checks, whichever order the two actually interleave.
            reader_closed.store(true, Ordering::SeqCst);
            for (_, sender) in reader_pending.lock().unwrap().drain() {
                let _ = sender.send(Err(Self::stale_connection_error()));
            }
        });

        Ok(Self { child: Mutex::new(child), writer: Mutex::new(BufWriter::new(stdin)), next_id: AtomicU64::new(1), pending, incoming: Mutex::new(incoming_rx), closed })
    }

    /// Sends a `Request` to the child and blocks until its matching
    /// `Response` arrives (or the connection dies first). `binary` is an
    /// optional raw, unencoded attachment sent alongside the request; the
    /// returned tuple's second element is whatever binary attachment (if
    /// any) the child's response carried back.
    pub fn call(&self, method: &str, params: serde_json::Value, binary: Option<&[u8]>) -> Result<(serde_json::Value, Option<Vec<u8>>), RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);

        let message = RpcMessage::Request { id, method: method.to_string(), params, binary_len: binary.map(|bytes| bytes.len() as u64) };
        if let Err(err) = write_message(&mut *self.writer.lock().unwrap(), &message, binary) {
            self.pending.lock().unwrap().remove(&id);
            return Err(RpcError { code: -32000, message: format!("writing request to child stdin failed: {err}"), data: None });
        }

        // Closes the race documented on `closed`'s own field comment: if the
        // reader thread's one-time drain already ran *before* our own
        // `insert` above, nothing will ever deliver to `rx` — check here and
        // resolve it ourselves rather than blocking forever. If `remove`
        // finds nothing, the drain (or a real response) already claimed our
        // entry, so falling through to `rx.recv()` below is correct either
        // way.
        if self.closed.load(Ordering::SeqCst) && self.pending.lock().unwrap().remove(&id).is_some() {
            return Err(Self::stale_connection_error());
        }

        match rx.recv() {
            Ok(reply) => reply,
            // The reader thread's pending-table drain (on connection close)
            // is the normal way this arrives; a bare disconnected `tx`
            // (dropped without a send at all) would be a bug in this file,
            // not something a caller should see differently.
            Err(_) => Err(Self::stale_connection_error()),
        }
    }

    fn stale_connection_error() -> RpcError {
        RpcError { code: -32001, message: "child process connection closed before a response arrived".to_string(), data: None }
    }

    /// Runs the reverse direction — the child calling into the host — on
    /// the calling thread, for as long as the connection stays open.
    /// `handler` is invoked for every `Request`/`Notification` the child
    /// sends; its return value becomes the `Response` sent back for a
    /// `Request` (ignored for a `Notification`, which has no reply).
    /// Returns once the connection closes (mirrors `call`'s own "connection
    /// closed" handling — there's no error to report here since there's no
    /// single blocked caller to report it to; the loop just ends).
    pub fn serve_incoming<F>(&self, handler: F)
    where
        F: Fn(&str, serde_json::Value, Option<Vec<u8>>) -> Result<(serde_json::Value, Option<Vec<u8>>), RpcError>,
    {
        while let Ok((message, binary)) = self.incoming.lock().unwrap().recv() {
            match message {
                RpcMessage::Request { id, method, params, .. } => {
                    let (result, out_binary) = match handler(&method, params, binary) {
                        Ok((value, out_binary)) => (Ok(value), out_binary),
                        Err(err) => (Err(err), None),
                    };
                    let _ = self.reply(id, result, out_binary.as_deref());
                }
                RpcMessage::Notification { method, params, .. } => {
                    let _ = handler(&method, params, binary);
                }
                RpcMessage::Response { .. } => {
                    unreachable!("the reader thread routes Response messages to the pending table, never to `incoming`")
                }
            }
        }
    }

    fn reply(&self, id: u64, result: Result<serde_json::Value, RpcError>, binary: Option<&[u8]>) -> std::io::Result<()> {
        let message = RpcMessage::Response { id, result, binary_len: binary.map(|bytes| bytes.len() as u64) };
        write_message(&mut *self.writer.lock().unwrap(), &message, binary)
    }

    /// Kills the child process, which is what actually lets a running
    /// `serve_incoming` loop return: killing it closes its stdout, the
    /// reader thread sees that as EOF and exits, which drops the `incoming`
    /// channel's sender and unblocks `serve_incoming`'s `recv()`.
    ///
    /// Exists as an explicit `&self` method (not just relying on `Drop`)
    /// because of a real deadlock this crate's own tests hit: if a
    /// `serve_incoming` loop is running on a thread that holds its own
    /// `Arc<RpcChildProcess>` clone, `Drop::drop` never runs until *every*
    /// `Arc` is gone — including that thread's own clone — but that thread
    /// can't return (and drop its clone) until the connection closes, which
    /// only `Drop` (or this method) can trigger. Whichever code owns the
    /// `RpcChildProcess` and knows it's time to tear the conversation down
    /// needs to call this *before* dropping its last reference, not rely on
    /// the drop itself to do it.
    pub fn shutdown(&self) {
        let _ = self.child.lock().unwrap().kill();
    }
}

impl Drop for RpcChildProcess {
    /// Checked *before* killing it, same reasoning as `web-standards-tests`'
    /// own driver cleanup: if the child already exited on its own, `kill()`
    /// would just be a harmless no-op-with-an-error, but `try_wait()` first
    /// means a genuinely still-running child actually gets reaped instead
    /// of left as a zombie.
    fn drop(&mut self) {
        let mut child = self.child.lock().unwrap();
        match child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            Err(_) => {}
        }
    }
}
