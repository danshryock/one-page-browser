# Move the keybindings screen into the settings screen

**Roadmap item:** "Move the keybindings screen into the settings screen."

## What changed

`crates/browser-linux-gtk3/src/lib.rs` — the keybindings editor is no longer its own overlay/toolbar
destination; its row list now lives directly inside the settings overlay.

- **Removed entirely**: the standalone `keybindings_overlay`/`keybindings_scrim`/`keybindings_box`/
  `keybindings_close_row`/`keybindings_close_button` construction, the toolbar's keyboard-icon
  `keybindings_button`, `AppState::open_keybindings`/`close_keybindings`/`is_keybindings_open`, and the
  `keybindings_panel: gtk::Widget` field. The Escape-chain in the window's `key-press-event` handler lost its
  `is_keybindings_open()` branch (folded into the existing `is_settings_open()` one).
- **Added to `settings_box`** (after the existing start-page/search-engine/loaded-pages-limit fields, before
  the Cancel/Save row): a separator, a "Keybindings" section title (reusing `.settings-title`), and
  `keybindings_list_box` wrapped in a `gtk::ScrolledWindow` (`set_max_content_height(260)` +
  `set_propagate_natural_height(true)`) — with `Action::ALL` now up to 10 actions, packing the rows straight
  into the box would have made the combined overlay uncomfortably tall; the scrolled window caps it while
  still growing naturally for a shorter list.
- `open_settings()` now also calls `self.listening_for.set(None)` and `self.rebuild_keybindings_list()` (previously
  `open_keybindings`'s job) so the merged editor is freshly populated every time settings opens.
- `close_settings()` now also clears `listening_for` (previously `close_keybindings`'s job), so Escape/Cancel/
  the scrim still correctly cancels an in-progress "press keys…" capture — same end behavior as before, just
  consolidated into one close path instead of two.
- The other three `open_*` methods (`open_switcher_common`, `open_profile_picker`, `open_bookmarks`) had their
  now-nonexistent `self.close_keybindings()` calls removed — `close_settings()` covers it since keybindings
  is now part of settings.
- New `AppState::keybindings_row_count()` test/inspection helper.

## Design notes

- Keybinding add/remove already saved immediately on every change (independent of the Settings overlay's own
  Save/Cancel buttons) — that didn't need to change, and still doesn't: keybinding edits persist right away
  even if you then hit Settings' Cancel.
- Kept the section reachable only by opening Settings — no separate keyboard shortcut or button was added to
  jump straight to "settings, scrolled to keybindings," since the roadmap phrasing was specifically about
  consolidating the *destination*, not preserving a separate fast path to just the keybindings section.

## Testing

- New test `keybindings_editor_lives_inside_settings` in `crates/browser-linux-gtk3/tests/gtk_tests.rs`:
  confirms opening settings populates `keybindings_row_count()` with exactly `Action::ALL.len()` rows, and
  that closing works normally.
- `cargo check --tests` / `cargo clippy --all-targets` on `browser-linux-gtk3`/`browser-core`/
  `browser-windows-winui`: clean.
- `cargo build` (workspace), `cargo build --target x86_64-pc-windows-gnu --workspace --exclude browser-wx`,
  `cargo build-windows-winui`: all succeed — this is a `browser-linux-gtk3`-only structural change (no
  `browser-core` API touched), so nothing else needed changes.
- Full headless GTK suite via `wlheadless-run -c cage -- xwayland-run -- cargo test -p browser-linux-gtk3`:
  all passing (existing `settings_overlay_mutual_exclusion_and_save` test, which doesn't reference
  keybindings at all, kept passing unchanged — confirming the merge didn't disturb the settings fields'
  existing behavior).

## Scope notes

`browser-linux-gtk3`-only, as with every other item in this session; `browser-windows-winui`'s keybindings
editor is untouched (still its own overlay there) — folding it in too is left as a follow-up if wanted.
