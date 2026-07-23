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

## Next

Nothing specifically queued — pick the next item from the backlog below.

## Backlog (not yet started, roughly in the order raised)

- macOS: native chrome via AppKit, following the same `RenderEngine`-trait pattern as the other front ends.
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
