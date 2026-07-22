#!/bin/sh
# Builds browser-wx for x86_64-pc-windows-gnu via `cargo zigbuild` and runs
# the resulting .exe under Wine, via wine-runner.sh.
#
# This isn't a plain `[alias]` entry in .cargo/config.toml (like run-gtk3/
# run-win32/run-nwg) because Cargo aliases only substitute arguments and
# re-dispatch through Cargo's own subcommand resolution — which correctly
# reaches `cargo-zigbuild`'s top-level `zigbuild` verb (that's what "cargo
# zigbuild ..." does), but can't reach that same binary's *other* internal
# verbs like `run`: Cargo re-passes "zigbuild" as the first argument to the
# cargo-zigbuild binary either way, so "cargo zigbuild run ..." ends up
# calling the zigbuild verb's own parser with an unrecognized "run" argument,
# not the binary's separate run verb.
set -eu

# Resolved from this script's own location (same reasoning as
# wine-runner.sh: $PWD/$CARGO_MANIFEST_DIR aren't reliably the workspace
# root here), so this works no matter which directory cargo was invoked from.
script_dir=$(cd "$(dirname "$0")" && pwd)
project_root=$(cd "$script_dir/.." && pwd)

# Cargo itself looks for cargo-zigbuild in $CARGO_HOME/bin (where `cargo
# install` puts it) *regardless* of PATH, not just via a plain PATH search —
# that's how "cargo zigbuild" keeps working in any shell without needing
# $CARGO_HOME/bin on PATH. Check the same place `command -v` alone would miss.
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
if ! command -v cargo-zigbuild >/dev/null 2>&1 && [ ! -x "$cargo_home/bin/cargo-zigbuild" ]; then
    echo "error: cargo-zigbuild not found — run 'cargo install cargo-zigbuild' first" \
        "(see README.md's \"browser-wx: building and running\" section)" >&2
    exit 1
fi

# Zig itself: prefer a project-local install under .zig/ (gitignored, same
# pattern as .wine/ for the project-local Wine build below) over whatever's
# on PATH, since a bare `export PATH=...` done in one shell doesn't persist
# to any other terminal — this script needs to find it itself rather than
# assume the caller's shell already has it. Falls through to PATH if no
# .zig/ install exists, so this still works if Zig is installed some other
# way (e.g. a system package).
zig_dir=$(find "$project_root/.zig" -mindepth 1 -maxdepth 1 -type d -name 'zig-*' 2>/dev/null | head -n1)
if [ -n "$zig_dir" ] && [ -x "$zig_dir/zig" ]; then
    PATH="$zig_dir:$PATH"
elif ! command -v zig >/dev/null 2>&1; then
    echo "error: no Zig toolchain found — install one under $project_root/.zig/" \
        "or put 'zig' on PATH (see README.md's \"browser-wx: building and running\" section)" >&2
    exit 1
fi
export PATH

cargo zigbuild --target x86_64-pc-windows-gnu -p browser-wx
exec "$script_dir/wine-runner.sh" "$project_root/target/x86_64-pc-windows-gnu/debug/browser-wx.exe"
