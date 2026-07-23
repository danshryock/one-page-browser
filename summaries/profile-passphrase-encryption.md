# Passphrase support for profiles (libsql native encryption)

**Roadmap item:** "Add passphrase support to profiles, use native encryption features of libsql if available."

## What's encrypted

Only the **history database** — the one piece of profile data that's already SQL-backed, and so the one
thing libsql's own native encryption actually applies to. `Settings`/`Keybindings`/`Bookmarks` are plain JSON
files with no existing encryption mechanism; encrypting those too would be a materially different feature
(file-level encryption, a different design entirely) beyond "use native encryption features of libsql."
Flagging the scope explicitly rather than silently narrowing it.

## How libsql's encryption actually works (confirmed, not assumed)

`libsql-sys` (a real, non-stub capability, gated behind its own `encryption` Cargo feature) bundles SQLite3
Multiple Ciphers and exposes `EncryptionConfig { cipher: Cipher::Aes256Cbc, encryption_key: Bytes }`. The "key"
is the **raw passphrase bytes** handed straight to `sqlite3_key()` — the same SQLCipher-style convention where
the cipher extension does its own key derivation internally; this code never touches key derivation itself.
The *first* successful open of a brand-new, empty database file with a passphrase is what *establishes* its
encryption — there's no separate "set up encryption" step, opening *is* setting up. Opening an *existing*
encrypted database with the **wrong** passphrase doesn't fail immediately either — the cipher extension only
discovers the key is wrong once it actually tries to decrypt a page, which for this code means the schema
`execute_batch` call right after opening. Both of these behaviors were verified with real tests (see below),
not taken on faith from the API shape.

## A real cross-compile break found and fixed

Enabling libsql's `encryption` feature unconditionally broke `cargo build-windows-winui`: it builds the
bundled cipher extension via CMake, which needs `llvm-lib` when cross-compiling for the MSVC target through
this environment's `xwin`/clang-cl toolchain — not installed here, confirmed by actually hitting the build
failure. Fixed by scoping the `encryption` Cargo feature to `target_os = "linux"` only (in
`crates/browser-core/Cargo.toml`, via a target-specific `[dependencies]` table that unions with the base
`libsql` dependency), and correspondingly `#[cfg(target_os = "linux")]`-gating the real
`HistoryStore::open_encrypted` implementation in `history.rs`, with a `#[cfg(not(target_os = "linux"))]`
fallback that returns a plain error. Deliberately **not** a silent fallback to an unencrypted open — that
would be a silent security downgrade for anyone who thinks they got encryption. This also happens to match the
roadmap item's own "if available" wording.

## Passphrases never cross a process boundary via argv

Since switching profiles always launches a **new process** (established earlier this session), and process
arguments are visible to any other user on the system (`ps`, `/proc/<pid>/cmdline`), a passphrase can never be
collected in one process and handed to another via a CLI arg. Both "set up a new passphrase" and "unlock an
existing one" are instead prompted for *inside* the process that will actually use the passphrase, via a new
small standalone window (`show_passphrase_prompt`, modeled on the existing `show_external_link_chooser`
pattern). What differs between the launching and launched process is only a **flag** (`--setup-passphrase`,
via a new `resolve_passphrase_setup_requested`) or a filesystem check (`Profile::has_passphrase()`), never a
secret.

## What changed

- **`crates/browser-core`**:
  - `Cargo.toml`: `libsql`'s `encryption` feature (+ `bytes` for `EncryptionConfig`) moved to a
    Linux-only target dependency (see above).
  - `history.rs`: `HistoryStore::open_encrypted(profile, passphrase)` (Linux-only real implementation +
    non-Linux stub, as described above). `open_at` reverted to its original single-argument shape — its
    encryption branch got pulled out into `open_encrypted` directly instead of threading an
    `Option<&[u8]>` through it, since burying the security-relevant branch inside a generic helper felt
    like the wrong place for it.
  - `profile.rs`: new `Profile::passphrase_marker_path()` (an empty sentinel file — never the passphrase or
    anything derived from it — whose existence means "prompt before opening"), `has_passphrase()` (always
    `false` for an `ephemeral` profile), `enable_passphrase()` (creates the marker, called once right after
    successfully establishing encryption). New `resolve_passphrase_setup_requested` (mirrors
    `resolve_ephemeral_requested`) and `launch_new_encrypted_profile_process` (mirrors
    `launch_new_profile_process`, adds `--setup-passphrase`).
