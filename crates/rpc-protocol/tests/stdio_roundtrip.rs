//! Exercises `RpcChildProcess` against a real spawned process (the
//! `echo-extension-fixture` binary in `tests/fixtures/echo_extension.rs`),
//! not a mock — matching this codebase's established preference for
//! testing IPC/process-boundary code end-to-end (e.g. `browser_core::
//! bitwarden`'s tests spin up a real `tiny_http` server rather than mocking
//! `PasswordBackend`).

use rpc_protocol::RpcChildProcess;
use std::process::Command;
use std::sync::Arc;

fn spawn_fixture() -> RpcChildProcess {
    RpcChildProcess::spawn(Command::new(env!("CARGO_BIN_EXE_echo-extension-fixture"))).expect("spawning the echo fixture should succeed")
}

#[test]
fn json_only_request_response_round_trips() {
    let child = spawn_fixture();
    let (result, binary) = child.call("echo", serde_json::json!({"x": 1, "y": "two"}), None).expect("echo call should succeed");
    assert_eq!(result, serde_json::json!({"x": 1, "y": "two"}));
    assert_eq!(binary, None);
}

#[test]
fn binary_attachment_round_trips_in_both_directions() {
    let child = spawn_fixture();
    let payload = vec![0u8, 1, 2, 255, 254, b'\n'];
    let (result, binary) = child.call("echo", serde_json::json!({"note": "has binary"}), Some(&payload)).expect("echo call with binary should succeed");
    assert_eq!(result, serde_json::json!({"note": "has binary"}));
    assert_eq!(binary, Some(payload));
}

#[test]
fn concurrent_out_of_order_requests_are_correlated_correctly() {
    let child = Arc::new(spawn_fixture());
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let child = Arc::clone(&child);
            std::thread::spawn(move || child.call("echo", serde_json::json!({"i": i}), None).expect("echo call should succeed").0)
        })
        .collect();
    for (i, handle) in handles.into_iter().enumerate() {
        assert_eq!(handle.join().unwrap(), serde_json::json!({"i": i}));
    }
}

#[test]
fn extension_initiated_request_round_trips_through_serve_incoming() {
    let child = Arc::new(spawn_fixture());
    let server = Arc::clone(&child);
    let serving = std::thread::spawn(move || {
        server.serve_incoming(|method, params, _binary| {
            assert_eq!(method, "hello_from_extension");
            assert_eq!(params, serde_json::json!({"greeting": "hi"}));
            Ok((serde_json::json!({"acknowledged": true}), None))
        });
    });

    // Round trip: this call tells the fixture to call *us* first
    // ("hello_from_extension", handled by `serve_incoming` above), and the
    // fixture then replies to this very call with whatever result it got
    // back from us — so asserting on this call's own result proves the
    // whole extension-initiated exchange actually happened, not just that
    // the outer request/response worked.
    let (result, _) = child.call("trigger_extension_initiated_call", serde_json::json!({}), None).expect("trigger call should succeed");
    assert_eq!(result, serde_json::json!({"acknowledged": true}));

    // `shutdown()`, not just `drop(child)`: the `serving` thread holds its
    // own `Arc` clone (`server`), so `Drop` alone would never run until
    // *that* thread already exited — which it can't do until the
    // connection closes. `shutdown()` is what actually closes it (see its
    // own doc comment).
    child.shutdown();
    drop(child);
    serving.join().expect("serve_incoming thread should exit cleanly once the connection closes");
}

#[test]
fn a_malformed_line_from_the_child_fails_the_call_without_wedging_the_connection() {
    let child = spawn_fixture();
    let err = child.call("send_garbage", serde_json::json!({}), None).expect_err("a malformed reply should surface as an error, not hang");
    assert!(!err.message.is_empty());

    // The whole connection is torn down as a unit on a framing error (see
    // `RpcChildProcess::spawn`'s doc comment) — a later call on the same
    // handle should fail cleanly too, not hang waiting on a reader thread
    // that already gave up.
    let second_err = child.call("echo", serde_json::json!({}), None).expect_err("the connection should already be considered closed");
    assert!(!second_err.message.is_empty());
}

#[test]
fn a_crashed_child_fails_a_pending_call_instead_of_hanging() {
    let child = spawn_fixture();
    let err = child.call("crash", serde_json::json!({}), None).expect_err("a child that exits mid-request should surface an error, not hang");
    assert!(!err.message.is_empty());
}
