// Gated the same way as the rest of this crate (see src/lib.rs's top-level
// `#![cfg(...)]`): `windows-reactor-setup` is only a dependency at all on
// the windows-msvc target (see Cargo.toml's `[target.cfg(...).build-dependencies]`),
// so referencing it unconditionally here would fail to resolve when this
// build script itself is compiled for any other target (e.g. a plain `cargo
// check --workspace` on the Linux dev machine).
#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn main() {
    // Bootstrap DLL + resources.pri copied next to the exe; the Windows App
    // SDK framework package itself is installed separately (matching
    // windows.yml's CI setup and the dockur/windows VM's provisioning), so
    // the lighter framework-dependent option is right here, not
    // `as_self_contained()`.
    windows_reactor_setup::as_framework_dependent();
}

#[cfg(not(all(target_os = "windows", target_env = "msvc")))]
fn main() {}
