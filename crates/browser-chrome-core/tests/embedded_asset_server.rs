//! Drives a real `EmbeddedAssetServer` (over the browser's own real
//! embedded `assets/`, not a synthetic fixture) with plain HTTP calls (via
//! `ureq`) to prove the server's own behavior — no GUI needed for this;
//! `fetch()` from real page JS is exercised separately, for real, in
//! `browser-linux-gtk3`'s test suite, matching `webview_rpc.rs`'s own test
//! split.

use browser_chrome_core::{embedded_assets, EmbeddedAssetServer};

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(ureq::Agent::config_builder().http_status_as_error(false).build())
}

fn start_server() -> EmbeddedAssetServer {
    EmbeddedAssetServer::start(embedded_assets(), "index.html").expect("starting the embedded asset server should succeed")
}

#[test]
fn serves_index_html_at_the_root_path() {
    let server = start_server();
    let url = format!("http://127.0.0.1:{}/", server.port());
    let mut response = agent().get(&url).call().expect("request should succeed");
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers().get("Content-Type").unwrap(), "text/html; charset=utf-8");
    let body = response.body_mut().read_to_string().unwrap();
    assert!(body.contains("<title>embedded assets example</title>"), "unexpected body: {body}");
}

#[test]
fn serves_css_and_js_with_correct_content_types() {
    let server = start_server();

    let css_url = format!("http://127.0.0.1:{}/style.css", server.port());
    let mut css_response = agent().get(&css_url).call().expect("request should succeed");
    assert_eq!(css_response.status(), 200);
    assert_eq!(css_response.headers().get("Content-Type").unwrap(), "text/css; charset=utf-8");
    assert!(css_response.body_mut().read_to_string().unwrap().contains("rgb(18, 52, 86)"));

    let js_url = format!("http://127.0.0.1:{}/app.js", server.port());
    let mut js_response = agent().get(&js_url).call().expect("request should succeed");
    assert_eq!(js_response.status(), 200);
    assert_eq!(js_response.headers().get("Content-Type").unwrap(), "text/javascript; charset=utf-8");
    assert!(js_response.body_mut().read_to_string().unwrap().contains("embedded_assets_loaded"));
}

#[test]
fn a_missing_path_returns_404() {
    let server = start_server();
    let url = format!("http://127.0.0.1:{}/does-not-exist.html", server.port());
    let response = agent().get(&url).call().expect("request should succeed");
    assert_eq!(response.status(), 404);
}

#[test]
fn responses_carry_a_permissive_cors_header() {
    let server = start_server();
    let url = format!("http://127.0.0.1:{}/", server.port());
    let response = agent().get(&url).call().expect("request should succeed");
    assert_eq!(response.headers().get("Access-Control-Allow-Origin").unwrap(), "*");
}

#[test]
fn each_server_gets_its_own_distinct_ephemeral_port() {
    let first = start_server();
    let second = start_server();
    assert_ne!(first.port(), second.port());
}
