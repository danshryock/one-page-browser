# Vector search for page/history search (libsql native vector search)

**Roadmap item:** "Add vector search to the page/history search using libsql's native vector search."

## Why this was previously left undone, and what changed

The earlier pass on this backlog investigated and deliberately stopped short: libsql's vector SQL functions
(`vector32`, `vector_distance_cos`, etc.) are real and already present in the bundled SQLite build — nothing
new to add there — but vector search is only as good as the embeddings behind it, and generating genuine
*semantic* embeddings needs either a local ML model (a real dependency-size/complexity decision) or a network
embedding API (a cost/API-key/network-access decision). Neither is available in this environment, and neither
felt like something to pick unsupervised.

Asked to look at this again, the approach taken is a **deterministic, dependency-free local embedding** (the
"hashing trick" — a real, long-established technique, not something invented for this) rather than either of
those. This is **explicitly not semantic search** — it can't tell "car" and "automobile" are related — but it
does something genuinely more useful than exact substring matching: it finds entries sharing *vocabulary* with
a query regardless of word order or which exact substring matched. That's a real, verifiable capability (see
the tests below), just a more modest one than "vector search" might imply at first read. Swapping in a real
embedding model or API later only means changing one function (`embedding::embed`) — every SQL statement
downstream of it stays the same.

## How it works

- `crates/browser-core/src/embedding.rs` (new module): `embed(text) -> [f32; 64]` tokenizes (lowercase,
  split on non-alphanumeric), hashes each token with a hand-rolled FNV-1a (not `std::hash::DefaultHasher`,
  which isn't guaranteed stable across Rust versions — and a stored embedding needs to stay comparable against
  one computed by a rebuilt binary months later), adds a hash-derived `+1`/`-1` into one of 64 fixed
  dimensions per token, then L2-normalizes the result. `to_sql_literal` renders it as the JSON-array text
  `vector32(...)` expects (e.g. `"[0.1,-0.2,...]"`).
- `crates/browser-core/src/history.rs`:
  - `history` table gains an `embedding BLOB` column, with a best-effort `ALTER TABLE ... ADD COLUMN`
    migration for any database created before this feature existed (`CREATE TABLE IF NOT EXISTS` silently
    no-ops against an existing table with an older shape).
  - `record_visit` now also computes and stores the title's embedding (updated on every revisit, since the
    title can change).
  - New `search_similar(query, limit)`: embeds the query, runs `ORDER BY vector_distance_cos(embedding,
    vector32(?)) ASC LIMIT ?` — a real, native SQL function doing the actual similarity math, nothing
    hand-rolled at the SQL layer. Caps results to `distance < 0.9` (titles sharing no real vocabulary land
    close to `1.0`) so a query with nothing genuinely similar returns nothing, rather than the least-bad
    match regardless of how bad — "meaningfully similar or empty," not "the N closest no matter what."
- `crates/browser-linux-gtk3/src/lib.rs`: `rebuild_switcher_grid` gained a **third** tile category (after
  open-page and history/bookmark substring matches, before which this whole session's earlier "show bookmarks
  in grid search" work already established the dedup pattern for): lexically-similar history entries, styled
  with a new `.similar-tile` class (teal-tinted, distinct from history's neutral gray and bookmarks' amber),
  deduped against everything already shown.

## No Cargo/cross-compile changes needed

Unlike the passphrase-encryption feature earlier this session (which needed scoping libsql's `encryption`
feature to Linux only after a real MSVC cross-compile break), `vector32`/`vector_distance_cos` are present in
libsql-ffi's **base** bundled SQLite (confirmed directly in the vendored source, in both the plain and
cipher-enabled builds) — no new Cargo feature, no new dependency, works identically on every target already
verified this session (native Linux, `x86_64-pc-windows-gnu`, and the `x86_64-pc-windows-msvc` winui3
cross-compile that broke for the *encryption* feature specifically).

## Testing

- `browser-core`: 7 new tests in `embedding.rs` (determinism, case-insensitivity, word-order independence —
  documented as a real limitation of this approach, not hidden — shared-vocabulary similarity actually scoring
  higher than unrelated text, empty-text handling, L2-normalization, and the SQL literal format), plus 5 new
  tests in `history.rs`: `search_similar_finds_lexically_related_titles_not_matched_by_substring_search` (the
  core claim of this feature, verified directly — a query matching zero substring results still finds the
  vocabulary-sharing entry via `search_similar`), `search_similar_returns_nothing_for_an_unrelated_query`
  (confirms the distance threshold actually excludes unrelated entries rather than always returning
  something), `search_similar_on_an_empty_store_returns_no_results_without_erroring`, and
  `opening_a_database_created_before_the_embedding_column_existed_still_works` (builds a real pre-migration
  database by hand, confirms the `ALTER TABLE` migration runs cleanly, old rows are correctly excluded from
  `search_similar` since they have no embedding, and freshly recorded rows get one and are searchable).
  `cargo test -p browser-core`: 79/79 passing.
- `crates/browser-linux-gtk3/tests/gtk_tests.rs`: new `switcher_grid_shows_lexically_similar_history_matches`
  (using a new `AppState::record_history_visit_for_test` helper, mirroring the existing
  `bookmark_url_for_test` — needed since the fixture pages' titles are single words with no vocabulary to test
  similarity against) — records a title sharing 3 of 4 words with a search query that has zero literal
  substring overlap, and confirms a `.similar-tile` appears.
- `cargo clippy --all-targets` on `browser-core`/`browser-linux-gtk3`/`render-engine`/`browser-windows-winui`/
  `browser-wx`: clean.
- `cargo build` (workspace), `cargo build --target x86_64-pc-windows-gnu --workspace --exclude browser-wx`,
  `cargo build-windows-winui`: all three succeed, confirming no cross-compile impact this time.
- Full headless GTK suite via `wlheadless-run -c cage -- xwayland-run -- cargo test -p browser-linux-gtk3`:
  all passing.

## Scope notes / honest limitations

- This is lexical (shared-vocabulary), not semantic, similarity — it will not connect "espresso machine
  reviews" with "best coffee makers" the way a real embedding model would, only things that share actual
  words.
- 64 dimensions is a judgment call, not tuned against real usage — collisions between unrelated common words
  are possible at this scale, more likely with a large, vocabulary-diverse history.
- The `distance < 0.9` cutoff is likewise a reasonable-looking default, not empirically tuned against a real
  browsing history — worth revisiting once there's real usage data to look at.
- Only history titles are embedded — bookmarks and open-page titles aren't part of `search_similar` at all
  (their own substring-search paths are untouched).
