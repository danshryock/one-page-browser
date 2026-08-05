#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use browser_core::{
        resolve_ephemeral_requested, resolve_passphrase_setup_requested, resolve_profile_name, resolve_url_argument, Profile,
    };
    use browser_linux_gtk3::{build_window_and_app, show_external_link_chooser, show_passphrase_prompt};

    gtk::init()?;

    let args: Vec<String> = std::env::args().collect();
    // Must run before anything touches a `Profile` path — resolves
    // `--app-id`/`CLAUDE_BROWSER_APP_ID` if given, and migrates an
    // existing user's data forward from a previous `APP_ID` otherwise.
    browser_core::init_app_id(args.clone());
    if let Some(url) = resolve_url_argument(args.clone()) {
        show_external_link_chooser(url, resolve_profile_name(args))?;
    } else {
        // --incognito/--private/--guest (three names for the same thing —
        // see `Profile::ephemeral`'s doc comment) take priority over
        // --profile: a private window is never "the work profile, but
        // private", it's always its own unlinked session.
        let profile = if resolve_ephemeral_requested(args.clone()) {
            Profile::ephemeral()
        } else {
            Profile::new(resolve_profile_name(args.clone()))
        };

        // A passphrase must never cross a process boundary via argv (see
        // `resolve_passphrase_setup_requested`'s doc comment) — so both
        // setting one up and unlocking an existing one are collected right
        // here, in whichever process actually needs it, via a window
        // instead. Either way, `gtk::main()` below drives it exactly like it
        // drives `build_window_and_app`'s window in the plain case.
        if resolve_passphrase_setup_requested(args.clone()) {
            show_passphrase_prompt(profile, true)?;
        } else if profile.has_passphrase() {
            show_passphrase_prompt(profile, false)?;
        } else {
            let (_window, app) = build_window_and_app(profile)?;
            app.open_start_page_or_restored_session();
        }
    }

    gtk::main();
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!(
        "browser-linux-gtk3 is a Linux-only binary; nothing to run on this platform. \
         Build browser-windows-winui or browser-windows-reactor instead (--target \
         x86_64-pc-windows-msvc, via cargo build-windows-winui/cargo build-windows-reactor), \
         or browser-macos-appkit (--target aarch64-apple-darwin/x86_64-apple-darwin, via \
         .cargo/build-macos-appkit.sh)."
    );
}
