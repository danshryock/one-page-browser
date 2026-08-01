#!/bin/sh
# Cross-compiles browser-macos-appkit from Linux via `cargo zigbuild` (Zig as
# the C/C++ cross-compiler and linker — same tool browser-wx's Windows build
# already uses, see run-wx-wine.sh) plus a macOS SDK for the Apple framework
# .tbd stubs (AppKit, WebKit, Foundation, ...) the final link step needs.
#
# Not a plain `[alias]` entry in .cargo/config.toml because this needs to set
# up PATH (to find Zig) and SDKROOT (to find the SDK) first — aliases only
# substitute arguments, they can't set environment variables or search the
# filesystem. See README.md's "browser-macos-appkit: building" section for
# the one-time setup (`.zig/` + `.macos-sdk/`, both gitignored) this expects,
# and for the SDK's provenance/licensing caveat (an unofficial mirror of
# Apple SDK content — a deliberate, discussed choice, not an oversight).
#
# Usage: .cargo/build-macos-appkit.sh <aarch64-apple-darwin|x86_64-apple-darwin> [extra cargo args...]
set -eu

if [ $# -lt 1 ]; then
    echo "usage: $0 <aarch64-apple-darwin|x86_64-apple-darwin> [extra cargo args...]" >&2
    exit 1
fi
target="$1"
shift

# Resolved from this script's own location, same reasoning as
# wine-runner.sh/run-wx-wine.sh: $PWD/$CARGO_MANIFEST_DIR aren't reliably the
# workspace root here.
script_dir=$(cd "$(dirname "$0")" && pwd)
project_root=$(cd "$script_dir/.." && pwd)

zig_dir=$(find "$project_root/.zig" -mindepth 1 -maxdepth 1 -type d -name 'zig-*' 2>/dev/null | head -n1)
if [ -n "$zig_dir" ] && [ -x "$zig_dir/zig" ]; then
    PATH="$zig_dir:$PATH"
elif ! command -v zig >/dev/null 2>&1; then
    echo "error: no Zig toolchain found — install one under $project_root/.zig/" \
        "or put 'zig' on PATH (see README.md's \"browser-macos-appkit: building\" section)" >&2
    exit 1
fi
export PATH

sdk_dir=$(find "$project_root/.macos-sdk" -mindepth 1 -maxdepth 1 -type d -name 'MacOSX*.sdk' 2>/dev/null | sort -V | tail -n1)
if [ -n "$sdk_dir" ]; then
    export SDKROOT="$sdk_dir"
elif [ -z "${SDKROOT:-}" ]; then
    echo "error: no macOS SDK found under $project_root/.macos-sdk/ and SDKROOT is unset" \
        "(see README.md's \"browser-macos-appkit: building\" section)" >&2
    exit 1
fi

cargo_home="${CARGO_HOME:-$HOME/.cargo}"
if ! command -v cargo-zigbuild >/dev/null 2>&1 && [ ! -x "$cargo_home/bin/cargo-zigbuild" ]; then
    echo "error: cargo-zigbuild not found — run 'cargo install cargo-zigbuild' first" \
        "(see README.md's \"browser-macos-appkit: building\" section)" >&2
    exit 1
fi

exec cargo zigbuild --target "$target" -p browser-macos-appkit "$@"
