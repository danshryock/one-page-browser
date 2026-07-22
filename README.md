# claude-browser

A cross-platform web browser written in Rust. No tabs, by design.

## Architecture

- **Content rendering** is handled by the OS-native webview via [`wry`](https://github.com/tauri-apps/wry) (WebView2 on Windows, WKWebView on macOS/iOS, WebKitGTK on Linux, system WebView on Android).
- **Chrome (window, address bar, buttons)** is built with each platform's native UI toolkit rather than a shared cross-platform GUI toolkit — GTK (`gtk-rs`) on Linux today, with Win32/WinUI and AppKit planned for Windows/macOS. This avoids an event-loop conflict that shared toolkits like `iced` would introduce with `wry`/`tao`.
- Chrome code never depends on `wry` directly — it talks to a `RenderEngine` trait (`crates/render-engine`), so the underlying engine can be swapped later (e.g. Servo, CEF, a custom engine) without touching the app.

Linux (`crates/browser-linux`) is implemented and working. Windows (`crates/browser-windows`) has a
first-milestone native Win32 chrome (single page, back/forward/reload, address bar — no switcher grid
yet). It's been cross-compiled and run from this same Linux dev machine (see below) — the window, toolbar,
and message loop are confirmed working under Wine, but WebView2 itself (the actual Edge-based rendering
engine, distinct from the small `WebView2Loader.dll` stub) isn't available under Wine, so the content area
won't render there. A window still opens if the webview fails to initialize (logged, not fatal) rather
than the whole app silently failing to launch — test the actual browsing on a real Windows machine.
macOS isn't started yet.

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
cargo run -p browser-windows # Windows (native build)
```

Each opens a native window (GTK on Linux, Win32 on Windows) with an address bar and back / forward /
reload buttons. Type a URL into the address bar and press Enter (or, on Windows, click Go) to navigate.

### Cross-compiling and running `browser-windows` from Linux

Useful for testing the native window/chrome/message-loop code without a Windows machine — WebView2 itself
won't work this way (see below), but everything else can be exercised end-to-end.

```sh
rustup target add x86_64-pc-windows-gnu
sudo apt install -y mingw-w64 wine
cargo build --target x86_64-pc-windows-gnu -p browser-windows
```

`webview2-com-sys`'s build script vendors `WebView2Loader.dll` but doesn't copy it next to the binary —
real Windows/WebView2 projects need this DLL alongside the exe, so copy it manually the first time (or
after a clean build):

```sh
cp target/x86_64-pc-windows-gnu/debug/build/webview2-com-sys-*/out/x64/WebView2Loader.dll \
   target/x86_64-pc-windows-gnu/debug/
```

Then run it under Wine:

```sh
wine target/x86_64-pc-windows-gnu/debug/browser-windows.exe
```

The window, toolbar, and address bar come up normally. Expect (and it's fine) to see this in the
output — Wine has no real WebView2 Runtime, so the content area never initializes, but the window and
chrome still work:

```
failed to create webview: WebView2 error: WindowsError(Error { code: HRESULT(0x80070002), message: "File not found." })
```

## Testing

`browser-core`'s page/tab-management logic (load/unload tracking, the loaded-pages limit) is pure state
machine logic tested with real unit tests against a mock engine — no GTK or webview involved:

```sh
cargo test -p browser-core
```

`browser-linux` has two example-based regression tests that drive the actual GTK app end-to-end against
local fixture pages, printing `PASS`/`FAIL` per check and exiting non-zero if anything fails:

```sh
cargo run --example nav_test -p browser-linux       # navigate/back/forward/reload
cargo run --example switcher_test -p browser-linux  # multi-page switcher, search, loaded/unloaded limit
```
