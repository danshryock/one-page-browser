# Roadmap

Tracks what's done and what's next, since this spans many sessions. See `README.md` for architecture and
build/run instructions.

## Done

- Linux (`browser-linux-gtk3`) native chrome — implemented, working, with real regression tests.
- `browser-windows-winui` (WinUI 3) and `browser-windows-reactor` (WinUI 3, Microsoft's own `windows-reactor`)
  — the two Windows front ends going forward. Both cross-compile/link-verified; `browser-windows-reactor`
  has also been run for real, in a Windows VM used for interactive testing.
- `browser-macos-appkit` (AppKit) — the macOS front end, at feature parity with the two Windows front ends'
  scope. Cross-compiles from this Linux dev machine; real runtime verification via GitHub's native macOS
  runners.
- **Deleted**: `browser-windows-win32`, `browser-windows-nwg` (both hand-rolled/NWG-based Win32 chrome, no
  in-app profile picker or keybindings editor), and `browser-wx` (wxDragon experiment) — removed to reduce
  the number of frontends carrying near-duplicate logic (see `ARCHITECTURE.md`). All three had been
  cross-compiled and run under Wine from this dev machine at one point, but were unmaintained and behind
  the other frontends in scope; recoverable from git history if ever needed again.
- New `browser-chrome-core` crate (see `ARCHITECTURE.md` §4/§7) holding toolkit-agnostic decision logic
  shared across frontends, starting with `SwitcherModel` (`build_switcher_rows`/`activate_row`, 13 unit
  tests, zero native toolkit or real I/O in the test suite). All four frontends migrated onto it, replacing
  each one's hand-rolled tile-building/activation logic. `SettingsController`/`KeybindingsController`/
  `PageController` are the same treatment, not yet started.
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
- Password manager: `browser_core::passwords` (a `Login` struct — a password, a passkey, or both under one
  record, see `PasskeyCredential` — a libsql-encrypted `PasswordStore`, and a `PasswordBackend` trait — same
  pattern as `HistoryStore`/`HistoryBackend` — so an external/alternate password manager can later be swapped
  in wherever code is generic over `impl PasswordBackend`) plus a `browser-linux-gtk3` toolbar button and
  overlay (add/view/copy/delete credentials; no in-page autofill or actual passkey creation/assertion yet —
  the schema is ready, but that needs a WebAuthn virtual authenticator hooked into each render engine, not
  built). The vault always requires a passphrase (no unencrypted mode, unlike every other store in this app)
  — independent of `HistoryStore`'s own passphrase (`Profile::has_vault_passphrase`/`enable_vault_passphrase`,
  separate marker file from `Profile::has_passphrase`), but when a profile has both, they share one passphrase
  value: whichever store gets unlocked first in a session caches the passphrase, and the other one reuses it
  silently, no second prompt (see `browser_core::decide_vault_unlock_action`). `browser-windows-winui`/
  `browser-windows-reactor`/`browser-macos-appkit` compile against the new module but have no overlay UI yet —
  same scope pattern as bookmarks.
- Bitwarden/Vaultwarden integration: `browser_core::bitwarden::BitwardenBackend`, a `PasswordBackend` impl
  talking to a locally running `bw` CLI's `bw serve` (loopback-only REST API, JSON in/out) — Vaultwarden is
  wire-compatible with real Bitwarden, so one backend covers both, no separate code path. Checks `/status`
  itself and surfaces a distinguishable "locked" vs. "unreachable" error rather than assuming some other
  process already unlocked it; `browser-linux-gtk3`'s password manager overlay renders Bitwarden entries in
  their own section alongside the local vault's, with a settings checkbox + URL field
  (`Settings::bitwarden_server_url`) to enable it and a small "Unlock Bitwarden" prompt when locked. **Caveat,
  called out in `bitwarden.rs`'s own doc comment**: there was no real `bw serve` instance reachable to verify
  against while building this — the request/response shapes are this module's best-effort understanding of
  `bw serve`'s conventions (confirmed synthetically via a fake local HTTP server in its tests), not something
  confirmed against a live instance; whether `bw`'s login-item JSON exposes FIDO2/passkey data at all is also
  unverified, so `BitwardenBackend` always reports `passkey: None`.
- Full read/write for the Bitwarden section: Edit and Delete now work the same way for Bitwarden rows as
  local-vault ones (the add-credential form doubles as the edit form for both — the overlay's first-ever edit
  capability at all, previously it only had Copy/Delete for local rows and Copy-only for Bitwarden), plus a
  "Save to: Local vault / Bitwarden" picker on new entries and an inline error label for failures against
  either backend. A local (gtk3-only, not `browser-core`) `LoginSource` enum is what a login's Edit/Delete
  buttons use to route to whichever backend it actually came from.