- **`crates/browser-linux-gtk3`**:
  - `build_window_and_app` split into a thin wrapper (unchanged signature/behavior, all 19 existing call
    sites untouched) over a new `build_window_and_app_with_history(profile, history)`, which takes an
    already-opened `HistoryStore` instead of opening one itself — needed since the passphrase flow has to
    open (and verify) the encrypted store *before* the rest of the window gets built.
  - New `show_passphrase_prompt(profile, setup: bool)`: a standalone window collecting a passphrase, either
    to set up new encryption or unlock an existing store (retrying in place on a wrong passphrase rather than
    closing), then building the real browser window on success.
  - `main.rs`: routes to `show_passphrase_prompt` (setup or unlock mode) instead of `build_window_and_app`
    directly, whenever `--setup-passphrase` was passed or `profile.has_passphrase()`.
  - Profile picker gained an "Encrypt with a passphrase" checkbox next to "Create & Open", launching via
    `launch_new_encrypted_profile_process` instead of the plain one when checked.

## Testing

- `browser-core`: three real, security-meaningful tests in `history.rs` —
  `encrypted_store_round_trips_with_the_right_passphrase`,
  `encrypted_store_rejects_the_wrong_passphrase` (confirms a wrong passphrase genuinely fails, not silently
  succeeds), `a_plain_unencrypted_open_cannot_read_an_encrypted_store` (confirms `open` can't read what
  `open_encrypted` wrote) — plus `Profile`-level tests for the marker file
  (`has_passphrase_reflects_the_marker_file`, `ephemeral_profiles_never_report_having_a_passphrase`) and the
  new CLI flag (`resolve_passphrase_setup_requested_recognizes_the_flag`). `cargo test -p browser-core`:
  68/68 passing.
- `crates/browser-linux-gtk3/tests/gtk_tests.rs`: new `encrypted_profile_records_visits_through_the_real_app`
  — builds a real `AppState` from an encrypted `HistoryStore` via `build_window_and_app_with_history`, adds a
  page, and confirms the visit is readable back from a **separate** connection opened with the same
  passphrase — proving the running app actually writes through to the real encrypted store end-to-end, not
  just that `open_encrypted` works in isolation.
- `cargo clippy --all-targets` on `browser-linux-gtk3`/`browser-core`/`browser-windows-winui`/`browser-wx`/
  `render-engine`: clean.
- `cargo build` (workspace), `cargo build --target x86_64-pc-windows-gnu --workspace --exclude browser-wx`,
  `cargo build-windows-winui`: all three succeed — the winui3/msvc build specifically failed before the
  Linux-only feature scoping fix above, and was verified to succeed again after it.
- Full headless GTK suite via `wlheadless-run -c cage -- xwayland-run -- cargo test -p browser-linux-gtk3`:
  all passing.

## Scope notes / what's not done

- No UI for *changing* or *removing* a passphrase, or for migrating an existing unencrypted profile to
  encrypted — a passphrase can currently only be set at profile-creation time, on a brand-new (empty) history
  database. Re-keying (`sqlite3_rekey`, which `libsql-sys` also exposes) or a wipe-and-restart migration are
  both real options for a follow-up.
- No "forgot passphrase" recovery — there isn't one, by design (that's what encryption means), but it's worth
  being explicit that a lost passphrase means a lost history database for that profile.
- `browser-windows-winui`/win32/nwg/`browser-wx`: only `HistoryStore::open_encrypted`'s non-Linux stub landed
  there (needed to keep compiling); no UI, no `--setup-passphrase` handling in those frontends' own
  `main.rs`es.
