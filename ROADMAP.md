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

**`crates/browser-windows-reactor`** — a new, sixth front end, replacing `browser-windows-winui`'s
`winio-winui3` dependency with Microsoft's own in-tree `windows-reactor`/`windows-webview` (see
`summaries/windows-github-actions-ci.md`'s comparison-test section for why). Being built incrementally,
feature by feature, not ported in one pass — `windows-reactor`'s declarative render-function-of-state model
is a genuinely different shape from `winio-winui3`'s imperative widget-tree-with-handles style. First
milestone done and verified running (not just compiled) in the local `dockur/windows` VM: a real window,
toolbar (back/forward/reload), an address bar with working state and Enter-to-navigate (via
`.keyboard_accelerator(..)` — a real, working per-element keyboard-shortcut API, unlike `winio-winui3`'s
missing `KeyDown`/`Window::Closed` that forced a raw `HWND` subclass workaround), and a single `WebView2`
page, reusing `browser_core::resolve_address_input`. Ran through ~40 real render cycles (one per keystroke)
with no crash. The `WebView2` content area itself stayed blank — the same pre-existing issue seen in the
comparison test, believed to be this eval VM image's WebView2 Runtime, not an app bug.

**Multi-page hosting** is done next and also verified running in the VM: an arbitrary number of pages, each
with its own independently-alive `WebView` (added/switched via a minimal page-button row — the real switcher
grid is still to come). The design question was real: `windows-reactor` has no `Visibility`-style
show/hide modifier at all (checked by reading its source), so `winio-winui3`'s per-page-`Grid`-with-
`Visibility::Collapsed` approach doesn't translate. Solved by keeping every loaded page's `webview(..)`
element permanently mounted (each `.with_key(page_id)`, the same identity mechanism
`crates/samples/reactor/samples/examples/tab_view_add_button.rs` uses for a dynamic tab list, so the
reconciler never tears down a page's `WebView2` control just because it's not currently shown) and stacking
them all in one `Grid` cell, active page last so it paints on top — real WinUI 3 `Grid` behavior, not a hack.
Added a second page, switched back and forth, no crash either direction.

**`browser_core::PageManager<ReactorWebViewEngine>` is now wired in for real** (new `engine.rs`: a
`RenderEngine` impl backed by `windows-webview`'s `WebView`, kept out of the shared `render-engine` crate so
`browser-windows-winui`'s build doesn't pick up `windows-reactor`/`windows-webview`'s git dependency
transitively), replacing the placeholder page-button row with a working switcher overlay: a search box plus a
tile grid of open pages (via `PageManager::matching_ids`) and history matches (via `HistoryStore::search`,
`browser_core::HOME_URL`/`resolve_address_input` reused throughout), matching
`browser-windows-winui`'s `rebuild_switcher_grid`. Uses reactor's native `grid_view` control (real wrapping
tile layout, handled by the control itself) rather than `winio-winui3`'s fixed-column-count workaround (that
crate has no working `SizeChanged` event to react to the real window width with). Verified running in the VM:
opened the switcher, it rendered a real tile from live `PageManager` data plus the add-page tile, no crash.
Two honest rough edges to revisit: the tile grid's visual layout doesn't yet look like distinct bounded
squares (needs a border/background per tile), and clicking through `grid_view`'s selection wasn't fully
verified interactively (no visible keyboard-focus indicator inside the control after tabbing into it — Tab
navigation *into* the toolbar/overlay controls around it works fine).

**Settings, profile picker, and keybindings overlays are done**, plus a real architectural win: global keyboard
shortcuts now go through `windows-reactor`'s `KeyboardAccelerator` (new `shortcuts.rs`: converts
`browser_core::KeyChord` to/from it) attached to the root element, entirely replacing the need for
`winio-winui3`'s raw `HWND`-subclass `WM_KEYDOWN` dispatch (`browser-windows-winui`'s
`install_hwnd_subclass`/`subclass_proc`) — a real simplification, not just a workaround swap. Verified in the
VM: the settings overlay opens pre-filled from real `Settings` (start page, search engine, loaded-page limit),
Escape closes it via the new global accelerator (not a button), and `Ctrl+T` opens the switcher via the
keyboard shortcut alone, matching `Keybindings::default()` — the actual test of whether
`KeyboardAccelerator`-based dispatch works as a real replacement. The keybindings editor itself renders every
default binding correctly (removable tags, "Add binding" per action). One honest, deliberate gap:
`windows-reactor` has no generic "capture the next keypress" API (only `.keyboard_accelerator(..)` for
*known* combinations — checked by reading `element.rs`/`widget.rs`), so the editor's "add a new binding" flow
uses a text field (`"Ctrl+Shift+P"` format, parsed by `shortcuts::parse_chord`) instead of
`browser-windows-winui`'s live "press keys…" capture — reintroducing a raw `HWND` subclass just for that one
feature would undercut the point of moving off `winio-winui3` in the first place.

