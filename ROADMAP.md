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

## Next

Nothing specifically queued — pick the next item from the backlog below.

## Backlog (not yet started, roughly in the order raised)

- macOS: native chrome via AppKit, following the same `RenderEngine`-trait pattern as the other front ends.
- Ctrl+Enter to force-open a new page even when the typed text matches an existing open page/history entry
  (today, a single match always switches to it instead).
- `browser-windows-winui`: unified search/URL bar and bookmarks, matching what `browser-linux-gtk3` now has
  (both landed there only, per scope — see "Done" above).
- Private/incognito/guest profile.
- Page screenshotting.
- Reader mode.
- External password manager integration.
- Internal password manager.
- Color themes (light/dark, possibly custom) across the native chrome in each frontend.
- `browser-windows-winui` debugging — it's been cross-compile/link-verified only all along (see "Done"
  above), never actually run; once it can be run on real Windows, expect a real debugging pass (custom
  title bar drag, the `WM_KEYDOWN` HWND-subclass keybinding capture, `WebView2` control behavior, etc. are
  all unverified at runtime).
