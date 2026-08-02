#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use browser_core::Profile;
    use browser_macos_appkit::{build_window_and_app, resolve_args, run_chooser};

    let args: Vec<String> = std::env::args().collect();
    let (url, profile_name) = resolve_args(args.clone());
    let setup_passphrase = browser_core::resolve_passphrase_setup_requested(args);

    match url {
        // Matches the other front ends' `--url`-argument handoff: launched
        // with a URL (e.g. from the OS's "open with"/default-browser
        // handling) shows the small profile-picker chooser first, rather
        // than opening the real browser window directly.
        Some(url) => run_chooser(url, profile_name),
        None => {
            let profile = Profile::new(profile_name);
            let app = build_window_and_app(profile, setup_passphrase)?;
            app.run();
            Ok(())
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!(
        "browser-macos-appkit is a macOS-only binary; nothing to run on this platform. \
         Build with --target aarch64-apple-darwin/x86_64-apple-darwin on real macOS, or cross-compile \
         from Linux via .cargo/build-macos-appkit.sh (see README.md's \"browser-macos-appkit: building\" \
         section) — same as browser-windows-winui/win32/nwg, just still never actually run this way, \
         since there's no macOS equivalent of running the cross-compiled .exe under Wine."
    );
}