**The custom title bar and external-link chooser are both done** — `browser-windows-reactor` is now at feature
parity with `browser-windows-winui`. The title bar uses reactor's native `TitleBar` widget (real WinUI 3
`Microsoft.UI.Xaml.Controls.TitleBar`, with a `.content()` slot) hosting the toolbar, rather than
`winio-winui3`'s manual `window.SetExtendsContentIntoTitleBar(true)` + `window.SetTitleBar(&toolbar)` — a more
idiomatic path, and verified in the VM: the toolbar now sits directly alongside the native minimize/maximize/
close buttons instead of in a separate row below a plain window title bar.

`run_chooser` (the external-link-launch handoff) has a real architectural difference from
`browser-windows-winui::show_external_link_chooser`, documented in `lib.rs`'s module doc comment:
`windows-reactor` has no public way to close the *primary* window opened via `App::render` (`WindowHandle`,
the type with a working `.close()`, is only returned for *secondary* windows via `ReactorWindow`). Rather than
fight that, `run_chooser` reuses a pattern this codebase already has for exactly this shape of problem —
`browser_core::launch_new_profile_process` (used by the profile picker) spawns a new process instead of
swapping state in place. `run_chooser`'s "Open" button does the same (`exe --profile <name> <url>`) and exits
the small chooser process outright. Verified in the VM end-to-end: launching with a URL argument shows the
small chooser (URL, profile field pre-filled, suggestions, Cancel/Open); clicking Open spawns exactly one new
`browser-windows-reactor.exe` process with the right arguments (confirmed via `tasklist`) and the chooser
process exits (confirmed via `taskkill`/process list, not just visually — the dead chooser window kept
rendering stale pixels for a few seconds after its process actually exited, the same stale-repaint artifact
seen elsewhere with dead windows in this VM, not a real hang).

