# Show bookmarks when searching the grid

**Roadmap item:** "Show bookmarks when searching the grid."

## What changed

`crates/browser-linux-gtk3/src/lib.rs`'s `rebuild_switcher_grid` already added extra tiles for matching
history entries not already open, when the search box has a non-empty query. It now does the same for
matching bookmarks: `Bookmarks::search(&query)` (already existed, unused by the grid until now), capped at 8
results the same way history is, skipping any URL already shown either as an open page's tile or as a history
tile (`shown_urls` now accumulates across both loops instead of just tracking open pages).

Bookmark tiles get a distinct `.bookmark-tile` CSS class — a warm amber-tinted dashed border, visually
paired with (but distinguishable from) the existing cooler-toned `.history-tile` — clicking one opens it as a
new page and closes the switcher, same as a history tile.

**Refactor along the way**: history and bookmark tiles were near-identical blocks of tile-building code
(title/domain labels, click handler, `FlowBoxChild` wrapping) with only the CSS class and click-handler body
differing. Extracted the shared shape into a new `build_search_result_tile(extra_css_class, title, domain,
on_click)` helper, used by both, rather than duplicating the ~20-line tile-construction block a third time.

## Testing

- **A real bug in my first version of this test, caught before committing**: my first attempt bookmarked an
  *open* page, closed it, then asserted the grid's total tile count increased after searching. It failed —
  not because the feature was broken, but because closing the fixture page (via `add_page`, which records a
  history visit once the title loads) meant the URL already had a history entry too, so the search matched it
  via the *history*-tile path (which runs first and claims the URL, per the dedup logic) instead of the
  bookmark-tile path — and separately, the aggregate tile count is fundamentally the wrong thing to assert on
  here anyway: the fallback page that `close_page` opens automatically also drops out of the filtered results
  when the query no longer matches it, so "tile count before vs. after" can't reliably distinguish "a new tile
  appeared" from "a different tile disappeared and one appeared." Fixed by adding a new
  `AppState::bookmark_url_for_test()` helper that bookmarks a URL directly without ever opening it as a real
  page (so it can never end up in history), and a new `AppState::switcher_grid_has_tile_with_class()` helper
  that inspects the actual flowbox children's CSS classes — a precise check for "a bookmark tile is present"
  rather than an aggregate count.
- The resulting test, `switcher_grid_shows_bookmark_matches_not_currently_open`: bookmarks a URL directly,
  searches for it in the switcher, and confirms a `.bookmark-tile`-classed tile appears.
- `cargo check --tests` / `cargo clippy --all-targets` on `browser-linux-gtk3`: clean.
- `cargo build` (workspace), `cargo build --target x86_64-pc-windows-gnu --workspace --exclude browser-wx`,
  `cargo build-windows-winui`: all succeed unchanged.
- Full headless GTK suite via `wlheadless-run -c cage -- xwayland-run -- cargo test -p browser-linux-gtk3`:
  all passing, including the existing `switcher_search_and_grid`/`bookmarks_toggle_and_overlay` tests
  (confirming the refactor didn't change open-page or plain bookmark-toggle behavior).

## Scope notes

`browser-linux-gtk3`-only, no `browser-core` changes needed — `Bookmarks::search` already existed and did
exactly what was needed.
