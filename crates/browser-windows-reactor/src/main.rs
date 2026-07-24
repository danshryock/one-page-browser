#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn main() -> anyhow::Result<()> {
    browser_windows_reactor::trace("main: start");
    windows_reactor::bootstrap()?;
    browser_windows_reactor::trace("main: after bootstrap");
    let args: Vec<String> = std::env::args().collect();
    let profile = browser_core::Profile::new(browser_core::resolve_profile_name(args));
    let result = browser_windows_reactor::run(profile);
    browser_windows_reactor::trace(&format!("main: run() returned {result:?}"));
    result?;
    Ok(())
}

#[cfg(not(all(target_os = "windows", target_env = "msvc")))]
fn main() {
    eprintln!(
        "browser-windows-reactor is an MSVC-Windows-only binary; nothing to run on this platform. \
         Build with --target x86_64-pc-windows-msvc via `cargo xwin build` (see README.md)."
    );
}
