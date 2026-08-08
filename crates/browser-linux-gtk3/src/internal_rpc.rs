//! The RPC method registry backing the switcher/profile/passwords pages
//! (`browser_chrome_core::internal_pages`) — see `crates/browser-chrome-
//! core/assets/{switcher,profile,passwords}/app.js` for the client side.
//!
//! `WebviewRpcServer` invokes every handler on its own background thread
//! (see `webview_rpc.rs`'s `start`), but every real bit of state a handler
//! needs to touch here — `AppState`'s `Rc`/`RefCell` fields, and GTK itself
//! — is only safe to touch from the single dedicated GTK thread this whole
//! app runs on. So a handler doesn't touch `AppState` directly: it posts a
//! `PendingCall` across a `glib::MainContext` channel to that thread, blocks
//! on a plain `mpsc` reply channel, and `dispatch` (running on the GTK
//! thread, inside the channel's `attach` callback) does the real work and
//! sends the result back. `glib::MainContext::channel`/`sync_channel` are
//! deprecated in favor of `spawn_future_local` + an async channel crate, but
//! this project deliberately avoids async runtimes for work that's already
//! synchronous under the hood (see `browser-core/src/history.rs`'s own doc
//! comment on the same tradeoff) — the deprecated, plain channel is still
//! the better fit here, not an oversight.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};

use browser_chrome_core::{internal_pages, RpcBody, RpcHandler};
use browser_core::{
    domain_of, launch_new_encrypted_profile_process, launch_new_ephemeral_process, launch_new_profile_process, list_profile_names,
    palette_color_for, Action, BitwardenStatus, KeyChord, Login, LoginFields, PasswordBackend, Theme,
};
use rpc_protocol::RpcError;
use serde_json::{json, Value};

use crate::{AppState, VaultState};

struct PendingCall {
    method: &'static str,
    params: Value,
    reply: mpsc::Sender<Result<Value, RpcError>>,
}

const METHODS: &[&str] = &[
    "switcher.open_pages",
    "switcher.close_page",
    "switcher.switch_to",
    "switcher.bookmarks",
    "switcher.history",
    "profile.info",
    "profile.switch",
    "profile.new_ephemeral",
    "profile.create",
    "profile.settings.get",
    "profile.settings.update_general",
    "profile.search_engines.add",
    "profile.search_engines.remove",
    "profile.search_engines.set_default",
    "profile.password_managers.list",
    "profile.password_managers.connect_bitwarden",
    "profile.password_managers.disconnect_bitwarden",
    "profile.keybindings.list",
    "profile.keybindings.set_bindings",
    "switcher.remove_bookmark",
    "passwords.list",
    "passwords.reveal",
    "passwords.add",
    "passwords.update",
    "passwords.delete",
    "navigation.open_settings",
    "navigation.open_passwords",
];

/// Builds the complete method registry for `WebviewRpcServer::start` — see
/// this module's own doc comment for why every handler is a thin
/// cross-thread proxy rather than touching `app` directly.
pub fn build_handlers(app: &Rc<AppState>) -> HashMap<String, RpcHandler> {
    #[allow(deprecated)]
    let (tx, rx) = gtk::glib::MainContext::channel::<PendingCall>(gtk::glib::Priority::DEFAULT);
    let app_for_dispatch = Rc::clone(app);
    rx.attach(None, move |call| {
        let result = dispatch(&app_for_dispatch, call.method, &call.params);
        let _ = call.reply.send(result);
        gtk::glib::ControlFlow::Continue
    });

    // `glib::Sender` is `Send` but not `Sync` (see `main_context_channel.rs`
    // upstream), and `RpcHandler` requires `Fn(..) + Send + Sync` since
    // `WebviewRpcServer` may in principle be called from more than one
    // connection — `Arc<Mutex<_>>` is `Sync` regardless, the simplest way to
    // satisfy that bound around a type that itself isn't.
    let tx = Arc::new(Mutex::new(tx));

    let mut handlers: HashMap<String, RpcHandler> = HashMap::new();
    for &method in METHODS {
        let tx = Arc::clone(&tx);
        let handler: RpcHandler = Box::new(move |body: RpcBody| -> Result<RpcBody, RpcError> {
            let params = match body {
                RpcBody::Json(value) => value,
                RpcBody::Binary(_) => return Err(rpc_err("this method only accepts JSON params")),
            };
            let (reply_tx, reply_rx) = mpsc::channel();
            tx.lock()
                .expect("the internal RPC dispatch mutex should never be poisoned")
                .send(PendingCall { method, params, reply: reply_tx })
                .map_err(|_| rpc_err("the internal RPC dispatcher is no longer running"))?;
            let result = reply_rx.recv().map_err(|_| rpc_err("no reply from the internal RPC dispatcher"))?;
            result.map(RpcBody::Json)
        });
        handlers.insert(method.to_string(), handler);
    }
    handlers
}

