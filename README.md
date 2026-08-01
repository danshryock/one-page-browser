# claude-browser

A cross-platform web browser written in Rust. No tabs, by design.

## Architecture

- **Content rendering** is handled by the OS-native webview via [`wry`](https://github.com/tauri-apps/wry) (WebView2 on Windows, WKWebView on macOS/iOS, WebKitGTK on Linux, system WebView on Android) — except `browser-windows-winui`, which wraps WinUI 3's own native `WebView2` XAML control directly (see below).
- **Chrome (window, address bar, buttons)** is built with each platform's native UI toolkit rather than a shared cross-platform GUI toolkit — GTK (`gtk-rs`) on Linux, WinUI 3 on Windows, AppKit on macOS. This avoids an event-loop conflict that shared toolkits like `iced` would introduce with `wry`/`tao`.
- Chrome code never depends on `wry` directly — it talks to a `RenderEngine` trait (`crates/render-engine`), so the underlying engine can be swapped later (e.g. Servo, CEF, a custom engine) without touching the app.

See `ARCHITECTURE.md` for a deeper look at what's shared across platforms today, where duplication remains,
and planned refactoring to close that gap.

Linux (`crates/browser-linux-gtk3`) is implemented, working, and has real `cargo test`-integrated regression
tests (see "Testing" below).

Windows has two front ends. `crates/browser-windows-winui` uses WinUI 3 (`Microsoft.UI.Xaml`) — the modern
Fluent-design Windows toolkit, via the [`winio-winui3`](https://github.com/compio-rs/winio3-rs) bindings
crate — wrapping WinUI 3's own native `Microsoft.UI.Xaml.Controls.WebView2` XAML control directly
(`render-engine`'s `WebView2Engine`, gated to the `x86_64-pc-windows-msvc` target) rather than going through
`wry`, since it's a real XAML `FrameworkElement` that participates in ordinary Grid/StackPanel layout with
no manual resize plumbing needed. It's cross-compile-only — see "browser-windows-winui: building" below —
the Windows App SDK runtime it needs isn't available under Wine, so it's never actually been run, only
cross-compiled, cross-linked, and inspected.

`crates/browser-windows-reactor` is a second WinUI 3 front end, built on Microsoft's own in-tree
`windows-reactor`/`windows-webview` crates (a declarative, React-like UI model) instead of `winio-winui3`'s
imperative widget-tree style — see that crate's module doc comment for why both exist side by side. Unlike
`browser-windows-winui`, this one *has* been run for real, in a Windows VM used for interactive testing —
see `ROADMAP.md` for the debugging history.

`crates/browser-macos-appkit` uses AppKit directly via `objc2`/`objc2-app-kit`, with `wry`'s `WKWebView`
embedding for content — see "browser-macos-appkit: building" below. Cross-compiles from this Linux dev
machine to real, linked Mach-O binaries for both `aarch64-apple-darwin` and `x86_64-apple-darwin`; real
runtime verification happens on GitHub's native macOS runners (`.github/workflows/macos.yml`), since there's
no way to *run* a macOS binary from Linux the way the Windows front ends can be cross-compiled and run
under Wine.

## Installing dependencies

Everything below is still manual, run-by-hand setup — see `BUILD_AUTOMATION.md` for what's already
automated, what isn't yet, and the planned scripts that would close the gap.

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

This builds the whole workspace on any host. Each `target_os`-gated crate (`browser-linux-gtk3`,
`browser-windows-winui`, `browser-windows-reactor`, `browser-macos-appkit`) compiles to an empty stub
binary on a platform it doesn't apply to (which just prints a one-line explanation if you run it) instead
of failing, so a bare `cargo build` never needs `--exclude` flags no matter which of these you're on.

## Running

```sh
cargo run -p browser-linux-gtk3     # Linux
```

Opens a native GTK window with an address bar and back / forward / reload buttons. Type a URL into the
address bar and press Enter to navigate.

```sh
cargo run-gtk3   # same as `cargo run -p browser-linux-gtk3`
```

### Launching with a URL

`browser-linux-gtk3`/`browser-windows-winui`/`browser-windows-reactor`/`browser-macos-appkit` all accept a
bare URL argument (the shape a real OS-level "open with"/default-browser handoff would use, e.g.
`browser-linux-gtk3 https://example.com`) — instead of opening the normal window straight away, this shows a
small standalone chooser first: the URL, a profile field pre-filled from `--profile` (or `"default"`), and
Open/Cancel. Picking a profile and clicking Open opens the real browser window scoped to that profile with
the URL as its first page. Handing the URL off to an already-running instance of the browser, and a
separate "choose which installed browser to use at all" picker, are both later work, not implemented yet.

### browser-windows-winui / browser-windows-reactor: building

Both are cross-compile-only — never run, even under Wine, since WinUI 3 needs the real Windows App SDK
runtime installed (`browser-windows-reactor` *has* been run for real, but only in an actual Windows VM used
for interactive testing — see `ROADMAP.md`). Cross-compiling to `x86_64-pc-windows-msvc` needs
[`cargo-xwin`](https://github.com/rust-cross/cargo-xwin) (which downloads and caches the Windows SDK + MSVC
CRT via [`xwin`](https://github.com/Jake-Shadle/xwin) on first use) plus a system `clang`/`lld`/`llvm-lib`
install (`cargo-xwin` doesn't bundle its own compiler toolchain the way `cargo-zigbuild` does) — on Ubuntu,
the `clang-21`/`lld-21`/`llvm-21` packages provide these, just not symlinked under their plain,
unversioned names by default.

```sh
cargo install cargo-xwin
rustup target add x86_64-pc-windows-msvc

cargo build-windows-winui    # alias for: cargo xwin build --target x86_64-pc-windows-msvc -p browser-windows-winui
cargo build-windows-reactor  # alias for: cargo xwin build --target x86_64-pc-windows-msvc -p browser-windows-reactor
```

The `.cargo/config.toml`'s `[env]` section pins `CC`/`AR` for this target to absolute paths under
`/usr/lib/llvm-21/bin/` so this works from any terminal without needing `clang`/`llvm-lib` on `PATH`
manually — if your system's LLVM install lives elsewhere, update those paths.

### browser-macos-appkit: building

Also cross-compile-only from this Linux dev machine — no macOS hardware here, and unlike Windows, there's
no way to *run* a cross-compiled macOS binary locally either (no Wine equivalent). Real verification
happens on GitHub's native macOS runners (see `.github/workflows/macos.yml`); this local build exists so a
change can be compile-and-link checked before pushing, not to actually launch the app.

Uses [`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) and
[Zig](https://ziglang.org/download/) (Zig as the C/C++ cross-compiler and linker), plus a macOS SDK for the
Apple framework `.tbd` stubs (`AppKit`, `WebKit`, `Foundation`, ...) the final link step needs, since Zig
itself doesn't bundle those.

**On the SDK's provenance**: there's no official, freely-redistributable way to get Apple's SDK — it
normally comes bundled with Xcode, gated behind an Apple Developer account and a EULA that's squarely
about *running macOS itself*, not about cross-compiling third-party software against SDK headers/stubs
from a non-Apple host. This project uses an unofficial community mirror
([`joseluisq/macosx-sdks`](https://github.com/joseluisq/macosx-sdks)) of just those headers/stubs — common
practice in OSS cross-compilation CI (`osxcross`, `cargo-zigbuild`'s own docs point at similar mirrors),
but genuinely a legal gray area, not something Apple has explicitly sanctioned. This was a deliberate,
discussed choice (not an oversight) — reconsider it if that calculus matters for your use of this repo.

```sh
cargo install cargo-zigbuild
rustup target add aarch64-apple-darwin x86_64-apple-darwin

# Zig (a plain tarball, no root needed) — see https://ziglang.org/download/
# for the current release and other platforms. Put it under .zig/ (gitignored)
# to match where build-macos-appkit.sh looks for it:
curl -fsSL -o /tmp/zig.tar.xz https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz
mkdir -p .zig && tar -C .zig -xf /tmp/zig.tar.xz

# SDK: pick a recent release from https://github.com/joseluisq/macosx-sdks/releases
# (this repo was set up against 14.0) and extract it under .macos-sdk/ (gitignored):
mkdir -p .macos-sdk
curl -fsSL -o /tmp/macos-sdk.tar.xz \
    https://github.com/joseluisq/macosx-sdks/releases/download/14.0/MacOSX14.0.sdk.tar.xz
tar -C .macos-sdk -xf /tmp/macos-sdk.tar.xz

.cargo/build-macos-appkit.sh aarch64-apple-darwin   # or: x86_64-apple-darwin
```

Not a plain Cargo `[alias]` because `cargo-zigbuild` needs Zig on `PATH` and `SDKROOT` pointing at the
extracted SDK, and Cargo aliases can only substitute arguments — they can't search the filesystem or set
environment variables first. `.cargo/build-macos-appkit.sh` finds both under `.zig/`/`.macos-sdk/`
automatically (falling back to whatever's already in your environment if those directories don't exist).

## Testing

`browser-core`'s page/tab-management logic (load/unload tracking, the loaded-pages limit) is pure state
machine logic tested with real unit tests against a mock engine — no GTK or webview involved:

```sh
cargo test -p browser-core
```

`browser-linux-gtk3` has real `cargo test`-integrated regression tests (`tests/gtk_tests.rs`, using
`gtk-test` as a dev-dependency) that drive the actual GTK app end-to-end against local fixture pages:

```sh
cargo test -p browser-linux-gtk3
```

These need a real display — `DISPLAY` pointed at a working X11/Xwayland server, same as running the app
itself. Each test uses its own disposable profile (so nothing pollutes real user data) and GTK's main loop
is only ever driven from one thread at a time (a process-wide `Mutex` serializes the tests — GTK's default
backend isn't safe to drive from multiple threads concurrently, which is otherwise exactly how Rust's test
harness runs `#[test]` functions), so no `--test-threads=1` is needed on the invocation above.

If there's no physical display available (a plain terminal, CI), `xwayland-run` gets a genuinely isolated
one — a headless Wayland compositor plus `Xwayland` on top of it, unlike `Xephyr`, which needs a real host
display to nest a window inside:

```sh
sudo apt-get install -y xwayland-run cage  # cage: a minimal ~76 KB compositor backend, enough for this
xwayland-run -- cargo test -p browser-linux-gtk3
```
