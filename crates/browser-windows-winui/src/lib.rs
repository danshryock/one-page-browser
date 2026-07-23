//! WinUI 3 (Microsoft.UI.Xaml) native chrome experiment — the fifth front
//! end tried this session, deliberately scoped down from the other four:
//! the goal here is only getting it to *cross-compile* from Linux, not to
//! run it. WinUI 3 needs the real Windows App SDK runtime installed, which
//! isn't available under Wine, so unlike `browser-wx`/`browser-windows-win32`/
//! `browser-windows-nwg` this one has never been run at all.
//!
//! Gated on `target_env = "msvc"`, not just `target_os = "windows"` like the
//! other Windows front ends: WinRT/COM interop (which WinUI 3 is built
//! entirely on) needs MSVC specifically, so this crate's dependencies (see
//! Cargo.toml) are only resolved for `x86_64-pc-windows-msvc`, not the
//! `x86_64-pc-windows-gnu` target `browser-windows-win32`/
//! `browser-windows-nwg` use. Gating just on `target_os` here would try to
//! compile real code against those unresolved dependencies whenever the
//! workspace is built for the gnu target, breaking that existing, working
//! workflow — compiling to an empty no-op everywhere else (including
//! windows-gnu) is what keeps a bare `cargo build`/`cross build`/`cargo build
//! --target x86_64-pc-windows-gnu` across the whole workspace working
//! unchanged.
#![cfg(all(target_os = "windows", target_env = "msvc"))]

/// Minimal proof-of-linkage: initializes a WinRT apartment and activates one
/// real `Microsoft.UI.Xaml.Controls.Button` via `winio-winui3`'s generated
/// WinRT bindings — not full app content (this crate's whole point this
/// session is just proving it *cross-compiles*, not running it), but enough
/// to genuinely exercise real COM/WinRT activation (`RoInitialize`,
/// `RoActivateInstance`/`RoGetActivationFactory` under the hood), not just a
/// dependency that happens to resolve but is never actually called.
///
/// The `PackageDependency::initialize_version` call below is not optional:
/// `Microsoft.UI.Xaml.*` types (unlike plain OS-native WinRT namespaces such
/// as `Windows.Foundation`) aren't in the OS's WinRT catalog at all until the
/// Windows App SDK's framework package has been located and loaded via its
/// "Dynamic Dependencies" bootstrap API — skipping this step is exactly what
/// produces `Class not registered (0x80040154)` (`REGDB_E_CLASSNOTREG`) when
/// activating `Button` below, since `RoGetActivationFactory` has nowhere to
/// resolve that runtime class from. The returned `PackageDependency` must
/// stay alive for as long as any `Microsoft.UI.Xaml` type is in use — its
/// `Drop` unregisters the dependency.
///
/// This also requires the Windows App SDK runtime to actually be installed
/// on the machine running this (e.g. via `winget install
/// Microsoft.WindowsAppRuntime.1.7` or the redistributable installer) —
/// something no amount of cross-compilation from Linux can substitute for,
/// since it's resolved at genuine runtime, not link time (see this crate's
/// module doc comment).
///
/// `Microsoft.UI.Xaml.Controls.Button` can't be activated as a free-standing
/// object at all — a plain Win32 process has no XAML thread context until
/// `Microsoft.UI.Xaml.Application::Start` sets one up (this, not a manually
/// created `DispatcherQueueController` — that's the separate "XAML
/// Islands"/`DesktopWindowXamlSource` hosting pattern for embedding XAML in
/// a non-XAML window, not what a full `Application`/`Window`-based WinUI 3
/// app uses — is the standard bootstrap `Application::Start` in the
/// official unpackaged-desktop template does). Constructing anything
/// `Microsoft.UI.Xaml.*` before or outside that callback is exactly what
/// produced "The application called an interface that was marshalled for a
/// different thread" (`RPC_E_WRONG_THREAD`): not a literal cross-thread bug
/// in this code, but XAML having no valid context on this thread to target
/// yet. `Start` blocks pumping messages until something calls `Exit()`, so
/// the callback below does its one-time activation check, stashes the
/// result, and exits immediately rather than actually running an app.
pub fn smoke_test() -> anyhow::Result<()> {
    use std::sync::{Arc, Mutex};

    winui3::init_apartment(winui3::ApartmentType::SingleThreaded)?;
    let _dependency = winui3::bootstrap::PackageDependency::initialize_version(
        winui3::bootstrap::WindowsAppSDKVersion::V2,
    )?;

    let outcome: Arc<Mutex<Option<windows_core::Result<()>>>> = Arc::new(Mutex::new(None));
    let outcome_in_callback = Arc::clone(&outcome);

    winui3::Microsoft::UI::Xaml::Application::Start(
        &winui3::Microsoft::UI::Xaml::ApplicationInitializationCallback::new(move |_params| {
            let result = (|| -> windows_core::Result<()> {
                let _app = winui3::Microsoft::UI::Xaml::Application::new()?;
                let _button = winui3::Microsoft::UI::Xaml::Controls::Button::new()?;
                Ok(())
            })();
            *outcome_in_callback.lock().unwrap() = Some(result);
            winui3::Microsoft::UI::Xaml::Application::Current()?.Exit()
        }),
    )?;

    outcome
        .lock()
        .unwrap()
        .take()
        .expect("Application::Start callback never ran")?;
    Ok(())
}
