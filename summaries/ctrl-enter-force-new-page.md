# Ctrl+Enter forces a new page

**Roadmap item:** "Ctrl+Enter to force-open a new page even when the typed text matches an existing open
page/history entry (today, a single match always switches to it instead)."

## What changed

- `crates/browser-linux-gtk3/src/lib.rs`:
  - Added `AppState::force_new_page_from_search(text: &str)` — resolves `text` via `resolve_address_input`
    and always calls `add_page` + `close_switcher`, regardless of whether it matches an open page or
    history entry. Placed next to `switch_to`, which is the plain-Enter counterpart it's an escape hatch
    from.
  - Wired a `key-press-event` handler on the address bar (ahead of its existing `connect_activate`
    handler) that checks for `Return`/`KP_Enter` with the Ctrl modifier held while the switcher is open; if
    both are true, it calls `force_new_page_from_search` and stops event propagation so the plain-Enter
    `activate` handler doesn't also fire. A bare Enter (no Ctrl) is left to propagate normally, so nothing
    about the existing plain-Enter behavior changed.

## Why this design

The existing `connect_activate` handler only fires on GtkEntry's higher-level "activate" signal, which
carries no modifier-key information — there's no way to ask "was Ctrl held when Enter activated this?"
from inside that handler. Intercepting the raw `key-press-event` first (the same mechanism the window-level
Escape/keybinding dispatch already uses) is the only place the Ctrl state is available before GTK's default
handling turns Enter into "activate".

## Testing

- New test `ctrl_enter_forces_a_new_page_even_when_one_match_exists` in
  `crates/browser-linux-gtk3/tests/gtk_tests.rs`: opens two pages, opens the switcher, and calls
  `force_new_page_from_search("page b")` (bypassing the raw synthetic-key-event problem the same way
  `search_activate`/`address_bar_activate` already do, by testing the underlying `AppState` method directly
  rather than trying to synthesize a real GDK key event with modifier state). Confirms a *new* page is
  created (not a switch to the existing "Page B"), that it becomes active, and that the switcher closes —
  the same closing behavior as plain Enter.
- `cargo test -p browser-core`: 51/51 passing (unaffected by this change, included for completeness).
- `cargo clippy --all-targets` on `browser-linux-gtk3`/`browser-core`/`browser-windows-winui`: clean (only
  pre-existing, unrelated warnings).
- `cargo build-windows-winui` (msvc cross-compile) and `cargo build --target x86_64-pc-windows-gnu
  --workspace --exclude browser-wx`: both succeed — this change is gtk3-only and doesn't touch
  `browser-core`'s public surface, so no other frontend needed changes.
- Full headless GTK suite via `wlheadless-run -c cage -- xwayland-run -- cargo test -p browser-linux-gtk3`:
  all tests passing, including the new one.

## Scope notes

`browser-windows-winui` and macOS were explicitly out of scope for this pass (per the "work on backlog items
that don't involve Windows and macOS" instruction) — this is a gtk3-only change, using an existing
`browser_core` function (`resolve_address_input`) with no new core API needed.
