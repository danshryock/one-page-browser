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
        deploy_webview2_core_dll();
    }
}

/// Root cause of a real, long-standing "page never renders" bug: the XAML
/// `WebView2` control used by `windows-webview`'s reactor bridge
/// (`webview()`, used by `lib.rs`'s `page_element`) loads
/// `Microsoft.Web.WebView2.Core.dll` at runtime — a WinRT projection
/// assembly that, per `windows-reactor-setup`'s own docs ("How it's built"),
/// is *not* present on the machine by default, unlike the COM-only
/// `webview2loader.dll` the Evergreen runtime does supply. Confirmed
/// empirically in the dockur/windows VM: with this DLL missing, no
/// `msedgewebview2.exe` process ever spawns, `on_ready` never fires (checked
/// via this crate's own trace log across many launches), and the app is
/// silently stuck showing a blank control forever — no error, no exception,
/// nothing surfaced anywhere, since `reactor.rs`'s `make_begin` treats every
/// step (the `IWebView2` cast, `EnsureCoreWebView2Async()`) as a silent
/// early-return on failure.
///
/// `windows-reactor-setup` only deploys this DLL from its
/// `as_self_contained()` path (`deploy_webview2()`, private to that crate) —
/// never from `as_framework_dependent()`, the one this build actually uses.
/// Switching to `as_self_contained()` isn't a drop-in fix either: its
/// package staging (`dl_nupkg`/`extract_tar`) shells out to
/// `%SystemRoot%\System32\curl.exe`/`tar.exe`, which only exist when
/// build.rs itself *runs* on a real Windows machine — not here, where
/// build.rs runs on this Linux host during `cargo xwin build`
/// cross-compilation (`CARGO_CFG_TARGET_OS` reflects the target, but the
/// build script process itself is still native Linux). So this fetches and
/// stages the same DLL, from the same NuGet package, using Linux-native
/// `curl`/`unzip` instead.
fn deploy_webview2_core_dll() {
    const PKG: &str = "Microsoft.Web.WebView2";
    const VER: &str = "1.0.4078.44";
    const DLL: &str = "Microsoft.Web.WebView2.Core.dll";

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    // Mirrors `windows-reactor-setup`'s own `target_dir_from_out`: three
    // levels up from `OUT_DIR` (.../target/<triple>/<profile>/build/<pkg>/out)
    // lands on the actual profile directory the exe itself is placed in.
    let dest_dir = std::path::Path::new(&out_dir)
        .ancestors()
        .nth(3)
        .expect("OUT_DIR had fewer than 3 ancestors")
        .to_path_buf();
    let dest = dest_dir.join(DLL);
    if dest.is_file() {
        return;
    }

    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        _ => "x64",
    };

    let cache_dir = webview2_cache_dir();
    let nupkg = cache_dir.join(format!("{PKG}.{VER}.nupkg"));
    if !nupkg.is_file() {
        let _ = std::fs::create_dir_all(&cache_dir);
        let url = format!("https://www.nuget.org/api/v2/package/{PKG}/{VER}");
        let status = std::process::Command::new("curl")
            .args(["-sL", "-o"])
            .arg(&nupkg)
            .arg(&url)
            .status();
        if !matches!(status, Ok(s) if s.success()) {
            println!("cargo:warning=failed to download {PKG} {VER} from {url}; WebView2 will not render (see build.rs)");
            return;
        }
    }

    let _ = std::fs::create_dir_all(&dest_dir);
    // The real nupkg layout is `runtimes/win-<arch>/native_uap/<dll>` (unzip
    // needs the full internal path, unlike `windows-reactor-setup`'s copy of
    // this same package, which strips the top-level `runtimes/` dir first).
    let internal_path = format!("runtimes/win-{arch}/native_uap/{DLL}");
    let status = std::process::Command::new("unzip")
        .args(["-p"])
        .arg(&nupkg)
        .arg(&internal_path)
        .stdout(std::fs::File::create(&dest).expect("failed to create dest file for webview2 core dll"))
        .status();
    if !matches!(status, Ok(s) if s.success()) {
        println!("cargo:warning=failed to extract {internal_path} from {}; WebView2 will not render (see build.rs)", nupkg.display());
        let _ = std::fs::remove_file(&dest);
    }
}

fn webview2_cache_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .expect("neither XDG_CACHE_HOME nor HOME is set");
    base.join("claude-browser-webview2-nuget")
}
