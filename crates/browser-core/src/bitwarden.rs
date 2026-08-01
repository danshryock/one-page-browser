//! `BitwardenBackend`: a `PasswordBackend` (see `passwords.rs`) that talks to
//! a locally running `bw serve` — the official Bitwarden CLI's local,
//! loopback-only REST API (`bw serve`, default `http://127.0.0.1:8087`).
//! Vaultwarden is wire-compatible with real Bitwarden, so this same backend
//! covers both; there's no separate Vaultwarden-specific code path.
//!
//! Deliberately talks to `bw serve`'s HTTP API rather than reimplementing
//! Bitwarden's own sync/crypto protocol (master-password stretching,
//! wrapped item keys, etc.) against a Vaultwarden/bitwarden.com server
//! directly — that would duplicate real cryptography the official CLI
//! already implements correctly, for no benefit here.
//!
//! **Honest caveat**: there's no `bw serve` instance reachable from this
//! dev environment, and Bitwarden's own docs site didn't yield the full
//! item JSON schema on request — the request/response shapes below are this
//! module's best-effort understanding of the (fairly stable, well-known)
//! `bw serve` envelope convention (`{"success": bool, "data": ..., "message":
//! ...}` on every response — see Bitwarden's "Bringing a RESTful API to the
//! Bitwarden CLI" blog post) and Bitwarden's item-type numbering (`1` =
//! Login), not something verified against a live server. In particular,
//! whether `bw`'s login-item JSON exposes FIDO2/passkey credential data at
//! all is unverified — `list`/`search` here always report `passkey: None`
//! rather than guess at an unconfirmed field shape. Revisit against a real
//! `bw serve` instance before relying on this in production.
//!
//! Read-only from `browser-linux-gtk3`'s UI today (see its
//! `rebuild_passwords_list`) even though `add`/`update`/`delete` are
//! implemented for real here, for trait completeness/testability.

use serde::{Deserialize, Serialize};

use crate::passwords::{Login, LoginFields, PasswordBackend};
use crate::domain_of;

pub enum BitwardenStatus {
    Locked,
    Unlocked,
}

pub struct BitwardenBackend {
    base_url: String,
    agent: ureq::Agent,
}

