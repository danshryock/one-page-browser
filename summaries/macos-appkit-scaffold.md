# macOS native chrome scaffold (`browser-macos-appkit` + `render-engine::macos`)

**Roadmap item:** "macOS: native chrome via AppKit, following the same `RenderEngine`-trait pattern as the
other front ends" — requested as a companion piece to the Windows GitHub Actions CI work (see
`summaries/windows-github-actions-ci.md`), specifically as "also scaffold a minimal browser-macos-appkit
crate."

## What this is — and isn't

A **minimal scaffold**: a single native `NSWindow` with a toolbar strip (back/forward/reload `NSButton`s + an
address-bar `NSTextField`) and a `WKWebView` filling the rest, embedded as a real AppKit view hierarchy. It
deliberately does **not** match `browser-linux-gtk3`'s feature set yet — no switcher grid, no settings/
bookmarks/keybindings/profile-picker overlays, no history integration, no menu bar. It establishes the
`RenderEngine`-trait pattern for macOS (the actual roadmap ask) and gives a real starting point to build
those out from, rather than attempting full parity in one pass.

## Why this needed a design decision before writing code: no `tao`

An existing memory of this project's architecture said windowing was meant to go through `tao` (a `winit`
fork). That turned out to be stale — none of the actual crates (`browser-linux-gtk3`, `browser-windows-win32`,
`browser-windows-winui`) depend on `tao` at all; each creates its native window directly (`gtk::Window`, raw
Win32 `CreateWindowEx`, WinUI 3's own `Window` class) and uses `wry`'s `build_as_child`/`build_gtk` to embed
the webview as a child of that native window, wrapping a `HasWindowHandle` impl around the raw handle by hand
(see `render-engine/src/windows.rs`'s `HwndHandle`). `browser-macos-appkit` follows that same established
pattern instead: a real `NSWindow` built directly via `objc2-app-kit`, with `render_engine::macos::WryEngine`
wrapping an `NSView` pointer the same way `windows.rs` wraps an `HWND`. (The stale memory has been corrected.)

## What was built

- **`render_engine::macos`** (new, `#[cfg(target_os = "macos")]`): `WryEngine::new(parent: &NSView, ...)`,
  mirroring `windows.rs`'s `WryEngine` — wraps the `NSView` as a `raw_window_handle::AppKitWindowHandle`,
  embeds via `wry`'s `build_as_child`. Implements the `RenderEngine` trait (`navigate`/`go_back`/`go_forward`/
  `reload`/`current_url`); `screenshot` is stubbed (`"not yet implemented"`), matching the existing precedent
  in `windows.rs`/`winui.rs` for backends that haven't gotten that far yet — a real one would use WKWebView's
  `takeSnapshotWithConfiguration:completionHandler:`.
- **`browser-macos-appkit`** (new crate): `build_window_and_app(profile) -> anyhow::Result<App>` builds the
  window/toolbar/webview and wires up button/address-bar actions via a custom `AppDelegate` (`objc2`'s
  `define_class!` macro) that's also the `NSWindowDelegate` (handles `windowDidResize:` for manual re-layout,
  since AppKit has no layout manager for views added without an autoresizing mask — same situation
  `windows.rs`'s `set_bounds` doc comment describes for Win32 — and `windowWillClose:` to quit the app).
  `main.rs` follows the same `resolve_profile_name`/`Profile::new`/`build_window_and_app`/`.run()` shape as
  every other front end's entry point.
- Added to the workspace (`Cargo.toml`'s `members`), using `objc2`/`objc2-app-kit`/`objc2-foundation` (current
  crates.io versions as of this session: `0.6.4`/`0.3.2`/`0.3.2`) with **default features** rather than a
  hand-picked subset — deliberate, since trimming to only the classes referenced risks missing a transitively-
  needed feature in a way that's only discoverable by actually compiling on macOS, which isn't possible here
  (see below).

## This has never been compiled — real, but unverified, code

Written entirely on this Linux dev machine, which has no macOS toolchain and no way to get one: unlike
Windows (where `cargo-xwin`/`cargo-zigbuild`/Wine give a real local cross-compile-and-sometimes-run story),
there's no cross-compilation path to macOS from Linux, and running an actual macOS VM here would violate
Apple's EULA on this non-Apple hardware (see `summaries/windows-github-actions-ci.md`'s "why not local VMs"
section — this was investigated and rejected earlier in the same session).

Given that constraint, every `objc2`/`objc2-app-kit` API call in this scaffold was checked against that
crate's actual generated source (vendored locally under
`~/.cargo/registry/src/.../objc2-app-kit-0.3.2/src/generated/*.rs` and
`~/.cargo/registry/src/.../objc2-0.6.4/src/`) — exact method names, parameter types, and `unsafe`-ness were
read directly from the real bindings, not guessed or recalled from general knowledge. That's verification of
*signatures*, though, not of the whole file actually compiling and running end-to-end (order-of-operations
mistakes, missing trait imports, or subtly wrong `objc2` macro usage are all still possible and would only
surface at a real compile). Treat this the same way `browser-windows-winui` was treated for months (see
`ROADMAP.md`'s "Done" section): real work that needs a real debugging pass against real hardware — or a
future `macos-latest` GitHub Actions workflow — before it's trustworthy enough to build further features on.

Known gaps, documented up front rather than silently left out:
- The address bar doesn't update on in-page navigation (link clicks) — `RenderEngine` only exposes a
  document-title-changed callback, not a URL-changed one.
- No menu bar (`NSMenu`) at all — no `Cmd+W`/`Cmd+Q`, no standard Edit/Window menus.
- Only one page — no `PageManager`/switcher grid integration yet.

## Testing

- `cargo check --workspace --exclude browser-wx`: passes, including `browser-macos-appkit` and
  `render-engine` — on this Linux machine both compile to essentially nothing (`#![cfg(target_os = "macos")]`
  on `browser-macos-appkit`'s `lib.rs`, `#[cfg(target_os = "macos")]` on `render-engine`'s `macos` module),
  the same pattern `browser-windows-winui` already used for its MSVC-only gating — so this only confirms the
  crate is wired into the workspace correctly and doesn't break dependency resolution, **not** that the macOS
  code itself compiles.
- `cargo clippy --workspace --exclude browser-wx --all-targets`: clean (same two pre-existing, unrelated
  warnings as before this change).
- `cargo build --target x86_64-pc-windows-gnu --workspace --exclude browser-wx` and `cargo build-windows-winui`:
  both still succeed, confirming the `render-engine`/workspace `Cargo.toml` changes didn't affect either
  existing Windows build path.

## Update: feature parity + real Linux cross-compilation

A later pass brought this crate to feature parity with `browser-windows-reactor`'s scope (multi-page via
`PageManager<WryEngine>`, switcher/settings/profile overlays, keybindings editor folded into settings,
`NSMenu`-based global shortcuts, an external-link chooser window — see `ROADMAP.md`'s "Done" entry for the
full rundown and the honest list of what's still not implemented, e.g. bookmarks/theme). The "no macOS
toolchain available, never compiled" limitation this file originally documented is also no longer true:
`cargo zigbuild` plus an unofficial macOS SDK mirror (`.cargo/build-macos-appkit.sh`, see README.md) now
produces real, linked Mach-O binaries for both `aarch64-apple-darwin` and `x86_64-apple-darwin` from this
same Linux machine, and every change past this point has been compile-and-link checked that way before
being pushed — a real improvement over eyeballing `objc2-app-kit`'s generated source, though still not a
substitute for actually running the app, which still only happens on GitHub's native macOS runners.
