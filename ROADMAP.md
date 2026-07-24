# Roadmap

Tracks what's done and what's next, since this spans many sessions. See `README.md` for architecture and
build/run instructions.

## Done

- Linux (`browser-linux-gtk3`) and Windows (`browser-windows-win32`, `browser-windows-nwg`) native chrome,
  cross-compiled and running under Wine from this dev machine.
- `browser-wx` (wxDragon) — a fourth, now-unmaintained front end kept in the repo.
- `browser-windows-winui` (WinUI 3) — the fifth front end, and the one under active development for
  Windows going forward. Cross-compile/link-verified only (never run — no Windows App SDK runtime under
  Wine).
- History tracking and search, integrated into the switcher grid.
- External link launch: a standalone "open in which profile?" chooser when launched with a URL argument.
- In-app profile picker (create/switch profiles; switching launches a new process).
- Configurable keybindings, with an editor UI.
- GTK unit tests (`crates/browser-linux-gtk3/tests/gtk_tests.rs`, using `gtk-test`) — real
  `cargo test`-integrated regression tests, running headlessly via `xwayland-run`/`cage` (see `README.md`'s
  Testing section). Replaced the old `cargo run --example nav_test`/`switcher_test` binaries. Along the way,
  found and worked around a real gtk-rs constraint: `gtk::init()` permanently binds to whichever thread calls
  it first and panics if any other thread ever calls it again, even sequentially — incompatible with Rust's
  test harness spawning a fresh thread per `#[test]` even under `--test-threads=1`. Fixed with a single
  persistent worker thread that owns GTK for the whole process; each test sends its body there and blocks for
  the result.
- Unified search/URL bar (`browser-core` + `browser-linux-gtk3`) — the switcher grid's separate search box is
  gone; the toolbar address bar now doubles as the switcher's search box while it's open (cleared/focused on
  open, filters the grid on every keystroke, Enter does the switcher's search-activate behavior), and is
  restored to the active page's URL on close without a selection. `browser-windows-winui` untouched (was
  never scoped to it).
- Bookmarks (`browser_core::Bookmarks`, a small per-profile JSON file — deliberately not `HistoryStore`'s
  SQLite treatment, since bookmarks stay small and are edited rarely) + a `browser-linux-gtk3` toolbar
  star-toggle button and a bookmarks overlay (same overlay pattern as settings/switcher/profile-picker/
  keybindings), with a new `ToggleBookmark`/`OpenBookmarks` pair of keybindable actions.
  `browser-windows-winui` untouched (was never scoped to it).
- Ctrl+Enter in the switcher's search box now forces a brand-new page open even when the typed text matches
  an open page/history entry (plain Enter still switches to a single match, as before). See
  `summaries/ctrl-enter-force-new-page.md`.
- Separate `EditUrl`/`OpenSwitcher` actions (`browser-core` + `browser-linux-gtk3`): Ctrl+L now opens the
  switcher with the current URL preloaded and fully selected (not blanked); Ctrl+T/F1 keep the old
  blank-search behavior. See `summaries/edit-url-vs-new-page-actions.md`.
- Private/incognito/guest profile (`browser-core` + `browser-linux-gtk3`): a single ephemeral
  `Profile::ephemeral()` session covers all three names — `Settings`/`Keybindings` always start from
  defaults, `Bookmarks` always starts empty, `HistoryStore` opens in-memory, and none of it is ever written
  to disk. Launch via `--incognito`/`--private`/`--guest`, or the profile picker's new "New Private Window"
  button. See `summaries/private-incognito-guest-profile.md`.
- Fixed illegible label colors in the settings/profile-picker/keybindings/bookmarks overlays: plain labels
  (row labels, the "Unlimited" checkbox, bookmark rows) and flat-button text had no explicit color and fell
  back to the system theme's default (low-contrast against the overlays' dark background). See
  `summaries/fix-overlay-label-colors.md`.
- Moved the keybindings editor into the settings overlay (`browser-linux-gtk3`) instead of its own overlay/
  toolbar button — one "app configuration" destination instead of two, with the row list in a scrollable
  section so the combined overlay doesn't grow too tall. See `summaries/move-keybindings-into-settings.md`.
- Search engine management (`browser-core` + `browser-linux-gtk3`): add/remove custom search engines from the
  settings overlay, plus a fix for the default-engine dropdown always showing a fixed list instead of the
  live per-profile `Settings::search_engines`. See `summaries/search-engine-management.md`.
