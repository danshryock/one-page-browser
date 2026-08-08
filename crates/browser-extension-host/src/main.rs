//! `browser-extension-host`: the Deno-embedded process a running extension's
//! backend runs inside — one OS process per extension, spawned by the main
//! browser via `rpc_protocol::RpcChildProcess`, and talking the same stdio
//! JSON-RPC protocol as any other extension backend (`rpc_protocol::{read_message,
//! write_message}`, used directly here — this binary is the "child" side of
//! that same conversation, not a second protocol).
//!
//! No sandboxing is applied to *this process itself* — that's a separate,
//! deferred piece of work (see `ROADMAP.md`). What this crate does provide
//! is Deno's own in-V8-isolate capability sandboxing (`deno_permissions`):
//! by default an extension gets no filesystem, network, environment,
//! subprocess, or FFI access at all; grants are explicit command-line
//! flags, deny-by-default otherwise.
//!
//! `console.log` relay, and why it's a stdout-pipe redirect rather than a
//! custom op: the first, more obvious design — a custom `#[op2]` the
//! extension's own JS calls directly — doesn't work, confirmed directly:
//! `deno_runtime`'s own bootstrap (`removeImportedOps` in its `99_main.js`)
//! deletes every op not on its own hardcoded internal allowlist from
//! `Deno.core.ops` before *any* extension-authored script (including this
//! crate's own `esm_entry_point`, which runs after that bootstrap step, not
//! before it) ever gets a chance to see it — a real, deliberate Deno
//! security boundary, not a bug to route around. Fighting that boundary
//! isn't the right move; instead, this leaves Deno's own, completely
//! unmodified `console.log` alone and redirects the *worker's own stdout*
//! (`WorkerOptions.stdio.stdout`, via `deno_io::StdioPipe::file`) to a pipe
//! this process reads itself, relaying each line to the host as an
//! `RpcMessage::Notification` over this process's own *real* stdout — the
//! one `rpc_protocol`'s framing owns. No JS-side trickery needed at all.
//!
//! Usage: `browser-extension-host --extension-zip <path> [--entry <path-in-zip>]
//! [--allow-read <path>]...`
mod module_loader;

use deno_core::error::AnyError;
use deno_core::url::Url;
use deno_permissions::{Permissions, PermissionsContainer, PermissionsOptions, RuntimePermissionDescriptorParser};
use deno_runtime::worker::{MainWorker, WorkerOptions, WorkerServiceOptions};
use module_loader::ZipModuleLoader;
use std::io::BufRead;
use std::rc::Rc;
use std::sync::Arc;

struct Args {
    extension_zip: std::path::PathBuf,
    entry: String,
    allow_read: Vec<String>,
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().collect();
    let mut extension_zip = None;
    let mut entry = "main.ts".to_string();
    let mut allow_read = Vec::new();

    let mut i = 1;
    while i < raw.len() {
        match raw[i].as_str() {
            "--extension-zip" => {
                extension_zip = raw.get(i + 1).cloned();
                i += 2;
            }
            "--entry" => {
                entry = raw.get(i + 1).cloned().unwrap_or(entry);
                i += 2;
            }
            "--allow-read" => {
                if let Some(path) = raw.get(i + 1) {
                    allow_read.push(path.clone());
                }
                i += 2;
            }
            other => panic!("unknown argument {other:?}"),
        }
    }

    Args {
        extension_zip: extension_zip.expect("--extension-zip <path> is required").into(),
        entry,
        allow_read,
    }
}

