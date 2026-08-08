//! `ZipModuleLoader` — a `deno_core::ModuleLoader` that resolves and loads
//! every module straight out of a `zip_assets::ZipAssets`, decompressed
//! into memory, never extracted to disk. Modules live under a `zip:///`
//! specifier namespace (e.g. `zip:///main.ts`), which behaves like any
//! other hierarchical URL scheme for relative-import resolution (the same
//! way `file:///` does) — nothing extension-specific needed for `resolve`.
//!
//! `.ts`/`.tsx`/`.mts`/`.cts` sources get transpiled to JavaScript via
//! `deno_ast` (the `"transpiling"` feature — the standard, documented
//! mechanism for this, not hand-rolled) before being handed to V8; plain
//! `.js`/`.mjs` sources pass through unchanged.
use deno_ast::{MediaType, ParseParams};
use deno_core::error::ModuleLoaderError;
use deno_core::{ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind};
use std::rc::Rc;
use zip_assets::ZipAssets;

pub struct ZipModuleLoader {
    assets: Rc<ZipAssets>,
}

impl ZipModuleLoader {
    pub fn new(assets: Rc<ZipAssets>) -> Self {
        Self { assets }
    }
}

impl ModuleLoader for ZipModuleLoader {
    fn resolve(&self, specifier: &str, referrer: &str, _kind: ResolutionKind) -> Result<ModuleSpecifier, ModuleLoaderError> {
        deno_core::resolve_import(specifier, referrer).map_err(|err| deno_error::JsErrorBox::from_err(err))
    }

    fn load(&self, module_specifier: &ModuleSpecifier, _maybe_referrer: Option<&ModuleLoadReferrer>, _options: ModuleLoadOptions) -> ModuleLoadResponse {
        if module_specifier.scheme() != "zip" {
            return ModuleLoadResponse::Sync(Err(deno_error::JsErrorBox::generic(format!(
                "ZipModuleLoader only loads zip:///... specifiers, got {module_specifier}"
            ))));
        }
        let path = module_specifier.path().trim_start_matches('/');

        let result = (|| -> anyhow::Result<ModuleSource> {
            let raw = self.assets.read(path)?;
            let media_type = MediaType::from_path(std::path::Path::new(path));
            let code = match media_type {
                MediaType::TypeScript | MediaType::Mts | MediaType::Cts | MediaType::Tsx | MediaType::Jsx => {
                    let text: std::sync::Arc<str> = String::from_utf8(raw)?.into();
                    let parsed = deno_ast::parse_module(ParseParams {
                        specifier: module_specifier.clone(),
                        text,
                        media_type,
                        capture_tokens: false,
                        scope_analysis: false,
                        maybe_syntax: None,
                    })?;
                    let emitted = parsed
                        .transpile(&Default::default(), &Default::default(), &Default::default())
                        .map_err(|err| anyhow::anyhow!("transpiling {module_specifier} failed: {err}"))?
                        .into_source();
                    ModuleSourceCode::Bytes(emitted.text.into_bytes().into_boxed_slice().into())
                }
                _ => ModuleSourceCode::Bytes(raw.into_boxed_slice().into()),
            };
            Ok(ModuleSource::new(ModuleType::JavaScript, code, module_specifier, None))
        })();

        ModuleLoadResponse::Sync(result.map_err(|err| deno_error::JsErrorBox::generic(err.to_string())))
    }
}
