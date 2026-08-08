//! Trivial test fixture for `rpc-protocol`'s `tests/stdio_roundtrip.rs` —
//! reads `RpcMessage`s from stdin, over the same `rpc_protocol::stdio`
//! framing the real client uses, and:
//! - `"echo"`: replies with the same params (and binary attachment, if
//!   any) it was given.
//! - `"trigger_extension_initiated_call"`: sends its own request back to
//!   the host (method `"hello_from_extension"`) *before* replying, waits
//!   for the host's response, then replies to the original request with
//!   whatever the host sent back — this is what `stdio_roundtrip.rs`'s
//!   extension-initiated-request test drives, keeping that whole exchange
//!   self-contained in one request/response from the test's own point of
//!   view rather than needing the test itself to juggle two threads.
//! - `"crash"`: exits immediately without replying, to exercise the real
//!   client's "child died mid-request" error path.
//! - `"send_garbage"`: writes a deliberately malformed (non-JSON) line
//!   straight to stdout instead of a real `Response`, to exercise the real
//!   client's "malformed message from the child" error path.
//!
//! Deliberately not built with the same rigor as `stdio_client.rs`'s real
//! `RpcChildProcess` (e.g. the hardcoded `id: 1` below would need to be
//! unique per call in a general-purpose client) — this only ever talks to
//! one controlled test, doing one thing at a time, so a small hand-rolled
//! loop is enough; it isn't meant to be a second reusable client
//! implementation.

use rpc_protocol::{read_message, write_message, RpcMessage};
use std::io::{stdin, stdout, BufReader, Write};

fn main() {
    let mut reader = BufReader::new(stdin());
    let mut writer = stdout();

    loop {
        let Some((message, binary)) = read_message(&mut reader).expect("fixture: malformed message from host") else {
            break; // host closed stdin — nothing left to do
        };
        let RpcMessage::Request { id, method, params, .. } = message else {
            continue; // fixture only ever reacts to Requests
        };

        match method.as_str() {
            "echo" => {
                let response = RpcMessage::Response { id, result: Ok(params), binary_len: binary.as_ref().map(|bytes| bytes.len() as u64) };
                write_message(&mut writer, &response, binary.as_deref()).expect("fixture: writing echo response failed");
            }
            "trigger_extension_initiated_call" => {
                let outbound = RpcMessage::Request { id: 1, method: "hello_from_extension".to_string(), params: serde_json::json!({"greeting": "hi"}), binary_len: None };
                write_message(&mut writer, &outbound, None).expect("fixture: writing extension-initiated request failed");

                let (reply, _) = read_message(&mut reader)
                    .expect("fixture: malformed reply to extension-initiated request")
                    .expect("fixture: host closed the connection before replying");
                let RpcMessage::Response { result, .. } = reply else {
                    panic!("fixture: expected a Response to the extension-initiated request, got {reply:?}");
                };

                let response = RpcMessage::Response { id, result, binary_len: None };
                write_message(&mut writer, &response, None).expect("fixture: writing trigger response failed");
            }
            "crash" => std::process::exit(1),
            "send_garbage" => {
                writer.write_all(b"not valid json\n").expect("fixture: writing garbage failed");
                writer.flush().expect("fixture: flushing garbage failed");
            }
            other => panic!("fixture: unknown method {other:?}"),
        }
    }
}
