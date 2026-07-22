# claude-browser

A cross-platform web browser written in Rust. No tabs, by design.

## Architecture

- **Content rendering** is handled by the OS-native webview via [`wry`](https://github.com/tauri-apps/wry) (WebView2 on Windows, WKWebView on macOS/iOS, WebKitGTK on Linux, system WebView on Android).
- **Chrome (window, address bar, buttons)** is built with each platform's native UI toolkit rather than a shared cross-platform GUI toolkit — GTK (`gtk-rs`) on Linux today, with Win32/WinUI and AppKit planned for Windows/macOS. This avoids an event-loop conflict that shared toolkits like `iced` would introduce with `wry`/`tao`.
- Chrome code never depends on `wry` directly — it talks to a `RenderEngine` trait (`crates/render-engine`), so the underlying engine can be swapped later (e.g. Servo, CEF, a custom engine) without touching the app.

Linux (`crates/browser-linux-gtk3`) is implemented and working. Windows (`crates/browser-windows-win32`) has a
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

This builds the whole workspace on any host. Each platform-specific crate
(`browser-linux-gtk3`, `browser-windows-win32`, `browser-windows-nwg`) is gated on its own
`target_os` — on a platform it doesn't apply to, it compiles to an empty stub binary (which just
prints a one-line explanation if you run it) instead of failing, so a bare `cargo build` never
needs `--exclude` flags no matter which of these you're on.

The same is true cross-target: `cargo build --target x86_64-pc-windows-gnu` (see below) or
[`cross build --target x86_64-pc-windows-gnu`](https://github.com/cross-rs/cross) both build the
whole workspace too — the Windows crates build for real and `browser-linux-gtk3` becomes the stub.

## Running

```sh
cargo run -p browser-linux-gtk3     # Linux
cargo run -p browser-windows-win32  # Windows (native build)
```

Each opens a native window (GTK on Linux, Win32 on Windows) with an address bar and back / forward /
reload buttons. Type a URL into the address bar and press Enter (or, on Windows, click Go) to navigate.

### Cross-compiling and running the Windows crates from Linux

Useful for testing the native window/chrome/message-loop code without a Windows machine — WebView2 itself
won't work this way (see below), but everything else can be exercised end-to-end. Two ways to get the
mingw-w64 toolchain in place:

**Option A — local mingw-w64:**

```sh
rustup target add x86_64-pc-windows-gnu
sudo apt install -y mingw-w64 wine
cargo build --target x86_64-pc-windows-gnu -p browser-windows-win32
```

**Option B — [`cross`](https://github.com/cross-rs/cross), via Docker (no local mingw-w64 install needed):**

```sh
cargo install cross --locked
cross build --target x86_64-pc-windows-gnu -p browser-windows-win32
```

`cross`'s default image for this target bundles a mingw-w64 too old for the `GetHostNameW` symbol current
Rust `std` needs — if you see a linker error mentioning it, pin the `:edge` image tag in a `Cross.toml`:

```toml
[target.x86_64-pc-windows-gnu]
image = "ghcr.io/cross-rs/x86_64-pc-windows-gnu:edge"
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
wine target/x86_64-pc-windows-gnu/debug/browser-windows-win32.exe
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

`browser-linux-gtk3` has two example-based regression tests that drive the actual GTK app end-to-end against
local fixture pages, printing `PASS`/`FAIL` per check and exiting non-zero if anything fails:

```sh
cargo run --example nav_test -p browser-linux-gtk3       # navigate/back/forward/reload
cargo run --example switcher_test -p browser-linux-gtk3  # multi-page switcher, search, loaded/unloaded limit
```