- In-page credential injection ("Fill"): `render_engine::linux::WryEngine::fill_login` (mirrored in
  `macos.rs`, same underlying `wry::WebView` type, though nothing calls it there yet — `browser-macos-appkit`
  has no password-manager UI at all) injects JS that finds the login form's fields, sets them via the native
  property setter + dispatches real `input`/`change` events (plain `el.value = ...` doesn't trigger React/
  Vue-style controlled-input state), and is a plain inherent method rather than part of the `RenderEngine`
  trait — same shape as the pre-existing `toggle_reader_mode`, since a hypothetical non-web engine (or
  `WebView2Engine` today, whose bindings expose no JS-eval hook) can't implement arbitrary JS injection the
  same way. Field detection prefers the standard `autocomplete` attribute (`"current-password"` for the
  password field, `"username"`/`"email"` for the identifier field) when a page marks its fields that way,
  falling back to a positional heuristic (first `input[type="password"]`, and whichever text/email/tel input
  most immediately precedes it) only when `autocomplete` is absent — verified with a fixture
  (`login_form_autocomplete.html`) deliberately laid out so the positional heuristic alone would pick the
  wrong fields, proving the attribute-based selection actually takes priority rather than coincidentally
  agreeing with it. `browser-linux-gtk3`'s password manager overlay gets a "Fill" button per row, shown (and,
  independently, actually enforced in the action itself, not just gated at the UI level) only when the
  login's domain matches the active page's own — filling credentials into a domain they weren't saved for is
  a real phishing-adjacent footgun, not just a UX nicety to restrict. Verified end-to-end against real fixture
  login pages and a real WebKitGTK-rendered DOM read back via a new `evaluate_script_for_test` hook — not just
  "the call didn't error." Known limitations: still only handles one form on the page and doesn't handle
  multi-step (username-page-then-password-page) login flows, forms with multiple password fields (signup/
  change-password) aren't specifically detected and skipped, and same-origin iframes aren't searched — see
  "Backlog" below.