fn rpc_err(message: impl Into<String>) -> RpcError {
    RpcError { code: -32000, message: message.into(), data: None }
}

fn param_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

/// Runs on the GTK thread (inside the channel `attach` callback above) —
/// every handler function below may freely touch `app`'s `Rc`/`RefCell`
/// fields and call GTK-backed `AppState` methods.
fn dispatch(app: &Rc<AppState>, method: &str, params: &Value) -> Result<Value, RpcError> {
    match method {
        "switcher.open_pages" => switcher_open_pages(app),
        "switcher.close_page" => switcher_close_page(app, params),
        "switcher.switch_to" => switcher_switch_to(app, params),
        "switcher.bookmarks" => switcher_bookmarks(app, params),
        "switcher.history" => switcher_history(app, params),
        "profile.info" => profile_info(app),
        "profile.switch" => profile_switch(params),
        "profile.new_ephemeral" => profile_new_ephemeral(),
        "profile.create" => profile_create(params),
        "profile.settings.get" => profile_settings_get(app),
        "profile.settings.update_general" => profile_settings_update_general(app, params),
        "profile.search_engines.add" => search_engines_add(app, params),
        "profile.search_engines.remove" => search_engines_remove(app, params),
        "profile.search_engines.set_default" => search_engines_set_default(app, params),
        "profile.password_managers.list" => password_managers_list(app),
        "profile.password_managers.connect_bitwarden" => password_managers_connect_bitwarden(app, params),
        "profile.password_managers.disconnect_bitwarden" => password_managers_disconnect_bitwarden(app),
        "profile.keybindings.list" => keybindings_list(app),
        "profile.keybindings.set_bindings" => keybindings_set_bindings(app, params),
        "switcher.remove_bookmark" => switcher_remove_bookmark(app, params),
        "passwords.list" => passwords_list(app),
        "passwords.reveal" => passwords_reveal(app, params),
        "passwords.add" => passwords_add(app, params),
        "passwords.update" => passwords_update(app, params),
        "passwords.delete" => passwords_delete(app, params),
        "navigation.open_settings" => navigation_open_settings(app),
        "navigation.open_passwords" => navigation_open_passwords(app),
        _ => Err(rpc_err(format!("unknown internal method {method:?}"))),
    }
}

fn switcher_open_pages(app: &Rc<AppState>) -> Result<Value, RpcError> {
    let core = app.core.borrow();
    let pages: Vec<Value> = core
        .pages()
        .iter()
        .map(|p| {
            let url = p.current_url();
            let title = p.title.borrow();
            let title = if title.is_empty() { url.clone() } else { title.clone() };
            json!({ "id": p.id, "title": title, "url": url, "domain": domain_of(&url), "color": p.color })
        })
        .collect();
    Ok(json!({ "pages": pages }))
}

fn switcher_close_page(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let id = param_str(params, "id").ok_or_else(|| rpc_err("missing \"id\""))?;
    app.close_page(id);
    Ok(json!({}))
}

fn switcher_switch_to(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let id = param_str(params, "id").ok_or_else(|| rpc_err("missing \"id\""))?;
    app.switch_to(id);
    Ok(json!({}))
}

fn switcher_bookmarks(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let query = param_str(params, "query").unwrap_or("");
    let bookmarks = app.bookmarks.borrow();
    let entries = if query.is_empty() { bookmarks.all() } else { bookmarks.search(query) };
    let bookmarks_json: Vec<Value> = entries
        .iter()
        .map(|b| json!({ "url": b.url, "title": b.title, "domain": b.domain, "color": palette_color_for(&b.url) }))
        .collect();
    Ok(json!({ "bookmarks": bookmarks_json }))
}

fn switcher_history(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let query = param_str(params, "query").unwrap_or("");
    let entries = app.history.search(query, 50).map_err(|err| rpc_err(format!("history search failed: {err}")))?;
    let history_json: Vec<Value> = entries
        .iter()
        .map(|h| json!({ "url": h.url, "title": h.title, "domain": h.domain, "time": h.visited_at, "color": palette_color_for(&h.url) }))
        .collect();
    Ok(json!({ "history": history_json }))
}