impl BitwardenBackend {
    /// `base_url` is `bw serve`'s address, e.g. `"http://127.0.0.1:8087"` —
    /// see `Settings::bitwarden_server_url`. Doesn't touch the network at
    /// all; connecting (or failing to, if `bw serve` isn't running) only
    /// happens on the first real call.
    pub fn new(base_url: impl Into<String>) -> Self {
        // Every response is read in full and status-code-inspected
        // manually (see `request_json`) rather than treating 4xx/5xx as an
        // `Err` the way ureq does by default — `bw serve` uses ordinary
        // HTTP status codes for its own `{"success": false, ...}` envelope
        // (e.g. a locked vault), which this backend needs to distinguish
        // from "couldn't reach bw serve at all," not lump into one generic
        // error type.
        let config = ureq::Agent::config_builder().http_status_as_error(false).build();
        Self { base_url: base_url.into(), agent: ureq::Agent::new_with_config(config) }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    /// GETs `path` and parses the `{"success", "data", "message"}` envelope
    /// every `bw serve` response uses, regardless of HTTP status code (see
    /// `new`'s doc comment).
    fn get_envelope<T: for<'de> Deserialize<'de>>(&self, path: &str) -> anyhow::Result<BwEnvelope<T>> {
        let mut response = self
            .agent
            .get(self.url(path))
            .call()
            .map_err(|err| anyhow::anyhow!("couldn't reach bw serve at {}: {err}", self.base_url))?;
        response
            .body_mut()
            .read_json()
            .map_err(|err| anyhow::anyhow!("bw serve returned an unexpected response for {path}: {err}"))
    }

    fn post_envelope<B: Serialize, T: for<'de> Deserialize<'de>>(&self, path: &str, body: &B) -> anyhow::Result<BwEnvelope<T>> {
        let mut response = self
            .agent
            .post(self.url(path))
            .send_json(body)
            .map_err(|err| anyhow::anyhow!("couldn't reach bw serve at {}: {err}", self.base_url))?;
        response
            .body_mut()
            .read_json()
            .map_err(|err| anyhow::anyhow!("bw serve returned an unexpected response for {path}: {err}"))
    }

    fn put_envelope<B: Serialize, T: for<'de> Deserialize<'de>>(&self, path: &str, body: &B) -> anyhow::Result<BwEnvelope<T>> {
        let mut response = self
            .agent
            .put(self.url(path))
            .send_json(body)
            .map_err(|err| anyhow::anyhow!("couldn't reach bw serve at {}: {err}", self.base_url))?;
        response
            .body_mut()
            .read_json()
            .map_err(|err| anyhow::anyhow!("bw serve returned an unexpected response for {path}: {err}"))
    }

    fn delete_envelope<T: for<'de> Deserialize<'de>>(&self, path: &str) -> anyhow::Result<BwEnvelope<T>> {
        let mut response = self
            .agent
            .delete(self.url(path))
            .call()
            .map_err(|err| anyhow::anyhow!("couldn't reach bw serve at {}: {err}", self.base_url))?;
        response
            .body_mut()
            .read_json()
            .map_err(|err| anyhow::anyhow!("bw serve returned an unexpected response for {path}: {err}"))
    }

    /// Whether the Bitwarden vault behind `bw serve` is currently locked —
    /// checked before every `list`/`search`/`add`/`update`/`delete` call
    /// would otherwise fail confusingly, and by `browser-linux-gtk3` to
    /// decide whether to show a real "Unlock Bitwarden" prompt (see
    /// `show_bitwarden_unlock_prompt`) instead of an empty/error section.
    pub fn status(&self) -> anyhow::Result<BitwardenStatus> {
        let envelope: BwEnvelope<BwStatusData> = self.get_envelope("/status")?;
        if !envelope.success {
            anyhow::bail!(envelope.message.unwrap_or_else(|| "bw serve reported a failure checking status".to_string()));
        }
        let status = envelope.data.ok_or_else(|| anyhow::anyhow!("bw serve's /status response had no data"))?.template.status;
        match status.as_str() {
            "unlocked" => Ok(BitwardenStatus::Unlocked),
            "locked" => Ok(BitwardenStatus::Locked),
            other => anyhow::bail!("bw serve reported status \"{other}\" — is it logged in to a Bitwarden account?"),
        }
    }

    /// Unlocks the vault with the Bitwarden account's master password —
    /// distinct from, and unrelated to, this browser's own vault passphrase
    /// (`Profile::has_vault_passphrase`). A wrong password surfaces as a
    /// real error here (bw serve reports `success: false`), not a silent
    /// no-op.
    pub fn unlock(&self, master_password: &str) -> anyhow::Result<()> {
        let envelope: BwEnvelope<serde_json::Value> = self.post_envelope("/unlock", &BwUnlockRequest { password: master_password })?;
        if !envelope.success {
            anyhow::bail!(envelope.message.unwrap_or_else(|| "incorrect Bitwarden master password".to_string()));
        }
        Ok(())
    }

    fn list_items(&self, search: Option<&str>) -> anyhow::Result<Vec<Login>> {
        let path = match search {
            Some(query) => format!("/list/object/items?search={}", percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC)),
            None => "/list/object/items".to_string(),
        };
        let envelope: BwEnvelope<BwListData> = self.get_envelope(&path)?;
        if !envelope.success {
            anyhow::bail!(envelope.message.unwrap_or_else(|| "bw serve reported a failure listing items".to_string()));
        }
        let items = envelope.data.map(|d| d.data).unwrap_or_default();
        Ok(items.into_iter().filter_map(bw_item_to_login).collect())
    }
}

