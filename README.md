# claude-browser

A cross-platform web browser written in Rust. No tabs, by design.

## Architecture

- **Content rendering** is handled by the OS-native webview via [`wry`](https://github.com/tauri-apps/wry) (WebView2 on Windows, WKWebView on macOS/iOS, WebKitGTK on Linux, system WebView on Android).
- **Chrome (window, address bar, buttons)** is built with each platform's native UI toolkit rather than a shared cross-platform GUI toolkit — GTK (`gtk-rs`) on Linux today, with Win32/WinUI and AppKit planned for Windows/macOS. This avoids an event-loop conflict that shared toolkits like `iced` would introduce with `wry`/`tao`.
- Chrome code never depends on `wry` directly — it talks to a `RenderEngine` trait (`crates/render-engine`), so the underlying engine can be swapped later (e.g. Servo, CEF, a custom engine) without touching the app.

Currently only the Linux target (`crates/browser-linux`) is implemented.

## Installing dependencies

### Rust

Install via [rustup](https://rustup.rs):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable
```

### Linux (GTK3 + WebKitGTK)

On Ubuntu/Debian:

```sh
sudo apt update
sudo apt install -y build-essential libgtk-3-dev libwebkit2gtk-4.1-dev
```

(If `libwebkit2gtk-4.1-dev` isn't available on your distro version, use `libwebkit2gtk-4.0-dev` instead.)

## Building

```sh
cargo build
```

## Running

```sh
cargo run -p browser-linux
```

This opens a native GTK window with an address bar and back / forward / reload buttons. Type a URL into the address bar and press Enter to navigate.

## Testing

There's an example-based regression test that drives navigation (load, navigate, back, forward, reload) against local fixture pages and checks the webview's URL after each step:

```sh
cargo run --example nav_test -p browser-linux
```

It prints `PASS`/`FAIL` per step and exits non-zero if anything fails.
