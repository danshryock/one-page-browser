#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use browser_core::{resolve_profile_name, Profile};
    use browser_macos_appkit::build_window_and_app;

    let args: Vec<String> = std::env::args().collect();
    let profile = Profile::new(resolve_profile_name(args));

    // External-link chooser (see the other front ends' `--url`-argument
    // handling) isn't scaffolded here yet — this crate only ever opens
    // `settings.start_page`, matching its "minimal, single-page" scope (see
    // lib.rs's module doc comment).
    let app = build_window_and_app(profile)?;
    app.run();
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!(
        "browser-macos-appkit is a macOS-only binary; nothing to run on this platform. \
         Build with --target aarch64-apple-darwin/x86_64-apple-darwin on real macOS (see ROADMAP.md — \
         this crate has no cross-compile story from Linux, unlike browser-windows-winui/win32/nwg)."
    );
}