fn profile_info(app: &Rc<AppState>) -> Result<Value, RpcError> {
    let others: Vec<String> = list_profile_names().into_iter().filter(|name| name != &app.profile.name).collect();
    let initial = app.profile.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".to_string());
    Ok(json!({ "name": app.profile.name, "ephemeral": app.profile.ephemeral, "initial": initial, "other_profiles": others }))
}

fn profile_switch(params: &Value) -> Result<Value, RpcError> {
    let name = param_str(params, "name").ok_or_else(|| rpc_err("missing \"name\""))?;
    launch_new_profile_process(name).map_err(|err| rpc_err(format!("failed to launch profile: {err}")))?;
    Ok(json!({}))
}

fn profile_new_ephemeral() -> Result<Value, RpcError> {
    launch_new_ephemeral_process().map_err(|err| rpc_err(format!("failed to launch a private window: {err}")))?;
    Ok(json!({}))
}

fn profile_create(params: &Value) -> Result<Value, RpcError> {
    let name = param_str(params, "name").ok_or_else(|| rpc_err("missing \"name\""))?;
    let encrypted = params.get("encrypted").and_then(Value::as_bool).unwrap_or(false);
    let result = if encrypted { launch_new_encrypted_profile_process(name) } else { launch_new_profile_process(name) };
    result.map_err(|err| rpc_err(format!("failed to create profile: {err}")))?;
    Ok(json!({}))
}

fn profile_settings_get(app: &Rc<AppState>) -> Result<Value, RpcError> {
    serde_json::to_value(&*app.settings.borrow()).map_err(|err| rpc_err(format!("failed to serialize settings: {err}")))
}

fn profile_settings_update_general(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    if let Some(start_page) = param_str(params, "start_page") {
        app.settings.borrow_mut().start_page = start_page.to_string();
    }
    if let Some(theme) = param_str(params, "theme") {
        app.settings.borrow_mut().theme = if theme == "Light" { Theme::Light } else { Theme::Dark };
    }
    if let Some(max_loaded_pages) = params.get("max_loaded_pages") {
        app.set_max_loaded_pages(max_loaded_pages.as_u64().map(|n| n as usize));
    }
    save_settings(app)?;
    app.apply_theme();
    Ok(json!({}))
}

fn search_engines_add(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let name = param_str(params, "name").ok_or_else(|| rpc_err("missing \"name\""))?;
    let query_url_template = param_str(params, "query_url_template").ok_or_else(|| rpc_err("missing \"query_url_template\""))?;
    app.settings.borrow_mut().add_search_engine(name, query_url_template);
    save_settings(app)
}

fn search_engines_remove(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let name = param_str(params, "name").ok_or_else(|| rpc_err("missing \"name\""))?;
    app.settings.borrow_mut().remove_search_engine(name);
    save_settings(app)
}

fn search_engines_set_default(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let name = param_str(params, "name").ok_or_else(|| rpc_err("missing \"name\""))?;
    app.settings.borrow_mut().default_search_engine = name.to_string();
    save_settings(app)
}

fn save_settings(app: &Rc<AppState>) -> Result<Value, RpcError> {
    app.settings().save(&app.profile).map_err(|err| rpc_err(format!("failed to save settings: {err}")))?;
    Ok(json!({}))
}

fn password_managers_list(app: &Rc<AppState>) -> Result<Value, RpcError> {
    let bitwarden_url = app.settings.borrow().bitwarden_server_url.clone();
    let (connected, detail) = match &bitwarden_url {
        // The mockup shows "Connected · alex@example.com" — this app has no
        // notion of a Bitwarden account email, so the configured `bw serve`
        // URL is shown instead: real data, not a fabricated one.
        Some(url) => (true, format!("Connected \u{b7} {url}")),
        None => (false, "Not connected".to_string()),
    };
    Ok(json!({
        "managers": [
            { "id": "browser", "label": "Browser", "connected": true, "builtin": true, "detail": "Built-in, always available" },
            { "id": "bitwarden", "label": "Bitwarden", "connected": connected, "builtin": false, "detail": detail },
        ]
    }))
}

fn password_managers_connect_bitwarden(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let server_url = param_str(params, "server_url").ok_or_else(|| rpc_err("missing \"server_url\""))?;
    app.settings.borrow_mut().bitwarden_server_url = Some(server_url.to_string());
    save_settings(app)
}