- Password manager UI for `browser-macos-appkit`: local vault (add/view/copy/edit/delete/fill) + Bitwarden,
  matching `browser-linux-gtk3`'s feature set, adapted to this crate's own established idioms rather than a
  literal port — there's no separate-`NSWindow`-that-hands-back-to-the-main-window precedent here (`run_chooser`
  is spawn-and-exit only), so the local vault's and Bitwarden's passphrase setup/unlock flows fold into the
  passwords overlay itself (`setHidden`-toggled sub-groups) instead of popup windows. First use of
  `NSSecureTextField`/`NSPopUpButton`/`NSPasteboard` in this crate. Real vault encryption on macOS was
  confirmed to actually work by direct experiment this session (libsql's `encryption` feature builds and links
  cleanly via `cargo zigbuild` for both architectures) — previously assumed untried, not blocked — so
  `browser-core/Cargo.toml`/`passwords.rs`'s `#[cfg]` gates were widened to include macOS alongside Linux
  (`history.rs`'s own gate, and thus "encrypted profiles"/history encryption, stays Linux-only and untouched —
  the vault's passphrase has always been independent of that). As with everything else in this crate, this is
  compile/link-verified only from this dev machine (no macOS hardware here to actually run it) — real
  behavioral proof needs `.github/workflows/macos.yml` triggered for real (a `v*.*` tag push or manual
  dispatch), which will also be the first time the widened encrypted-vault tests execute on genuine Apple
  hardware.
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
- `browser-macos-appkit` (new crate) + `render-engine::macos`: brought to feature parity with
  `browser-windows-reactor`'s scope — multi-page via `PageManager<WryEngine>` (a per-page container `NSView`,
  shown/hidden on switch, load/unload eviction wired the same way as every other front end), switcher/
  settings/profile overlays (plain vertical lists rather than a wrapping tile grid — `NSCollectionView` is a
  much bigger lift than this pass had time for; a real, working simplification, not a stub), keybindings
  editor folded into settings (same design `browser-windows-reactor` settled on per explicit user feedback),
  global keyboard shortcuts via real `NSMenu` key equivalents (`shortcuts.rs`; `KeyChord::ctrl` maps to ⌘
  Command on this platform, not literal Control — see that file's doc comment), and an external-link chooser
  window. Deliberately *not* `browser-linux-gtk3`'s superset (no bookmarks, light/dark theme, encrypted
  profiles — matches `browser-windows-winui`/`browser-windows-reactor`'s scope, a consistent bar across every
  native-chrome-plus-`PageManager` front end, not gtk3's). One genuine improvement over both Windows front
  ends: `EditUrl` (⌘L) actually works — AppKit's `NSWindow::makeFirstResponder` is a real, unrestricted
  programmatic-focus API, unlike `windows-reactor`'s crate gap (see `browser-windows-reactor/src/lib.rs`'s
  `dispatch_action` doc comment) or `winio-winui3`'s equivalent limitation.

  Also now has a real cross-compile story from Linux for the first time (previously: "no macOS toolchain is
  available here... needs a real compile pass"): `cargo zigbuild` (the same tool `browser-wx`'s Windows
  cross-build already used) plus an unofficial mirror of Apple's SDK stub files
  (`joseluisq/macosx-sdks` — a deliberate, discussed choice given the legal gray area of redistributed SDK
  content, not an oversight; see README.md's "browser-macos-appkit: building" section for the full
  reasoning) via `.cargo/build-macos-appkit.sh`, mirroring `.zig/`'s existing project-local convention.
  Confirmed producing real, linked Mach-O binaries for both `aarch64-apple-darwin` and `x86_64-apple-darwin`
  — every change here is now compile-and-link checked before pushing, not just eyeballed against
  `objc2-app-kit`'s generated source. There's still no way to *run* a macOS binary from this Linux machine
  (no Wine-for-macOS equivalent), so real behavioral verification still only happens on GitHub's native
  macOS runners (see below) — treat runtime behavior as link-checked, not yet proven correct end-to-end.

  `.github/workflows/macos.yml` now matrixes across both architectures on their own native runners —
  `macos-14` (Apple Silicon, arm64) and `macos-13` (Intel, x64) — rather than cross-arch building one target
  on the other's runner (would need Rosetta 2 to actually *run* the x64 binary on the arm64 runner, and
  there's no reverse path for the arm64 binary on the Intel runner at all), plus `cargo test -p
  browser-macos-appkit` to actually execute `shortcuts.rs`'s chord-conversion unit tests for the first time
  (previously compile-checked only, via the crate being `#![cfg(target_os = "macos")]`-gated to an empty
  stub everywhere else). See `summaries/macos-appkit-scaffold.md`.

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
- **Blank page — real root cause found and fixed**: `on_ready` (the callback `windows-webview`'s
  `webview()` calls once `WebView2` actually initializes) never fired, in *any* build, for the entire
  session up to this point — confirmed by re-reading every trace log captured so far and finding zero
  occurrences, ever, not just in one bad run. Decisive proof came from checking `tasklist` for
  `msedgewebview2.exe`: zero instances, meaning `EnsureCoreWebView2Async()` (called by
  `windows-webview`'s `reactor.rs`) never even got the browser process off the ground, regardless of the
  `WEBVIEW2_USER_DATA_FOLDER` fix below. Root cause: the XAML `WebView2` control loads
  `Microsoft.Web.WebView2.Core.dll` at runtime — per `windows-reactor-setup`'s own docs, this WinRT
  projection assembly is *not* present on the machine by default (unlike the COM-only
  `webview2loader.dll` the Evergreen runtime does supply), and is deployed only by that crate's
  `as_self_contained()` path, never by `as_framework_dependent()` — the one this build actually uses.
  Switching to `as_self_contained()` isn't a drop-in fix either: its own package staging shells out to
  `%SystemRoot%\System32\curl.exe`/`tar.exe`, which only exist when build.rs *runs* on a real Windows
  machine, not here, where it runs on this Linux host during `cargo xwin build` cross-compilation. Fixed
  by adding `deploy_webview2_core_dll()` to `build.rs`: fetches the same `Microsoft.Web.WebView2` NuGet
  package (pinned to the same version `windows-reactor-setup` uses) via Linux-native `curl`, extracts
  `Microsoft.Web.WebView2.Core.dll` via `unzip` (a `.nupkg` is just a zip), and copies it next to the exe
  — cached locally so it's only fetched once. **Verified working**: redeployed to the dockur/windows VM,
  and for the first time all session, `on_ready: page 0 WebView2 ready` appeared in the trace log, six
  `msedgewebview2.exe` processes were running, and `https://www.google.com` (see below) rendered as a
  real page — logo, search box, nav links, all real, confirmed via screenshot.
  Two smaller, real fixes made along the way while chasing this (neither was the actual blocker, but both
  are genuine bugs):
  - `ReactorWebViewEngine::navigate()`/`go_back()`/`go_forward()`/`reload()` (`engine.rs`) were silently
    returning `Ok(())` when the webview wasn't ready yet, instead of a real error — masking "successfully
    navigated" and "silently did nothing" as indistinguishable in logs. Fixed to return a real error.
  - `WEBVIEW2_USER_DATA_FOLDER` set to `%LOCALAPPDATA%\claude-browser\webview2` (`main.rs`) — a real,
    documented `WebView2` behavior (default user-data folder next to the exe fails silently on
    unwritable paths like a UNC share), worth keeping, but not what was actually blocking rendering.
  `HOME_URL` (`browser-core/src/lib.rs`) changed from `about:blank` to `https://www.google.com` — a real
  page to actually exercise rendering against, not just prove `WebView2` initializes.
- **Shortcuts**: confirmed working correctly via a rigorous retest in the VM (`Ctrl+T`, `Escape` all fire
  — verified via trace logging, not assumption, across multiple fresh launches). One specific action,
  `EditUrl` (`Ctrl+L`), *does* dispatch correctly (`dispatch_action: fired for EditUrl` traced reliably)
  but has an intentionally-empty handler — not a bug, a real crate-level gap: focusing a control
  programmatically needs a `Focus()`-style call neither `TextBox` nor `Element`/`Widget` expose anywhere
  in `windows-reactor` (checked directly), and there's no way to get a raw handle to the underlying XAML
  element from application code either. Documented in `lib.rs`'s `dispatch_action` doc comment rather than
  silently left alongside genuinely-unbuilt features (bookmarks, reader mode).
  There is still one *real*, documented, narrower gap: `windows-webview`'s reactor bridge (`webview()`)
  doesn't expose the underlying `Controller` object, so there's no way to wire up
  `Controller::on_accelerator_key_pressed` — meaning shortcuts genuinely won't fire while keyboard focus
  is *inside* the `WebView2` content area itself (a real WinUI 3/WebView2 integration requirement, not
  specific to this codebase — see `shortcuts.rs`'s doc comment).
- **Toolbar was click-dead**: found by testing an actual toolbar click (the Settings gear icon) and
  seeing zero `dispatch_action` trace, ever, while the same action fired instantly via a keyboard
  accelerator moments earlier. Root cause: the toolbar was hosted inside `TitleBar::new(...)
  .content(toolbar)` (`lib.rs`) — `windows-reactor`'s `host.rs` wires that slot up via
  `Window.SetTitleBar(element)`, which marks it as the draggable caption region; real WinUI apps that put
  interactive controls there need to separately register non-client hit-test passthrough rectangles
  (`InputNonClientPointerSource.SetRegionRects`), which `windows-reactor` doesn't do anywhere in its own
  source (checked directly). Fixed by moving the toolbar out of `.content()` into its own ordinary grid
  row, right below a `TitleBar` that now hosts only the native drag/window-chrome area.
- Also found and fixed a real, separate bug while investigating the click issue: the binary had no
  `#![windows_subsystem = "windows"]` attribute, so it launched with a visible console window attached
  (visible throughout manual testing as a black window titled with the exe's path) in addition to the
  actual WinUI 3 window. Added the attribute; confirmed via Task View that only one window exists now.
- **Still open, deprioritized per explicit direction**: clicking anywhere on the app window (toolbar or
  content) still reliably knocks it out of the foreground in VM testing — confirmed still alive and
  unminimized via Task View every time, just not topmost — even after both fixes above. Keyboard-driven
  interaction is unaffected and confirmed reliable. Root cause not yet identified; set aside for now to
  focus on rendering, not because it's resolved.

See `summaries/windows-github-actions-ci.md` for the full incremental build log.

Repo is pushed to `danshryock/one-page-browser` (`git@github.com:danshryock/one-page-browser.git`), with `gh`
installed and authenticated on this dev machine — real job logs are pulled via `gh run view --log-failed`/the
Actions API, not guessed at.

**`.github/workflows/macos.yml`** passed completely on every run against the original minimal scaffold,
including the full `build-and-smoke-appkit` job (build + launch + screenshot) — genuine confirmation that
version compiled and ran on real macOS, not just `cargo check`-clean on Linux. The substantially expanded,
feature-parity version (multi-page, overlays, `NSMenu` shortcuts — see "Done" above) hasn't been through
this workflow yet as of this writing: it's compile-and-link checked via the new Linux cross-compile path,
but real behavior on native macOS hardware is still unconfirmed. Expect real iteration on the first run.

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

- `browser-macos-appkit`: bookmarks, light/dark theme, and encrypted profiles (history encryption) — the
  remaining parts of `browser-linux-gtk3`'s scope neither Windows front end has either (see "Done" above —
  the password manager is now at parity; these three are unrelated to it and still open). A wrapping tile
  grid (`NSCollectionView`) instead of the current plain-list switcher/profile/passwords overlays is a
  smaller, separate follow-up.
- `browser-windows-winui`: unified search/URL bar and bookmarks, matching what `browser-linux-gtk3` now has
  (both landed there only, per scope — see "Done" above).
- Other external password managers beyond Bitwarden/Vaultwarden (see "Done" above for that one) — KeePassXC/
  secret-service, 1Password, etc. Each would be its own `PasswordBackend` impl; no shared "generic external
  manager" abstraction beyond the trait itself is needed until a second one is actually built.
- Verify `BitwardenBackend`'s request/response shapes against a real `bw serve` instance — flagged as
  unverified in its own doc comment, since none was reachable while building it. In particular: whether
  `bw`'s login-item JSON exposes FIDO2/passkey data at all (`BitwardenBackend` always reports `passkey: None`
  today, rather than guess), and whether `bw serve`'s in-memory unlocked state genuinely persists across
  requests to the same process the way this code assumes.
- Actual passkey creation/assertion in pages — the schema (`PasskeyCredential`, see "Done" above) is ready,
  but this needs a WebAuthn virtual authenticator hooked into `navigator.credentials.create()/get()` at the
  render-engine layer, differently for WebKitGTK/WebView2/WKWebView — comparable in scope to the in-page
  autofill item below, not something the schema work alone unblocks.
- Autofill: skip forms with multiple password fields (signup/change-password forms have current+new+confirm)
  rather than guessing which one is "the" login password — not attempted yet, deliberately deferred when the
  `autocomplete`-attribute preference landed (see "Done" above).
- Autofill: search same-origin iframes too (some login forms are embedded in one rather than the top-level
  document) — `querySelector` today only searches the main document. Cross-origin iframes still couldn't be
  touched regardless, by design (the same-origin policy).
- Autofill correctness on real, complex login pages (multi-step username-then-password flows) — the current
  heuristic (see "Done" above) doesn't handle a password field that only appears after a JS-driven step
  transition with no page navigation; verified against controlled fixture pages, not real sites. Varies a lot
  site-to-site in ways headless fixture-page testing can't validate — deserves focused real-site testing, not
  a rushed pass.
- Get libsql's `"encryption"` feature (SQLite3 Multiple Ciphers, via libsql-ffi) building for the
  cross-compiled targets, not just native Linux — today it's scoped to `target_os = "linux"` only in
  `browser-core/Cargo.toml`, so `HistoryStore::open_encrypted`/`PasswordStore::open_encrypted` are stubs that
  always error on Windows/macOS builds from this dev machine. The confirmed blocker for the Windows MSVC
  target (`cargo build-windows-winui`/`-reactor`, via `cargo-xwin`) is that libsql-ffi's CMake build needs
  `llvm-lib`, not available in this toolchain — worth revisiting once/if that's installable. The macOS
  zigbuild cross-compile path hasn't actually been tried with the feature enabled at all (the Linux-only
  scoping was chosen for simplicity, not because macOS was tested and failed) — that's the cheaper first
  thing to check.