impl PasswordBackend for BitwardenBackend {
    fn list(&self) -> anyhow::Result<Vec<Login>> {
        self.list_items(None)
    }

    fn search(&self, query: &str) -> anyhow::Result<Vec<Login>> {
        self.list_items(Some(query))
    }

    fn add(&self, fields: LoginFields) -> anyhow::Result<Login> {
        let envelope: BwEnvelope<BwItem> = self.post_envelope("/object/item", &login_fields_to_bw_item(&fields))?;
        if !envelope.success {
            anyhow::bail!(envelope.message.unwrap_or_else(|| "bw serve reported a failure creating the item".to_string()));
        }
        let item = envelope.data.ok_or_else(|| anyhow::anyhow!("bw serve's create-item response had no data"))?;
        bw_item_to_login(item).ok_or_else(|| anyhow::anyhow!("bw serve created a non-login item unexpectedly"))
    }

    fn update(&self, id: &str, fields: LoginFields) -> anyhow::Result<()> {
        let envelope: BwEnvelope<serde_json::Value> = self.put_envelope(&format!("/object/item/{id}"), &login_fields_to_bw_item(&fields))?;
        if !envelope.success {
            anyhow::bail!(envelope.message.unwrap_or_else(|| "bw serve reported a failure updating the item".to_string()));
        }
        Ok(())
    }

    fn delete(&self, id: &str) -> anyhow::Result<()> {
        let envelope: BwEnvelope<serde_json::Value> = self.delete_envelope(&format!("/object/item/{id}"))?;
        if !envelope.success {
            anyhow::bail!(envelope.message.unwrap_or_else(|| "bw serve reported a failure deleting the item".to_string()));
        }
        Ok(())
    }
}

/// The envelope every `bw serve` response is wrapped in.
#[derive(Debug, Deserialize)]
struct BwEnvelope<T> {
    success: bool,
    // Deliberately no `#[serde(default)]` here: serde_derive's bound
    // inference conservatively adds `T: Default` for any field using it,
    // even on `Option<T>` (which doesn't actually need `T: Default` to have
    // its own `None` default) — plain `Option<T>` fields already deserialize
    // to `None` when the key is missing without needing the attribute.
    data: Option<T>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BwStatusData {
    template: BwStatusTemplate,
}

#[derive(Debug, Deserialize)]
struct BwStatusTemplate {
    status: String,
}

#[derive(Debug, Serialize)]
struct BwUnlockRequest<'a> {
    password: &'a str,
}

#[derive(Debug, Deserialize)]
struct BwListData {
    data: Vec<BwItem>,
}

/// Bitwarden's item type enum — only `Login` (`1`) is relevant here; every
/// other type (secure note, card, identity, SSH key) is skipped by
/// `bw_item_to_login` returning `None`.
const BW_ITEM_TYPE_LOGIN: i64 = 1;

