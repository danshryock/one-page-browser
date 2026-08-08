//! The browser's own pages/assets, embedded into the compiled executable at
//! build time — see `build.rs`'s own doc comment for how `assets/` becomes
//! `$OUT_DIR/embedded_assets.zip`, and `zip_assets::ZipAssets` for how an
//! entry gets read back out of it (decompressed straight into memory, never
//! extracted to disk). `embedded_asset_server::EmbeddedAssetServer` is what
//! actually serves this to a webview.

use std::sync::{Arc, OnceLock};
use zip_assets::ZipAssets;

/// Returns `Arc<ZipAssets>`, not `&'static ZipAssets` — the embedded
/// archive's *bytes* are genuinely `'static` (`include_bytes!`), but
/// `EmbeddedAssetServer::start` takes `Arc<ZipAssets>` uniformly, since it
/// also has to work for an extension's own zip (`zip_assets::ZipAssets`
/// built from a file read off disk at runtime, never `'static`) — one
/// server API for both cases is simpler than two. Cloning the `Arc` here is
/// cheap (a refcount bump, not a copy of the archive).
///
/// `ZipAssets::from_bytes` is deferred to first use via `OnceLock` rather
/// than eagerly parsed at static-init time, matching this codebase's
/// general preference for lazy `OnceLock`s over `lazy_static`-style eager
/// globals (see e.g. `browser-linux-gtk3/tests/gtk_tests.rs`'s own
/// `gtk_thread()`).
pub fn assets() -> Arc<ZipAssets> {
    static ASSETS: OnceLock<Arc<ZipAssets>> = OnceLock::new();
    Arc::clone(ASSETS.get_or_init(|| {
        let bytes: &'static [u8] = include_bytes!(concat!(env!("OUT_DIR"), "/embedded_assets.zip"));
        Arc::new(ZipAssets::from_bytes(std::borrow::Cow::Borrowed(bytes)).expect("build.rs should always produce a well-formed zip"))
    }))
}