- Changing/removing a profile's passphrase, or migrating an existing unencrypted profile (history or vault)
  to encrypted (`sqlite3_rekey` is available via libsql-sys but not wired up yet) — i.e. key rotation.
- Investigate key derivation for the two encrypted stores: today `HistoryStore::open_encrypted`/
  `PasswordStore::open_encrypted` both hand the *same raw passphrase bytes* straight to libsql's
  `EncryptionConfig`, and whatever key-derivation SQLite3 Multiple Ciphers does internally from those bytes
  happens the same way for both databases — worth investigating whether deriving a separate, store-specific
  key from the shared passphrase (e.g. via HKDF with a per-store context/salt) would be meaningfully safer
  than reusing identical key material across two independent database files, and whether that's compatible
  with `decide_vault_unlock_action`'s "one passphrase, both stores" UX or would need to change it.
- `browser-windows-winui` debugging — it's been cross-compile/link-verified only all along (see "Done"
  above), never actually run; once it can be run on real Windows, expect a real debugging pass (custom
  title bar drag, the `WM_KEYDOWN` HWND-subclass keybinding capture, `WebView2` control behavior, etc. are
  all unverified at runtime).
- A real semantic embedding for vector search (swapping in a local ML model or a network embedding API in
  place of the current lexical hashing-trick embedding) — see `summaries/vector-search.md`'s "Scope notes."
