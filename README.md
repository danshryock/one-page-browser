# claude-browser

A cross-platform web browser written in Rust. No tabs, by design.

## Architecture

- **Content rendering** is handled by the OS-native webview via [`wry`](https://github.com/tauri-apps/wry) (WebView2 on Windows, WKWebView on macOS/iOS, WebKitGTK on Linux, system WebView on Android).
- **Chrome (window, address bar, buttons)** is built with each platform's native UI toolkit rather than a shared cross-platform GUI toolkit — GTK (`gtk-rs`) on Linux today, with Win32/WinUI and AppKit planned for Windows/macOS. This avoids an event-loop conflict that shared toolkits like `iced` would introduce with `wry`/`tao`.
- Chrome code never depends on `wry` directly — it talks to a `RenderEngine` trait (`crates/render-engine`), so the underlying engine can be swapped later (e.g. Servo, CEF, a custom engine) without touching the app.

Linux (`crates/browser-linux`) is implemented and working. Windows (`crates/browser-windows`) has a
first-milestone native Win32 chrome (single page, back/forward/reload, address bar — no switcher grid
yet) that type-checks cleanly against the real `windows` crate, but **has never been linked or run** —
it was written on a Linux machine with no Windows/WebView2 toolchain available. Build and run it on an
actual Windows machine and report back anything that breaks. macOS isn't started yet.

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

### Windows (WebView2)

The [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) — preinstalled on
current Windows 10/11, otherwise download and install the Evergreen Bootstrapper from that page. No
other native dependency is needed beyond the Windows SDK headers/libs that ship with the standard Rust
MSVC toolchain (`rustup default stable-msvc`) or with a full Visual Studio / Build Tools install.

## Building

```sh
cargo build
```

On Linux, this fails on `browser-windows` — it depends on the `windows` crate, which doesn't build
outside Windows. Build the Linux pieces explicitly instead:

```sh
cargo build --workspace --exclude browser-windows
```

## Running

```sh
cargo run -p browser-linux   # Linux
cargo run -p browser-windows # Windows — untested, see the warning above
```

Each opens a native window (GTK on Linux, Win32 on Windows) with an address bar and back / forward /
reload buttons. Type a URL into the address bar and press Enter (or, on Windows, click Go) to navigate.

## Testing

There's an example-based regression test that drives navigation (load, navigate, back, forward, reload) against local fixture pages and checks the webview's URL after each step:

```sh
cargo run --example nav_test -p browser-linux
```

It prints `PASS`/`FAIL` per step and exits non-zero if anything fails.
