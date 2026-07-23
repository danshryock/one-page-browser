# Search engine management

**Roadmap item:** "Add the search engine management features."

## A pre-existing bug found and fixed along the way

The settings overlay's default-search-engine dropdown (`engine_combo`) was populated **once**, at window
construction time, from `Settings::default().search_engines` — the hardcoded fallback list, not the profile's
actual live `Settings`. It never refreshed on reopen either. This was harmless as long as every profile's
engine list was identical to the hardcoded default (true before this change, since there was no way to add or
remove one), but it would have silently broken the moment any management UI existed — a newly added engine
would never appear in the dropdown at all. Fixed by adding `refresh_engine_combo()`, which clears and
repopulates the combo from `self.settings.borrow().search_engines` and re-selects the real current default,
called every time `open_settings()` runs (and after every add/remove).

## What changed

- `crates/browser-core/src/settings.rs`: `Settings` gained two methods:
  - `add_search_engine(&mut self, name, query_url_template)` — adds a new entry, or updates the query URL
    template in place if the name already exists (same dedupe-by-key convention as `Bookmarks::add`).
  - `remove_search_engine(&mut self, name) -> bool` — removes by name, **refuses to remove the last remaining
    engine** (there must always be at least one to fall back to for `resolve_address_input`), and reassigns
    `default_search_engine` to the first remaining entry if the one removed was the default. Returns whether
    anything was actually removed.
- `crates/browser-linux-gtk3/src/lib.rs`: the settings overlay gained a management section right below the
  existing default-engine dropdown — a list of current engines (name + query URL template, each with a "×"
  to remove, omitted entirely when only one engine remains since removal would be refused anyway) and an
  "add engine" row (name + URL-template fields, with a hint placeholder showing the `{query}` token, plus an
  "Add engine" button/Enter-to-submit).

## Design decision: immediate save, not staged with the rest of the form

The existing settings fields (start page, default engine choice, loaded-pages limit) are staged in their
widgets and only committed to `Settings`/disk when "Save" is clicked (discarded on "Cancel"). Engine
add/remove instead take effect and save **immediately** on each click — matching the convention this
session's bookmarks and keybindings editors already use, rather than building a separate staged/undo-able
list-editing model just for this one section. Concretely: if you add an engine and then hit the settings
overlay's "Cancel", the new engine stays; only the start-page/default-choice/loaded-pages fields are
discarded. Flagging this explicitly in case the intended design was for engine edits to be staged too.

## Testing

- `browser-core`: four new tests in `settings.rs` — `add_search_engine_appends_a_new_one`,
  `add_search_engine_with_an_existing_name_updates_instead_of_duplicating`,
  `remove_search_engine_reassigns_the_default_if_it_was_removed`,
  `remove_search_engine_refuses_to_remove_the_last_one`. `cargo test -p browser-core`: 61/61 passing.
- `crates/browser-linux-gtk3/tests/gtk_tests.rs`: new `search_engine_management_add_and_remove` — adds an
  engine and confirms it's reflected in both the real `Settings` data (`settings_engine_names()`, a new test
  helper) and the management list's row count; confirms re-adding the same name doesn't duplicate a row;
  removes engines down to the last one and confirms the dropdown's active id (`engine_combo_active_id()`,
  another new test helper) correctly tracks whichever engine ends up as the reassigned default — this is the
  test that would have caught the stale-dropdown bug described above, had it existed before the fix.
- `cargo clippy --all-targets` on `browser-linux-gtk3`/`browser-core`/`browser-windows-winui`: clean.
- `cargo build` (workspace), `cargo build --target x86_64-pc-windows-gnu --workspace --exclude browser-wx`,
  `cargo build-windows-winui`: all succeed unchanged.
- Full headless GTK suite via `wlheadless-run -c cage -- xwayland-run -- cargo test -p browser-linux-gtk3`:
  all passing.

## Scope notes

`browser-core` + `browser-linux-gtk3` only, consistent with every other item this session.