One real bug found and fixed along the way, worth remembering for any future build script in this repo:
`build.rs` had gated its `windows_reactor_setup::as_framework_dependent()` call with
`#[cfg(target_os = "windows", target_env = "msvc")]`, which reflects the *host* build.rs itself compiles for
(always true, since Cargo always compiles/runs build scripts on the build machine) — not the crate's actual
`--target`. This silently skipped the bootstrap-DLL copy for every *cross-compiled* build from this Linux
machine (invisible until now since every test so far used a *native* Windows build in the VM, where host and
target coincide), and shipped the user a `.exe` that failed at launch with "microsoft.windowsappruntime.
bootstrap.dll was not found." Fixed by checking `CARGO_CFG_TARGET_OS`/`CARGO_CFG_TARGET_ENV` at runtime
instead (the correct way for a build script to ask what it's really building for) and making the dependency
itself unconditional rather than target-gated.

**Real user feedback investigated**: keybindings moved out of their own toolbar button/overlay into a
section within the settings overlay (per explicit request — a worse fit as a separate entry point than in
`browser-windows-winui`, where the extra toolbar icon made more sense). Two bug reports ("keyboard shortcuts
don't work," "the page never renders") led to a careful re-diagnosis, not just a guess:
- **Shortcuts**: confirmed working correctly via a rigorous retest in the VM (`Ctrl+T`, `Escape`,
  address-bar Enter-to-navigate all fire — verified via trace logging, not assumption). The one *reported*
  failure during testing turned out to be a testing artifact (not enough time for the newly-launched window
  to claim real focus before sending input — confirmed by the fact keystrokes were landing on the launching
  `cmd` window instead, visible in its own command history). There is one real, current, documented gap
  though: `windows-webview`'s reactor bridge (`webview()`) doesn't expose the underlying `Controller` object,
  so there's no way to wire up `Controller::on_accelerator_key_pressed` — meaning shortcuts genuinely won't
  fire while keyboard focus is *inside* the `WebView2` content area itself (a real WinUI 3/WebView2
  integration requirement, not specific to this codebase — see `shortcuts.rs`'s doc comment). If a user's
  focus ends up there (plausible if they click into a blank/not-yet-rendered page trying to interact with
  it), shortcuts won't reach the app until focus moves back out.
- **Blank page**: root-caused to `on_ready` (the callback `windows-webview`'s `webview()` calls once
  `WebView2` actually initializes) never firing — confirmed via trace logging showing zero occurrences
  across a full navigate attempt. Also found and fixed a real bug in `engine.rs` along the way:
  `ReactorWebViewEngine::navigate()`/`go_back()`/`go_forward()`/`reload()` were silently returning `Ok(())`
  when the webview wasn't ready yet, instead of a real error — meaning "successfully navigated" and
  "silently did nothing because not ready" were indistinguishable in logs, which got in the way of this
  exact diagnosis. Fixed to return a real error instead. **Update**: the user confirmed `WebView2` itself
  works fine on their machine (other `WebView2` apps run there), ruling out "runtime missing/broken" —
  meaning the real explanation is a genuine initialization failure, not an absent dependency. The most
  likely cause, given `WebView2`'s well-documented behavior for unpackaged apps: it defaults its user data
  folder to a location *next to the executable*, which silently fails if that location isn't writable
  (Program Files, a network share — every VM test this session ran the exe from exactly such a UNC path).
  `windows-webview`'s reactor bridge makes this worse to diagnose: it doesn't bind
  `CoreWebView2InitializedEventArgs`'s `Exception` property at all (checked by reading its generated
  bindings — the vtable has no method beyond the `IInspectable` base), so a real initialization failure and
  "never even tried" look identical from our code's perspective. Fixed by setting the documented
  `WEBVIEW2_USER_DATA_FOLDER` override (in `main.rs`, before `bootstrap()` — must happen before the first
  `WebView2` control initializes) to `%LOCALAPPDATA%\claude-browser\webview2`, a location guaranteed
  writable by the current user regardless of where the exe lives. Not yet verified running (the VM's
  interactive session became unreliable after ~12 hours of continuous use — window/focus issues, not a
  code problem — so this fix is verified by compiling/reasoning about `WebView2`'s documented behavior, not
  by a fresh screenshot yet).

See `summaries/windows-github-actions-ci.md` for the full incremental build log.

Repo is pushed to `danshryock/one-page-browser` (`git@github.com:danshryock/one-page-browser.git`), with `gh`
installed and authenticated on this dev machine — real job logs are pulled via `gh run view --log-failed`/the
Actions API, not guessed at.

**`.github/workflows/macos.yml`** has passed completely on every run so far, including the full
`build-and-smoke-appkit` job (build + launch + screenshot) — genuine confirmation `browser-macos-appkit`
compiles and runs on real macOS, not just `cargo check`-clean on Linux.

**`.github/workflows/windows.yml`** — `test-core` (`cargo test -p browser-core` + `cargo check -p
render-engine`) is fully green. `build-and-smoke-winui` (the `browser-windows-winui` build itself) also
succeeds; only the interactive launch/screenshot smoke-test still fails. This has been a long debugging
session with a lot of real progress ruling things out — but **the actual cause is still unknown**, and an
earlier version of this note wrongly concluded the fault must be inside Microsoft's own WinRT/Composition
internals. That conclusion was premature: WinRT/WinUI 3 are heavily used, well-tested libraries running far
more complex production apps than this one elsewhere without this problem. The far more likely explanations
are (a) how this codebase calls the APIs, (b) a bug in `winio-winui3` (the community-maintained Rust binding
subset this crate depends on — its own doc comments already note real gaps, like several delegate types
having no working `add` accessor at all), or (c) something about the build/cross-compile setup. Corrected
here rather than left standing.
- Two genuine bugs found and fixed along the way: a Linux-only `clang-cl` path leaking into the native build
  via `.cargo/config.toml`, and the Windows App SDK runtime install step silently swallowing its own exit
  code.
- The app crashes at launch with `STATUS_STOWED_EXCEPTION` (`0xC000027B`), well after a fully successful
  startup (window built, page added, activated — proven via checkpoint tracing,
  `crates/browser-windows-winui/src/lib.rs`'s `trace()`, kept permanently rather than removed).
- Forcing WARP (software) rendering via `d3dconfig` (confirmed actually applied) made no difference, ruling
  out a simple GPU-driver theory.
- **Ten bisection binaries so far** (`crates/browser-windows-winui/src/bin/*_smoke_test.rs`) — a bare window,
  the custom title bar alone, `WebView2` alone, HWND subclassing alone, three combinations of those up to all
  of them together, `HistoryStore` with real queries run and displayed, the real app's *exact* construction
  order/timing, a `libsql`-free `MemoryHistoryStore` in that same exact order, and (temporarily) the real
  production binary itself with `MemoryHistoryStore` swapped in — **all survived cleanly except the real
  binary, which still crashes**. But every one of these tests used only `Window`/`Grid`/`TextBlock`/
  `WebView2`, and only one event handler (`WebView2::NavigationCompleted`) — none of them exercised
  `window.AppWindow()?.Resize(...)` (the real app's very first action after `Window::new()`), nor any of
  `Button.Click`, `TextBox` `GotFocus`/`LostFocus`/`TextChanged`, `ComboBox`, or `CheckBox` — all real,
  specific API calls the real app makes constantly (toolbar buttons, address bar, search box, settings
  overlay) that a genuine usage or wrapper-crate bug could plausibly live in. That's real, substantial,
  previously-untested surface area, not yet ruled out.
- A real crash dump was captured and analyzed locally (`minidump-stackwalk`, no Windows machine needed): the
  crashing thread's stack (via stack-scanning, not a reliable unwind — no matching public symbols were found
  either) shows frames in `combase.dll`/`ucrtbase.dll`/`KERNELBASE.dll`, with `browser-windows-winui.exe`'s
  own module loaded but not appearing in the (unreliable) scanned frames. That's real data, but on its own
  it's not strong enough to conclude the fault is in Microsoft's code rather than in how a specific API is
  being called from this codebase or `winio-winui3` — a real, specific, previously-untested call
  (`AppWindow().Resize(...)`) is being checked in isolation next.
- Along the way, replaced `browser_core::HistoryStore`'s `tokio` runtime with `futures_executor::block_on` —
  libsql's local backend is never actually async (confirmed by reading its source), so tokio's reactor/thread
  machinery (already about as minimal as tokio gets, but still unnecessary) was dead weight regardless of
  whether it was implicated in the crash. A real simplification either way.
- Also abstracted `HistoryStore` behind a new `HistoryBackend` trait (`record_visit`/`search`/
  `search_similar`), additive to `HistoryStore`'s own unchanged inherent methods, and added
  `MemoryHistoryStore`: a genuine `libsql`-free implementation (a `Vec` behind a `RefCell`, no SQL) mirroring
  `HistoryStore`'s exact behavior — a real, tested (7 new tests) alternative now available for future use
  beyond this investigation. All 86 `browser-core` tests and all 20 `browser-linux-gtk3` GTK tests pass.

CI triggers are now restricted to `workflow_dispatch`/tags only (`v*.*`), not every push, and both workflows
upload their compiled binaries as downloadable artifacts.

**A local Windows VM (`dockur/windows`, Docker + QEMU/KVM) is now running on this dev machine** for much
faster iteration than round-tripping through GitHub Actions. Built and ran a comparison app,
`reactor_smoke_test`, against Microsoft's own `windows-reactor`/`windows-webview` (in-tree in
`microsoft/windows-rs`, not the community `winio-winui3` wrapper) — same toolbar-plus-`WebView2` shape as the
real app, adapted from Microsoft's own sample. **It launched successfully and ran stably for 10+ seconds: a
real window, no crash, no exception.** That's one more real point toward "usage or wrapper bug" and away from
"Microsoft's platform code" — see `summaries/windows-github-actions-ci.md`'s new section for the full list of
non-obvious problems hit getting the VM working (Windows' internal-reboot/`restart:always` requirement,
session-scoped `Z:` drives vs. UNC paths, Session 0 isolation blocking GUI launches from a SYSTEM-context
task, and a missing `Microsoft.WindowsAppRuntime.Bootstrap.dll` fixed via `windows-reactor-setup`).

Still an open investigation — see `summaries/windows-github-actions-ci.md` for the full blow-by-blow (every
round, every ruled-out theory, the crash dump analysis, the `windows-reactor` comparison result, and what's
still untested).

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