#[derive(Debug, Deserialize, Serialize)]
struct BwItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(rename = "type")]
    item_type: i64,
    name: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    login: Option<BwLogin>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BwLogin {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    uris: Vec<BwUri>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BwUri {
    uri: String,
}

fn bw_item_to_login(item: BwItem) -> Option<Login> {
    if item.item_type != BW_ITEM_TYPE_LOGIN {
        return None;
    }
    let login = item.login.unwrap_or(BwLogin { username: None, password: None, uris: Vec::new() });
    let site = login.uris.first().map(|u| u.uri.clone()).unwrap_or_else(|| item.name.clone());
    Some(Login {
        id: item.id.unwrap_or_default(),
        domain: domain_of(&site),
        site,
        username: login.username.unwrap_or_default(),
        password: login.password,
        // See this module's doc comment: unverified whether bw serve
        // exposes FIDO2/passkey data at all, so never populated from here.
        passkey: None,
        notes: item.notes,
        // Bitwarden's own `revisionDate` isn't a unix timestamp (it's an
        // ISO 8601 string) and parsing it isn't worth a new dependency for
        // a value `browser-linux-gtk3` doesn't actually sort Bitwarden rows
        // by (see its sectioned, not-interleaved, overlay rendering) — left
        // at 0 rather than a misleading "now."
        created_at: 0,
        updated_at: 0,
    })
}

fn login_fields_to_bw_item(fields: &LoginFields) -> BwItem {
    BwItem {
        id: None,
        item_type: BW_ITEM_TYPE_LOGIN,
        name: fields.site.clone(),
        notes: fields.notes.clone(),
        login: Some(BwLogin {
            username: Some(fields.username.clone()),
            password: fields.password.clone(),
            uris: vec![BwUri { uri: fields.site.clone() }],
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Spins up a real (loopback) HTTP server standing in for `bw serve`,
    /// so `BitwardenBackend`'s actual request/response parsing gets
    /// exercised end-to-end rather than through a mocked trait — see this
    /// module's doc comment for the important caveat that the *shape* of
    /// these canned responses is this module's best-effort understanding
    /// of `bw serve`'s real wire format, not something verified against a
    /// live instance. `handler` decides the status/body for every request;
    /// the server shuts down (via `Server::unblock`) when the returned
    /// guard is dropped.
    struct FakeServer {
        server: Arc<tiny_http::Server>,
        join: Option<std::thread::JoinHandle<()>>,
        pub base_url: String,
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            self.server.unblock();
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    fn spawn_fake_server<F>(handler: F) -> FakeServer
    where
        F: Fn(&str, &str, String) -> (u16, String) + Send + 'static,
    {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("binding a loopback test server should succeed"));
        let addr = server.server_addr().to_ip().expect("this test server always binds an IP socket, not a unix one");
        let base_url = format!("http://{addr}");
        let server_for_thread = Arc::clone(&server);
        let join = std::thread::spawn(move || {
            while let Ok(mut request) = server_for_thread.recv() {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let method = request.method().to_string();
                let url = request.url().to_string();
                let (status, response_body) = handler(&method, &url, body);
                let _ = request.respond(tiny_http::Response::from_string(response_body).with_status_code(status));
            }
        });
        FakeServer { server, join: Some(join), base_url }
    }

    #[test]
    fn status_reports_locked_and_unlocked() {
        let locked = spawn_fake_server(|_method, url, _body| {
            assert_eq!(url, "/status");
            (200, r#"{"success":true,"data":{"template":{"status":"locked"}}}"#.to_string())
        });
        let backend = BitwardenBackend::new(locked.base_url.clone());
        assert!(matches!(backend.status().unwrap(), BitwardenStatus::Locked));
        drop(locked);

        let unlocked = spawn_fake_server(|_method, _url, _body| (200, r#"{"success":true,"data":{"template":{"status":"unlocked"}}}"#.to_string()));
        let backend = BitwardenBackend::new(unlocked.base_url.clone());
        assert!(matches!(backend.status().unwrap(), BitwardenStatus::Unlocked));
    }

    #[test]
    fn status_errors_when_bw_serve_is_unreachable() {
        // No server bound at this address at all — a real "bw serve isn't
        // running" scenario, which should surface as a clear error rather
        // than a panic or a hang.
        let backend = BitwardenBackend::new("http://127.0.0.1:1");
        assert!(backend.status().is_err());
    }

    #[test]
    fn unlock_succeeds_with_the_right_password_and_fails_with_the_wrong_one() {
        let server = spawn_fake_server(|method, url, body| {
            assert_eq!(method, "POST");
            assert_eq!(url, "/unlock");
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
            if parsed["password"] == "correct horse battery staple" {
                (200, r#"{"success":true,"data":{"raw":"fake-session-key"}}"#.to_string())
            } else {
                (400, r#"{"success":false,"message":"Invalid master password."}"#.to_string())
            }
        });
        let backend = BitwardenBackend::new(server.base_url.clone());

        let err = backend.unlock("wrong password").unwrap_err();
        assert!(err.to_string().contains("Invalid master password"));

        backend.unlock("correct horse battery staple").expect("the right password should unlock successfully");
    }

    #[test]
    fn list_maps_login_items_and_skips_non_login_types() {
        let server = spawn_fake_server(|_method, url, _body| {
            assert_eq!(url, "/list/object/items");
            (
                200,
                r#"{"success":true,"data":{"data":[
                    {"id":"1","type":1,"name":"Example","notes":"a note","login":{"username":"alice","password":"hunter2","uris":[{"uri":"https://example.com"}]}},
                    {"id":"2","type":2,"name":"A secure note"}
                ]}}"#
                    .to_string(),
            )
        });
        let backend = BitwardenBackend::new(server.base_url.clone());

        let entries = backend.list().unwrap();
        assert_eq!(entries.len(), 1, "the secure-note item (type 2) should be skipped, only the login (type 1) kept");
        assert_eq!(entries[0].id, "1");
        assert_eq!(entries[0].site, "https://example.com");
        assert_eq!(entries[0].domain, "example.com");
        assert_eq!(entries[0].username, "alice");
        assert_eq!(entries[0].password.as_deref(), Some("hunter2"));
        assert_eq!(entries[0].notes, "a note");
        assert!(entries[0].passkey.is_none(), "BitwardenBackend never populates passkey data (see this module's doc comment)");
    }

    #[test]
    fn search_passes_the_query_through_as_a_url_parameter() {
        let server = spawn_fake_server(|_method, url, _body| {
            assert!(url.starts_with("/list/object/items?search="), "search should hit the same list endpoint with a ?search= param, got {url}");
            assert!(url.contains("example"), "the query text should appear in the request url, got {url}");
            (200, r#"{"success":true,"data":{"data":[]}}"#.to_string())
        });
        let backend = BitwardenBackend::new(server.base_url.clone());
        assert!(backend.search("example").unwrap().is_empty());
    }

    #[test]
    fn add_sends_a_login_type_item_and_returns_the_created_login() {
        let server = spawn_fake_server(|method, url, body| {
            assert_eq!(method, "POST");
            assert_eq!(url, "/object/item");
            let sent: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(sent["type"], 1, "should send Bitwarden's Login item type code");
            assert_eq!(sent["login"]["username"], "alice");
            assert_eq!(sent["login"]["password"], "hunter2");
            (
                200,
                r#"{"success":true,"data":{"id":"new-id","type":1,"name":"https://example.com","notes":"","login":{"username":"alice","password":"hunter2","uris":[{"uri":"https://example.com"}]}}}"#
                    .to_string(),
            )
        });
        let backend = BitwardenBackend::new(server.base_url.clone());

        let created = backend.add(LoginFields::password_only("https://example.com", "alice", "hunter2", "")).unwrap();
        assert_eq!(created.id, "new-id");
        assert_eq!(created.username, "alice");
    }

    #[test]
    fn update_and_delete_send_the_expected_requests() {
        let server = spawn_fake_server(|method, url, _body| match (method, url) {
            ("PUT", "/object/item/abc") => (200, r#"{"success":true}"#.to_string()),
            ("DELETE", "/object/item/abc") => (200, r#"{"success":true}"#.to_string()),
            _ => (404, r#"{"success":false,"message":"not found"}"#.to_string()),
        });
        let backend = BitwardenBackend::new(server.base_url.clone());

        backend.update("abc", LoginFields::password_only("https://example.com", "alice", "new-pw", "")).expect("update should succeed");
        backend.delete("abc").expect("delete should succeed");
    }

    #[test]
    fn a_failure_envelope_surfaces_bw_serves_own_message() {
        let server = spawn_fake_server(|_method, _url, _body| (500, r#"{"success":false,"message":"Something went wrong."}"#.to_string()));
        let backend = BitwardenBackend::new(server.base_url.clone());

        let err = backend.list().unwrap_err();
        assert!(err.to_string().contains("Something went wrong."));
    }
}
