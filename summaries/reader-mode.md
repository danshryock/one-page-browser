# Reader mode

**Roadmap item:** "Reader mode."

## Design

Reader mode is pure JavaScript injected into the existing WebKitGTK webview via `evaluate_script` — no new
`browser-core` state, no new persistent data, nothing tracked in Rust beyond triggering the toggle. This is
**not** a vendored copy of Mozilla's Readability.js (the library most browsers actually use) — there's no
network access available in this environment to fetch it, and nothing already in the dependency tree provides
it. Instead, `render-engine::linux::WryEngine::toggle_reader_mode` runs a small, hand-rolled extraction
heuristic:

1. Prefer an existing `<article>`, `<main>`, or `[role="main"]` element if the page has one.
2. Otherwise, score every `<div>`/`<section>` with at least 2 `<p>` children by `text length + paragraph
   count × 100`, and take the highest-scoring one.
3. Fall back to `<body>` itself if nothing scores.
4. Replace the whole document with just that content, wrapped in clean serif typography (a fixed reading
   width, larger line height, no ads/nav/sidebar markup left behind since only the winning element's HTML is
   kept).
5. Retitle the page `"Reader: <original title>"` — both a visible cue that reader mode is active (shows up in
   the OS window title/task list) and, not incidentally, the signal this feature's own test checks for.

Calling it again while active restores the original page: the script stashes the pre-reader-mode HTML and
title on `window` before replacing anything, and swaps back on the next call. Nothing survives a real
navigation or reload — reader mode is inherently per-page-load, which is expected.

This is honestly a simpler, less robust heuristic than real Readability.js (no link-density scoring, no
class/id pattern matching against known ad/nav naming conventions, no handling for multi-page articles) — it
works well on pages with one clear "this is the article" container, and will do a mediocre job on more
unusual layouts. Flagging this limitation directly rather than overstating what a hand-rolled heuristic can
do.

## Why this isn't part of the `RenderEngine` trait

Unlike `navigate`/`reload`/`screenshot`, reader mode is pure JS injection specific to what a *webview-backed*
engine can do — not something a hypothetical non-web `RenderEngine` implementation would meaningfully support
the same way. It's a plain additional method on the concrete `WryEngine` type instead, called directly through
`browser-linux-gtk3`'s existing `with_active::<WryEngine>` (which already operates on the concrete type, not
the trait, confirmed by reading its signature before writing this). No stub needed on `browser-windows-winui`/
win32/nwg/`browser-wx` for the engine method itself — only the new `Action::ToggleReaderMode` keybinding
variant needed the usual no-op arm to keep winui3's exhaustive `match` compiling.

## What changed

- `crates/render-engine/src/linux.rs`: `WryEngine::toggle_reader_mode()` (inherent method, not part of the
  `RenderEngine` trait) + the `READER_MODE_SCRIPT` constant described above.
- `crates/browser-core/src/keybindings.rs`: new `Action::ToggleReaderMode` (unbound by default —
  toolbar-button-only, same as `OpenSettings`/`OpenProfilePicker`/`OpenBookmarks`).
- `crates/browser-linux-gtk3/src/lib.rs`: `AppState::toggle_reader_mode()` (calls the engine method through
  `with_active`), a new toolbar button, and a `dispatch_action` arm for the new `Action`.
- `crates/browser-windows-winui/src/lib.rs`: added `Action::ToggleReaderMode` to the existing no-op
  dispatch arm (alongside bookmarks/`EditUrl`) to keep the exhaustive match compiling.

## Testing

- New test `reader_mode_toggles_on_and_off` in `crates/browser-linux-gtk3/tests/gtk_tests.rs` — builds a raw
  `WryEngine` directly (same pattern as the existing `navigation_back_forward_reload` test), toggles reader
  mode on and confirms the title becomes `"Reader: Page A"`, then toggles it off again and confirms the
  original title (`"Page A"`) comes back. This is a real, working end-to-end check of the toggle mechanism —
  the test fixture (`page_a.html`) is a single line of HTML with no paragraphs at all, so it exercises the
  `<body>` fallback path rather than the `<article>`/scored-`<div>` extraction paths, which weren't separately
  covered by a test; worth a manual look at a real article page to judge the extraction heuristic's actual
  quality.
- `cargo check --tests` / `cargo clippy --all-targets` on `render-engine`/`browser-linux-gtk3`/`browser-core`/
  `browser-windows-winui`: clean.
- `cargo build` (workspace), `cargo build --target x86_64-pc-windows-gnu --workspace --exclude browser-wx`,
  `cargo build-windows-winui`: all succeed.
- Full headless GTK suite via `wlheadless-run -c cage -- xwayland-run -- cargo test -p browser-linux-gtk3`:
  all passing.

## Scope notes

`render-engine::linux` + `browser-core` (just the new `Action` variant) + `browser-linux-gtk3` only, as with
every other item this session.