fn password_managers_disconnect_bitwarden(app: &Rc<AppState>) -> Result<Value, RpcError> {
    app.settings.borrow_mut().bitwarden_server_url = None;
    save_settings(app)
}

fn keybindings_list(app: &Rc<AppState>) -> Result<Value, RpcError> {
    let keybindings = app.keybindings.borrow();
    let bindings: Vec<Value> = Action::ALL
        .iter()
        .map(|action| {
            let chords: Vec<Value> = keybindings.bindings_for(*action).iter().map(chord_to_json).collect();
            // `serde_json::to_value` on a fieldless enum like `Action` (external
            // tagging, the serde default) serializes to its bare variant name —
            // exactly the string `keybindings_set_bindings` parses back with
            // `serde_json::from_value::<Action>`, so the two stay in sync by
            // construction rather than a hand-maintained name list on each side.
            let action_name = serde_json::to_value(action).expect("Action always serializes");
            json!({ "action": action_name, "label": action.label(), "chords": chords })
        })
        .collect();
    Ok(json!({ "bindings": bindings }))
}

fn chord_to_json(chord: &KeyChord) -> Value {
    json!({ "ctrl": chord.ctrl, "alt": chord.alt, "shift": chord.shift, "key": chord.key, "display": chord.to_string() })
}

/// Replaces one action's chords wholesale — the web Keyboard Shortcuts
/// editor's add/remove both do a full read-modify-write of the current list,
/// same as the native keybindings editor this replaces (see `Keybindings::
/// set_bindings`'s own doc comment).
fn keybindings_set_bindings(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let action_value = params.get("action").ok_or_else(|| rpc_err("missing \"action\""))?;
    let action: Action = serde_json::from_value(action_value.clone()).map_err(|err| rpc_err(format!("invalid action: {err}")))?;
    let chords_value = params.get("chords").and_then(Value::as_array).ok_or_else(|| rpc_err("missing \"chords\" array"))?;
    let chords: Vec<KeyChord> = chords_value
        .iter()
        .map(|c| {
            let ctrl = c.get("ctrl").and_then(Value::as_bool).unwrap_or(false);
            let alt = c.get("alt").and_then(Value::as_bool).unwrap_or(false);
            let shift = c.get("shift").and_then(Value::as_bool).unwrap_or(false);
            let key = c.get("key").and_then(Value::as_str).unwrap_or("").to_string();
            KeyChord::new(ctrl, alt, shift, key)
        })
        .collect();

    app.keybindings.borrow_mut().set_bindings(action, chords);
    app.keybindings.borrow().save(&app.profile).map_err(|err| rpc_err(format!("failed to save keybindings: {err}")))?;
    Ok(json!({}))
}

fn switcher_remove_bookmark(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let url = param_str(params, "url").ok_or_else(|| rpc_err("missing \"url\""))?;
    app.bookmarks.borrow_mut().remove(url);
    app.bookmarks.borrow().save(&app.profile).map_err(|err| rpc_err(format!("failed to save bookmarks: {err}")))?;
    Ok(json!({}))
}

fn login_fields_from_params(params: &Value) -> Result<LoginFields, RpcError> {
    let site = param_str(params, "site").ok_or_else(|| rpc_err("missing \"site\""))?.to_string();
    let username = param_str(params, "username").unwrap_or("").to_string();
    let password = param_str(params, "password").filter(|p| !p.is_empty()).map(str::to_string);
    let notes = param_str(params, "notes").unwrap_or("").to_string();
    Ok(LoginFields { site, username, password, passkey: None, notes })
}

fn passwords_add(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let fields = login_fields_from_params(params)?;
    let source = param_str(params, "source").unwrap_or("browser");
    if source == "bitwarden" {
        let backend = app.bitwarden_backend().ok_or_else(|| rpc_err("Bitwarden isn't connected"))?;
        backend.add(fields).map_err(|err| rpc_err(format!("failed to add Bitwarden login: {err}")))?;
    } else {
        match &*app.passwords.borrow() {
            VaultState::Unlocked(store) => {
                store.add(fields).map_err(|err| rpc_err(format!("failed to add login: {err}")))?;
            }
            VaultState::Locked | VaultState::NotSetUp => return Err(rpc_err("the password vault is locked")),
        }
    }
    Ok(json!({}))
}

