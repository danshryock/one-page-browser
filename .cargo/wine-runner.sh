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

# Resolved from this script's own location (not $PWD/$CARGO_MANIFEST_DIR,
# neither of which is reliably the workspace root here — Cargo runs this
# runner with whatever directory the user invoked cargo from, which is
# usually but not necessarily the workspace root) so this works regardless
# of where cargo was invoked from.
script_dir=$(cd "$(dirname "$0")" && pwd)
project_root=$(cd "$script_dir/.." && pwd)

cargo_home="${CARGO_HOME:-$HOME/.cargo}"
webview2_dir=$(find "$cargo_home/registry/src" -mindepth 2 -maxdepth 2 -type d -name 'webview2-com-sys-*' 2>/dev/null | head -n1)

if [ -n "$webview2_dir" ] && [ -f "$webview2_dir/x64/WebView2Loader.dll" ]; then
    if [ -n "${WINEPATH:-}" ]; then
        export WINEPATH="$webview2_dir/x64;$WINEPATH"
    else
        export WINEPATH="$webview2_dir/x64"
    fi
fi

# Optional: a project-local Wine 11.0 build + a "webview2" bottle with the
# real WebView2 Runtime actually installed in it, both under .wine/ (gitignored
# — see README.md's "WebView2 under Wine" section for the setup). The
# Ubuntu-packaged wine (9.0 as of this writing) is too old for WebView2 to
# initialize at all; it just errors with "File not found" instead of
# rendering anything. Both of these are no-ops (this script falls through to
# whatever "wine"/WINEPREFIX are already on PATH/in the environment) if that
# setup hasn't been done, so this still works — just without real WebView2
# content rendering — either way.
wine_11_bin="$project_root/.wine/wine-11.0/opt/wine-stable/bin"
if [ -x "$wine_11_bin/wine" ]; then
    PATH="$wine_11_bin:$PATH"
    export WINESERVER="$wine_11_bin/wineserver"
fi

webview2_prefix="$project_root/.wine/bottle"
if [ -z "${WINEPREFIX:-}" ] && [ -d "$webview2_prefix" ]; then
    export WINEPREFIX="$webview2_prefix"
fi

exec wine "$@"
