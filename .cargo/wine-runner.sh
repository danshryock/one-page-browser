#!/bin/sh
# Cargo's runner for the x86_64-pc-windows-gnu target — see .cargo/config.toml.
#
# WebView2Loader.dll is vendored inside the webview2-com-sys crate's own
# source checkout, not copied next to the built .exe by anything in the
# build: that crate's build.rs copies it into its own OUT_DIR and points the
# *linker* at it (cargo:rustc-link-search), which is enough to satisfy the
# *.lib import library at link time — but the actual .dll still has to be
# findable when the exe actually *runs*. Rather than copying it next to the
# exe (a duplicate that'd go stale if webview2-com-sys is ever upgraded),
# this points Wine at the vendored copy directly via WINEPATH, which Wine
# also searches for DLLs alongside a process's own directory. Confirmed
# empirically: Wine accepts a plain Unix directory here (translating it
# itself, no need to spell out a Z:\... drive path), with ';' — not ':' — as
# the separator between multiple entries.
set -eu

cargo_home="${CARGO_HOME:-$HOME/.cargo}"
webview2_dir=$(find "$cargo_home/registry/src" -mindepth 2 -maxdepth 2 -type d -name 'webview2-com-sys-*' 2>/dev/null | head -n1)

if [ -n "$webview2_dir" ] && [ -f "$webview2_dir/x64/WebView2Loader.dll" ]; then
    if [ -n "${WINEPATH:-}" ]; then
        export WINEPATH="$webview2_dir/x64;$WINEPATH"
    else
        export WINEPATH="$webview2_dir/x64"
    fi
fi

exec wine "$@"