- The switcher grid now shows matching bookmarks (not just open pages/history) when searching, with a
  distinct `.bookmark-tile` style — deduped against open-page and history matches for the same URL. See
  `summaries/show-bookmarks-in-switcher-grid.md`.
- Page screenshotting: `RenderEngine` gained an async `screenshot` method, implemented for real in
  `render-engine::linux` (WebKitGTK's snapshot → cairo → PNG) and wired to a new toolbar button in
  `browser-linux-gtk3` with a native save dialog. Other `RenderEngine` implementers got a stub to stay
  compiling. See `summaries/page-screenshotting.md`.
- Color themes (Light/Dark, not yet arbitrary "custom") + overlay backgrounds (`browser-core` +
  `browser-linux-gtk3`): `Settings::theme`, applied by swapping a dedicated `CssProvider`'s content —
  covering the settings/profile/keybindings/bookmarks overlays' background and the switcher grid's
  history/bookmark tiles, the only surfaces with a real theme-dependent background. See
  `summaries/color-themes.md`.
- Passphrase support for profiles (`browser-core` + `browser-linux-gtk3`), using libsql's native encryption
  (SQLite3 Multiple Ciphers, AES-256-CBC) for the history database only — `Settings`/`Keybindings`/
  `Bookmarks` stay plain JSON. Passphrases are collected in-process (a new standalone prompt window), never
  passed cross-process via argv. Found and fixed a real cross-compile break along the way: libsql's
  `encryption` feature needs `llvm-lib` when cross-compiling for MSVC, not available here, so it's now scoped
  to Linux only with a non-Linux stub. See `summaries/profile-passphrase-encryption.md`.
- Reader mode (`render-engine::linux` + `browser-core` + `browser-linux-gtk3`): a hand-rolled content-
  extraction heuristic (favors `<article>`/`<main>`, else the highest-scoring `<div>`/`<section>` by
  paragraph count) injected via JS — not a vendored Readability.js (no network access here to fetch one).
  See `summaries/reader-mode.md`.
- Vector search for the switcher grid's history search (`browser-core` + `browser-linux-gtk3`), using
  libsql's native `vector32`/`vector_distance_cos` — needs no new Cargo feature, unlike encryption, since
  they're in libsql-ffi's base bundled SQLite. Embeddings are a deterministic, dependency-free local
  "hashing trick" (lexical/shared-vocabulary similarity, not semantic — no local model or network API
  available here, and picking between those is a real decision, not something to guess at), so this finds
  entries sharing vocabulary with a query regardless of word order/exact substring, shown as a third
  `.similar-tile` category in the switcher grid. See `summaries/vector-search.md`.
- Windows CI via GitHub Actions (`.github/workflows/windows.yml`): runs on a genuine `windows-latest` runner —
  `cargo test -p browser-core` natively (not just cross-compile-checked), and a `browser-windows-winui` build
  + real launch + screenshot, the first environment able to actually run WinUI 3 rather than only
  cross-compile/link-verify it (WinUI 3 needs the real Windows App SDK runtime, unavailable under Wine).
  Local VMs (Docker/KVM) were considered and rejected: a local macOS VM would violate Apple's EULA on
  non-Apple hardware, and `cross` only cross-compiles, it doesn't run foreign-OS binaries. See
  `summaries/windows-github-actions-ci.md`.
- `browser-macos-appkit` (new crate) + `render-engine::macos`: a **minimal scaffold**, not feature parity —
  a single native `NSWindow` with a toolbar strip (back/forward/reload `NSButton`s + an address-bar
  `NSTextField`) and a `WKWebView` (via `wry`, embedded as a real AppKit view, no `tao`/`winit`, matching
  every other front end's "native widget wraps native webview child" pattern), using `objc2`/`objc2-app-kit`.
  Written and dependency-resolved/`cargo check`-clean on this Linux dev machine, but genuinely **never
  compiled** — no macOS toolchain is available here, and (per the Windows CI entry above) a local macOS VM
  isn't an option on this non-Apple hardware either. Needs a real compile pass (on real macOS, or a future
  `macos-latest` GitHub Actions workflow) before it's trustworthy. See
  `summaries/macos-appkit-scaffold.md`.

## Next

Repo is pushed to `danshryock/one-page-browser` (`git@github.com:danshryock/one-page-browser.git`), with `gh`
installed and authenticated on this dev machine — real job logs are pulled via `gh run view --log-failed`/the
Actions API, not guessed at.

**`.github/workflows/macos.yml`** has passed completely on every run so far, including the full
`build-and-smoke-appkit` job (build + launch + screenshot) — genuine confirmation `browser-macos-appkit`
compiles and runs on real macOS, not just `cargo check`-clean on Linux.

**`.github/workflows/windows.yml`** — `test-core` (`cargo test -p browser-core` + `cargo check -p
render-engine`) is fully green. `build-and-smoke-winui` (the `browser-windows-winui` build itself) also
succeeds; only the interactive launch/screenshot smoke-test still fails, and it's now been diagnosed about as
thoroughly as practically possible from this environment:
- Two genuine bugs found and fixed along the way: a Linux-only `clang-cl` path leaking into the native build
  via `.cargo/config.toml`, and the Windows App SDK runtime install step silently swallowing its own exit
  code.
- The app crashes at launch with `STATUS_STOWED_EXCEPTION` (`0xC000027B`), well after a fully successful
  startup (window built, page added, activated — proven via checkpoint tracing,
  `crates/browser-windows-winui/src/lib.rs`'s `trace()`, kept permanently rather than removed).
- Forcing WARP (software) rendering via `d3dconfig` (confirmed actually applied) made no difference, ruling
  out a simple GPU-driver theory.
- **Seven bisection binaries** (`crates/browser-windows-winui/src/bin/*_smoke_test.rs`) — a bare window,
  the custom title bar alone, `WebView2` alone, HWND subclassing alone, three combinations of those up to all
  of them together, and finally `HistoryStore`/`browser_core`'s `tokio` runtime alongside the WinRT STA
  apartment (with real queries run and displayed, not just opened) — **all survived cleanly**, several going
  well past the exact point the real app crashes at. None of this codebase's own unusual code, individually or
  combined, reproduces the crash.
- **A genuine crash dump**, finally captured and analyzed locally (`minidump-stackwalk`, no Windows machine
  needed): the crashing thread's entire stack lives inside Microsoft's own `combase.dll`/`ucrtbase.dll`/
  `KERNELBASE.dll` — `browser-windows-winui.exe`'s own module is loaded but appears **nowhere** in any thread's
  stack. The fault is inside WinUI 3's/WinRT's own Composition internals, not this codebase's code — a real,
  documented category of issue in Microsoft's own trackers (stowed exceptions rooted in
  `combase!RoOriginateLanguageException`), consistent with GitHub Actions' Windows runners having their own
  known WinAppSDK compatibility rough edges.

This is being left as a well-documented, open environment-compatibility issue rather than chased further —
doing so would need Microsoft's own private symbols or a live debugger on a matching machine, beyond what's
practical here. See `summaries/windows-github-actions-ci.md` for the full blow-by-blow (every round, every
ruled-out theory, the full crash dump analysis).

## Backlog (not yet started, roughly in the order raised)

- `browser-macos-appkit`: bring it to feature parity with `browser-linux-gtk3` (switcher grid, settings/
  bookmarks/keybindings/profile-picker overlays, history integration) — see "Done" above for what exists so
  far (a minimal single-page scaffold only).
- `browser-windows-winui`: unified search/URL bar and bookmarks, matching what `browser-linux-gtk3` now has
  (both landed there only, per scope — see "Done" above).
- External password manager integration — not attempted: which manager(s) and which integration protocol
  (native messaging, a browser-extension-equivalent, direct API) is a real design decision, not something
  to guess at unsupervised.
- Internal password manager — not attempted: this is genuinely large (encrypted credential storage — could
  reuse the new profile-passphrase infrastructure, gated on a profile actually having one set — plus a
  management UI, plus autofill via detecting login forms and injecting values into arbitrary third-party
  page DOMs) and the highest-risk remaining item to get subtly wrong. Autofill correctness in particular
  varies a lot site-to-site in ways headless fixture-page testing can't validate — this deserves focused
  attention with real-site testing, not a rushed pass at the end of an already-long session.
- Changing/removing a profile's passphrase, or migrating an existing unencrypted profile to encrypted
  (`sqlite3_rekey` is available via libsql-sys but not wired up yet).
- `browser-windows-winui` debugging — it's been cross-compile/link-verified only all along (see "Done"
  above), never actually run; once it can be run on real Windows, expect a real debugging pass (custom
  title bar drag, the `WM_KEYDOWN` HWND-subclass keybinding capture, `WebView2` control behavior, etc. are
  all unverified at runtime).
- A real semantic embedding for vector search (swapping in a local ML model or a network embedding API in
  place of the current lexical hashing-trick embedding) — see `summaries/vector-search.md`'s "Scope notes."
