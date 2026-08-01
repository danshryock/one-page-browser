//! Per-profile password vault: a small, always-encrypted libsql database
//! (see `PasswordStore::open_encrypted`) surfaced through a password manager
//! overlay in each native chrome, plus a `PasswordBackend` trait so the
//! embedded vault can later be swapped for an external/alternate password
//! manager without touching any call site that's generic over it — the same
//! role `HistoryBackend` (`history.rs`) already plays for history. See
//! `bitwarden.rs` for the first such alternate backend.
//!
//! Unlike every other store in this app, there is no plain, unencrypted
//! `open()` here — a password vault with no encryption at all would be a
//! real security footgun, not a reasonable "simple" default, so encryption
//! is mandatory rather than opt-in. See `Profile::has_vault_passphrase`/
//! `enable_vault_passphrase` for the independent (from history's) marker
//! that tracks whether a given profile's vault has been set up yet, and
//! `decide_vault_unlock_action` for how a profile that already has a
//! passphrase (for history, the vault, or both) reuses one passphrase value
//! across whichever of the two are actually turned on.

use std::cell::{Cell, RefCell};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{domain_of, Profile};

/// Abstracts over what a "password backend" actually needs to do,
/// independent of how it's backed — the extension point for an external/
/// alternate password manager (`bitwarden.rs`'s `BitwardenBackend`, a
/// KeePassXC/secret-service integration, etc.) to become a drop-in
/// replacement for the embedded `PasswordStore` wherever a call site is
/// generic over `impl PasswordBackend`, the same way `HistoryBackend`
/// already works for history. `PasswordStore`'s own inherent methods have
/// this exact signature shape, so this trait doesn't change any existing
/// direct call site's behavior — it only matters for code that wants to be
/// generic over which backend it's talking to.
///
/// `add`/`update` take one `LoginFields` rather than a growing list of
/// positional arguments — `BitwardenBackend` builds one of these from JSON,
/// not from typed-in form fields, so a named-field struct fits both
/// backends better than positional strings would.
pub trait PasswordBackend {
    fn list(&self) -> anyhow::Result<Vec<Login>>;
    fn search(&self, query: &str) -> anyhow::Result<Vec<Login>>;
    fn add(&self, fields: LoginFields) -> anyhow::Result<Login>;
    fn update(&self, id: &str, fields: LoginFields) -> anyhow::Result<()>;
    fn delete(&self, id: &str) -> anyhow::Result<()>;
}

#[cfg(target_os = "linux")]
const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS passwords (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    site TEXT NOT NULL,
    domain TEXT NOT NULL,
    username TEXT NOT NULL DEFAULT '',
    password TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
";

/// A WebAuthn/passkey credential, stored alongside (or instead of) a
/// password under one `Login` — the forward-compatibility piece for
/// eventual passkey creation/assertion support, which needs a WebAuthn
/// virtual authenticator hooked into each render engine (not yet built —
/// see `ROADMAP.md`). Nothing populates this yet; the schema/API just
/// doesn't need another migration once that lands. Field shapes follow the
/// WebAuthn spec's own credential model directly (relying party id,
/// credential id, user handle, COSE public key, signature counter) rather
/// than inventing a different one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasskeyCredential {
    /// Base64url-encoded credential id.
    pub credential_id: String,
    /// The relying party id (e.g. `"example.com"`) this credential is
    /// scoped to — not necessarily identical to `Login::domain`, since a
    /// relying party can register a passkey for a parent domain.
    pub rp_id: String,
    /// Base64url-encoded user handle (the WebAuthn `userHandle`).
    pub user_handle: String,
    /// Base64url-encoded COSE public key.
    pub public_key: String,
    /// The private key material — stored the same way `password` is:
    /// plaintext at this layer, protected by the vault database's own
    /// encryption (see this module's doc comment), not double-encrypted.
    pub private_key: String,
    /// The WebAuthn signature counter. Software/synced passkeys (the kind a
    /// password manager stores, as opposed to a hardware security key's own
    /// non-exportable one) commonly report a static `0` here, since a real
    /// incrementing counter can't be kept consistent across sync — kept as
    /// a real field anyway so a future authenticator implementation can
    /// choose either behavior.
    pub sign_count: i64,
}

