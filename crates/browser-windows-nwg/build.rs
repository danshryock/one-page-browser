// Embeds app.manifest (Common Controls v6 + DPI awareness — see that file)
// into the .exe as an RT_MANIFEST resource via app.rc. A no-op on
// non-Windows targets, where this crate itself compiles to an empty stub
// (see src/lib.rs).
//
// This must check CARGO_CFG_TARGET_OS at runtime, not #[cfg(target_os =
// "windows")]: build.rs is compiled for and run on the HOST doing the
// build, so a #[cfg] here reflects the host, not the --target being
// cross-compiled for.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("app.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