/// Spawns a background thread relaying every line the extension's own
/// stdout (redirected here, see this file's own doc comment) produces to
/// the host, as a `Notification` over this process's real stdout. Returns
/// the `File` end to hand to `deno_io::StdioPipe::file`, plus a
/// `JoinHandle` the caller must join *after* the worker (and therefore its
/// end of the pipe) is dropped, and *before* the process actually exits.
///
/// That join is not optional: a bare `std::thread::spawn`'d thread is not
/// joined automatically when `main` returns — the process exits
/// immediately regardless of whether this thread has finished draining the
/// pipe and relaying everything yet. Confirmed as a real, not just
/// theoretical, bug: a real `RpcChildProcess`-driven test reliably saw zero
/// relayed messages even though the exact same binary run manually (a
/// shell pipe, not `RpcChildProcess`'s own `Stdio::piped()` reader thread)
/// reliably saw all of them — a timing race, not a logic error, that a
/// slower/differently-scheduled reader on the other end made much more
/// likely to lose, not something that happened to only work by luck in the
/// manual case.
fn spawn_stdout_relay() -> (std::fs::File, std::thread::JoinHandle<()>) {
    let (reader, writer) = std::io::pipe().expect("creating a pipe for the worker's own stdout should succeed");
    let join = std::thread::spawn(move || {
        let mut lines = std::io::BufReader::new(reader).lines();
        while let Some(Ok(line)) = lines.next() {
            let notification = rpc_protocol::RpcMessage::Notification { method: "console.log".to_string(), params: serde_json::json!(line), binary_len: None };
            let _ = rpc_protocol::write_message(&mut std::io::stdout(), &notification, None);
        }
    });
    let owned_fd: std::os::fd::OwnedFd = writer.into();
    (owned_fd.into(), join)
}

fn main() -> Result<(), AnyError> {
    let args = parse_args();

    let zip_bytes = std::fs::read(&args.extension_zip).unwrap_or_else(|err| panic!("reading extension zip {:?} failed: {err}", args.extension_zip));
    let assets = Rc::new(zip_assets::ZipAssets::from_bytes(zip_bytes.into()).unwrap_or_else(|err| panic!("opening extension zip {:?} failed: {err}", args.extension_zip)));

    let main_module = Url::parse(&format!("zip:///{}", args.entry)).expect("entry path should form a valid zip:// URL");

    // Deny-by-default: every field left `None`/absent here means that
    // capability is denied outright, not "unspecified" — `allow_read` is
    // the only one this pass exposes a way to grant at all (matching the
    // "grants are plain constructor arguments/CLI flags, not a manifest
    // system" scope this pass stuck to elsewhere).
    let descriptor_parser = Arc::new(RuntimePermissionDescriptorParser::new(sys_traits::impls::RealSys));
    let permissions_options =
        PermissionsOptions { allow_read: if args.allow_read.is_empty() { None } else { Some(args.allow_read.clone()) }, ..Default::default() };
    let permissions = Permissions::from_options(descriptor_parser.as_ref(), &permissions_options).expect("constructing Permissions from the given options should succeed");
    let permissions_container = PermissionsContainer::new(descriptor_parser, permissions);

    let services = WorkerServiceOptions::<deno_resolver::npm::ByonmInNpmPackageChecker, deno_resolver::npm::ByonmNpmResolver<sys_traits::impls::RealSys>, sys_traits::impls::RealSys> {
        module_loader: Rc::new(ZipModuleLoader::new(assets)),
        blob_store: deno_web::BlobStore::default_arc(),
        broadcast_channel: Default::default(),
        deno_rt_native_addon_loader: None,
        feature_checker: Arc::new(deno_runtime::FeatureChecker::default()),
        fs: Arc::new(deno_fs::RealFs),
        node_services: None,
        npm_process_state_provider: None,
        permissions: permissions_container,
        root_cert_store_provider: None,
        fetch_dns_resolver: Default::default(),
        shared_array_buffer_store: None,
        compiled_wasm_module_store: None,
        v8_code_cache: None,
        bundle_provider: None,
    };

    let (stdout_file, relay_join) = spawn_stdout_relay();
    let mut options = WorkerOptions::default();
    options.stdio.stdout = deno_io::StdioPipe::file(stdout_file);

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("building the tokio runtime should succeed");
    let result = runtime.block_on(async move {
        let mut worker = MainWorker::bootstrap_from_options(&main_module, services, options);
        worker.execute_main_module(&main_module).await?;
        worker.run_event_loop(false).await?;
        // Explicit, not just falling out of scope: makes it obvious this
        // drop (closing `worker`'s end of the stdout pipe, the relay
        // thread's EOF signal — see `spawn_stdout_relay`'s own doc comment)
        // has to happen *before* `block_on` returns to `main`, which then
        // joins that thread next.
        drop(worker);
        Ok::<(), AnyError>(())
    });

    relay_join.join().expect("the stdout relay thread should exit cleanly once the worker's own stdout pipe closes");
    result
}
