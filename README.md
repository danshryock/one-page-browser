# claude-browser

A cross-platform web browser written in Rust. No tabs, by design.

## Architecture

- **Content rendering** is handled by the OS-native webview via [`wry`](https://github.com/tauri-apps/wry) (WebView2 on Windows, WKWebView on macOS/iOS, WebKitGTK on Linux, system WebView on Android).
- **Chrome (window, address bar, buttons)** is built with each platform's native UI toolkit rather than a shared cross-platform GUI toolkit — GTK (`gtk-rs`) on Linux today, with Win32/WinUI and AppKit planned for Windows/macOS. This avoids an event-loop conflict that shared toolkits like `iced` would introduce with `wry`/`tao`.
- Chrome code never depends on `wry` directly — it talks to a `RenderEngine` trait (`crates/render-engine`), so the underlying engine can be swapped later (e.g. Servo, CEF, a custom engine) without touching the app.

Linux (`crates/browser-linux-gtk3`) is implemented and working. Windows has two native Win32 chrome
implementations (`crates/browser-windows-win32`, hand-rolled; `crates/browser-windows-nwg`, built on
`native-windows-gui`) with feature parity to Linux. Both have been cross-compiled and run from this same
Linux dev machine (see below) — the window, toolbar, switcher, and message loop are confirmed working
under Wine, and with the Wine setup documented in "WebView2 under Wine" below, WebView2 itself actually
renders real page content too, not just the native chrome — confirmed by loading a live page over the
network and seeing it render correctly. A window still opens even if the webview fails to initialize
(logged, not fatal) rather than the whole app silently failing to launch. macOS isn't started yet.

There's also a fourth, experimental front end, `crates/browser-wx`, built on
[wxDragon](https://github.com/AllenDang/wxDragon) (Rust bindings for wxWidgets) instead of `wry` —
wxWidgets' own `wxWebView` widget wraps the OS webview directly (WebView2 on Windows, WebKitGTK on Linux),
so this crate doesn't touch `wry`/`render-engine`'s `WryEngine` at all, only the shared `RenderEngine`
trait. Unlike the other three, it isn't `target_os`-gated — wxWidgets is itself cross-platform, so this is
one source tree that builds natively on Linux *and* cross-compiles to Windows unchanged. At feature parity
with `browser-linux-gtk3` (switcher grid, settings dialog, profile label, keyboard shortcuts) and confirmed
cross-compiled and running under Wine, chrome and real WebView2 content both — see "browser-wx: building
and running" below for the (different, `zig`-based) cross-compile path this one needs.

A fifth front end, `crates/browser-windows-winui`, uses WinUI 3 (`Microsoft.UI.Xaml`) — the modern
Fluent-design Windows toolkit, via the [`winio-winui3`](https://github.com/compio-rs/winio3-rs) bindings
crate. Unlike every other Windows front end here, it doesn't use `render-engine`'s wry-based `WryEngine` at
all — it wraps WinUI 3's own native `Microsoft.UI.Xaml.Controls.WebView2` XAML control directly
(`render-engine`'s `WebView2Engine`, gated to the `x86_64-pc-windows-msvc` target), since it's a real XAML
`FrameworkElement` that participates in ordinary Grid/StackPanel layout with no manual resize plumbing
needed. At feature parity with `browser-linux-gtk3` (multi-page browsing, a switcher grid, settings,
keyboard shortcuts, profile support), with two notable, binding-driven simplifications: the settings surface
is an in-window overlay rather than a modal dialog (avoids `ContentDialog`'s async `ShowAsync`, since this
app is deliberately synchronous throughout), and per-page tile color-coding is dropped (this crate's WinUI 3
bindings have no constructible flat-color brush type at all). It's cross-compile-only — see
"browser-windows-winui: building" below — the Windows App SDK runtime it needs isn't available under Wine,
so unlike every other frontend here, it's never actually been run, only cross-compiled, cross-linked, and
inspected. `browser-wx`/`browser-windows-win32`/`browser-windows-nwg` remain in the repo but are no longer
under active development — `browser-windows-winui` is the Windows front end going forward.

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

This builds the whole workspace on any host. Each `target_os`-gated crate
(`browser-linux-gtk3`, `browser-windows-win32`, `browser-windows-nwg`) compiles to an empty stub binary
on a platform it doesn't apply to (which just prints a one-line explanation if you run it) instead of
failing, so a bare `cargo build` never needs `--exclude` flags no matter which of these you're on.
`browser-wx` isn't gated at all and just builds for real on any host.

The same is true cross-target for `cargo build --target x86_64-pc-windows-gnu` (see below) or
[`cross build --target x86_64-pc-windows-gnu`](https://github.com/cross-rs/cross) — **with one
exception**: add `--workspace --exclude browser-wx` (or target the other packages by name) when
cross-compiling to `x86_64-pc-windows-gnu` this way. `browser-wx`'s `wxdragon-sys` dependency builds
wxWidgets from source when cross-compiling from Linux, and its build script only knows how to do that
via [`cargo zigbuild`](#browser-wx-building-and-running) — plain `cargo build`/`cross build` pick a CMake
generator (`MinGW Makefiles`) that assumes a native Windows host and fails outright on Linux. This is a
gap in `wxdragon-sys`'s own build script, not something fixable from this repo.

## Running

```sh
cargo run -p browser-linux-gtk3     # Linux
cargo run -p browser-windows-win32  # Windows (native build)
```

Each opens a native window (GTK on Linux, Win32 on Windows) with an address bar and back / forward /
reload buttons. Type a URL into the address bar and press Enter (or, on Windows, click Go) to navigate.

`.cargo/config.toml` also defines a build-and-run shortcut per chrome, each pinned to whichever target
that crate actually applies to (the two Windows ones cross-compile and launch under Wine automatically,
same as the manual `--target x86_64-pc-windows-gnu` invocations below):

```sh
cargo run-gtk3   # same as `cargo run -p browser-linux-gtk3`
cargo run-win32  # same as `cargo run --target x86_64-pc-windows-gnu -p browser-windows-win32`
cargo run-nwg    # same as `cargo run --target x86_64-pc-windows-gnu -p browser-windows-nwg`
```

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

`.cargo/config.toml` sets a small wrapper script (`.cargo/wine-runner.sh`) as the runner for this target,
so `cargo run`/`cargo test --target x86_64-pc-windows-gnu` invoke the built `.exe` under Wine
automatically — no need to type `wine target/...` by hand, and no manual `WebView2Loader.dll` copying
either: real Windows/WebView2 projects need that DLL alongside the exe, but `webview2-com-sys` only
vendors it inside its own crate source and copies it into its own build output for linking purposes, it
never places it next to the final binary. Rather than duplicating the file (which would go stale if
`webview2-com-sys` is ever upgraded), the runner script points Wine at the crate's vendored copy directly
via `WINEPATH`, which Wine also searches for DLLs. (On a real Windows machine, without Wine, you'd still
need the manual copy — or add its directory to `PATH` — since this trick is Wine-specific.)

```sh
cargo run --target x86_64-pc-windows-gnu -p browser-windows-win32
```

The window, toolbar, and address bar come up normally. Without the WebView2-under-Wine setup below, expect
(and it's fine) to see this in the output — Wine has no WebView2 Runtime installed by default, so the
content area never initializes, but the window and chrome still work:

```
Error: WebView2 error: WindowsError(Error { code: HRESULT(0x80070002), message: "File not found." })
```

The same applies to `browser-windows-nwg` — swap the `-p` above.

### WebView2 under Wine (optional, for real content rendering)

By default Wine has no WebView2 Runtime installed at all, hence the "File not found" error above — the
window/toolbar/switcher are real and testable regardless, but the content area stays blank. Getting an
actual WebView2 Runtime running under Wine is possible, but Ubuntu 24.04's packaged Wine (9.0) is too old:
the runtime's own process (`msedgewebview2.exe`) fails to start on it. This project uses a **dedicated
Wine 11.0 build and bottle, both kept project-local under `.wine/`** (gitignored — large, machine-specific
binary artifacts, not source) rather than anywhere in your home directory, so they're easy to find,
easy to blow away and redo, and don't leak into other projects' Wine setups — confirmed working
end-to-end by actually loading `https://example.com` over the network and seeing it render correctly, not
just opening a blank window.

**1. Get Wine 11.0 without root** (no `sudo`/apt-repo access needed — a `.deb` is just an ar/tar archive,
so `dpkg-deb -x` extracts its contents anywhere without touching the system package database). Run this
from the repo root:

```sh
mkdir -p /tmp/wine11 && cd /tmp/wine11
for pkg in wine-stable wine-stable-amd64 wine-stable-i386; do
  arch=amd64; [ "$pkg" = wine-stable-i386 ] && arch=i386
  packages=$(curl -fsSL "https://dl.winehq.org/wine-builds/ubuntu/dists/noble/main/binary-$arch/Packages")
  url=$(printf '%s\n' "$packages" | awk -v p="$pkg" '$0=="Package: "p{f=1} f&&/^Filename:/{print $2; exit}')
  curl -fsSL -o "$pkg.deb" "https://dl.winehq.org/wine-builds/ubuntu/$url"
  dpkg-deb -x "$pkg.deb" "$OLDPWD/.wine/wine-11.0/"
done
cd "$OLDPWD"
.wine/wine-11.0/opt/wine-stable/bin/wine --version   # should print wine-11.0 (or newer)
```

(Wine's own binaries resolve their `lib/wine` directory relative to their own location, not a hardcoded
absolute path — this works regardless of where you extract it to, confirmed empirically.)

**2. Create the `webview2` bottle and set it to report Windows 11** (community reports indicate WebView2
needs a fairly recent Windows version reported; Windows 7 mode is applied automatically to
`msedgewebview2.exe` specifically by winetricks' `webview2` verb below, which is the actual per-exe
workaround needed — see [wine bug 58921](https://bugs.winehq.org/show_bug.cgi?id=58921)). Still from the
repo root:

```sh
WINE_BIN="$PWD/.wine/wine-11.0/opt/wine-stable/bin"
export PATH="$WINE_BIN:$PATH"
export WINESERVER="$WINE_BIN/wineserver"
export WINEPREFIX="$PWD/.wine/bottle"

wineboot --init
```

Then, still with those env vars set, fetch a current `winetricks` (the `webview2` verb is new enough that
Ubuntu's packaged `winetricks` — 20240105 as of this writing — doesn't have it yet) and use it to set the
reported Windows version and install the runtime:

```sh
curl -fsSL https://raw.githubusercontent.com/Winetricks/winetricks/master/src/winetricks -o /tmp/winetricks
chmod +x /tmp/winetricks
/tmp/winetricks -q win11
/tmp/winetricks -q webview2
```

The second command downloads and silently installs the real Microsoft Edge WebView2 Runtime from
Microsoft's servers — it's a genuine ~1.4 GB install (Chromium-based), so it takes a few minutes.

**3. That's it** — `.cargo/wine-runner.sh` (the runner configured in `.cargo/config.toml`) automatically
detects and uses this Wine build and bottle if present at `.wine/wine-11.0` and `.wine/bottle` (resolved
relative to the script's own location, so this works no matter which directory inside the repo you invoke
`cargo` from), falling back to whatever `wine`/`WINEPREFIX` are already on your `PATH`/in your environment
if you skip this setup. No further configuration needed — `cargo run-win32`, `cargo run-nwg`, or a plain
`cargo run --target x86_64-pc-windows-gnu -p ...` all pick it up transparently.

### browser-wx: building and running

Native, on any host (Linux here):

```sh
cargo run -p browser-wx      # or: cargo run-wx
```

Cross-compiling to Windows needs [`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) and
[Zig](https://ziglang.org/download/) — `wxdragon-sys` builds wxWidgets from source when cross-compiling
from Linux, and `cargo zigbuild` (using Zig as the C/C++ cross-compiler and linker) is the only path its
build script actually supports for that; plain `cargo build --target x86_64-pc-windows-gnu` and
`cross build` both fail (see the note in "Building" above).

```sh
cargo install cargo-zigbuild
# Download Zig (a plain tarball, no root needed) and put it on PATH — see
# https://ziglang.org/download/ for the current release and other platforms:
curl -fsSL -o /tmp/zig.tar.xz https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz
tar -C ~/opt -xf /tmp/zig.tar.xz   # or wherever you keep local tool installs
export PATH="$HOME/opt/zig-x86_64-linux-0.16.0:$PATH"

cargo zigbuild --target x86_64-pc-windows-gnu -p browser-wx
```

This produces a real, statically-linked `target/x86_64-pc-windows-gnu/debug/browser-wx.exe` — confirmed
running under this project's Wine 11.0 + WebView2 setup (see "WebView2 under Wine" above), native chrome
and real WebView2 content both.

There's no `[alias]` equivalent of `run-win32`/`run-nwg` for this one: Cargo's alias mechanism just
substitutes arguments and re-dispatches through Cargo's own subcommand resolution, which always invokes an
external plugin as `cargo-<name> <name> <rest>` — so even though `cargo-zigbuild` has its own internal
`run` verb (which builds *and* launches), there's no way to make `cargo <alias>` land on it rather than its
`zigbuild` (build-only) verb. Use the wrapper script instead, which runs `cargo zigbuild` and then
`wine-runner.sh` in one step:

```sh
.cargo/run-wx-wine.sh
```

### browser-windows-winui: building

This one is cross-compile-only — it's never been run, even under Wine, since WinUI 3 needs the real Windows
App SDK runtime installed. Cross-compiling to `x86_64-pc-windows-msvc` needs
[`cargo-xwin`](https://github.com/rust-cross/cargo-xwin) (which downloads and caches the Windows SDK + MSVC
CRT via [`xwin`](https://github.com/Jake-Shadle/xwin) on first use) plus a system `clang`/`lld`/`llvm-lib`
install (`cargo-xwin` doesn't bundle its own compiler toolchain the way `cargo-zigbuild` does) — on Ubuntu,
the `clang-21`/`lld-21`/`llvm-21` packages provide these, just not symlinked under their plain,
unversioned names by default.

```sh
cargo install cargo-xwin
rustup target add x86_64-pc-windows-msvc

cargo build-windows-winui   # alias for: cargo xwin build --target x86_64-pc-windows-msvc -p browser-windows-winui
```

The `.cargo/config.toml`'s `[env]` section pins `CC`/`AR` for this target to absolute paths under
`/usr/lib/llvm-21/bin/` so this works from any terminal without needing `clang`/`llvm-lib` on `PATH`
manually — if your system's LLVM install lives elsewhere, update those paths.

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
