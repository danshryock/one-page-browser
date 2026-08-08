//! Drives a real `WebviewRpcServer` with plain HTTP calls (via `ureq`) to
//! prove the transport works — no GUI/webview needed for this: `fetch()`
//! from real page JS is exercised separately, for real, in
//! `browser-linux-gtk3`'s test suite (see that crate's own new test), since
//! that's the thing this module exists to be reachable from; this test is
//! about the server's own HTTP-layer behavior in isolation.

use browser_chrome_core::{RpcBody, RpcHandler, WebviewRpcServer};
use rpc_protocol::RpcError;
use std::collections::HashMap;

fn start_server_with_ping_and_echo_binary() -> WebviewRpcServer {
    let mut handlers: HashMap<String, RpcHandler> = HashMap::new();
    handlers.insert(
        "ping".to_string(),
        Box::new(|body: RpcBody| -> Result<RpcBody, RpcError> {
            match body {
                RpcBody::Json(value) => Ok(RpcBody::Json(value)),
                RpcBody::Binary(_) => Err(RpcError { code: -32602, message: "ping expects a JSON body".to_string(), data: None }),
            }
        }),
    );
    handlers.insert(
        "echo_binary".to_string(),
        Box::new(|body: RpcBody| -> Result<RpcBody, RpcError> {
            match body {
                RpcBody::Binary(bytes) => Ok(RpcBody::Binary(bytes)),
                RpcBody::Json(_) => Err(RpcError { code: -32602, message: "echo_binary expects a binary body".to_string(), data: None }),
            }
        }),
    );
    WebviewRpcServer::start(handlers).expect("starting the webview RPC server should succeed")
}

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(ureq::Agent::config_builder().http_status_as_error(false).build())
}

#[test]
fn json_request_and_response_round_trip() {
    let server = start_server_with_ping_and_echo_binary();
    let url = format!("http://127.0.0.1:{}/rpc/ping", server.port());
    let mut response = agent().post(&url).send_json(serde_json::json!({"hello": "world"})).expect("request should succeed");
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers().get("Content-Type").unwrap(), "application/json");
    let body: serde_json::Value = response.body_mut().read_json().unwrap();
    assert_eq!(body, serde_json::json!({"hello": "world"}));
}

#[test]
fn binary_request_and_response_round_trip_unencoded() {
    let server = start_server_with_ping_and_echo_binary();
    let url = format!("http://127.0.0.1:{}/rpc/echo_binary", server.port());
    let payload: Vec<u8> = vec![0, 1, 2, 255, 254, b'\n', b'{', b'"', b'}'];
    let mut response = agent().post(&url).header("Content-Type", "application/octet-stream").send(&payload).expect("request should succeed");
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers().get("Content-Type").unwrap(), "application/octet-stream");
    let body = response.body_mut().read_to_vec().unwrap();
    assert_eq!(body, payload);
}

#[test]
fn a_handler_error_becomes_a_json_error_body_with_a_non_2xx_status() {
    let server = start_server_with_ping_and_echo_binary();
    let url = format!("http://127.0.0.1:{}/rpc/ping", server.port());
    let mut response = agent().post(&url).header("Content-Type", "application/octet-stream").send(&[1, 2, 3]).expect("request should succeed");
    assert_eq!(response.status(), 400);
    let body: RpcError = response.body_mut().read_json().unwrap();
    assert_eq!(body.code, -32602);
}

#[test]
fn an_unknown_method_returns_404_with_a_json_error_body() {
    let server = start_server_with_ping_and_echo_binary();
    let url = format!("http://127.0.0.1:{}/rpc/does_not_exist", server.port());
    let mut response = agent().post(&url).send_json(serde_json::json!({})).expect("request should succeed");
    assert_eq!(response.status(), 404);
    let body: RpcError = response.body_mut().read_json().unwrap();
    assert_eq!(body.code, -32601);
}

#[test]
fn malformed_json_body_returns_400_with_a_json_error_body() {
    let server = start_server_with_ping_and_echo_binary();
    let url = format!("http://127.0.0.1:{}/rpc/ping", server.port());
    let mut response = agent().post(&url).header("Content-Type", "application/json").send("not valid json").expect("request should succeed");
    assert_eq!(response.status(), 400);
    let body: RpcError = response.body_mut().read_json().unwrap();
    assert_eq!(body.code, -32700);
}

#[test]
fn a_cors_preflight_request_is_answered_so_a_real_page_fetch_can_get_past_it() {
    // The page calling this server is essentially never same-origin with it
    // (see `webview_rpc.rs`'s own `handle_request` doc comment) — a real
    // `POST` with a JSON/binary `Content-Type` isn't a CORS "simple
    // request," so a real browser sends this `OPTIONS` preflight *before*
    // ever attempting the real call. Exercised directly here (not just
    // implicitly via the JSON/binary tests above, which only ever send the
    // real `POST`) since a browser — not `ureq` — is what actually enforces
    // this, so nothing here proves it works from a real page; the real
    // proof is `browser-linux-gtk3`'s own new test, driving a genuine
    // cross-origin `fetch()` from inside a real webview.
    let server = start_server_with_ping_and_echo_binary();
    let url = format!("http://127.0.0.1:{}/rpc/ping", server.port());
    let request = ureq::http::Request::builder().method("OPTIONS").uri(&url).body(()).unwrap();
    let response = agent().run(request).expect("preflight request should succeed");
    assert_eq!(response.status(), 204);
    assert_eq!(response.headers().get("Access-Control-Allow-Origin").unwrap(), "*");
    assert_eq!(response.headers().get("Access-Control-Allow-Methods").unwrap(), "POST, OPTIONS");
    assert_eq!(response.headers().get("Access-Control-Allow-Headers").unwrap(), "Content-Type");
}

#[test]
fn each_server_gets_its_own_distinct_ephemeral_port() {
    let first = start_server_with_ping_and_echo_binary();
    let second = start_server_with_ping_and_echo_binary();
    assert_ne!(first.port(), second.port());
}