fn passwords_update(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let id = param_str(params, "id").ok_or_else(|| rpc_err("missing \"id\""))?;
    let fields = login_fields_from_params(params)?;
    let source = param_str(params, "source").unwrap_or("browser");
    if source == "bitwarden" {
        let backend = app.bitwarden_backend().ok_or_else(|| rpc_err("Bitwarden isn't connected"))?;
        backend.update(id, fields).map_err(|err| rpc_err(format!("failed to update Bitwarden login: {err}")))?;
    } else {
        match &*app.passwords.borrow() {
            VaultState::Unlocked(store) => {
                store.update(id, fields).map_err(|err| rpc_err(format!("failed to update login: {err}")))?;
            }
            VaultState::Locked | VaultState::NotSetUp => return Err(rpc_err("the password vault is locked")),
        }
    }
    Ok(json!({}))
}

fn passwords_delete(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let id = param_str(params, "id").ok_or_else(|| rpc_err("missing \"id\""))?;
    let source = param_str(params, "source").unwrap_or("browser");
    if source == "bitwarden" {
        let backend = app.bitwarden_backend().ok_or_else(|| rpc_err("Bitwarden isn't connected"))?;
        backend.delete(id).map_err(|err| rpc_err(format!("failed to delete Bitwarden login: {err}")))?;
    } else {
        match &*app.passwords.borrow() {
            VaultState::Unlocked(store) => {
                store.delete(id).map_err(|err| rpc_err(format!("failed to delete login: {err}")))?;
            }
            VaultState::Locked | VaultState::NotSetUp => return Err(rpc_err("the password vault is locked")),
        }
    }
    Ok(json!({}))
}

/// The profile-menu popover's Settings/Passwords links call these instead of
/// navigating themselves: a page inside a small popover webview has no way
/// to tell the host to navigate the *real* active page other than RPC. Both
/// just forward to the same singleton-focus-or-open logic the (native)
/// toolbar buttons use directly — see `AppState::open_or_focus_internal_page`.
fn navigation_open_settings(app: &Rc<AppState>) -> Result<Value, RpcError> {
    app.open_or_focus_internal_page(internal_pages::PROFILE);
    Ok(json!({}))
}

fn navigation_open_passwords(app: &Rc<AppState>) -> Result<Value, RpcError> {
    app.open_or_focus_internal_page(internal_pages::PASSWORDS);
    Ok(json!({}))
}

fn passwords_list(app: &Rc<AppState>) -> Result<Value, RpcError> {
    let local_entries: Vec<Login> = match &*app.passwords.borrow() {
        VaultState::Unlocked(store) => store.list().unwrap_or_default(),
        VaultState::Locked | VaultState::NotSetUp => return Ok(json!({ "locked": true, "entries": [] })),
    };

    let mut entries: Vec<Value> = local_entries
        .iter()
        .map(|entry| json!({ "id": entry.id, "source": "browser", "site": entry.site, "domain": entry.domain, "username": entry.username }))
        .collect();

    if let Some(backend) = app.bitwarden_backend() {
        if let Ok(BitwardenStatus::Unlocked) = backend.status() {
            if let Ok(bitwarden_entries) = backend.list() {
                entries.extend(bitwarden_entries.iter().map(|entry| {
                    json!({ "id": entry.id, "source": "bitwarden", "site": entry.site, "domain": entry.domain, "username": entry.username })
                }));
            }
        }
    }

    Ok(json!({ "locked": false, "entries": entries }))
}

/// Fetches one entry's plaintext password on demand — `passwords.list`
/// deliberately never includes it, so a page load never pulls every saved
/// password into the DOM at once (see `assets/passwords/app.js`'s eye/copy
/// icons, the only place this is called from).
fn passwords_reveal(app: &Rc<AppState>, params: &Value) -> Result<Value, RpcError> {
    let id = param_str(params, "id").ok_or_else(|| rpc_err("missing \"id\""))?;
    let source = param_str(params, "source").unwrap_or("browser");

    let password = if source == "bitwarden" {
        app.bitwarden_backend()
            .and_then(|backend| backend.list().ok())
            .and_then(|entries| entries.into_iter().find(|entry| entry.id == id))
            .and_then(|entry| entry.password)
    } else {
        match &*app.passwords.borrow() {
            VaultState::Unlocked(store) => {
                store.list().ok().and_then(|entries| entries.into_iter().find(|entry| entry.id == id)).and_then(|entry| entry.password)
            }
            VaultState::Locked | VaultState::NotSetUp => None,
        }
    };

    Ok(json!({ "password": password }))
}
