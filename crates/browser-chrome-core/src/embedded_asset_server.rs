//! A loopback HTTP server for serving static pages/assets straight out of a
//! `zip_assets::ZipAssets` — no disk extraction at any point. Used both for
//! the browser's own embedded pages (`embedded_assets::assets()`, backed by
//! a compile-time `include_bytes!`'d archive) and, later, an extension's
//! own frontend files (a `ZipAssets` built from a zip read off disk at
//! runtime instead) — same server, same mechanism, different byte source.
//!
//! Deliberately a separate server/module from `webview_rpc::WebviewRpcServer`,
//! not a routing branch bolted onto it: that module is narrowly scoped to
//! JSON-RPC-shaped `/rpc/<method>` request/response (see its own doc
//! comment), with no `Content-Type`-by-extension logic, no default-index
//! behavior, and no notion of "serve whatever's at this path" at all —
//! forcing static-asset serving into that shape would be scope creep on a
//! module that's deliberately stayed narrow. This one reuses the *pattern*
//! (`tiny_http::Server::http("127.0.0.1:0")`, a background accept thread,
//! `Server::unblock()` + join in `Drop`), not the code.
use std::sync::Arc;
use zip_assets::ZipAssets;

pub struct EmbeddedAssetServer {
    server: Arc<tiny_http::Server>,
    join: Option<std::thread::JoinHandle<()>>,
    port: u16,
}

impl EmbeddedAssetServer {
    /// Binds and starts serving immediately, on a background thread. `/`
    /// serves `index`'s entry (e.g. `"index.html"`); any other path serves
    /// whatever `assets` has at that path (leading `/` stripped), or a 404
    /// if nothing matches.
    pub fn start(assets: Arc<ZipAssets>, index: impl Into<String>) -> std::io::Result<Self> {
        let server = Arc::new(
            tiny_http::Server::http("127.0.0.1:0").map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, format!("binding the embedded asset server failed: {err}")))?,
        );
        let port = server.server_addr().to_ip().expect("this server always binds an IP socket, not a unix one").port();

        let index = index.into();
        let server_for_thread = Arc::clone(&server);
        let join = std::thread::spawn(move || {
            while let Ok(request) = server_for_thread.recv() {
                handle_request(request, &assets, &index);
            }
        });

        Ok(Self { server, join: Some(join), port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for EmbeddedAssetServer {
    /// Same shutdown shape as `webview_rpc::WebviewRpcServer` (and every
    /// other `tiny_http`-backed server in this workspace) — `unblock()` is
    /// `tiny_http`'s own documented way to interrupt a blocking `recv()`
    /// call so the accept-loop thread actually notices it's time to stop.
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn handle_request(request: tiny_http::Request, assets: &ZipAssets, index: &str) {
    let requested = request.url().trim_start_matches('/');
    let path = if requested.is_empty() { index } else { requested };

    match assets.read(path) {
        Ok(bytes) => {
            let response = tiny_http::Response::from_data(bytes)
                .with_status_code(200)
                .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], mime_type_for(path).as_bytes()).expect("computed content-type header should be valid"))
                .with_header(cors_origin_header());
            let _ = request.respond(response);
        }
        Err(_) => {
            let response = tiny_http::Response::from_data(format!("not found: {path}").into_bytes())
                .with_status_code(404)
                .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/plain; charset=utf-8"[..]).expect("static header is valid"))
                .with_header(cors_origin_header());
            let _ = request.respond(response);
        }
    }
}

/// A small, static extension→MIME-type table — deliberately not a new
/// dependency (e.g. the `mime_guess` crate) for the handful of types a
/// page's own assets realistically need; `application/octet-stream` covers
/// anything unlisted.
fn mime_type_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Same reasoning as `webview_rpc.rs`'s identically-named helper: every
/// origin this server could ever be fetched from is loopback-only by
/// construction (`127.0.0.1:0`), so allowing any of them costs nothing
/// real. A page served from here might need to `fetch()` a *different*
/// loopback server (a `WebviewRpcServer` instance, or another
/// `EmbeddedAssetServer`) — same cross-origin situation, same fix.
fn cors_origin_header() -> tiny_http::Header {
    tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).expect("static header is valid")
}