/// One saved login — a password, a passkey, or both, for a given site.
/// "Login" rather than "password entry" now that it can hold either kind of
/// credential (or both) under one record.
#[derive(Debug, Clone, PartialEq)]
pub struct Login {
    pub id: String,
    pub site: String,
    pub domain: String,
    pub username: String,
    pub password: Option<String>,
    pub passkey: Option<PasskeyCredential>,
    pub notes: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// The writable fields of a `Login` — what `PasswordBackend::add`/`update`
/// take, instead of a positional-argument list (see the trait's doc
/// comment).
#[derive(Debug, Clone, Default)]
pub struct LoginFields {
    pub site: String,
    pub username: String,
    pub password: Option<String>,
    pub passkey: Option<PasskeyCredential>,
    pub notes: String,
}

impl LoginFields {
    /// Convenience constructor for the common case (a plain password, no
    /// passkey) — keeps simple call sites (the add-credential form, most
    /// tests) close to the shape this API had before passkey support.
    pub fn password_only(site: impl Into<String>, username: impl Into<String>, password: impl Into<String>, notes: impl Into<String>) -> Self {
        Self { site: site.into(), username: username.into(), password: Some(password.into()), passkey: None, notes: notes.into() }
    }
}

pub struct PasswordStore {
    /// Kept alive alongside `conn` even though nothing reads it directly —
    /// same reasoning as `HistoryStore`'s identically-named field.
    _db: libsql::Database,
    conn: libsql::Connection,
}

impl PasswordStore {
    /// Opens (creating if necessary) `profile`'s password vault, encrypted
    /// with `passphrase` — there is no unencrypted counterpart (see this
    /// module's doc comment for why). Uses libsql's native encryption
    /// support exactly the way `HistoryStore::open_encrypted` does (see its
    /// doc comment for the full explanation of the underlying cipher and
    /// why a wrong passphrase only ever surfaces as a generic error from the
    /// first real read, not immediately on open).
    ///
    /// Only actually implemented on Linux, same constraint and same reason
    /// as `HistoryStore::open_encrypted` (see `Cargo.toml`'s
    /// `[target.'cfg(target_os = "linux")']` scoping of libsql's
    /// `encryption` feature) — every other target gets a plain error
    /// instead of a silent, unencrypted fallback.
    #[cfg(target_os = "linux")]
    pub fn open_encrypted(profile: &Profile, passphrase: &str) -> anyhow::Result<Self> {
        let path = profile
            .passwords_db_path()
            .ok_or_else(|| anyhow::anyhow!("no data directory available on this platform"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let builder = libsql::Builder::new_local(&path).encryption_config(libsql::EncryptionConfig::new(
            libsql::Cipher::Aes256Cbc,
            bytes::Bytes::copy_from_slice(passphrase.as_bytes()),
        ));
        let db = futures_executor::block_on(builder.build())?;
        let conn = db.connect()?;
        futures_executor::block_on(conn.execute_batch(SCHEMA_SQL))?;
        migrate_add_passkey_column(&conn);
        Ok(Self { _db: db, conn })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn open_encrypted(_profile: &Profile, _passphrase: &str) -> anyhow::Result<Self> {
        anyhow::bail!("the password vault isn't available on this platform yet")
    }

    /// All entries, most-recently-added first. Orders by `id DESC` as well
    /// as `created_at DESC` — `created_at` has one-second resolution, so two
    /// adds within the same second would otherwise tie and fall back to
    /// SQLite's unspecified order for equal keys; `id` (autoincrement, so
    /// always insertion-ordered) breaks that tie correctly.
    pub fn list(&self) -> anyhow::Result<Vec<Login>> {
        let mut rows = futures_executor::block_on(self.conn.query(
            "SELECT id, site, domain, username, password, notes, created_at, updated_at, passkey \
             FROM passwords ORDER BY created_at DESC, id DESC",
            (),
        ))?;
        collect_entries(&mut rows)
    }

    /// Entries whose `site`, `domain`, or `username` contains `query`
    /// (case-insensitive), most-recently-added first (see `list`'s doc
    /// comment for the `id DESC` tiebreaker).
    pub fn search(&self, query: &str) -> anyhow::Result<Vec<Login>> {
        let pattern = format!("%{query}%");
        let mut rows = futures_executor::block_on(self.conn.query(
            "SELECT id, site, domain, username, password, notes, created_at, updated_at, passkey \
             FROM passwords \
             WHERE site LIKE ?1 OR domain LIKE ?1 OR username LIKE ?1 \
             ORDER BY created_at DESC, id DESC",
            libsql::params![pattern],
        ))?;
        collect_entries(&mut rows)
    }

    /// Adds a new login, computing `domain` from `site` the same way
    /// `Bookmarks::add`/`HistoryStore::record_visit` do. Always creates a
    /// new row — unlike `Bookmarks::add`, a repeat `site` is not upserted,
    /// since it's entirely normal to have more than one account on the same
    /// site.
    pub fn add(&self, fields: LoginFields) -> anyhow::Result<Login> {
        let domain = domain_of(&fields.site);
        let now = now_unix();
        let password_sql = fields.password.as_deref().unwrap_or("");
        let passkey_sql = passkey_to_sql(&fields.passkey)?;
        futures_executor::block_on(self.conn.execute(
            "INSERT INTO passwords (site, domain, username, password, notes, created_at, updated_at, passkey) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
            libsql::params![fields.site.clone(), domain.clone(), fields.username.clone(), password_sql, fields.notes.clone(), now, passkey_sql],
        ))?;
        let id = self.conn.last_insert_rowid();
        Ok(Login {
            id: id.to_string(),
            site: fields.site,
            domain,
            username: fields.username,
            password: fields.password,
            passkey: fields.passkey,
            notes: fields.notes,
            created_at: now,
            updated_at: now,
        })
    }

    /// Updates every field of the login identified by `id` (re-deriving
    /// `domain` from `site`, same as `add`) and bumps `updated_at`. Errors
    /// if `id` isn't a valid row id — which should never happen in
    /// practice, since every `id` a caller has came from this store's own
    /// `list`/`search`/`add`.
    pub fn update(&self, id: &str, fields: LoginFields) -> anyhow::Result<()> {
        let row_id: i64 = id.parse().map_err(|_| anyhow::anyhow!("invalid login id: {id}"))?;
        let domain = domain_of(&fields.site);
        let now = now_unix();
        let password_sql = fields.password.as_deref().unwrap_or("");
        let passkey_sql = passkey_to_sql(&fields.passkey)?;
        futures_executor::block_on(self.conn.execute(
            "UPDATE passwords SET site = ?1, domain = ?2, username = ?3, password = ?4, notes = ?5, updated_at = ?6, passkey = ?7 \
             WHERE id = ?8",
            libsql::params![fields.site, domain, fields.username, password_sql, fields.notes, now, passkey_sql, row_id],
        ))?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let row_id: i64 = id.parse().map_err(|_| anyhow::anyhow!("invalid login id: {id}"))?;
        futures_executor::block_on(self.conn.execute("DELETE FROM passwords WHERE id = ?1", libsql::params![row_id]))?;
        Ok(())
    }
}

impl PasswordBackend for PasswordStore {
    fn list(&self) -> anyhow::Result<Vec<Login>> {
        self.list()
    }

    fn search(&self, query: &str) -> anyhow::Result<Vec<Login>> {
        self.search(query)
    }

    fn add(&self, fields: LoginFields) -> anyhow::Result<Login> {
        self.add(fields)
    }

    fn update(&self, id: &str, fields: LoginFields) -> anyhow::Result<()> {
        self.update(id, fields)
    }

    fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.delete(id)
    }
}

/// A `PasswordBackend` with no `libsql`/SQL involved at all — entries live
/// in a plain `Vec` behind a `RefCell`, ids are a simple in-process counter.
/// Exists so tests (and any future non-Linux/no-libsql smoke test, same
/// motivation as `MemoryHistoryStore`) can exercise the exact same
/// `list`/`search`/`add`/`update`/`delete` call shape the real vault makes,
/// without any real encryption or I/O.
#[derive(Default)]
pub struct MemoryPasswordStore {
    entries: RefCell<Vec<Login>>,
    next_id: Cell<u64>,
}

impl MemoryPasswordStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PasswordBackend for MemoryPasswordStore {
    fn list(&self) -> anyhow::Result<Vec<Login>> {
        let mut entries: Vec<Login> = self.entries.borrow().clone();
        entries.sort_by_key(|e| std::cmp::Reverse((e.created_at, e.id.parse::<u64>().unwrap_or(0))));
        Ok(entries)
    }

    fn search(&self, query: &str) -> anyhow::Result<Vec<Login>> {
        let query = query.to_lowercase();
        let mut entries: Vec<Login> = self
            .entries
            .borrow()
            .iter()
            .filter(|e| {
                e.site.to_lowercase().contains(&query)
                    || e.domain.to_lowercase().contains(&query)
                    || e.username.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse((e.created_at, e.id.parse::<u64>().unwrap_or(0))));
        Ok(entries)
    }

    fn add(&self, fields: LoginFields) -> anyhow::Result<Login> {
        let now = now_unix();
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let entry = Login {
            id: id.to_string(),
            site: fields.site.clone(),
            domain: domain_of(&fields.site),
            username: fields.username,
            password: fields.password,
            passkey: fields.passkey,
            notes: fields.notes,
            created_at: now,
            updated_at: now,
        };
        self.entries.borrow_mut().push(entry.clone());
        Ok(entry)
    }

    fn update(&self, id: &str, fields: LoginFields) -> anyhow::Result<()> {
        let mut entries = self.entries.borrow_mut();
        let entry = entries.iter_mut().find(|e| e.id == id).ok_or_else(|| anyhow::anyhow!("no such entry: {id}"))?;
        entry.domain = domain_of(&fields.site);
        entry.site = fields.site;
        entry.username = fields.username;
        entry.password = fields.password;
        entry.passkey = fields.passkey;
        entry.notes = fields.notes;
        entry.updated_at = now_unix();
        Ok(())
    }

    fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.entries.borrow_mut().retain(|e| e.id != id);
        Ok(())
    }
}

/// What should happen when a profile's password vault is about to be
/// opened, given whether it has its own passphrase set up yet
/// (`Profile::has_vault_passphrase`) and whether a passphrase is already
/// known this session (populated the moment *either* the history store or
/// the vault gets unlocked with a real passphrase — see
/// `browser-linux-gtk3`'s `AppState::session_passphrase`). This is what
/// implements "the same passphrase unlocks both, when both are on": if
/// history was already unlocked at startup, opening the vault for the first
/// time silently reuses that same passphrase, no second prompt at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultUnlockAction {
    /// No vault passphrase yet, and none known this session either — prompt
    /// to create a brand new one.
    PromptToSetUp,
    /// No vault passphrase yet, but one is already known this session
    /// (from unlocking history) — silently establish the vault under it,
    /// no prompt.
    SilentlySetUpWith(String),
    /// The vault already has a passphrase, and it's already known this
    /// session — silently unlock, no prompt.
    SilentlyUnlockWith(String),
    /// The vault already has a passphrase, but it isn't known yet this
    /// session — prompt to unlock.
    PromptToUnlock,
}

pub fn decide_vault_unlock_action(profile: &Profile, session_passphrase: Option<&str>) -> VaultUnlockAction {
    match (profile.has_vault_passphrase(), session_passphrase) {
        (false, None) => VaultUnlockAction::PromptToSetUp,
        (false, Some(p)) => VaultUnlockAction::SilentlySetUpWith(p.to_string()),
        (true, Some(p)) => VaultUnlockAction::SilentlyUnlockWith(p.to_string()),
        (true, None) => VaultUnlockAction::PromptToUnlock,
    }
}

/// Shared by `list`/`search`: both select the same nine columns in the same
/// order.
fn collect_entries(rows: &mut libsql::Rows) -> anyhow::Result<Vec<Login>> {
    let mut entries = Vec::new();
    while let Some(row) = futures_executor::block_on(rows.next())? {
        let id: i64 = row.get(0)?;
        let password: String = row.get(4)?;
        let passkey_json: Option<String> = row.get(8)?;
        entries.push(Login {
            id: id.to_string(),
            site: row.get(1)?,
            domain: row.get(2)?,
            username: row.get(3)?,
            password: if password.is_empty() { None } else { Some(password) },
            notes: row.get(5)?,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
            passkey: passkey_from_sql(passkey_json)?,
        });
    }
    Ok(entries)
}

fn passkey_to_sql(passkey: &Option<PasskeyCredential>) -> anyhow::Result<Option<String>> {
    passkey.as_ref().map(serde_json::to_string).transpose().map_err(Into::into)
}

fn passkey_from_sql(json: Option<String>) -> anyhow::Result<Option<PasskeyCredential>> {
    json.map(|s| serde_json::from_str(&s)).transpose().map_err(Into::into)
}

/// Best-effort migration for a database created before the `passkey` column
/// existed — `CREATE TABLE IF NOT EXISTS` silently no-ops against an
/// existing table with an older shape, so a store opened before this
/// feature landed would otherwise be missing the column entirely. Ignores
/// the error `ALTER TABLE` raises when the column already exists (the
/// normal case, once every vault has been opened at least once after this
/// migration was added) — same pattern as `history.rs`'s
/// `migrate_add_embedding_column`.
#[cfg(target_os = "linux")]
fn migrate_add_passkey_column(conn: &libsql::Connection) {
    let _ = futures_executor::block_on(conn.execute("ALTER TABLE passwords ADD COLUMN passkey TEXT", ()));
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn temp_profile(name: &str) -> Profile {
        Profile::new(format!("passwords-crypto-test-{name}-{}", std::process::id()))
    }

    fn cleanup_profile(profile: &Profile) {
        if let Some(parent) = profile.passwords_db_path().as_deref().and_then(Path::parent) {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    fn sample_passkey() -> PasskeyCredential {
        PasskeyCredential {
            credential_id: "Y3JlZC0x".to_string(),
            rp_id: "example.com".to_string(),
            user_handle: "dXNlci0x".to_string(),
            public_key: "cHVibGljLWtleQ".to_string(),
            private_key: "cHJpdmF0ZS1rZXk".to_string(),
            sign_count: 0,
        }
    }

    mod memory_store {
        use super::*;

        #[test]
        fn add_makes_an_entry_findable_by_list_and_search() {
            let store = MemoryPasswordStore::new();
            store.add(LoginFields::password_only("https://example.com", "alice", "hunter2", "")).unwrap();

            let all = store.list().unwrap();
            assert_eq!(all.len(), 1);
            assert_eq!(all[0].domain, "example.com");
            assert_eq!(all[0].username, "alice");
            assert_eq!(all[0].password.as_deref(), Some("hunter2"));

            let found = store.search("EXAMPLE").unwrap();
            assert_eq!(found.len(), 1, "search should be case-insensitive");
        }

        #[test]
        fn adding_the_same_site_twice_creates_two_entries() {
            let store = MemoryPasswordStore::new();
            store.add(LoginFields::password_only("https://example.com", "alice", "pw1", "")).unwrap();
            store.add(LoginFields::password_only("https://example.com", "bob", "pw2", "")).unwrap();
            assert_eq!(store.list().unwrap().len(), 2, "multiple accounts on the same site should both be kept");
        }

        #[test]
        fn update_changes_fields_in_place() {
            let store = MemoryPasswordStore::new();
            let entry = store.add(LoginFields::password_only("https://example.com", "alice", "old-pw", "")).unwrap();
            store
                .update(&entry.id, LoginFields::password_only("https://example.com", "alice", "new-pw", "updated notes"))
                .unwrap();

            let all = store.list().unwrap();
            assert_eq!(all.len(), 1, "update shouldn't create a second entry");
            assert_eq!(all[0].password.as_deref(), Some("new-pw"));
            assert_eq!(all[0].notes, "updated notes");
        }

        #[test]
        fn delete_removes_the_entry() {
            let store = MemoryPasswordStore::new();
            let entry = store.add(LoginFields::password_only("https://example.com", "alice", "pw", "")).unwrap();
            store.delete(&entry.id).unwrap();
            assert!(store.list().unwrap().is_empty());
        }

        #[test]
        fn list_breaks_a_created_at_tie_by_insertion_order() {
            // Two entries added within the same second (a real, common case
            // — now_unix() only has one-second resolution) should still
            // list most-recently-added first, using id as the tiebreaker.
            let store = MemoryPasswordStore::new();
            store.add(LoginFields::password_only("https://a.example", "a", "pw", "")).unwrap();
            store.add(LoginFields::password_only("https://b.example", "b", "pw", "")).unwrap();

            let all = store.list().unwrap();
            assert_eq!(
                all.iter().map(|e| e.username.as_str()).collect::<Vec<_>>(),
                vec!["b", "a"],
                "b was added after a, so it should list first even if both share the same created_at second"
            );
        }

        #[test]
        fn a_login_can_hold_a_password_a_passkey_or_both() {
            let store = MemoryPasswordStore::new();

            let password_only = store.add(LoginFields::password_only("https://a.example", "a", "pw", "")).unwrap();
            assert!(password_only.password.is_some());
            assert!(password_only.passkey.is_none());

            let passkey_only = store
                .add(LoginFields { site: "https://b.example".to_string(), username: "b".to_string(), password: None, passkey: Some(sample_passkey()), notes: String::new() })
                .unwrap();
            assert!(passkey_only.password.is_none());
            assert_eq!(passkey_only.passkey, Some(sample_passkey()));

            let both = store
                .add(LoginFields {
                    site: "https://c.example".to_string(),
                    username: "c".to_string(),
                    password: Some("pw".to_string()),
                    passkey: Some(sample_passkey()),
                    notes: String::new(),
                })
                .unwrap();
            assert!(both.password.is_some());
            assert!(both.passkey.is_some());
        }

        #[test]
        fn is_usable_as_a_trait_object() {
            let store: Box<dyn PasswordBackend> = Box::new(MemoryPasswordStore::new());
            store.add(LoginFields::password_only("https://example.com", "alice", "pw", "")).unwrap();
            assert_eq!(store.list().unwrap().len(), 1);
        }
    }

    mod decide_unlock {
        use super::*;

        #[test]
        fn no_vault_passphrase_and_none_known_prompts_to_set_up() {
            let profile = temp_profile("decide-prompt-setup");
            cleanup_profile(&profile);
            assert_eq!(decide_vault_unlock_action(&profile, None), VaultUnlockAction::PromptToSetUp);
            cleanup_profile(&profile);
        }

        #[test]
        fn no_vault_passphrase_but_one_known_silently_sets_up() {
            let profile = temp_profile("decide-silent-setup");
            cleanup_profile(&profile);
            assert_eq!(
                decide_vault_unlock_action(&profile, Some("known-passphrase")),
                VaultUnlockAction::SilentlySetUpWith("known-passphrase".to_string())
            );
            cleanup_profile(&profile);
        }

        #[test]
        fn existing_vault_passphrase_known_this_session_silently_unlocks() {
            let profile = temp_profile("decide-silent-unlock");
            cleanup_profile(&profile);
            profile.enable_vault_passphrase().unwrap();

            assert_eq!(
                decide_vault_unlock_action(&profile, Some("known-passphrase")),
                VaultUnlockAction::SilentlyUnlockWith("known-passphrase".to_string())
            );
            cleanup_profile(&profile);
        }

        #[test]
        fn existing_vault_passphrase_not_yet_known_prompts_to_unlock() {
            let profile = temp_profile("decide-prompt-unlock");
            cleanup_profile(&profile);
            profile.enable_vault_passphrase().unwrap();

            assert_eq!(decide_vault_unlock_action(&profile, None), VaultUnlockAction::PromptToUnlock);
            cleanup_profile(&profile);
        }
    }

    // These exercise the *real* `open_encrypted` implementation, which only
    // exists on Linux (see its `#[cfg]` and doc comment) — gated so they
    // don't run (and fail on the stub's `Err`) on a genuine non-Linux CI
    // runner, same reasoning as `history.rs`'s equivalent tests.
    #[cfg(target_os = "linux")]
    #[test]
    fn encrypted_store_round_trips_with_the_right_passphrase() {
        let profile = temp_profile("round-trip");
        cleanup_profile(&profile);

        {
            let store = PasswordStore::open_encrypted(&profile, "correct horse battery staple")
                .expect("opening a fresh encrypted vault should succeed");
            store.add(LoginFields::password_only("https://example.com", "alice", "hunter2", "")).unwrap();
        }

        let reopened = PasswordStore::open_encrypted(&profile, "correct horse battery staple")
            .expect("reopening with the same passphrase should succeed");
        let entries = reopened.list().unwrap();
        assert_eq!(entries.len(), 1, "the added credential should still be there after reopening");
        assert_eq!(entries[0].username, "alice");

        cleanup_profile(&profile);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_login_with_a_passkey_round_trips_through_the_real_store() {
        let profile = temp_profile("passkey-round-trip");
        cleanup_profile(&profile);

        let store = PasswordStore::open_encrypted(&profile, "passphrase").expect("opening a fresh encrypted vault should succeed");
        store
            .add(LoginFields {
                site: "https://example.com".to_string(),
                username: "alice".to_string(),
                password: None,
                passkey: Some(sample_passkey()),
                notes: String::new(),
            })
            .unwrap();

        let entries = store.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].password.is_none(), "a passkey-only login shouldn't report a password");
        assert_eq!(entries[0].passkey, Some(sample_passkey()));

        cleanup_profile(&profile);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn opening_a_database_created_before_the_passkey_column_existed_still_works() {
        let profile = temp_profile("pre-passkey-migration");
        cleanup_profile(&profile);

        // Simulates a vault created by an older build: same encryption
        // setup, but the schema/insert predate the `passkey` column
        // entirely (no ALTER TABLE has ever run against this file).
        {
            let path = profile.passwords_db_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let builder = libsql::Builder::new_local(&path).encryption_config(libsql::EncryptionConfig::new(
                libsql::Cipher::Aes256Cbc,
                bytes::Bytes::copy_from_slice(b"passphrase"),
            ));
            let db = futures_executor::block_on(builder.build()).unwrap();
            let conn = db.connect().unwrap();
            futures_executor::block_on(conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS passwords (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    site TEXT NOT NULL,
                    domain TEXT NOT NULL,
                    username TEXT NOT NULL DEFAULT '',
                    password TEXT NOT NULL DEFAULT '',
                    notes TEXT NOT NULL DEFAULT '',
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );",
            ))
            .unwrap();
            futures_executor::block_on(conn.execute(
                "INSERT INTO passwords (site, domain, username, password, notes, created_at, updated_at) \
                 VALUES ('https://example.com', 'example.com', 'alice', 'hunter2', '', 100, 100)",
                (),
            ))
            .unwrap();
        }

        // Reopening through the real (migrating) open_encrypted should both
        // succeed and be able to add a passkey-bearing login afterward.
        let store = PasswordStore::open_encrypted(&profile, "passphrase").expect("reopening a pre-migration vault should succeed");
        let existing = store.list().unwrap();
        assert_eq!(existing.len(), 1, "the pre-existing row should still be readable");
        assert_eq!(existing[0].password.as_deref(), Some("hunter2"));
        assert!(existing[0].passkey.is_none());

        store
            .add(LoginFields { site: "https://b.example".to_string(), username: "b".to_string(), password: None, passkey: Some(sample_passkey()), notes: String::new() })
            .expect("adding a passkey-bearing login after migration should succeed");
        assert_eq!(store.list().unwrap().len(), 2);

        cleanup_profile(&profile);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn list_breaks_a_created_at_tie_by_insertion_order_against_the_real_store() {
        let profile = temp_profile("tie-breaking");
        cleanup_profile(&profile);

        let store = PasswordStore::open_encrypted(&profile, "passphrase").expect("opening a fresh encrypted vault should succeed");
        // Both adds land in the same wall-clock second in practice — the
        // real bug this guards against (confirmed via a failing test
        // before this ORDER BY fix landed): `list()` fell back to SQLite's
        // unspecified order for a `created_at` tie instead of always
        // putting the more-recently-added row first.
        store.add(LoginFields::password_only("https://a.example", "a", "pw", "")).unwrap();
        store.add(LoginFields::password_only("https://b.example", "b", "pw", "")).unwrap();

        let entries = store.list().unwrap();
        assert_eq!(
            entries.iter().map(|e| e.username.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"],
            "b was added after a, so it should list first even when both share the same created_at second"
        );

        cleanup_profile(&profile);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn encrypted_store_rejects_the_wrong_passphrase() {
        let profile = temp_profile("wrong-passphrase");
        cleanup_profile(&profile);

        PasswordStore::open_encrypted(&profile, "the right one").expect("opening a fresh encrypted vault should succeed");

        let result = PasswordStore::open_encrypted(&profile, "definitely not it");
        assert!(result.is_err(), "opening the vault with the wrong passphrase should fail, not silently succeed");

        cleanup_profile(&profile);
    }

    /// The non-Linux counterpart: confirms the stub `open_encrypted` (see
    /// its `#[cfg(not(target_os = "linux"))]` impl) reports an error rather
    /// than silently succeeding — there is no unencrypted fallback to worry
    /// about here (unlike history's equivalent test), since the vault has
    /// no unencrypted mode at all.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn open_encrypted_returns_an_error_rather_than_silently_succeeding_on_this_platform() {
        let profile = temp_profile("stub-platform");
        cleanup_profile(&profile);

        let result = PasswordStore::open_encrypted(&profile, "some passphrase");
        assert!(result.is_err(), "open_encrypted should report an error on this platform, not silently succeed");

        cleanup_profile(&profile);
    }
}
