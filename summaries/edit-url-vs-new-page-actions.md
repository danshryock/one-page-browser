# Separate actions for "grid, new page" vs. "grid, edit URL"

**Roadmap item:** "users need to be able to edit urls, so we need separate actions for grid/new page and
grid/edit url. Ctrl+L would move to the url box, select all text but not blank it, Ctrl+T and F1 would move
to the url box and start with it blank."

## What changed

- `crates/browser-core/src/keybindings.rs`:
  - Added `Action::EditUrl` (label "Edit current URL"), inserted right after `Action::OpenSwitcher` (now
    labeled "Open switcher (new page)" to disambiguate the two in the keybindings editor UI) in both the
    enum and `Action::ALL`.
  - Default bindings: `Action::OpenSwitcher` keeps F1 and Ctrl+T; **Ctrl+L moved off of it** and onto the new
    `Action::EditUrl`.
  - Updated `default_preserves_todays_hardcoded_bindings` to assert Ctrl+L now maps to `EditUrl`, not
    `OpenSwitcher`.
- `crates/browser-linux-gtk3/src/lib.rs`:
  - Refactored `open_switcher` into a shared private `open_switcher_common` (closes the other overlays,
    rebuilds the grid, disables the background stack, shows the panel, focuses the address bar) plus a thin
    `open_switcher` that blanks the address bar first — unchanged behavior for F1/Ctrl+T.
  - Added `open_switcher_editing_url`: preloads the address bar with the active page's **current URL**
    (not blank), calls the same `open_switcher_common`, then fully selects the text via
    `address_bar.select_region(0, -1)` — Ctrl+L's traditional "edit the URL" role.
  - `dispatch_action` gained `Action::EditUrl => self.open_switcher_editing_url()`.
- `crates/browser-windows-winui/src/lib.rs`: added `Action::EditUrl` to the existing no-op arm (alongside
  the bookmark actions from the previous session) so the crate keeps cross-compiling against the new
  `Action` variant — not a feature implementation there, just keeping the match exhaustive.

## Design decision worth flagging

The roadmap note uses "grid/new page" and "grid/edit url" as parallel names, which read as two flavors of
the *same* grid-opening action rather than "Ctrl+L should bypass the grid entirely" (the more traditional
browser convention). I went with the literal reading: **both still open the switcher grid** — you can still
click a different open page's tile either way — they differ only in what the address bar starts with (blank
vs. the current URL, selected). If this isn't the intended behavior (e.g. Ctrl+L should just focus the
address bar without showing the grid at all), that's a one-line change to `open_switcher_editing_url` to
skip `open_switcher_common`'s grid-showing steps — flagging it here rather than guessing further.

## Testing

- New test `edit_url_opens_switcher_with_current_url_selected_not_blanked` in
  `crates/browser-linux-gtk3/tests/gtk_tests.rs`, using a new `AppState::address_bar_is_fully_selected()`
  test helper (checks `selection_bounds() == Some((0, len))`). Confirms the switcher opens, the address bar
  shows the current URL (not blank), and it's fully selected.
- `default_preserves_todays_hardcoded_bindings` in `browser-core` updated and passing, confirming the new
  Ctrl+L → `EditUrl` / Ctrl+T,F1 → `OpenSwitcher` split.
- `cargo test -p browser-core`: 51/51 passing.
- `cargo clippy --all-targets` on `browser-linux-gtk3`/`browser-core`/`browser-windows-winui`: clean.
- `cargo build-windows-winui` (msvc): succeeds with the added no-op `Action::EditUrl` arm.
- Full headless GTK suite via `wlheadless-run -c cage -- xwayland-run -- cargo test -p browser-linux-gtk3`:
  all tests passing.

## Scope notes

`browser-windows-winui` only received the minimal no-op dispatch arm needed to keep compiling; the real
Ctrl+L/Ctrl+T split for winui3 is left as a follow-up backlog item, consistent with bookmarks and the
unified search bar before it.
