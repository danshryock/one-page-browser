// `#[cfg(target_os = "windows")]`/`#[cfg(target_env = "msvc")]` written in a
// build script's own source reflect the HOST platform build.rs itself
// compiles for — always true when *running* build scripts, since Cargo
// always compiles and runs them on the machine doing the build, even when
// cross-compiling the actual crate for a different target. That's a real
// bug this crate hit once already: on this Linux dev machine, `cargo xwin
// build --target x86_64-pc-windows-msvc` correctly pulled in
// `windows-reactor-setup` as a build-dependency (Cargo's
// `[target.'cfg(...)'.build-dependencies]` matching *does* use the crate's
// real target, unlike source-level `#[cfg]`), but an earlier version of
// this file gated the actual call with `#[cfg(target_os = "windows")]`,
// which evaluated against the host (Linux) and silently skipped it — so
// the cross-compiled .exe never got `Microsoft.WindowsAppRuntime.Bootstrap.dll`
// copied next to it, and failed at launch with "the code execution cannot
// proceed because microsoft.windowsappruntime.bootstrap.dll was not found"
// (only caught once someone actually ran that specific binary, since every
// build/test in the dockur/windows VM used a *native* Windows build, where
// host and target coincide and the bug was invisible).
//
// The correct target check is the pair of `CARGO_CFG_TARGET_OS`/
// `CARGO_CFG_TARGET_ENV` environment variables Cargo sets for every build
// script invocation specifically to answer "what am I actually building
// for" — read at runtime here, not as a compile-time `#[cfg]`. Since
// `windows-reactor-setup` is otherwise a normal (non-optional) dependency
// now (see Cargo.toml's comment), this function always compiles regardless
// of host or target; it just no-ops everywhere except a windows-msvc
// target.
fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        // Bootstrap DLL + resources.pri copied next to the exe; the Windows
        // App SDK framework package itself is installed separately
        // (matching windows.yml's CI setup and the dockur/windows VM's
        // provisioning), so the lighter framework-dependent option is
        // right here, not `as_self_contained()`.
        windows_reactor_setup::as_framework_dependent();
    }
}
