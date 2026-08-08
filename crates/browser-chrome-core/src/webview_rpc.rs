//! A loopback HTTP server for talking JSON-RPC-shaped requests, or raw
//! unencoded binary, between a webview page and the host — see
//! `ROADMAP.md`'s backlog for the item this exists for ("secure
//! cross-platform RPC for talking between webviews and built-in browser
//! services").
//!
//! Deliberately *not* the same envelope/transport as `rpc_protocol`'s
//! stdio-based extension protocol — see that crate's own top-of-file doc
//! comment for why the two protocols share only `RpcError`, not a whole
//! wire format: a raw stdio pipe has no framing or routing of its own and
//! needs a hand-rolled protocol for both, but a page already has `fetch()`,
//! and HTTP already provides method routing (the URL path), error
//! signaling (status codes), and binary-native request/response bodies —
//! reinventing an envelope on top of that would just be redoing what HTTP
//! already does.
//!
//! No native script injection, no `console.log` hijacking, and no
//! per-platform delivery code at all: a page just calls `fetch()`, which
//! behaves identically on WebKitGTK/WKWebView/WebView2. This sidesteps two
//! real, documented problems with the *other* existing webview↔host
//! channel (`render_engine::CONSOLE_CAPTURE_SCRIPT`) outright — `wry` has
//! no `postMessage`-equivalent for host→page delivery at all on Linux/
//! macOS, and a real `wry` bug panics the process if `window.ipc.postMessage`
//! is ever called from a page whose current URL is a bare `file:///...` URI
//! — neither of those is a concern here, since nothing native ever touches
//! the page's own script context.
//!
//! Host-initiated push to a page is deliberately out of scope: HTTP is
//! page-initiates-request-only. A request body can carry raw binary, and a
//! response body can carry raw binary — that's what satisfies "binary data
//! in either direction" — but the host can't proactively send something to
//! a page without polling or a separate mechanism (e.g. Server-Sent Events
//! on top of this same server) being added later, if a real need for that
//! ever comes up.
//!
//! Method registry for this pass is deliberately empty by default — callers
//! of `WebviewRpcServer::start` supply whatever `RpcHandler`s they want
//! registered under `/rpc/<method>`. Wiring real browser functionality
//! behind real methods (rather than a test's own trivial ping/echo
//! handlers) is later work, once real internal pages exist to call them.

use rpc_protocol::RpcError;
use std::collections::HashMap;
use std::sync::Arc;

/// A JSON value or a raw, unencoded byte payload — the two shapes a
/// request or response body can take here. `serde_json` payloads and raw
/// binary payloads are told apart by the `Content-Type` header
/// (`application/json` vs `application/octet-stream`), not by inspecting
/// the bytes themselves.
#[derive(Debug, Clone, PartialEq)]
pub enum RpcBody {
    Json(serde_json::Value),
    Binary(Vec<u8>),
}

/// Handles one `/rpc/<method>` call's body and produces the response body,
/// or an `RpcError` to report back as a non-2xx response instead.
pub type RpcHandler = Box<dyn Fn(RpcBody) -> Result<RpcBody, RpcError> + Send + Sync>;

/// A running loopback HTTP RPC server — see this module's own doc comment
/// for the protocol. Bound to an OS-chosen ephemeral port (see `port()`) on
/// `start`, so it can't collide with `browser_core::bitwarden`'s default
/// `bw serve` port (8087) or `browser-windows-reactor`'s test-command TCP
/// listener.
pub struct WebviewRpcServer {
    server: Arc<tiny_http::Server>,
    join: Option<std::thread::JoinHandle<()>>,
    port: u16,
}

impl WebviewRpcServer {
    /// Binds and starts serving immediately, on a background thread —
    /// `handlers` is the complete method registry for this server's
    /// lifetime (no dynamic registration after `start`; a new server with a
    /// new registry is the way to change it).
    pub fn start(handlers: HashMap<String, RpcHandler>) -> std::io::Result<Self> {
        let server = Arc::new(
            tiny_http::Server::http("127.0.0.1:0").map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, format!("binding the webview RPC server failed: {err}")))?,
        );
        let port = server.server_addr().to_ip().expect("this server always binds an IP socket, not a unix one").port();

        let handlers = Arc::new(handlers);
        let server_for_thread = Arc::clone(&server);
        let join = std::thread::spawn(move || {
            while let Ok(request) = server_for_thread.recv() {
                handle_request(request, &handlers);
            }
        });

        Ok(Self { server, join: Some(join), port })
    }

    /// The loopback port this server is listening on — pass this to
    /// whatever page needs to reach it (e.g. embedded into an internal
    /// page's own HTML at serve time).
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for WebviewRpcServer {
    /// Same shutdown shape as `browser_core::bitwarden`'s own test-only
    /// `FakeServer` (and `web-standards-tests`' fixture servers) —
    /// `Server::unblock` is `tiny_http`'s own documented way to interrupt a
    /// blocking `recv()` call so the accept-loop thread can actually
    /// notice it's time to stop, instead of staying parked in `recv()`
    /// forever.
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn handle_request(mut request: tiny_http::Request, handlers: &HashMap<String, RpcHandler>) {
    // A page calling this server is essentially never same-origin with it —
    // even a future "internal page" served for item 3 (moving UI into a
    // webview) has no guarantee of sharing this exact ephemeral port,
    // and every fixture/consumer so far genuinely doesn't (this server
    // and whatever serves the calling page are two separate loopback
    // listeners with two separate ports, hence two different origins).
    // `POST` with a `Content-Type` of `application/json`/`application/
    // octet-stream` isn't a CORS "simple request," so browsers send an
    // `OPTIONS` preflight first — without answering it, and without
    // `Access-Control-Allow-Origin` on the real response too, `fetch()`
    // from a real page would never get past the browser's own CORS check
    // at all, real handler logic notwithstanding. `*` (not echoing the
    // request's actual `Origin`) is deliberate: everything this server
    // binds is loopback-only by construction (`127.0.0.1:0`), so there's
    // no cross-machine exposure this could open up regardless of which
    // origin is allowed to call it.
    if matches!(request.method(), tiny_http::Method::Options) {
        let response = tiny_http::Response::from_data(Vec::new())
            .with_status_code(204)
            .with_header(cors_origin_header())
            .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Methods"[..], &b"POST, OPTIONS"[..]).expect("static header is valid"))
            .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Headers"[..], &b"Content-Type"[..]).expect("static header is valid"));
        let _ = request.respond(response);
        return;
    }

    let method_name = request.url().trim_start_matches('/').trim_start_matches("rpc/").to_string();

    let is_binary = request.headers().iter().any(|header| header.field.equiv("Content-Type") && header.value.as_str() == "application/octet-stream");

    let mut raw_body = Vec::new();
    if let Err(err) = request.as_reader().read_to_end(&mut raw_body) {
        respond_error(request, 400, &RpcError { code: -32700, message: format!("reading request body failed: {err}"), data: None });
        return;
    }

    let body = if is_binary {
        RpcBody::Binary(raw_body)
    } else {
        match serde_json::from_slice::<serde_json::Value>(&raw_body) {
            Ok(value) => RpcBody::Json(value),
            Err(err) => {
                respond_error(request, 400, &RpcError { code: -32700, message: format!("request body is not valid JSON: {err}"), data: None });
                return;
            }
        }
    };

    let Some(handler) = handlers.get(&method_name) else {
        respond_error(request, 404, &RpcError { code: -32601, message: format!("unknown method {method_name:?}"), data: None });
        return;
    };

    match handler(body) {
        Ok(RpcBody::Json(value)) => {
            let bytes = serde_json::to_vec(&value).expect("serde_json::Value always serializes");
            let response = tiny_http::Response::from_data(bytes)
                .with_status_code(200)
                .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("static header is valid"))
                .with_header(cors_origin_header());
            let _ = request.respond(response);
        }
        Ok(RpcBody::Binary(bytes)) => {
            let response = tiny_http::Response::from_data(bytes)
                .with_status_code(200)
                .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/octet-stream"[..]).expect("static header is valid"))
                .with_header(cors_origin_header());
            let _ = request.respond(response);
        }
        Err(rpc_error) => respond_error(request, 400, &rpc_error),
    }
}

fn respond_error(request: tiny_http::Request, status: u16, error: &RpcError) {
    let bytes = serde_json::to_vec(error).expect("RpcError always serializes");
    let response = tiny_http::Response::from_data(bytes)
        .with_status_code(status)
        .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("static header is valid"))
        .with_header(cors_origin_header());
    let _ = request.respond(response);
}

/// See the CORS explanation on `handle_request`'s own `OPTIONS` branch for
/// why every response (not just the preflight) needs this.
fn cors_origin_header() -> tiny_http::Header {
    tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).expect("static header is valid")
}
